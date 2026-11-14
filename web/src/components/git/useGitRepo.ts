import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import {
  checkoutGitBranch,
  checkoutGitHubPull,
  cherryPickGitCommit,
  commitGitChanges,
  createGitBranch,
  createGitHubRepo as createGitHubRepoApi,
  deleteGitBranch,
  discardGitPaths,
  fetchGitRemote,
  getGitBranches,
  getGitHubStatus,
  getGitLog,
  getGitRemotes,
  getGitStashes,
  getGitStatus,
  getGitTags,
  initGitRepo,
  manageGitRemote,
  manageGitTag,
  mergeGitBranch,
  pullGitRemote,
  pushGitRemote,
  rebaseGitBranch,
  renameGitBranch,
  resetGitHead,
  resolveGitConflict,
  revertGitCommit,
  runGitStash,
  setGitIdentity,
  stageGitPaths,
  unstageGitPaths,
  type GitStashAction,
} from '../../api/endpoints';
import type {
  GitBranch,
  GitCommit,
  GitHubStatus,
  GitMutationResult,
  GitRemote,
  GitRemoteBranch,
  GitStash,
  GitStatus,
  GitTag,
} from '../../api/types';
import * as store from '../../state/store';

export type GitLoadState = 'loading' | 'ready' | 'error';

export interface GitHistoryQuery {
  branch?: string;
  all?: boolean;
  search?: string;
  limit?: number;
}

