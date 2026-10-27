import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import { getGitDiff, getGitStatus } from '../api/endpoints';
import type { GitDiff, GitFile, GitStatus } from '../api/types';
import { DiffView } from './DiffView';
import { GitIcon, RefreshIcon } from './icons';

interface SelectedFile {
  path: string;
  staged: boolean;
}

type LoadState = 'loading' | 'ready' | 'error';

function statusLabel(status: string): string {
  switch (status) {
    case 'M':
      return 'modified';
    case 'A':
      return 'added';
    case 'D':
      return 'deleted';
    case 'R':
      return 'renamed';
    case '?':
      return 'untracked';
    case '??':
      return 'untracked';
    case 'U':
      return 'conflict';
    default:
      return status || 'changed';
  }
}

function FileGroup({
  label,
  files,
  selected,
  onSelect,
}: {
  label: string;
  files: GitFile[];
  selected: SelectedFile | null;
  onSelect: (file: GitFile) => void;
}): JSX.Element | null {
  if (files.length === 0) return null;
  return (
    <section class="git-group" aria-label={label}>
      <h3 class="git-group-title">
        {label} <span class="git-count">{files.length}</span>
      </h3>
      <ul class="git-files">
        {files.map((file) => {
          const active = selected?.path === file.path && selected.staged === file.staged;
          return (
            <li key={`${file.staged ? 'staged' : 'unstaged'}:${file.path}`}>
              <button
                type="button"
                class={`git-file${active ? ' is-active' : ''}`}
                aria-pressed={active}
                onClick={() => onSelect(file)}
                title={`${statusLabel(file.status)} · ${file.path}`}
              >
                <span class={`git-status status-${file.status === 'A' ? 'ok' : file.status === 'D' ? 'danger' : 'warn'}`}>
                  {file.status || '?'}
                </span>
                <span class="git-path">{file.path}</span>
                <span class="git-status-label">{statusLabel(file.status)}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

export function ChangesTab(): JSX.Element {
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [selected, setSelected] = useState<SelectedFile | null>(null);
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [diffState, setDiffState] = useState<LoadState>('loading');
  const [diffError, setDiffError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setSelected(null);
    setDiff(null);
    getGitStatus({ signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setStatus(result);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [refreshKey]);

  useEffect(() => {
    if (!selected) {
      setDiff(null);
      return;
    }
    const controller = new AbortController();
    setDiffState('loading');
    setDiffError(null);
    getGitDiff(selected.path, selected.staged, undefined, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setDiff(result);
        setDiffState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setDiffError(toErrorMessage(cause));
        setDiffState('error');
      });
    return () => controller.abort();
  }, [selected]);

  const staged = status?.files.filter((file) => file.staged) ?? [];
  const unstaged = status?.files.filter((file) => !file.staged) ?? [];

  return (
    <div class="inspector-section">
      <div class="section-toolbar">
        <div class="section-heading">
          <span class="eyebrow">Changes</span>
          {status?.is_repo ? (
            <span class="section-path">
              <GitIcon size={12} /> {status.branch ?? 'detached HEAD'}
            </span>
          ) : null}
        </div>
        <button
          type="button"
          class="icon-button"
          aria-label="Refresh git status"
          title="Refresh git status"
          disabled={state === 'loading'}
          onClick={() => setRefreshKey((key) => key + 1)}
        >
          <RefreshIcon class={state === 'loading' ? 'spin' : undefined} />
        </button>
      </div>

      {state === 'loading' ? (
        <div class="git-loading" aria-busy="true" aria-label="Loading git status">
          {[0, 1, 2, 3].map((index) => (
            <div class="skeleton skeleton-git-row" key={index} />
          ))}
        </div>
      ) : null}

      {state === 'error' ? (
        <div class="inline-error" role="alert">
          <p>{error}</p>
          <button type="button" class="button button-small" onClick={() => setRefreshKey((key) => key + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      {state === 'ready' && status ? (
        !status.is_repo ? (
          <div class="empty-inspector">
            <p class="empty-inspector-title">
              {status.error ? 'Git unavailable' : 'Not a git repository'}
            </p>
            <p>
              {status.error
                ? 'The git status probe failed. The git panel and git tools will stay unavailable until it works.'
                : 'This workspace is not tracked by git, so there is no status or diff to show. Git tools in the agent will report the same.'}
            </p>
            {status.error ? <p class="muted small">{status.error}</p> : null}
          </div>
        ) : (
          <>
            {status.error ? (
              <div class="inline-error" role="status">
                <p>{status.error}</p>
              </div>
            ) : null}
            {status.files.length === 0 ? (
              <div class="empty-inspector">
                <p class="empty-inspector-title">Working tree clean</p>
                <p>No staged or unstaged changes on {status.branch ?? 'the current branch'}.</p>
              </div>
            ) : null}
            <FileGroup
              label="Staged"
              files={staged}
              selected={selected}
              onSelect={(file) => setSelected({ path: file.path, staged: file.staged })}
            />
            <FileGroup
              label="Unstaged"
              files={unstaged}
              selected={selected}
              onSelect={(file) => setSelected({ path: file.path, staged: file.staged })}
            />

            {selected ? (
              <section class="git-diff" aria-label={`Diff for ${selected.path}`}>
                <div class="git-diff-head">
                  <span class="git-diff-path" title={selected.path}>
                    {selected.path}
                  </span>
                  <span class="badge badge-muted">{selected.staged ? 'staged' : 'unstaged'}</span>
                  <button
                    type="button"
                    class="link-button"
                    onClick={() => setSelected(null)}
                    aria-label="Close diff"
                  >
                    Close
                  </button>
                </div>
                {diffState === 'loading' ? (
                  <div class="skeleton skeleton-diff" aria-busy="true" aria-label="Loading diff" />
                ) : null}
                {diffState === 'error' ? (
                  <div class="inline-error" role="alert">
                    <p>{diffError}</p>
                    <button
                      type="button"
                      class="button button-small"
                      onClick={() => setSelected({ ...selected })}
                    >
                      Retry
                    </button>
                  </div>
                ) : null}
                {diffState === 'ready' && diff ? <DiffView diff={diff.diff} truncated={diff.truncated} /> : null}
              </section>
            ) : null}
          </>
        )
      ) : null}

      {state === 'ready' && status?.is_repo ? (
        <p class="inspector-note">Diffs are read-only. Files on disk may change while the agent works.</p>
      ) : null}
    </div>
  );
}