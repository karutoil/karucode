import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { RewindFile, RewindMode, RewindPreview } from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';

/**
 * Recall to an earlier message: the confirmation surface for a rewind.
 *
 * The dialog exists because a rewind has two independent consequences and both
 * need a decision made with the consequences visible:
 *
 * * **the transcript** - everything after the chosen turn disappears from what
 *   the model reads and from what the reader sees. That is always true, and the
 *   dialog says so with the count.
 * * **the workspace** - files the agent wrote after that turn can be put back.
 *   Reverting is *off* by default: it overwrites files, and a click that quietly
 *   discarded the work of the last ten minutes would be the worst possible
 *   default.
 *
 * The file list comes from `GET /api/sessions/:id/rewind`, which is the same
 * preview the server replays from - so the list here is what will happen, not an
 * approximation of it.
 */
export interface RewindDialogProps {
  /** Loads the preview for the open target. Injected so tests need no server. */
  loadPreview?: () => Promise<RewindPreview | null>;
  /** Applies the rewind. Injected so tests need no server. */
  apply?: (options: { mode: RewindMode; revert: boolean; note?: string }) => Promise<boolean>;
}

type Status = 'idle' | 'running' | 'error';

/** "1 file", "3 files". */
function fileCount(n: number): string {
  return n === 1 ? '1 file' : `${n} files`;
}

/** First line of a multi-line prompt, short enough for a dialog heading. */
function oneLine(text: string, max = 120): string {
  const flat = text.trim().split('\n')[0] ?? '';
  return flat.length > max ? `${flat.slice(0, max).trimEnd()}…` : flat;
}

function FileRow({ file }: { file: RewindFile }): JSX.Element {
  const action = file.removes_file ? 'Remove' : 'Restore';
  return (
    <li class={file.incomplete ? 'rewind-file is-incomplete' : 'rewind-file'}>
      <span class={`rewind-file-action ${file.removes_file ? 'is-remove' : 'is-restore'}`}>
        {action}
      </span>
      <code class="rewind-file-path">{file.path}</code>
      <span class="rewind-file-meta">
        {file.changes === 1 ? '1 change' : `${file.changes} changes`}
        {file.tools.length > 0 ? ` · ${file.tools.join(', ')}` : ''}
      </span>
      {file.incomplete ? (
        <span class="rewind-file-warn">
          No snapshot of the original was kept (too large or not text), so this file
          cannot be put back exactly.
        </span>
      ) : null}
    </li>
  );
}

