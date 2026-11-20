import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { TargetedKeyboardEvent } from 'preact';
import { ApiError, toErrorMessage } from '../api/client';
import { getWorkspaceFile, saveWorkspaceFile } from '../api/endpoints';
import type { WorkspaceFile } from '../api/types';
import { copyText } from '../lib/clipboard';
import { describeFile, hasRichPreview, hasSourceView, type FileCapability } from '../lib/fileKind';
import { formatBytes } from '../lib/format';
import { highlightCode, languageLabel, MAX_FILE_HIGHLIGHT_CHARS } from '../lib/highlight';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as workspaceTabs from '../state/workspaceTabs';
import { Badge, Button, Callout, IconButton, LinkButton, Overlay, Skeleton, Textarea } from '../ui';
import type { TabItem } from '../components/Tabs';
import { FilePreview } from './FilePreview';
import { Tabs } from './Tabs';
import {
  CheckIcon,
  CopyIcon,
  EditIcon,
  EyeIcon,
  FileCodeIcon,
  RefreshIcon,
  SidebarIcon,
} from './icons';

type LoadState = 'loading' | 'ready' | 'error';
type ViewMode = 'preview' | 'source';

/**
 * Where a file is being shown: the dialog (opened from the explorer, a search
 * result or a tool call) or a tab in the workspace strip beside the conversation.
 */
export type FileViewerHost = 'dialog' | 'tab';

/** Keep the editing textarea inside the browser's comfort zone, not the server's cap. */
const MAX_EDITOR_CHARS = 2 * 1024 * 1024;

/**
 * `content` split into its lines, worked out once per distinct document.
 *
 * The gutter draws one row per line and the status bar prints the count, and both
 * read it on every render — including the render of every keystroke while the
 * editor is open. Splitting there copied the whole loaded document and built one
 * array element per line only to read `.length`: for a 2 MiB file that is a 2 MiB
 * copy and a ~100 000-element array per keystroke, which is a visible hitch in
 * Firefox. A few entries rather than one, keyed by value: one viewer plus the
 * preview beside it renders two documents in the same commit, and a single slot
 * would make them evict each other on every keystroke - the exact cost this is
 * here to avoid. Insertion order is the recency order, and a hit is re-inserted.
 */
const SPLIT_CACHE_SIZE = 3;
const splitCache = new Map<string, string[]>();

export function splitLines(content: string): string[] {
  const cached = splitCache.get(content);
  if (cached) {
    // Move to the newest position so the file on screen is never the one evicted.
    splitCache.delete(content);
    splitCache.set(content, cached);
    return cached;
  }
  const lines = content.split('\n');
  splitCache.set(content, lines);
  while (splitCache.size > SPLIT_CACHE_SIZE) {
    const oldest = splitCache.keys().next();
    if (oldest.done) break;
    splitCache.delete(oldest.value);
  }
  return lines;
}

function isConflict(cause: unknown): boolean {
  return cause instanceof ApiError && cause.status === 409;
}

/**
 * The workspace file viewer, as a modal.
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
  if (!path) return null;
  return (
    <FileViewerPanel
      path={path}
      projectId={projectId}
      host="dialog"
      onClose={actions.closeFileViewer}
    />
  );
}

/**
 * The same viewer as one tab of the workspace strip.
 *
 * The strip keeps every open tab mounted (`ChatView`), so this panel's reads,
 * scroll position and unsaved edits survive switching to the conversation and
 * back; only closing the tab unmounts it.
 */
export function FileTabView({ path }: { path: string }): JSX.Element {
  const projectId = store.currentProjectId.value;
  const close = (): void => workspaceTabs.closeFileTab(path);
  return <FileViewerPanel path={path} projectId={projectId} host="tab" onClose={close} />;
}

interface FileViewerPanelProps {
  /** Workspace-relative path to show. */
  path: string;
  projectId: string | null;
  host: FileViewerHost;
  /** Unconditional close of this surface; the panel asks before discarding first. */
  onClose: () => void;
}