export interface GitRepo {
  /** The working tree, or `null` before the first response. */
  status: GitStatus | null;
  branches: GitBranch[];
  remoteBranches: GitRemoteBranch[];
  /** `null` until the history has been asked for (it is loaded lazily). */
  commits: GitCommit[] | null;
  stashes: GitStash[] | null;
  tags: GitTag[] | null;
  remotes: GitRemote[] | null;
  githubStatus: GitHubStatus | null;
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
  loadHistory: (options?: GitHistoryQuery) => void;
  /** Load the stash list now (idempotent). */
  loadStashes: () => void;
  loadTags: () => void;
  loadRemotes: () => void;
  loadGitHubStatus: () => void;
  initRepo: (options?: {
    default_branch?: string;
    initial_commit?: boolean;
    commit_message?: string;
    gitignore_template?: string;
  }) => Promise<boolean>;
  setIdentity: (userName: string, userEmail: string) => Promise<boolean>;
  stage: (paths: string[] | 'all', label?: string) => Promise<boolean>;
  unstage: (paths: string[] | 'all', label?: string) => Promise<boolean>;
  discard: (paths: string[] | 'all', staged: boolean, label?: string) => Promise<boolean>;
  commit: (message: string, options?: { amend?: boolean }) => Promise<boolean>;
  checkout: (name: string, options?: { track?: boolean; detach?: boolean }) => Promise<boolean>;
  createBranch: (name: string, options?: { checkout?: boolean; start?: string }) => Promise<boolean>;
  renameBranch: (oldName: string, newName: string) => Promise<boolean>;
  deleteBranch: (name: string, options?: { force?: boolean }) => Promise<boolean>;
  stash: (
    action: GitStashAction,
    options?: { index?: number; message?: string; includeUntracked?: boolean },
  ) => Promise<boolean>;
  manageRemote: (body: {
    action: 'add' | 'set_url' | 'remove' | 'rename';
    name: string;
    url?: string;
    new_name?: string;
  }) => Promise<boolean>;
  fetch: (options?: { remote?: string; prune?: boolean; all?: boolean; tags?: boolean }) => Promise<boolean>;
  pull: (options?: { remote?: string; branch?: string; rebase?: boolean }) => Promise<boolean>;
  push: (options?: {
    remote?: string;
    branch?: string;
    set_upstream?: boolean;
    force_with_lease?: boolean;
    tags?: boolean;
    confirm?: boolean;
  }) => Promise<boolean>;
  manageTag: (body: {
    action: 'create' | 'delete' | 'push';
    name: string;
    target?: string;
    message?: string;
    remote?: string;
  }) => Promise<boolean>;
  merge: (body: {
    branch?: string;
    action?: 'merge' | 'continue' | 'abort';
    no_ff?: boolean;
    squash?: boolean;
  }) => Promise<boolean>;
  rebase: (body: { branch?: string; action?: 'rebase' | 'continue' | 'abort' | 'skip' }) => Promise<boolean>;
  cherryPick: (body: { sha?: string; action?: 'pick' | 'continue' | 'abort' | 'skip' }) => Promise<boolean>;
  revertCommit: (sha: string) => Promise<boolean>;
  resetHead: (body: { target: string; mode?: 'soft' | 'mixed' | 'hard'; confirm?: boolean }) => Promise<boolean>;
  resolveConflict: (path: string, strategy: 'ours' | 'theirs') => Promise<boolean>;
  createGitHubRepo: (body: {
    name: string;
    description?: string;
    visibility?: 'private' | 'public' | 'internal';
    remote?: string;
    push?: boolean;
  }) => Promise<boolean>;
  checkoutPull: (number: number) => Promise<boolean>;
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
  const [remoteBranches, setRemoteBranches] = useState<GitRemoteBranch[]>([]);
  const [commits, setCommits] = useState<GitCommit[] | null>(null);
  const [stashes, setStashes] = useState<GitStash[] | null>(null);
  const [tags, setTags] = useState<GitTag[] | null>(null);
  const [remotes, setRemotes] = useState<GitRemote[] | null>(null);
  const [githubStatus, setGithubStatus] = useState<GitHubStatus | null>(null);
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<GitLoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  // Refs, not state: asking for the history must not itself trigger a second
  // effect pass, and the reload below has to see the latest answer.
  const historyWanted = useRef(false);
  const historyQuery = useRef<GitHistoryQuery>({});
  const stashWanted = useRef(false);
  const tagsWanted = useRef(false);
  const remotesWanted = useRef(false);
  const githubWanted = useRef(false);
  const historyRequest = useRef<AbortController | null>(null);
  const stashRequest = useRef<AbortController | null>(null);
  const tagsRequest = useRef<AbortController | null>(null);
  const remotesRequest = useRef<AbortController | null>(null);
  const githubRequest = useRef<AbortController | null>(null);
  const mutationRequest = useRef<AbortController | null>(null);
  const mutationSequence = useRef(0);
  const repositoryGeneration = useRef(0);
  const renderedProject = useRef(projectId);
  renderedProject.current = projectId;

  const loadHistory = useCallback(
    (options?: GitHistoryQuery): void => {
      historyWanted.current = true;
      if (options !== undefined) historyQuery.current = options;
      const q = historyQuery.current;
      historyRequest.current?.abort();
      const controller = new AbortController();
      const generation = repositoryGeneration.current;
      historyRequest.current = controller;
      const current = (): boolean =>
        repositoryGeneration.current === generation &&
        renderedProject.current === projectId &&
        historyRequest.current === controller;
      getGitLog(projectId, {
        signal: controller.signal,
        limit: q.limit ?? HISTORY_LIMIT,
        branch: q.branch,
        all: q.all,
        search: q.search,
      })
        .then((result) => {
          if (current()) setCommits(result.commits);
        })
        .catch((cause: unknown) => {
          if (!current() || controller.signal.aborted) return;
          store.pushToast(`Could not read the history: ${toErrorMessage(cause)}`, 'error');
          setCommits([]);
        })
        .finally(() => {
          if (historyRequest.current === controller) historyRequest.current = null;
        });
    },
    [projectId],
  );