export function RewindDialog({ loadPreview, apply }: RewindDialogProps): JSX.Element | null {
  const target = store.rewindTarget.value;
  const [preview, setPreview] = useState<RewindPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [mode, setMode] = useState<RewindMode>('none');
  const [revert, setRevert] = useState(false);
  const [note, setNote] = useState('');
  const [status, setStatus] = useState<Status>('idle');
  const [error, setError] = useState<string | null>(null);
  // Why the preview could not be read (a rejected request, not a rejected
  // rewind). Kept apart from `error` so the message sits next to the count it
  // replaces instead of under the submit button.
  const [previewError, setPreviewError] = useState<string | null>(null);

  const messageId = target?.messageId ?? null;

  // Reset the choices whenever the dialog opens for a different message: a
  // "revert" left ticked from the previous rewind is a destructive default
  // nobody asked for.
  useEffect(() => {
    if (messageId === null) {
      setMode('none');
      setRevert(false);
      setNote('');
      setPreview(null);
      setStatus('idle');
      setError(null);
      setPreviewError(null);
      return;
    }
    setMode('none');
    setRevert(false);
    setNote('');
    setError(null);
    setPreviewError(null);
    setStatus('idle');
    let cancelled = false;
    setLoading(true);
    const load = loadPreview ?? actions.loadRewindPreview;
    void load()
      .then((result) => {
        if (!cancelled) setPreview(result);
      })
      // A rejected preview (`GET /api/sessions/:id/rewind` 500s, the session was
      // deleted under the dialog, the network went away) used to be an unhandled
      // rejection: the count stayed on "Working out what this will change…" for
      // ever and the dialog offered no explanation. The reader still gets the
      // transcript half of the choice, so this is a line of text, not a dead end.
      .catch((cause: unknown) => {
        if (cancelled) return;
        setPreviewError(
          cause instanceof Error && cause.message
            ? `Could not read what this rewind would change: ${cause.message}`
            : 'Could not read what this rewind would change. The conversation part can still be applied.',
        );
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [messageId, loadPreview]);

  if (!target) return null;

  const files = preview?.files ?? [];
  const previewFailed = preview === null && !loading;
  const nothingToRevert = preview !== null && files.length === 0;
  const opaque = preview?.opaque_tools ?? [];
  const willReplace = mode !== 'none';
  const canSubmit =
    preview !== null &&
    status !== 'running' &&
    !loading &&
    (mode !== 'note' || note.trim() !== '');

  const submit = async (): Promise<void> => {
    if (!canSubmit) return;
    setStatus('running');
    setError(null);
    const run = apply ?? actions.applyRewind;
    try {
      const ok = await run({
        mode,
        // Never claim a revert that has nothing to do: the server would accept
        // it, and the toast would say "0 files restored" for no reason.
        revert: revert && !nothingToRevert,
        ...(mode === 'note' ? { note } : {}),
      });
      if (!ok) {
        setStatus('idle');
        setError('The rewind was not applied. The transcript is unchanged.');
        return;
      }
      actions.closeRewind();
      setStatus('idle');
    } catch (e) {
      setStatus('error');
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const heading = target.isAssistant
    ? 'Recall to your request'
    : 'Recall to this message';
  const summary = oneLine(target.preview);

  // One close path for the X, the backdrop, Escape (which `useFocusTrap` routes
  // here too) and the Cancel button. The destructive submit already disabled its
  // own Cancel while it runs; every other dismissal route ignored that and closed
  // the dialog mid-rewind, leaving the run to finish against a target the reader
  // could no longer see. A rewind is short, so refusing to dismiss while it is in
  // flight is the consistent reading of the existing contract.
  const requestClose = (): void => {
    if (status === 'running') return;
    actions.closeRewind();
  };

  return (
    <Dialog
      open
      title={heading}
      onClose={requestClose}
      class="dialog-rewind"
      description={
        target.isAssistant
          ? 'The agent’s answer and everything after it is removed from the conversation, and the request that produced it is re-sent.'
          : 'This message and everything after it is removed from the conversation.'
      }
    >
      <div class="rewind-body">
        <blockquote class="rewind-target">{summary || '(empty message)'}</blockquote>

        <p
          class={previewFailed ? 'rewind-count rewind-error' : 'rewind-count'}
          role={previewFailed ? 'alert' : undefined}
        >
          {preview === null
            ? loading
              ? 'Working out what this will change…'
              : 'The rewind preview could not be loaded. Close this dialog and try again before rewinding.'
            : preview.messages_removed === 0
              ? 'Nothing after this message: the agent will just answer it again.'
              : `${preview.messages_removed} message${
                  preview.messages_removed === 1 ? '' : 's'
                } after this one will be removed.`}
        </p>

        {previewError ? (
          <p class="rewind-error" role="alert" data-testid="rewind-preview-error">
            {previewError}
          </p>
        ) : null}

        <fieldset class="rewind-modes">
          <legend>What should happen next?</legend>
          <label class={mode === 'none' ? 'rewind-mode is-checked' : 'rewind-mode'}>
            <input
              type="radio"
              name="rewind-mode"
              value="none"
              checked={mode === 'none'}
              onChange={() => setMode('none')}
            />
            <span class="rewind-mode-title">Just remove it</span>
            <span class="rewind-mode-hint">
              The conversation ends at this message. Edit it yourself, or send something else.
            </span>
          </label>
          <label class={mode === 'retry' ? 'rewind-mode is-checked' : 'rewind-mode'}>
            <input
              type="radio"
              name="rewind-mode"
              value="retry"
              checked={mode === 'retry'}
              onChange={() => setMode('retry')}
            />
            <span class="rewind-mode-title">Ask again</span>
            <span class="rewind-mode-hint">
              The same request goes back to the agent, from a clean conversation.
            </span>
          </label>
          <label class={mode === 'note' ? 'rewind-mode is-checked' : 'rewind-mode'}>
            <input
              type="radio"
              name="rewind-mode"
              value="note"
              checked={mode === 'note'}
              onChange={() => setMode('note')}
            />
            <span class="rewind-mode-title">Send a note instead</span>
            <span class="rewind-mode-hint">
              Replace this request with a different instruction.
            </span>
          </label>
        </fieldset>

        {mode === 'note' ? (
          <label class="rewind-note">
            <span class="rewind-note-label">Your note</span>
            <textarea
              class="input rewind-note-input"
              rows={3}
              value={note}
              placeholder="Actually, do it this way…"
              data-autofocus
              onInput={(event) => setNote(event.currentTarget.value)}
            />
          </label>
        ) : null}

        <fieldset class="rewind-revert">
          <legend>File changes made after this message</legend>
          {preview === null ? (
            <p class="rewind-empty">
              {loading
                ? 'Checking which file changes could be put back…'
                : 'Whether any files changed is unknown. Rewind stays disabled until the preview can be loaded.'}
            </p>
          ) : nothingToRevert ? (
            <p class="rewind-empty">
              No file was changed after this message, so there is nothing to put back.
            </p>
          ) : (
            <>
              <label class="rewind-toggle">
                <input
                  type="checkbox"
                  checked={revert}
                  onChange={(event) => setRevert(event.currentTarget.checked)}
                />
                <span>
                  Revert {fileCount(files.length)} to how they were before
                </span>
              </label>
              {revert ? (
                <ul class="rewind-files">
                  {files.map((file) => (
                    <FileRow key={file.path} file={file} />
                  ))}
                </ul>
              ) : (
                <p class="rewind-empty">
                  Left unticked, the changes stay on disk and only the conversation is
                  rewound.
                </p>
              )}
            </>
          )}
          {preview?.incomplete ? (
            <p class="rewind-warn" role="status">
              Some files changed after this message have no saved copy of their previous
              contents, so reverting cannot restore them exactly.
            </p>
          ) : null}
          {opaque.length > 0 ? (
            <p class="rewind-warn" role="status">
              This turn also ran {opaque.join(', ')}, whose effects (processes, generated
              files, network calls) cannot be undone.
            </p>
          ) : null}
        </fieldset>

        {error ? (
          <p class="rewind-error" role="alert">
            {error}
          </p>
        ) : null}
      </div>

      <footer class="dialog-actions">
        <button
          type="button"
          class="button button-small"
          onClick={requestClose}
          disabled={status === 'running'}
        >
          Cancel
        </button>
        <button
          type="button"
          class="button button-small button-primary"
          disabled={!canSubmit}
          onClick={() => void submit()}
        >
          {status === 'running'
            ? 'Rewinding…'
            : willReplace
              ? mode === 'retry'
                ? 'Rewind and ask again'
                : 'Rewind and send note'
              : revert && !nothingToRevert
                ? 'Rewind and revert files'
                : 'Rewind'}
        </button>
      </footer>
    </Dialog>
  );
}
