import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../api/client';
import { getWorkspaceFile, saveWorkspaceFile } from '../api/endpoints';
import type { WorkspaceFile } from '../api/types';
import { copyText } from '../lib/clipboard';
import { describeFile, hasRichPreview, hasSourceView, type FileCapability } from '../lib/fileKind';
import { formatBytes } from '../lib/format';
import { highlightCode, languageLabel, MAX_FILE_HIGHLIGHT_CHARS } from '../lib/highlight';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import { FilePreview } from './FilePreview';
import { Tabs, type TabItem } from './Tabs';
import { CheckIcon, CopyIcon, EditIcon, EyeIcon, FileCodeIcon, RefreshIcon } from './icons';

type LoadState = 'loading' | 'ready' | 'error';
type ViewMode = 'preview' | 'source';

/** Keep the editing textarea inside the browser's comfort zone, not the server's cap. */
const MAX_EDITOR_CHARS = 2 * 1024 * 1024;

function isConflict(cause: unknown): boolean {
  return cause instanceof ApiError && cause.status === 409;
}

/**
 * The workspace file viewer.
 *
 * It renders what it can — Markdown, HTML, images and PDFs get a real preview,
 * everything textual gets highlighted source — and lets the user edit and save
 * any complete text file. Saves are optimistic-concurrency checked with the full
 * digest that was loaded (plus size for older servers), so a file the agent
 * rewrote underneath the editor is refused rather than silently clobbered.
 */