  const loadStashes = useCallback((): void => {
    stashWanted.current = true;
    stashRequest.current?.abort();
    const controller = new AbortController();
    const generation = repositoryGeneration.current;
    stashRequest.current = controller;
    const current = (): boolean =>
      repositoryGeneration.current === generation &&
      renderedProject.current === projectId &&
      stashRequest.current === controller;
    getGitStashes(projectId, { signal: controller.signal })
      .then((result) => {
        if (current()) setStashes(result.stashes);
      })
      .catch((cause: unknown) => {
        if (!current() || controller.signal.aborted) return;
        store.pushToast(`Could not read the stash list: ${toErrorMessage(cause)}`, 'error');
        setStashes([]);
      })
      .finally(() => {
        if (stashRequest.current === controller) stashRequest.current = null;
      });
  }, [projectId]);

  const loadTags = useCallback((): void => {
    tagsWanted.current = true;
    tagsRequest.current?.abort();
    const controller = new AbortController();
    const generation = repositoryGeneration.current;
    tagsRequest.current = controller;
    const current = (): boolean =>
      repositoryGeneration.current === generation &&
      renderedProject.current === projectId &&
      tagsRequest.current === controller;
    getGitTags(projectId, { signal: controller.signal })
      .then((result) => {
        if (current()) setTags(result.is_repo ? result.tags : []);
      })
      .catch(() => {
        if (current() && !controller.signal.aborted) setTags([]);
      })
      .finally(() => {
        if (tagsRequest.current === controller) tagsRequest.current = null;
      });
  }, [projectId]);

  const loadRemotes = useCallback((): void => {
    remotesWanted.current = true;
    remotesRequest.current?.abort();
    const controller = new AbortController();
    const generation = repositoryGeneration.current;
    remotesRequest.current = controller;
    const current = (): boolean =>
      repositoryGeneration.current === generation &&
      renderedProject.current === projectId &&
      remotesRequest.current === controller;
    getGitRemotes(projectId, { signal: controller.signal })
      .then((result) => {
        if (current()) setRemotes(result.is_repo ? result.remotes : []);
      })
      .catch(() => {
        if (current() && !controller.signal.aborted) setRemotes([]);
      })
      .finally(() => {
        if (remotesRequest.current === controller) remotesRequest.current = null;
      });
  }, [projectId]);

  const loadGitHubStatus = useCallback((): void => {
    githubWanted.current = true;
    githubRequest.current?.abort();
    const controller = new AbortController();
    const generation = repositoryGeneration.current;
    githubRequest.current = controller;
    const current = (): boolean =>
      repositoryGeneration.current === generation &&
      renderedProject.current === projectId &&
      githubRequest.current === controller;
    getGitHubStatus(projectId, { signal: controller.signal })
      .then((result) => {
        if (current()) setGithubStatus(result);
      })
      .catch(() => {
        if (current() && !controller.signal.aborted) setGithubStatus(null);
      })
      .finally(() => {
        if (githubRequest.current === controller) githubRequest.current = null;
      });
  }, [projectId]);

