import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import { getGitCommit, getGitDiff, getGitStashDetail } from '../../api/endpoints';
import type { GitCommitDetail, GitDiff, GitFile, GitStashDetail } from '../../api/types';
import { relativeTime } from '../../lib/format';
import { DiffView } from '../DiffView';
import { AlertIcon, FileCodeIcon, MinusIcon, PlusIcon, UndoIcon } from '../icons';
import {
  type GitSelection,
  isConflict,
  porcelainCode,
  statusLabel,
  statusTone,
} from './gitModel';

type LoadState = 'loading' | 'ready' | 'error';

function useGitFileDiff(
  path: string | null,
  staged: boolean,
  projectId: string | null,
  reloadKey: number,
): { diff: GitDiff | null; state: LoadState; error: string | null; retry: () => void } {
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (path === null) {
      setDiff(null);
      setState('ready');
      return;
    }
    const controller = new AbortController();
    setState('loading');
    setError(null);
    getGitDiff(path, staged, undefined, projectId, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setDiff(result);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [path, staged, projectId, reloadKey, attempt]);

  return { diff, state, error, retry: () => setAttempt((n) => n + 1) };
}

function useGitCommit(
  sha: string | null,
  projectId: string | null,
): {
  commit: GitCommitDetail | null;
  state: LoadState;
  error: string | null;
  retry: () => void;
} {
  const [commit, setCommit] = useState<GitCommitDetail | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (sha === null) {
      setCommit(null);
      setState('ready');
      return;
    }
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setCommit(null);
    getGitCommit(sha, projectId, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setCommit(result);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [sha, projectId, attempt]);

  return { commit, state, error, retry: () => setAttempt((n) => n + 1) };
}

function useGitStash(
  index: number | null,
  projectId: string | null,
): {
  stash: GitStashDetail | null;
  state: LoadState;
  error: string | null;
  retry: () => void;
} {
  const [stash, setStash] = useState<GitStashDetail | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (index === null) {
      setStash(null);
      setState('ready');
      return;
    }
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setStash(null);
    getGitStashDetail(index, projectId, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted) return;
        setStash(result);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [index, projectId, attempt]);

  return { stash, state, error, retry: () => setAttempt((n) => n + 1) };
}

function DiffSkeleton({ label }: { label: string }): JSX.Element {
  return (
    <div class="gitm-diff-loading" aria-busy="true" aria-label={label}>
      {[0, 1, 2, 3, 4, 5].map((line) => (
        <div class={`skeleton gitm-diff-skeleton${line % 3 === 0 ? ' is-wide' : ''}`} key={line} />
      ))}
    </div>
  );
}

function DiffError({ message, onRetry }: { message: string; onRetry: () => void }): JSX.Element {
  return (
    <div class="inline-error" role="alert">
      <p>
        <AlertIcon size={13} /> {message}
      </p>
      <button type="button" class="button button-small" onClick={onRetry}>
        Retry
      </button>
    </div>
  );
}

export interface GitDiffPaneProps {
  variant: 'page' | 'panel';
  selection: GitSelection | null;
  projectId: string | null;
  /** Bumped after a mutation so the open diff is re-read from the new tree. */
  reloadKey: number;
  busy: boolean;
  /** The status row for the selected path, for the header's badge and actions. */
  file: GitFile | null;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (file: GitFile, staged: boolean) => void;
  onResolveConflict?: (path: string, strategy: 'ours' | 'theirs') => void;
  onOpenFile?: (path: string) => void;
  onClear: () => void;
}

/**
 * The detail region: one file's patch, one commit, or one stash.
 *
 * All states load on demand and have their own retry, so a single failed
 * `git diff` does not take the change list down with it.
 */
