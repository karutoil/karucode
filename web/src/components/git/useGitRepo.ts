import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import {
  commitGitChanges,
  createGitBranch,
  checkoutGitBranch,
  deleteGitBranch,
  discardGitPaths,
  getGitBranches,
  getGitLog,
  getGitStashes,
  getGitStatus,
  runGitStash,
  stageGitPaths,
  unstageGitPaths,
  type GitStashAction,
} from '../../api/endpoints';
import type { GitBranch, GitCommit, GitStash, GitStatus } from '../../api/types';
import * as store from '../../state/store';

export type GitLoadState = 'loading' | 'ready' | 'error';

export interface GitRepo {
  /** The working tree, or `null` before the first response. */
  status: GitStatus | null;
  branches: GitBranch[];
  /** `null` until the history has been asked for (it is loaded lazily). */
  commits: GitCommit[] | null;
  stashes: GitStash[] | null;
  /**
   * Bumped every time a new status is applied. The diff pane watches it so an
   * open patch is re-read after a stage/discard instead of going stale.
   */
  revision: number;
  state: GitLoadState;
  /** The status probe's own failure (the panel shows it loudly). */
  error: string | null;
  /** The action in flight, e.g. `Staging 2 files`; `null` when idle. */
  busy: string | null;
  /** Why the last action failed; cleared by the next one. */
  actionError: string | null;
  /** Re-read status, branches and anything already loaded. */
  refresh: () => void;
  /** Load the history now (idempotent; also used when the section opens). */
  loadHistory: () => void;
  /** Load the stash list now (idempotent). */
  loadStashes: () => void;
  stage: (paths: string[] | 'all', label?: string) => Promise<boolean>;
  unstage: (paths: string[] | 'all', label?: string) => Promise<boolean>;
  discard: (paths: string[] | 'all', staged: boolean, label?: string) => Promise<boolean>;
  commit: (message: string, options?: { amend?: boolean }) => Promise<boolean>;
  checkout: (name: string) => Promise<boolean>;
  createBranch: (name: string, options?: { checkout?: boolean }) => Promise<boolean>;
  deleteBranch: (name: string, options?: { force?: boolean }) => Promise<boolean>;
  stash: (action: GitStashAction, options?: { index?: number; includeUntracked?: boolean }) => Promise<boolean>;
  clearActionError: () => void;
}

const HISTORY_LIMIT = 30;

/**
 * The source-control manager's state: the working tree, the branch list, the
 * history (on demand) and every mutation.
 *
 * Two rules make this safe to use from two mounted instances (the inspector tab
 * and the full-width view can both be alive at once):
 *
 * 1. every request is abortable and re-issued when the project changes, so a
 *    switch never leaves the previous repository's files on screen;
 * 2. a mutation's response *is* the new status - the server reads the tree after
 *    the command - so the panel applies it directly instead of racing a second
 *    GET against the very command it just ran.
 */
