import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import { getGitDiff, getGitStatus } from '../api/endpoints';
import type { GitDiff, GitFile, GitStatus } from '../api/types';
import { pluralise } from '../lib/format';
import * as store from '../state/store';
import { DiffView } from './DiffView';
import {
  AlertIcon,
  ChevronDownIcon,
  FileCodeIcon,
  GitIcon,
  RefreshIcon,
} from './icons';

interface SelectedFile {
  path: string;
  staged: boolean;
}

type LoadState = 'loading' | 'ready' | 'error';

/**
 * `panel` is the inspector tab (the mock's `.inspect-section` working tree);
 * `page` is the full-width Changes view (the mock's `.secondary-panel`).
 */
export type ChangesTabVariant = 'panel' | 'page';

export interface ChangesTabProps {
  variant?: ChangesTabVariant;
}

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
    case '??':
      return 'untracked';
    case 'U':
      return 'conflict';
    default:
      return status || 'changed';
  }
}

function statusClass(status: string): 'ok' | 'danger' | 'warn' {
  if (status === 'A' || status === '??') return 'ok';
  if (status === 'D' || status === 'U') return 'danger';
  return 'warn';
}

/** The mock prints a two-letter extension tile for each changed file. */
function splitPath(path: string): { name: string; dir: string; ext: string } {
  const slash = path.lastIndexOf('/');
  const name = slash === -1 ? path : path.slice(slash + 1);
  const dir = slash === -1 ? '' : path.slice(0, slash);
  const dot = name.lastIndexOf('.');
  const ext = dot <= 0 ? 'txt' : name.slice(dot + 1);
  return { name, dir, ext };
}

function StatusCell({ file }: { file: GitFile }): JSX.Element {
  return (
    <span class="change-status">
      <span class={`git-status status-${statusClass(file.status)}`} aria-hidden="true">
        {file.status || '?'}
      </span>
      <span class="change-status-label">
        {statusLabel(file.status)}
        {file.staged ? ' · staged' : ''}
      </span>
    </span>
  );
}

/** One line of the mock's `.change-row`: logo, name + directory, status. */
function ChangeRow({
  file,
  active,
  onSelect,
}: {
  file: GitFile;
  active: boolean;
  onSelect: (file: GitFile) => void;
}): JSX.Element {
  const { name, dir, ext } = splitPath(file.path);
  return (
    <li>
      <button
        type="button"
        class={`change-row${active ? ' is-active' : ''}`}
        aria-pressed={active}
        onClick={() => onSelect(file)}
        title={`${statusLabel(file.status)} · ${file.path}`}
      >
        <span class="file-logo" aria-hidden="true">
          {ext.slice(0, 2)}
        </span>
        <span class="change-desc">
          <span class="change-name">{name}</span>
          <span class="change-dir">{dir || 'workspace root'}</span>
        </span>
        <StatusCell file={file} />
      </button>
    </li>
  );
}

/** Lazy diff body shared by both variants; it renders its own loading/error states. */
function DiffBody({
  path,
  diff,
  state,
  error,
  onRetry,
}: {
  path: string;
  diff: GitDiff | null;
  state: LoadState;
  error: string | null;
  onRetry: () => void;
}): JSX.Element {
  if (state === 'loading') {
    return <div class="skeleton skeleton-diff" aria-busy="true" aria-label={`Loading diff for ${path}`} />;
  }
  if (state === 'error') {
    return (
      <div class="inline-error" role="alert">
        <p>{error}</p>
        <button type="button" class="button button-small" onClick={onRetry}>
          Retry
        </button>
      </div>
    );
  }
  if (diff) return <DiffView diff={diff.diff} truncated={diff.truncated} />;
  return <p class="inspector-note">No diff to show for this file.</p>;
}