export function GitDiffPane({
  variant,
  selection,
  projectId,
  reloadKey,
  busy,
  file,
  onStage,
  onUnstage,
  onDiscard,
  onResolveConflict,
  onOpenFile,
  onClear,
}: GitDiffPaneProps): JSX.Element {
  const filePath = selection?.kind === 'file' ? selection.path : null;
  const staged = selection?.kind === 'file' ? selection.staged : false;
  const sha = selection?.kind === 'commit' ? selection.sha : null;
  const stashIndex = selection?.kind === 'stash' ? selection.index : null;
  const fileDiff = useGitFileDiff(filePath, staged, projectId, reloadKey);
  const commit = useGitCommit(sha, projectId);
  const stash = useGitStash(stashIndex, projectId);

  if (selection === null) {
    return (
      <div class="gitm-detail-empty">
        <p class="gitm-empty-title">Nothing selected</p>
        <p class="gitm-empty-text">
          Pick a changed file to read its patch, or a commit in the history to review what
          landed.
        </p>
      </div>
    );
  }

  if (selection.kind === 'commit') {
    return (
      <div class={`gitm-detail gitm-detail-${variant}`}>
        <header class="gitm-detail-head">
          <div class="gitm-detail-titles">
            <h3 class="gitm-detail-title">{selection.subject || '(no message)'}</h3>
            <p class="gitm-detail-sub">
              <code class="gitm-sha">{sha}</code> ·{' '}
              {commit.commit ? `${commit.commit.author} · ${relativeTime(commit.commit.timestamp * 1000)}` : 'commit'}
            </p>
          </div>
          <button type="button" class="link-button" onClick={onClear}>
            Close
          </button>
        </header>
        {commit.state === 'loading' ? (
          <DiffSkeleton label="Loading the commit" />
        ) : commit.state === 'error' ? (
          <DiffError message={commit.error ?? 'Could not load this commit'} onRetry={commit.retry} />
        ) : commit.commit ? (
          <>
            {commit.commit.body ? (
              <pre class="gitm-commit-body">{commit.commit.body}</pre>
            ) : null}
            <ul class="gitm-commit-files" aria-label="Files changed in this commit">
              {commit.commit.files.map((changed) => (
                <li class="gitm-commit-file" key={changed.path}>
                  <span class="gitm-commit-file-path">{changed.path}</span>
                  <span class="gitm-commit-file-stat">
                    {changed.binary ? (
                      <em>binary</em>
                    ) : (
                      <>
                        <span class="is-add">+{changed.additions ?? 0}</span>{' '}
                        <span class="is-del">−{changed.deletions ?? 0}</span>
                      </>
                    )}
                  </span>
                </li>
              ))}
            </ul>
            <DiffView
              diff={commit.commit.diff}
              truncated={commit.commit.truncated}
              fileCount={commit.commit.files.length}
            />
          </>
        ) : null}
      </div>
    );
  }

  if (selection.kind === 'stash') {
    return (
      <div class={`gitm-detail gitm-detail-${variant}`}>
        <header class="gitm-detail-head">
          <div class="gitm-detail-titles">
            <h3 class="gitm-detail-title">{selection.ref}</h3>
            <p class="gitm-detail-sub">{selection.subject}</p>
          </div>
          <button type="button" class="link-button" onClick={onClear}>
            Close
          </button>
        </header>
        {stash.state === 'loading' ? (
          <DiffSkeleton label={`Loading ${selection.ref}`} />
        ) : stash.state === 'error' ? (
          <DiffError message={stash.error ?? 'Could not load this stash'} onRetry={stash.retry} />
        ) : stash.stash ? (
          <>
            <ul class="gitm-commit-files" aria-label="Files in this stash">
              {stash.stash.files.map((changed) => (
                <li class="gitm-commit-file" key={changed.path}>
                  <span class="gitm-commit-file-path">{changed.path}</span>
                  <span class="gitm-commit-file-stat">
                    {changed.binary ? (
                      <em>binary</em>
                    ) : (
                      <>
                        <span class="is-add">+{changed.additions ?? 0}</span>{' '}
                        <span class="is-del">−{changed.deletions ?? 0}</span>
                      </>
                    )}
                  </span>
                </li>
              ))}
            </ul>
            <DiffView
              diff={stash.stash.diff}
              truncated={stash.stash.truncated}
              fileCount={stash.stash.files.length}
            />
          </>
        ) : null}
      </div>
    );
  }

  const tone = file ? statusTone(file) : 'warn';
  const conflicted = file ? isConflict(file) : false;
  return (
    <div class={`gitm-detail gitm-detail-${variant}`}>
      <header class="gitm-detail-head">
        <div class="gitm-detail-titles">
          <h3 class="gitm-detail-title" title={selection.path}>
            {selection.path}
          </h3>
          <p class="gitm-detail-sub">
            {file ? (
              <>
                <span class={`gitm-badge is-${tone}`} aria-hidden="true">
                  {porcelainCode(file).trim()}
                </span>{' '}
                {statusLabel(file)} · {staged ? 'staged' : 'working tree'}
              </>
            ) : (
              staged ? 'staged' : 'working tree'
            )}
          </p>
        </div>
        <div class="gitm-detail-actions">
          {onOpenFile ? (
            <button
              type="button"
              class="button button-small"
              onClick={() => onOpenFile(selection.path)}
              title="Open file in workspace tab"
            >
              <FileCodeIcon size={13} /> Open file
            </button>
          ) : null}
          {conflicted && onResolveConflict ? (
            <>
              <button
                type="button"
                class="button button-small"
                disabled={busy}
                onClick={() => onResolveConflict(selection.path, 'ours')}
              >
                Accept ours
              </button>
              <button
                type="button"
                class="button button-small"
                disabled={busy}
                onClick={() => onResolveConflict(selection.path, 'theirs')}
              >
                Accept theirs
              </button>
            </>
          ) : null}
          {file ? (
            <>
              {staged ? (
                <button
                  type="button"
                  class="button button-small"
                  disabled={busy}
                  onClick={() => onUnstage([selection.path])}
                >
                  <MinusIcon size={13} /> Unstage
                </button>
              ) : (
                <button
                  type="button"
                  class="button button-small"
                  disabled={busy}
                  onClick={() => onStage([selection.path])}
                >
                  <PlusIcon size={13} /> Stage
                </button>
              )}
              <button
                type="button"
                class="button button-small button-danger-ghost"
                disabled={busy}
                onClick={() => onDiscard(file, staged)}
              >
                <UndoIcon size={13} /> Discard
              </button>
            </>
          ) : null}
          <button type="button" class="link-button" onClick={onClear}>
            Close
          </button>
        </div>
      </header>

      {fileDiff.state === 'loading' ? (
        <DiffSkeleton label={`Loading the diff for ${selection.path}`} />
      ) : fileDiff.state === 'error' ? (
        <DiffError message={fileDiff.error ?? 'Could not load this diff'} onRetry={fileDiff.retry} />
      ) : fileDiff.diff ? (
        <DiffView diff={fileDiff.diff.diff} truncated={fileDiff.diff.truncated} />
      ) : (
        <p class="gitm-note">No diff to show for this file.</p>
      )}
    </div>
  );
}