export function FileViewer(): JSX.Element | null {
  const path = store.fileViewerPath.value;
  const projectId = store.currentProjectId.value;

  const [file, setFile] = useState<WorkspaceFile | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [retryKey, setRetryKey] = useState(0);

  const [mode, setMode] = useState<ViewMode>('source');
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  const editorRef = useRef<HTMLTextAreaElement>(null);
  // Keystrokes can arrive while a save is awaiting the server. The completion
  // compares that request's snapshot with this live value before closing anything.
  const latestDraft = useRef('');
  // Loads and saves share one operation clock. Changing file/project invalidates
  // every older completion, including its error toast and `finally` cleanup.
  const operationGeneration = useRef(0);

  useEffect(() => {
    const generation = ++operationGeneration.current;
    if (!path) return undefined;
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setFile(null);
    setCopied(false);
    setEditing(false);
    setDraft('');
    setSaving(false);
    setSaveError(null);
    setConflict(false);
    setConfirmDiscard(false);
    setSavedAt(null);
    getWorkspaceFile(path, projectId, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted || operationGeneration.current !== generation) return;
        setFile(result);
        setDraft(result.content);
        // Open on the rendered view when there is one; source is the fallback and
        // the only choice for ordinary code.
        const capability = describeFile(result);
        setMode(hasRichPreview(capability.render) ? 'preview' : 'source');
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted || operationGeneration.current !== generation) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => {
      controller.abort();
      operationGeneration.current += 1;
    };
  }, [path, projectId, retryKey]);

  useEffect(() => {
    if (editing) editorRef.current?.focus();
  }, [editing]);

  const capability = useMemo<FileCapability | null>(
    () =>
      file
        ? describeFile({
            path: file.path,
            language: file.language,
            binary: file.binary,
            truncated: file.truncated,
          })
        : null,
    [file],
  );

  const highlighted = useMemo(() => {
    if (!file || file.binary) return '';
    return highlightCode(file.content, file.language, MAX_FILE_HIGHLIGHT_CHARS);
  }, [file]);

  const dirty = editing && file !== null && draft !== file.content;
  const previewable = capability !== null && hasRichPreview(capability.render);
  // The preview follows the editor while it is open, so "Preview" is a live
  // preview of the draft rather than of the bytes on disk.
  const previewContent = editing ? draft : (file?.content ?? '');

  if (!path) return null;

  const lineCount = file && !file.binary ? file.content.split('\n').length : 0;

  const close = (): void => {
    if (dirty && !confirmDiscard) {
      setConfirmDiscard(true);
      return;
    }
    actions.closeFileViewer();
  };

  const save = async (force = false): Promise<void> => {
    if (!file || saving) return;
    if (draft.length > MAX_EDITOR_CHARS) {
      setSaveError(
        `This file is ${formatBytes(draft.length)}, which is past the editor's limit. Edit it with the agent instead.`,
      );
      return;
    }

    // Nothing after `await` may use the changing render closures. This snapshot is
    // the exact project/path/version the PUT is allowed to report back into.
    const snapshot = {
      path,
      projectId,
      file,
      draft,
      capability,
      generation: ++operationGeneration.current,
    };
    const isCurrent = (): boolean =>
      operationGeneration.current === snapshot.generation &&
      store.fileViewerPath.value === snapshot.path &&
      store.currentProjectId.value === snapshot.projectId;

    latestDraft.current = snapshot.draft;
    setSaving(true);
    setSaveError(null);
    setConflict(false);
    try {
      const result = await saveWorkspaceFile(snapshot.path, snapshot.draft, {
        project: snapshot.projectId,
        // Digest catches same-length rewrites; size keeps older servers guarded.
        expectedDigest: force ? undefined : snapshot.file.digest,
        expectedSize: force ? undefined : snapshot.file.size,
        force,
      });
      if (!isCurrent()) return;
      setFile({
        ...snapshot.file,
        content: snapshot.draft,
        size: result.size,
        // An older server's write has no fresh digest; never retain the stale one.
        digest: result.digest,
        truncated: false,
        language: result.language ?? snapshot.file.language,
      });
      setSavedAt(Date.now());
      const hasNewerDraft = latestDraft.current !== snapshot.draft;
      // The PUT saved exactly the request snapshot. Anything typed afterwards is
      // still a live, unsaved draft, with the response digest as its new base.
      setEditing(hasNewerDraft);
      setConfirmDiscard(false);
      // Saving a Markdown or HTML file is almost always followed by "did that
      // render right?", but only when there is no newer draft still being edited.
      if (!hasNewerDraft && snapshot.capability && hasRichPreview(snapshot.capability.render)) {
        setMode('preview');
      }
      store.pushToast(
        hasNewerDraft
          ? `Saved ${result.path}; newer edits remain unsaved.`
          : `Saved ${result.path}`,
        'success',
        2600,
      );
      // The tree's sizes are now stale; a cheap bump keeps the explorer honest.
      actions.noteWorkspaceFileChanged(result.path);
    } catch (cause) {
      if (!isCurrent()) return;
      setSaveError(toErrorMessage(cause));
      setConflict(isConflict(cause));
    } finally {
      if (isCurrent()) setSaving(false);
    }
  };

  const onKeyDown = (event: JSX.TargetedKeyboardEvent<HTMLDivElement>): void => {
    if (!editing) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      void save();
    }
  };

  // The tab strip exists only when there is a choice to make. A code file has no
  // preview renderer, so offering "Preview" there was a dead tab: clicking it left
  // `.file-code` as the pane and rendered nothing (`hasSourceView('code')` is
  // true, but `hasRichPreview('code')` is not). `previewable` is the right
  // question - the same one the body below asks before drawing a preview.
  const showTabs = previewable && capability !== null && hasSourceView(capability.render);
  const viewTabs = useMemo<TabItem<ViewMode>[]>(() => {
    const items: TabItem<ViewMode>[] = [];
    if (previewable) items.push({ id: 'preview', label: 'Preview', icon: <EyeIcon size={13} /> });
    items.push({ id: 'source', label: 'Source', icon: <FileCodeIcon size={13} /> });
    return items;
  }, [previewable]);
  // Editing forces the source view, and a file with no preview has nothing else
  // to show: never leave `mode` pointing at a panel that is not in the strip.
  const activeMode: ViewMode = editing || !previewable ? 'source' : mode;
  const tabs = showTabs ? (
    <Tabs
      items={viewTabs}
      active={activeMode}
      onChange={setMode}
      label="File view"
      idPrefix="file-view"
      variant="fileview"
    />
  ) : null;

  return (
    <Dialog open title={path} onClose={close} class="dialog-file">
      <div class="file-viewer" onKeyDown={onKeyDown}>
        <div class="file-toolbar">
          <div class="file-meta">
            {file ? (
              <>
                <span class="badge badge-muted">{languageLabel(file.language)}</span>
                <span class="file-size">{formatBytes(file.size)}</span>
                {dirty ? <span class="badge badge-warn">unsaved</span> : null}
                {file.truncated ? <span class="badge badge-warn">truncated by server</span> : null}
                {file.binary ? <span class="badge badge-warn">binary</span> : null}
              </>
            ) : (
              <span class="muted small">Reading file…</span>
            )}
          </div>
          <div class="file-toolbar-actions">
            {editing ? (
              <>
                <button
                  type="button"
                  class="button button-small button-primary"
                  disabled={saving || !dirty}
                  onClick={() => void save()}
                >
                  <CheckIcon size={13} /> {saving ? 'Saving…' : 'Save'}
                </button>
                <button
                  type="button"
                  class="button button-small"
                  disabled={saving}
                  onClick={() => {
                    setDraft(file?.content ?? '');
                    setEditing(false);
                    setSaveError(null);
                    setConflict(false);
                  }}
                >
                  Cancel
                </button>
              </>
            ) : (
              <button
                type="button"
                class="button button-small"
                disabled={state !== 'ready' || capability?.canEdit !== true}
                title={capability?.editBlockedReason ?? 'Edit this file'}
                onClick={() => {
                  setDraft(file?.content ?? '');
                  setMode('source');
                  setEditing(true);
                }}
              >
                <EditIcon size={13} /> Edit
              </button>
            )}
            <button
              type="button"
              class="button button-small"
              onClick={() => {
                void copyText(path).then((ok) => {
                  setCopied(ok);
                  window.setTimeout(() => setCopied(false), 1400);
                });
              }}
            >
              <CopyIcon size={13} /> {copied ? 'Copied' : 'Copy path'}
            </button>
            <button
              type="button"
              class="icon-button"
              aria-label="Reload file"
              title={dirty ? 'Discard your edits and reload from disk' : 'Reload file'}
              disabled={saving}
              onClick={() => {
                if (dirty) {
                  setConfirmDiscard(true);
                  return;
                }
                setRetryKey((key) => key + 1);
              }}
            >
              <RefreshIcon class={state === 'loading' ? 'spin' : undefined} />
            </button>
          </div>
        </div>

        {tabs ? <div class="file-tabbar">{tabs}</div> : null}

        {state === 'loading' ? (
          <div class="file-loading" aria-busy="true" aria-label="Loading file">
            <div class="skeleton skeleton-code" />
            <div class="skeleton skeleton-code skeleton-short" />
            <div class="skeleton skeleton-code" />
          </div>
        ) : null}

        {state === 'error' ? (
          <div class="inline-error" role="alert">
            <p>{error}</p>
            <button
              type="button"
              class="button button-small"
              onClick={() => setRetryKey((key) => key + 1)}
            >
              Retry
            </button>
          </div>
        ) : null}

        {confirmDiscard ? (
          <div class="file-confirm" role="alert">
            <span>
              {dirty ? 'Discard your unsaved changes?' : 'Discard your edits and reload from disk?'}
            </span>
            <div class="file-confirm-actions">
              <button
                type="button"
                class="button button-small"
                data-autofocus
                onClick={() => setConfirmDiscard(false)}
              >
                Keep editing
              </button>
              <button
                type="button"
                class="button button-small button-danger"
                onClick={() => {
                  setConfirmDiscard(false);
                  if (dirty) actions.closeFileViewer();
                  else setRetryKey((key) => key + 1);
                }}
              >
                Discard
              </button>
            </div>
          </div>
        ) : null}

        {saveError ? (
          <div class="inline-error file-save-error" role="alert">
            <p>{saveError}</p>
            {conflict ? (
              <button
                type="button"
                class="button button-small button-danger"
                disabled={saving}
                onClick={() => void save(true)}
              >
                Overwrite anyway
              </button>
            ) : null}
          </div>
        ) : null}

        {state === 'ready' && file ? (
          capability?.render === 'binary' ? (
            <p class="file-notice">
              This looks like a binary file, so no text preview is available.
            </p>
          ) : capability?.render === 'image' || capability?.render === 'pdf' ? (
            // Images and PDFs are their own preview: the bytes come from the raw
            // endpoint, which is also the only case where `file.binary` may be
            // false (a PDF or SVG is ASCII, so the server's sniff calls it text).
            <FilePreview
              path={path}
              content={file.content}
              render={capability.render}
              project={projectId}
              onOpenFile={actions.openFileViewer}
            />
          ) : editing && activeMode === 'source' ? (
            <>
              {file.truncated ? (
                <p class="file-notice">
                  The server truncated this file at {formatBytes(file.size)}. Only the beginning
                  was loaded, which is why it cannot be saved from here.
                </p>
              ) : null}
              <textarea
                ref={editorRef}
                class="file-editor"
                aria-label={`Edit ${path}`}
                spellcheck={false}
                autocomplete="off"
                autocapitalize="off"
                wrap="off"
                value={draft}
                onInput={(event) => {
                  latestDraft.current = event.currentTarget.value;
                  setDraft(event.currentTarget.value);
                }}
              />
            </>
          ) : activeMode === 'preview' && previewable ? (
            // `Tabs` points `aria-controls` at `file-view-panel-preview`, so the
            // pane has to be the element with that id and the tabpanel role -
            // otherwise the reference dangles and a screen reader announces a tab
            // that controls nothing.
            <div
              id="file-view-panel-preview"
              class="file-view-pane"
              role="tabpanel"
              aria-labelledby="file-view-tab-preview"
              tabIndex={0}
            >
              <FilePreview
                path={path}
                content={previewContent}
                render={capability?.render ?? 'code'}
                project={projectId}
                dirty={dirty}
                onOpenFile={actions.openFileViewer}
              />
            </div>
          ) : (
            <>
              {file.truncated ? (
                <p class="file-notice">
                  The server truncated this file at {formatBytes(file.size)}. Only the beginning
                  is shown.
                </p>
              ) : null}
              <div
                id={showTabs ? 'file-view-panel-source' : undefined}
                class="file-view-pane"
                role={showTabs ? 'tabpanel' : undefined}
                aria-labelledby={showTabs ? 'file-view-tab-source' : undefined}
                tabIndex={showTabs ? 0 : undefined}
              >
                <div class="file-code-scroll" tabIndex={0} role="region" aria-label={`Contents of ${path}`}>
                  <div class="file-gutter" aria-hidden="true">
                    {Array.from({ length: lineCount }, (_, index) => (
                      <span key={index}>{index + 1}</span>
                    ))}
                  </div>
                  <pre class="file-code">
                    <code class="hljs" dangerouslySetInnerHTML={{ __html: highlighted }} />
                  </pre>
                </div>
              </div>
            </>
          )
        ) : null}

        <div class="file-statusbar" aria-live="polite">
          <span>
            {capability?.render === 'image'
              ? 'Image'
              : capability?.render === 'pdf'
                ? 'PDF'
                : capability?.render === 'binary'
                  ? 'Binary'
                  : `${lineCount} ${lineCount === 1 ? 'line' : 'lines'}`}
            {editing ? ' · editing' : ''}
            {dirty ? ' · unsaved changes' : ''}
          </span>
          {/* A separate element so the reason reads as a sentence, not as a
              suffix glued onto the line count. */}
          <span class="file-statusbar-hint">
            {savedAt !== null && !dirty
              ? 'Saved just now'
              : capability?.canEdit
                ? 'Ctrl/Cmd+S saves'
                : (capability?.editBlockedReason ?? '')}
          </span>
        </div>
      </div>
    </Dialog>
  );
}