export function ChangesTab({ variant = 'panel' }: ChangesTabProps): JSX.Element {
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
    getGitStatus(store.currentProjectId.value, { signal: controller.signal })
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
    getGitDiff(selected.path, selected.staged, undefined, store.currentProjectId.value, {
      signal: controller.signal,
    })
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

  const files = status?.files ?? [];
  const staged = files.filter((file) => file.staged).length;
  const unstaged = files.length - staged;
  const branch = status?.branch ?? null;
  const refresh = (): void => setRefreshKey((key) => key + 1);
  const select = (file: GitFile): void => {
    const next = { path: file.path, staged: file.staged };
    const same = selected?.path === next.path && selected.staged === next.staged;
    setSelected(same ? null : next);
  };

  const loadingBlock =
    state === 'loading' ? (
      <div class="git-loading" aria-busy="true" aria-label="Loading git status">
        {[0, 1, 2, 3].map((index) => (
          <div class="skeleton skeleton-git-row" key={index} />
        ))}
      </div>
    ) : null;

  const errorBlock =
    state === 'error' ? (
      <div class="inline-error" role="alert">
        <p>{error}</p>
        <button type="button" class="button button-small" onClick={refresh}>
          Retry
        </button>
      </div>
    ) : null;

  const notRepoBlock =
    state === 'ready' && status && !status.is_repo ? (
      <div class="empty-inspector">
        <p class="empty-inspector-title">{status.error ? 'Git unavailable' : 'Not a git repository'}</p>
        <p>
          {status.error
            ? 'The git status probe failed. The git panel and git tools will stay unavailable until it works.'
            : 'This workspace is not tracked by git, so there is no status or diff to show. Git tools in the agent will report the same.'}
        </p>
        {status.error ? <p class="muted small">{status.error}</p> : null}
      </div>
    ) : null;

  const cleanBlock =
    state === 'ready' && status?.is_repo && files.length === 0 ? (
      <div class="empty-inspector">
        <p class="empty-inspector-title">Working tree clean</p>
        <p>No staged or unstaged changes on {branch ?? 'the current branch'}.</p>
      </div>
    ) : null;

  const statusWarning =
    status?.is_repo && status.error ? (
      <div class="inline-error" role="status">
        <p>{status.error}</p>
      </div>
    ) : null;

  if (variant === 'page') {
    return (
      <div class="secondary-panel">
        <p class="panel-kicker">Source control</p>
        <h2 class="panel-title">Review changes</h2>
        <p class="panel-desc">
          {state === 'ready' && status?.is_repo
            ? `${pluralise(files.length, 'modified file')} in `
            : 'Review what the agent changed in '}
          {branch ? <code class="inline-path">{branch}</code> : 'this workspace'}. Diffs are read-only —
          staging and commits happen in the terminal.
        </p>

        {state === 'ready' && status?.is_repo ? (
          <div class="panel-actions">
            <span class="tag">
              <GitIcon size={13} />
              {files.length === 0 ? 'Clean working tree' : `${files.length} modified`}
              {staged > 0 ? ` · ${staged} staged` : ''}
              {unstaged > 0 ? ` · ${unstaged} unstaged` : ''}
            </span>
            <button
              type="button"
              class="panel-action-btn"
              onClick={refresh}
              disabled={state !== 'ready'}
            >
              <RefreshIcon size={13} /> Refresh status
            </button>
          </div>
        ) : null}

        {loadingBlock}
        {errorBlock}
        {notRepoBlock}
        {statusWarning}
        {cleanBlock}

        {state === 'ready' && status?.is_repo ? (
          <div class="panel-change-list">
            {files.map((file) => {
              const open = selected?.path === file.path && selected.staged === file.staged;
              return (
                <section
                  class={`panel-change-file${open ? ' is-open' : ''}`}
                  key={`${file.staged ? 'staged' : 'unstaged'}:${file.path}`}
                  aria-label={file.path}
                >
                  <button
                    type="button"
                    class="panel-file-title"
                    aria-expanded={open}
                    onClick={() => select(file)}
                  >
                    <FileCodeIcon size={15} />
                    <span class="panel-file-path" title={file.path}>
                      {file.path}
                    </span>
                    <StatusCell file={file} />
                    <span class={`panel-file-chevron${open ? ' is-open' : ''}`} aria-hidden="true">
                      <ChevronDownIcon size={13} />
                    </span>
                  </button>
                  {open ? (
                    <div class="panel-file-diff">
                      <DiffBody
                        path={file.path}
                        diff={diff}
                        state={diffState}
                        error={diffError}
                        onRetry={() => setSelected({ path: file.path, staged: file.staged })}
                      />
                    </div>
                  ) : null}
                </section>
              );
            })}
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div class="inspector-section">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Working tree</span>
        <button
          type="button"
          class="icon-button inspector-mini"
          aria-label="Refresh git status"
          title="Refresh git status"
          disabled={state === 'loading'}
          onClick={refresh}
        >
          <RefreshIcon size={13} class={state === 'loading' ? 'spin' : undefined} />
        </button>
      </div>

      {status?.is_repo && branch ? (
        <div class="workspace-path" title={branch}>
          <span class="path-icon" aria-hidden="true">
            <GitIcon size={13} />
          </span>
          <span>{branch}</span>
        </div>
      ) : null}

      {loadingBlock}
      {errorBlock}
      {notRepoBlock}
      {statusWarning}

      {state === 'ready' && status?.is_repo ? (
        <>
          <div class="change-summary">
            <div class="metric">
              <div class="metric-num">{files.length}</div>
              <div class="metric-label">Modified files</div>
            </div>
            <div class="metric">
              <div class="metric-num">
                {staged}
                <span class="metric-sub"> / {files.length}</span>
              </div>
              <div class="metric-label">Staged</div>
            </div>
          </div>
        </>
      ) : null}

      {cleanBlock}

      {state === 'ready' && status?.is_repo && files.length > 0 ? (
        <>
          <div class="inspector-kicker-row">
            <span class="inspector-kicker">Changed files</span>
            <span class="inspector-kicker-note">
              {staged > 0 ? `${staged} staged` : 'none staged'}
            </span>
          </div>
          <ul class="change-list">
            {files.map((file) => {
              const active = selected?.path === file.path && selected.staged === file.staged;
              return <ChangeRow key={`${file.staged ? 'staged' : 'unstaged'}:${file.path}`} file={file} active={active} onSelect={select} />;
            })}
          </ul>
        </>
      ) : null}

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
          <DiffBody
            path={selected.path}
            diff={diff}
            state={diffState}
            error={diffError}
            onRetry={() => setSelected({ path: selected.path, staged: selected.staged })}
          />
        </section>
      ) : null}

      {state === 'ready' && status?.is_repo ? (
        <p class="inspector-note">
          <AlertIcon size={12} /> Diffs are read-only. Files on disk may change while the agent works.
        </p>
      ) : null}
    </div>
  );
}