  // Project identity owns every outstanding request. Cleanup runs before the new
  // repository's effects, so an old response can no longer pass a generation test
  // even when a test double resolves after being aborted.
  useEffect(() => {
    repositoryGeneration.current += 1;
    setStatus(null);
    setBranches([]);
    setRemoteBranches([]);
    setCommits(null);
    setStashes(null);
    setTags(null);
    setRemotes(null);
    setGithubStatus(null);
    setState('loading');
    setError(null);
    setBusy(null);
    setActionError(null);
    historyWanted.current = false;
    historyQuery.current = {};
    stashWanted.current = false;
    tagsWanted.current = false;
    remotesWanted.current = false;
    githubWanted.current = false;
    return () => {
      repositoryGeneration.current += 1;
      mutationSequence.current += 1;
      historyRequest.current?.abort();
      stashRequest.current?.abort();
      tagsRequest.current?.abort();
      remotesRequest.current?.abort();
      githubRequest.current?.abort();
      mutationRequest.current?.abort();
      historyRequest.current = null;
      stashRequest.current = null;
      tagsRequest.current = null;
      remotesRequest.current = null;
      githubRequest.current = null;
      mutationRequest.current = null;
    };
  }, [projectId]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    let live = true;
    setState((current) => (current === 'loading' ? 'loading' : 'ready'));
    setError(null);

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
        setRemoteBranches(result.is_repo ? (result.remote_branches ?? []) : []);
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
    if (tagsWanted.current) loadTags();
    if (remotesWanted.current) loadRemotes();
    if (githubWanted.current) loadGitHubStatus();
    return () => {
      live = false;
      controller.abort();
    };
  }, [projectId, refreshKey, loadHistory, loadStashes, loadTags, loadRemotes, loadGitHubStatus]);

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
      action: (signal: AbortSignal) => Promise<GitMutationResult>,
    ): Promise<boolean> => {
      if (renderedProject.current !== projectId) return false;
      mutationRequest.current?.abort();
      const controller = new AbortController();
      const generation = repositoryGeneration.current;
      const operation = ++mutationSequence.current;
      mutationRequest.current = controller;
      const current = (): boolean =>
        repositoryGeneration.current === generation &&
        renderedProject.current === projectId &&
        mutationSequence.current === operation;
      setBusy(label);
      setActionError(null);
      try {
        const result = await action(controller.signal);
        if (!current()) return false;
        setStatus(result.status);
        setRevision((value) => value + 1);
        setState('ready');
        if (result.remotes !== undefined) setRemotes(result.remotes);
        if (result.message) store.pushToast(result.message, 'success');
        // A commit changes the history, a checkout changes the branch list. These
        // follow-ups belong to the same repository generation and operation too:
        // abort alone is not enough for a fetch mock or already-settled response.
        void getGitBranches(projectId, { signal: controller.signal })
          .then((list) => {
            if (current()) {
              setBranches(list.is_repo ? list.branches : []);
              setRemoteBranches(list.is_repo ? (list.remote_branches ?? []) : []);
            }
          })
          .catch(() => {
            // The previous list stands; the next refresh will try again.
          });
        if (historyWanted.current) loadHistory();
        if (stashWanted.current) loadStashes();
        if (tagsWanted.current) loadTags();
        if (remotesWanted.current && result.remotes === undefined) loadRemotes();
        if (githubWanted.current) loadGitHubStatus();
        return true;
      } catch (cause: unknown) {
        if (!current() || controller.signal.aborted) return false;
        const message = toErrorMessage(cause);
        setActionError(message);
        store.pushToast(message, 'error');
        return false;
      } finally {
        if (current()) {
          setBusy(null);
          mutationRequest.current = null;
        }
      }
    },
    [projectId, loadHistory, loadStashes, loadTags, loadRemotes, loadGitHubStatus],
  );

  const stage = useCallback(
    (paths: string[] | 'all', label?: string): Promise<boolean> =>
      run(
        label ?? (paths === 'all' ? 'Staging every change' : `Staging ${paths.length} file${paths.length === 1 ? '' : 's'}`),
        (signal) => stageGitPaths(paths === 'all' ? { all: true } : { paths }, projectId, { signal }),
      ),
    [projectId, run],
  );

  const unstage = useCallback(
    (paths: string[] | 'all', label?: string): Promise<boolean> =>
      run(
        label ?? (paths === 'all' ? 'Unstaging everything' : `Unstaging ${paths.length} file${paths.length === 1 ? '' : 's'}`),
        (signal) => unstageGitPaths(paths === 'all' ? { all: true } : { paths }, projectId, { signal }),
      ),
    [projectId, run],
  );

  const discard = useCallback(
    (paths: string[] | 'all', staged: boolean, label?: string): Promise<boolean> =>
      run(label ?? 'Discarding changes', (signal) =>
        discardGitPaths(
          {
            ...(paths === 'all' ? { all: true } : { paths }),
            staged,
            confirm: true,
          },
          projectId,
          { signal },
        ),
      ),
    [projectId, run],
  );

  const commit = useCallback(
    (message: string, options: { amend?: boolean } = {}): Promise<boolean> =>
      run(options.amend ? 'Amending the last commit' : 'Committing', (signal) =>
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
          { signal },
        ),
      ),
    [projectId, run, status],
  );

  const initRepo = useCallback(
    (
      options: {
        default_branch?: string;
        initial_commit?: boolean;
        commit_message?: string;
        gitignore_template?: string;
      } = {},
    ): Promise<boolean> =>
      run('Initializing git repository', (signal) => initGitRepo(options, projectId, { signal })),
    [projectId, run],
  );

  const setIdentity = useCallback(
    (userName: string, userEmail: string): Promise<boolean> =>
      run('Updating git identity', (signal) =>
        setGitIdentity(
          {
            user_name: userName.trim() || undefined,
            user_email: userEmail.trim() || undefined,
          },
          projectId,
          { signal },
        ),
      ),
    [projectId, run],
  );

  const checkout = useCallback(
    (name: string, options: { track?: boolean; detach?: boolean } = {}): Promise<boolean> =>
      run(`Switching to ${name}`, (signal) =>
        checkoutGitBranch(name, projectId, {
          signal,
          track: options.track,
          detach: options.detach,
        }),
      ),
    [projectId, run],
  );

  const createBranch = useCallback(
    (name: string, options: { checkout?: boolean; start?: string } = {}): Promise<boolean> =>
      run(`Creating ${name}`, (signal) =>
        createGitBranch(
          {
            name,
            checkout: options.checkout ?? true,
            ...(options.start ? { start: options.start } : {}),
          },
          projectId,
          { signal },
        ),
      ),
    [projectId, run],
  );

  const renameBranch = useCallback(
    (oldName: string, newName: string): Promise<boolean> =>
      run(`Renaming ${oldName}`, (signal) =>
        renameGitBranch({ old_name: oldName, new_name: newName }, projectId, { signal }),
      ),
    [projectId, run],
  );

  const deleteBranch = useCallback(
    (name: string, options: { force?: boolean } = {}): Promise<boolean> =>
      run(`Deleting ${name}`, (signal) =>
        deleteGitBranch(
          { name, force: options.force ?? false, confirm: options.force ?? false },
          projectId,
          { signal },
        ),
      ),
    [projectId, run],
  );

  const stash = useCallback(
    (
      action: GitStashAction,
      options: { index?: number; message?: string; includeUntracked?: boolean } = {},
    ): Promise<boolean> =>
      run(stashLabel(action, options.index ?? 0), (signal) =>
        runGitStash(
          {
            action,
            index: options.index,
            ...(options.message ? { message: options.message } : {}),
            include_untracked: options.includeUntracked ?? false,
            confirm: action === 'drop',
          },
          projectId,
          { signal },
        ),
      ),
    [projectId, run],
  );

  const manageRemote = useCallback(
    (body: {
      action: 'add' | 'set_url' | 'remove' | 'rename';
      name: string;
      url?: string;
      new_name?: string;
    }): Promise<boolean> =>
      run(`Updating remote ${body.name}`, (signal) => manageGitRemote(body, projectId, { signal })),
    [projectId, run],
  );

  const fetch = useCallback(
    (options: { remote?: string; prune?: boolean; all?: boolean; tags?: boolean } = {}): Promise<boolean> =>
      run('Fetching from remote', (signal) => fetchGitRemote(options, projectId, { signal })),
    [projectId, run],
  );

  const pull = useCallback(
    (options: { remote?: string; branch?: string; rebase?: boolean } = {}): Promise<boolean> =>
      run('Pulling from remote', (signal) => pullGitRemote(options, projectId, { signal })),
    [projectId, run],
  );

  const push = useCallback(
    (
      options: {
        remote?: string;
        branch?: string;
        set_upstream?: boolean;
        force_with_lease?: boolean;
        tags?: boolean;
        confirm?: boolean;
      } = {},
    ): Promise<boolean> =>
      run('Pushing to remote', (signal) => pushGitRemote(options, projectId, { signal })),
    [projectId, run],
  );

  const manageTag = useCallback(
    (body: {
      action: 'create' | 'delete' | 'push';
      name: string;
      target?: string;
      message?: string;
      remote?: string;
    }): Promise<boolean> =>
      run(`Tag ${body.name}`, (signal) => manageGitTag(body, projectId, { signal })),
    [projectId, run],
  );

  const merge = useCallback(
    (body: {
      branch?: string;
      action?: 'merge' | 'continue' | 'abort';
      no_ff?: boolean;
      squash?: boolean;
    }): Promise<boolean> =>
      run(body.action === 'abort' ? 'Aborting merge' : 'Merging', (signal) =>
        mergeGitBranch(body, projectId, { signal }),
      ),
    [projectId, run],
  );

  const rebase = useCallback(
    (body: { branch?: string; action?: 'rebase' | 'continue' | 'abort' | 'skip' }): Promise<boolean> =>
      run(body.action === 'abort' ? 'Aborting rebase' : 'Rebasing', (signal) =>
        rebaseGitBranch(body, projectId, { signal }),
      ),
    [projectId, run],
  );

  const cherryPick = useCallback(
    (body: { sha?: string; action?: 'pick' | 'continue' | 'abort' | 'skip' }): Promise<boolean> =>
      run(body.action === 'abort' ? 'Aborting cherry-pick' : 'Cherry-picking', (signal) =>
        cherryPickGitCommit(body, projectId, { signal }),
      ),
    [projectId, run],
  );

  const revertCommit = useCallback(
    (sha: string): Promise<boolean> =>
      run(`Reverting ${sha.slice(0, 7)}`, (signal) => revertGitCommit(sha, projectId, { signal })),
    [projectId, run],
  );

  const resetHead = useCallback(
    (body: { target: string; mode?: 'soft' | 'mixed' | 'hard'; confirm?: boolean }): Promise<boolean> =>
      run(`Resetting to ${body.target}`, (signal) => resetGitHead(body, projectId, { signal })),
    [projectId, run],
  );

  const resolveConflict = useCallback(
    (path: string, strategy: 'ours' | 'theirs'): Promise<boolean> =>
      run(`Resolving ${path}`, (signal) =>
        resolveGitConflict({ path, strategy }, projectId, { signal }),
      ),
    [projectId, run],
  );

  const createGitHubRepo = useCallback(
    (body: {
      name: string;
      description?: string;
      visibility?: 'private' | 'public' | 'internal';
      remote?: string;
      push?: boolean;
    }): Promise<boolean> =>
      run(`Creating GitHub repo ${body.name}`, (signal) =>
        createGitHubRepoApi(body, projectId, { signal }),
      ),
    [projectId, run],
  );

  const checkoutPull = useCallback(
    (number: number): Promise<boolean> =>
      run(`Checking out PR #${number}`, (signal) => checkoutGitHubPull(number, projectId, { signal })),
    [projectId, run],
  );

  return {
    status,
    branches,
    remoteBranches,
    commits,
    stashes,
    tags,
    remotes,
    githubStatus,
    revision,
    state,
    error,
    busy,
    actionError,
    refresh,
    loadHistory,
    loadStashes,
    loadTags,
    loadRemotes,
    loadGitHubStatus,
    initRepo,
    setIdentity,
    stage,
    unstage,
    discard,
    commit,
    checkout,
    createBranch,
    renameBranch,
    deleteBranch,
    stash,
    manageRemote,
    fetch,
    pull,
    push,
    manageTag,
    merge,
    rebase,
    cherryPick,
    revertCommit,
    resetHead,
    resolveConflict,
    createGitHubRepo,
    checkoutPull,
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