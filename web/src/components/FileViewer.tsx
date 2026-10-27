import type { JSX } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import { getWorkspaceFile } from '../api/endpoints';
import type { WorkspaceFile } from '../api/types';
import { copyText } from '../lib/clipboard';
import { formatBytes } from '../lib/format';
import { highlightCode, languageLabel, MAX_FILE_HIGHLIGHT_CHARS } from '../lib/highlight';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import { CopyIcon, RefreshIcon } from './icons';

type LoadState = 'loading' | 'ready' | 'error';

export function FileViewer(): JSX.Element | null {
  const path = store.fileViewerPath.value;
  const [file, setFile] = useState<WorkspaceFile | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [retryKey, setRetryKey] = useState(0);

  useEffect(() => {
    if (!path) return;
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setFile(null);
    setCopied(false);
    getWorkspaceFile(path, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setFile(result);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [path, retryKey]);

  const highlighted = useMemo(() => {
    if (!file || file.binary) return '';
    return highlightCode(file.content, file.language, MAX_FILE_HIGHLIGHT_CHARS);
  }, [file]);

  if (!path) return null;

  const lineCount = file && !file.binary ? file.content.split('\n').length : 0;

  return (
    <Dialog
      open
      title={path}
      onClose={actions.closeFileViewer}
      class="dialog-file"
    >
      <div class="file-viewer">
        <div class="file-toolbar">
          <div class="file-meta">
            {file ? (
              <>
                <span class="badge badge-muted">{languageLabel(file.language)}</span>
                <span class="file-size">{formatBytes(file.size)}</span>
                {file.truncated ? <span class="badge badge-warn">truncated by server</span> : null}
                {file.binary ? <span class="badge badge-warn">binary</span> : null}
              </>
            ) : (
              <span class="muted small">Reading file…</span>
            )}
          </div>
          <div class="file-toolbar-actions">
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
              title="Reload file"
              onClick={() => setRetryKey((key) => key + 1)}
            >
              <RefreshIcon class={state === 'loading' ? 'spin' : undefined} />
            </button>
          </div>
        </div>

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
            <button type="button" class="button button-small" onClick={() => setRetryKey((key) => key + 1)}>
              Retry
            </button>
          </div>
        ) : null}

        {state === 'ready' && file ? (
          file.binary ? (
            <p class="file-notice">This looks like a binary file, so no text preview is available.</p>
          ) : (
            <>
              {file.truncated ? (
                <p class="file-notice">
                  The server truncated this file at {formatBytes(file.size)}. Only the beginning is shown.
                </p>
              ) : null}
              <div class="file-code-scroll">
                <div class="file-gutter" aria-hidden="true">
                  {Array.from({ length: lineCount }, (_, index) => (
                    <span key={index}>{index + 1}</span>
                  ))}
                </div>
                <pre class="file-code">
                  <code
                    class="hljs"
                    dangerouslySetInnerHTML={{ __html: highlighted }}
                  />
                </pre>
              </div>
            </>
          )
        ) : null}
      </div>
    </Dialog>
  );
}