function FileViewerPanel({ path, projectId, host, onClose }: FileViewerPanelProps): JSX.Element {
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

  // The editor is mounted only while editing, so its own `autoFocus` is what
  // puts the caret in the buffer; there is no persistent ref to focus.
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

  const lineCount = file && !file.binary ? splitLines(file.content).length : 0;

  /**
   * Follow a link in a rendered preview to another workspace file. The dialog
   * replaces what it shows, which is what a reader who followed a link there
   * expects; a tab opens its own, because the strip is a set of documents and
   * following a link must not overwrite the one being read.
   */
  const openLinkedFile = host === 'dialog' ? actions.openFileViewer : workspaceTabs.openFileTab;

  const close = (): void => {
    if (dirty && !confirmDiscard) {
      setConfirmDiscard(true);
      return;
    }
    onClose();
  };

  // The tab strip's own close button lives outside this panel. Its guard answers
  // with the same `close` the toolbar uses, so neither route can drop a draft:
  // `close` shows the confirmation and closes on the second, informed click.
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    if (host !== 'tab') return undefined;
    return workspaceTabs.registerFileTabCloseGuard(path, () => {
      closeRef.current();
      return true;
    });
  }, [host, path]);

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
      (host === 'tab' || store.fileViewerPath.value === snapshot.path) &&
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

  const onKeyDown = (event: TargetedKeyboardEvent<HTMLDivElement>): void => {
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
  // question - the same one the body below asks before drawing a preview. Editing
  // is the other: it forces the editor, so the toggle would be a control that
  // cannot change anything. It comes back, on the saved content, when the editor
  // closes.
  const showTabs = !editing && previewable && capability !== null && hasSourceView(capability.render);
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

  // Moving the file to the other host remounts it, which reloads from disk: a
  // draft that only exists in this panel would be lost, so the switch waits until
  // there is nothing unsaved to lose.
  const switchTitle = dirty
    ? 'Save or cancel your changes before moving this file to the other view'
    : host === 'dialog'
      ? 'Open in a tab beside the conversation'
      : 'Open this file in the dialog';
  const switchHost = (): void => {
    if (dirty) return;
    if (host === 'dialog') actions.openFileInTab(path);
    else actions.openFileViewer(path);
  };

  // The panel's own chrome is one quiet bar: the view toggle, then the actions.
  // A file tab used to stack three grey rows above the document - a path header,
  // a metadata toolbar and the view tabs - and the path header said what the
  // strip's own tab and the session breadcrumb already said. The facts that are
  // worth knowing (path, language, size, line count) belong in the status bar at
  // the bottom, where an editor keeps them and where nothing repeats.
  const flags: string[] = [];
  if (file?.truncated) flags.push('truncated by server');
  if (capability?.render === 'binary') flags.push('binary');

  const kindWord =
    capability?.render === 'image'
      ? 'Image'
      : capability?.render === 'pdf'
        ? 'PDF'
        : capability?.render === 'binary'
          ? 'Binary'
          : null;
  const lineWord = `${lineCount} ${lineCount === 1 ? 'line' : 'lines'}`;

  /* No `host`-specific class: `.file-viewer-tabbed` was emitted here and defined
   * in no stylesheet, so it was a hook nobody could find and a reader could
   * mistake for styling (WB-07). The tab host is already distinguishable from the
   * dialog by `.tab-panel > .file-viewer` and by the absence of the dialog's
   * chrome; nothing keys off a modifier class. */
  const body = (
    <div class="file-viewer" onKeyDown={onKeyDown}>
      <div class="file-toolbar">
        {tabs}
        {flags.length > 0 ? (
          <div class="file-flags">
            {flags.map((flag) => (
              <Badge key={flag} tone="warn">
                {flag}
              </Badge>
            ))}
          </div>
        ) : null}
        <div class="file-toolbar-actions">
          {editing ? (
            <>
              <Button
                variant="primary"
                size="sm"
                icon={<CheckIcon size={13} />}
                title="Save this file (Ctrl/Cmd+S)"
                testId="file-viewer-save"
                disabled={saving || !dirty}
                onClick={() => void save()}
              >
                {saving ? 'Saving…' : 'Save'}
              </Button>
              <Button
                variant="secondary"
                size="sm"
                testId="file-viewer-cancel"
                disabled={saving}
                onClick={() => {
                  setDraft(file?.content ?? '');
                  setEditing(false);
                  setSaveError(null);
                  setConflict(false);
                }}
              >
                Cancel
              </Button>
            </>
          ) : (
            <Button
              variant="secondary"
              size="sm"
              icon={<EditIcon size={13} />}
              title={capability?.editBlockedReason ?? 'Edit this file'}
              testId="file-viewer-edit"
              disabled={state !== 'ready' || capability?.canEdit !== true}
              onClick={() => {
                setDraft(file?.content ?? '');
                setMode('source');
                setEditing(true);
              }}
            >
              Edit
            </Button>
          )}
          <LinkButton
            tone="muted"
            title={switchTitle}
            testId="file-viewer-switch-host"
            aria-disabled={dirty}
            class={dirty ? 'opacity-50' : undefined}
            onClick={switchHost}
          >
            <SidebarIcon size={13} />
            {host === 'dialog' ? 'Open in tab' : 'Open in dialog'}
          </LinkButton>
          <LinkButton
            tone="muted"
            title="Copy this file's workspace path"
            testId="file-viewer-copy-path"
            onClick={() => {
              void copyText(path).then((ok) => {
                setCopied(ok);
                window.setTimeout(() => setCopied(false), 1400);
              });
            }}
          >
            <CopyIcon size={13} /> {copied ? 'Copied' : 'Copy path'}
          </LinkButton>
          <IconButton
            size="sm"
            label="Reload file"
            title={dirty ? 'Discard your edits and reload from disk' : 'Reload file'}
            testId="file-viewer-reload"
            icon={<RefreshIcon class={state === 'loading' ? 'spin' : undefined} />}
            disabled={saving}
            onClick={() => {
              if (dirty) {
                setConfirmDiscard(true);
                return;
              }
              setRetryKey((key) => key + 1);
            }}
          />
        </div>
      </div>

      {state === 'loading' ? (
        <div class="file-loading" aria-busy="true" aria-label="Loading file">
          <Skeleton class="skeleton-code" />
          <Skeleton class="skeleton-code skeleton-short" />
          <Skeleton class="skeleton-code" />
        </div>
      ) : null}

      {state === 'error' ? (
        <Callout
          tone="danger"
          testId="file-viewer-error"
          class="file-load-error"
          actions={
            <Button
              variant="secondary"
              size="sm"
              testId="file-viewer-retry"
              onClick={() => setRetryKey((key) => key + 1)}
            >
              Retry
            </Button>
          }
        >
          {error}
        </Callout>
      ) : null}

      {confirmDiscard ? (
        <Callout
          tone="warn"
          testId="file-viewer-confirm-discard"
          class="file-confirm"
          actions={
            <div class="file-confirm-actions">
              <Button
                variant="secondary"
                size="sm"
                data-autofocus
                testId="file-viewer-keep-editing"
                onClick={() => setConfirmDiscard(false)}
              >
                Keep editing
              </Button>
              <Button
                variant="danger"
                size="sm"
                testId="file-viewer-discard"
                onClick={() => {
                  setConfirmDiscard(false);
                  if (dirty) onClose();
                  else setRetryKey((key) => key + 1);
                }}
              >
                Discard
              </Button>
            </div>
          }
        >
          {dirty ? 'Discard your unsaved changes?' : 'Discard your edits and reload from disk?'}
        </Callout>
      ) : null}

      {saveError ? (
        <Callout
          tone="danger"
          testId="file-viewer-save-error"
          class="file-save-error"
          actions={
            conflict ? (
              <Button
                variant="danger"
                size="sm"
                testId="file-viewer-overwrite"
                disabled={saving}
                onClick={() => void save(true)}
              >
                Overwrite anyway
              </Button>
            ) : undefined
          }
        >
          {saveError}
        </Callout>
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
            onOpenFile={openLinkedFile}
          />
        ) : editing && activeMode === 'source' ? (
          <>
            {file.truncated ? (
              <p class="file-notice">
                The server truncated this file at {formatBytes(file.size)}. Only the beginning
                was loaded, which is why it cannot be saved from here.
              </p>
            ) : null}
            <Textarea
              id="file-editor"
              class="file-editor border-0 rounded-none px-6 py-6 resize-none"
              rows={24}
              spellCheck={false}
              autoComplete="off"
              autoFocus
              value={draft}
              testId="file-viewer-editor"
              aria-label={`Edit ${path}`}
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
              onOpenFile={openLinkedFile}
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

      {/* The document's own facts, where an editor keeps them: the path (only in a
          tab - the dialog's title bar already names it), the language, the size and
          the line count. Nothing here repeats the toolbar above the file. */}
      <div class="file-statusbar" aria-live="polite">
        <span class="file-status">
          {host === 'tab' ? (
            <>
              <span class="file-status-path" title={path}>
                {path}
              </span>
              <span class="file-status-sep" aria-hidden="true">
                ·
              </span>
            </>
          ) : null}
          <span class="file-status-facts">
            {file === null || capability === null ? (
              state === 'error' ? 'Could not read this file' : 'Reading file…'
            ) : (
              <>
                {kindWord ?? languageLabel(file.language)}
                {' · '}
                <span class="file-size">{formatBytes(file.size)}</span>
                {kindWord === null ? <>{` · ${lineWord}`}</> : null}
                {editing ? ' · editing' : ''}
                {dirty ? ' · unsaved changes' : ''}
              </>
            )}
          </span>
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
  );

  if (host === 'dialog') {
    return (
      <Overlay open title={path} onClose={close} role="full" class="dialog-file" bodyLayout="contents">
        {body}
      </Overlay>
    );
  }
  return body;
}