export function useGitRepo(projectId: string | null): GitRepo {
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [branches, setBranches] = useState<GitBranch[]>([]);
  const [commits, setCommits] = useState<GitCommit[] | null>(null);
  const [stashes, setStashes] = useState<GitStash[] | null>(null);
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<GitLoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  // Refs, not state: asking for the history must not itself trigger a second
  // effect pass, and the reload below has to see the latest answer.
  const historyWanted = useRef(false);
  const stashWanted = useRef(false);

  const loadHistory = useCallback((): void => {
    historyWanted.current = true;
    const controller = new AbortController();
    getGitLog(projectId, { signal: controller.signal, limit: HISTORY_LIMIT })
      .then((result) => setCommits(result.commits))
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        store.pushToast(`Could not read the history: ${toErrorMessage(cause)}`, 'error');
        setCommits([]);
      });
  }, [projectId]);

  const loadStashes = useCallback((): void => {
    stashWanted.current = true;
    const controller = new AbortController();
    getGitStashes(projectId, { signal: controller.signal })
      .then((result) => setStashes(result.stashes))
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        store.pushToast(`Could not read the stash list: ${toErrorMessage(cause)}`, 'error');
        setStashes([]);
      });
  }, [projectId]);

  // Which repository the state belongs to. Only a *change* drops the loaded
  // history and stash list; a refresh keeps them (and re-reads them below) so the
  // sections do not collapse every time the tree is re-probed.
  const loadedProject = useRef(projectId);
  const projectChanged = loadedProject.current !== projectId;
  loadedProject.current = projectId;

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    let live = true;
    setState((current) => (current === 'loading' ? 'loading' : 'ready'));
    setError(null);
    if (projectChanged) {
      setCommits(null);
      setStashes(null);
      historyWanted.current = false;
      stashWanted.current = false;
    }

    const statusJob = getGitStatus(projectId, { signal })
      .then((result) => {
        if (!live) return;
        setStatus(result);
        setRevision((current) => current + 1);
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (!live || signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    const branchJob = getGitBranches(projectId, { signal })
      .then((result) => {
        if (!live || signal.aborted) return;
        setBranches(result.is_repo ? result.branches : []);
      })
      .catch(() => {
        // The branch list is secondary: a status that works but a branch list
        // that fails must not blank the panel.
      });
    void Promise.all([statusJob, branchJob]);
    // Anything already on screen is re-read too: a stale commit list after a
    // commit, or a stash list after a stash, would be worse than a longer probe.
    if (historyWanted.current) loadHistory();
    if (stashWanted.current) loadStashes();
    return () => {
      live = false;
      controller.abort();
    };
  }, [projectId, refreshKey, loadHistory, loadStashes]);

  // The agent writes files while this window is in the background; coming back
  // should not show a change list that no longer exists.
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const onFocus = (): void => {
      if (document.visibilityState === 'visible') setRefreshKey((key) => key + 1);
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, []);

  const refresh = useCallback((): void => setRefreshKey((key) => key + 1), []);

  const run = useCallback(
    async (
      label: string,
      action: () => Promise<{ status: GitStatus; message: string | null; commit: unknown }>,
    ): Promise<boolean> => {
      setBusy(label);
      setActionError(null);
      try {
        const result = await action();
        setStatus(result.status);
        setRevision((current) => current + 1);
        setState('ready');
        if (result.message) store.pushToast(result.message, 'success');
        // A commit changes the history, a checkout changes the branch list; one
        // reload keeps both honest without the caller having to know which. It is
        // best effort, so it carries its own catch: a rejection here would be an
        // unhandled promise (the tests report one per action, and a browser logs
        // it), and the status above is already up to date either way.
        void getGitBranches(projectId)
          .then((list) => setBranches(list.is_repo ? list.branches : []))
          .catch(() => {
            // The previous list stands; the next refresh will try again.
          });
        if (historyWanted.current) loadHistory();
        if (stashWanted.current) loadStashes();
        return true;
      } catch (cause: unknown) {
        const message = toErrorMessage(cause);
        setActionError(message);
        store.pushToast(message, 'error');
        return false;
      } finally {
        setBusy(null);
      }
    },
    [projectId, loadHistory, loadStashes],
  );

  const stage = useCallback(
    (paths: string[] | 'all', label?: string): Promise<boolean> =>
      run(
        label ?? (paths === 'all' ? 'Staging every change' : `Staging ${paths.length} file${paths.length === 1 ? '' : 's'}`),
        () => stageGitPaths(paths === 'all' ? { all: true } : { paths }, projectId),
      ),
    [projectId, run],
  );

  const unstage = useCallback(
    (paths: string[] | 'all', label?: string): Promise<boolean> =>
      run(
        label ?? (paths === 'all' ? 'Unstaging everything' : `Unstaging ${paths.length} file${paths.length === 1 ? '' : 's'}`),
        () => unstageGitPaths(paths === 'all' ? { all: true } : { paths }, projectId),
      ),
    [projectId, run],
  );

  const discard = useCallback(
    (paths: string[] | 'all', staged: boolean, label?: string): Promise<boolean> =>
      run(label ?? 'Discarding changes', () =>
        discardGitPaths(
          {
            ...(paths === 'all' ? { all: true } : { paths }),
            staged,
            confirm: true,
          },
          projectId,
        ),
      ),
    [projectId, run],
  );

  const commit = useCallback(
    (message: string, options: { amend?: boolean } = {}): Promise<boolean> =>
      run(options.amend ? 'Amending the last commit' : 'Committing', () =>
        commitGitChanges(
          {
            message,
            amend: options.amend ?? false,
            // With nothing staged, `Commit` means "commit everything": staging
            // first is what the person clicking it expects, and it is one round
            // trip instead of a second confirmation dialog.
            stage_all: countsNothingStaged(status),
          },
          projectId,
        ),
      ),
    [projectId, run, status],
  );

  const checkout = useCallback(
    (name: string): Promise<boolean> =>
      run(`Switching to ${name}`, () => checkoutGitBranch(name, projectId)),
    [projectId, run],
  );

  const createBranch = useCallback(
    (name: string, options: { checkout?: boolean } = {}): Promise<boolean> =>
      run(`Creating ${name}`, () =>
        createGitBranch({ name, checkout: options.checkout ?? true }, projectId),
      ),
    [projectId, run],
  );

  const deleteBranch = useCallback(
    (name: string, options: { force?: boolean } = {}): Promise<boolean> =>
      run(`Deleting ${name}`, () =>
        deleteGitBranch({ name, force: options.force ?? false, confirm: options.force ?? false }, projectId),
      ),
    [projectId, run],
  );

  const stash = useCallback(
    (action: GitStashAction, options: { index?: number; includeUntracked?: boolean } = {}): Promise<boolean> =>
      run(stashLabel(action, options.index ?? 0), () =>
        runGitStash(
          {
            action,
            index: options.index,
            include_untracked: options.includeUntracked ?? false,
            confirm: action === 'drop',
          },
          projectId,
        ),
      ),
    [projectId, run],
  );

  return {
    status,
    branches,
    commits,
    stashes,
    revision,
    state,
    error,
    busy,
    actionError,
    refresh,
    loadHistory,
    loadStashes,
    stage,
    unstage,
    discard,
    commit,
    checkout,
    createBranch,
    deleteBranch,
    stash,
    clearActionError: useCallback(() => setActionError(null), []),
  };
}

function countsNothingStaged(status: GitStatus | null): boolean {
  if (!status || !status.is_repo) return false;
  return !status.files.some((file) => file.staged);
}

function stashLabel(action: GitStashAction, index: number): string {
  switch (action) {
    case 'push':
      return 'Stashing the working tree';
    case 'pop':
      return `Popping stash@{${index}}`;
    case 'apply':
      return `Applying stash@{${index}}`;
    default:
      return `Dropping stash@{${index}}`;
  }
}