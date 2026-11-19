import { apiRequest, withQuery } from '../client';
import type {
  GitBranchList,
  GitCommitDetail,
  GitCommitList,
  GitDiff,
  GitMutationResult,
  GitRemoteList,
  GitStashDetail,
  GitStashList,
  GitStatus,
  GitTagList,
} from '../types/git';
import type { CallOptions } from './common';

export function getGitStatus(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitStatus> {
  return apiRequest<GitStatus>(
    withQuery('/api/git/status', { project: project ?? undefined }),
    options,
  );
}

export function getGitDiff(
  path: string,
  staged: boolean,
  base?: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitDiff> {
  return apiRequest<GitDiff>(
    withQuery('/api/git/diff', {
      path,
      staged,
      base: base ?? undefined,
      project: project ?? undefined,
    }),
    options,
  );
}

/** Local branches, newest commit first, with their upstream tracking counts. */
export function getGitBranches(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitBranchList> {
  return apiRequest<GitBranchList>(
    withQuery('/api/git/branches', { project: project ?? undefined }),
    options,
  );
}

/**
 * Commit history, newest first. `path` narrows it to one file's history and
 * `limit` is clamped by the server (1-100, default 30).
 */
export function getGitLog(
  project?: string | null,
  options: CallOptions & { path?: string; limit?: number; branch?: string; all?: boolean; search?: string } = {},
): Promise<GitCommitList> {
  return apiRequest<GitCommitList>(
    withQuery('/api/git/log', {
      project: project ?? undefined,
      path: options.path,
      limit: options.limit,
      branch: options.branch,
      all: options.all ? 'true' : undefined,
      search: options.search,
    }),
    options,
  );
}

/** One commit: metadata, its file stat and its patch. `sha` may be abbreviated. */
export function getGitCommit(
  sha: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitCommitDetail> {
  return apiRequest<GitCommitDetail>(
    withQuery(`/api/git/commits/${encodeURIComponent(sha)}`, {
      project: project ?? undefined,
    }),
    options,
  );
}

export function getGitStashes(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitStashList> {
  return apiRequest<GitStashList>(
    withQuery('/api/git/stash', { project: project ?? undefined }),
    options,
  );
}

/**
 * Stage paths (or the whole workspace with `all`).
 *
 * Every mutation below returns the same envelope: a message, the fresh status and
 * - for a commit - the new revision, so the panel can update from one response.
 */
export function stageGitPaths(
  body: { paths?: string[]; all?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/stage', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function unstageGitPaths(
  body: { paths?: string[]; all?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/unstage', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

/**
 * Throw away changes. `confirm: true` is required by the server: discarding
 * overwrites the working tree and deletes an untracked file, and neither can be
 * undone here.
 */
export function discardGitPaths(
  body: { paths?: string[]; all?: boolean; staged?: boolean; confirm: true },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/discard', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function commitGitChanges(
  body: { message: string; amend?: boolean; stage_all?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/commit', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function createGitBranch(
  body: { name: string; start?: string; checkout?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/branches', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function checkoutGitBranch(
  name: string,
  project?: string | null,
  options: CallOptions & { track?: boolean; detach?: boolean } = {},
): Promise<GitMutationResult> {
  const { track, detach, ...call } = options;
  const body: { name: string; track?: boolean; detach?: boolean } = { name };
  if (track) body.track = true;
  if (detach) body.detach = true;
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/checkout', { project: project ?? undefined }),
    { ...call, method: 'POST', body },
  );
}

export function renameGitBranch(
  body: { old_name: string; new_name: string },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/branches/rename', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function deleteGitBranch(
  body: { name: string; force?: boolean; confirm?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/branches/delete', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export type GitStashAction = 'push' | 'pop' | 'apply' | 'drop';

export function runGitStash(
  body: {
    action: GitStashAction;
    index?: number;
    message?: string;
    include_untracked?: boolean;
    confirm?: boolean;
  },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/stash', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function getGitStashDetail(
  index: number,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitStashDetail> {
  return apiRequest<GitStashDetail>(
    withQuery(`/api/git/stash/${index}`, { project: project ?? undefined }),
    options,
  );
}

export function initGitRepo(
  body: {
    default_branch?: string;
    initial_commit?: boolean;
    commit_message?: string;
    gitignore_template?: string;
  } = {},
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/init', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function setGitIdentity(
  body: { user_name?: string; user_email?: string },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/config', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function getGitRemotes(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitRemoteList> {
  return apiRequest<GitRemoteList>(
    withQuery('/api/git/remotes', { project: project ?? undefined }),
    options,
  );
}

export function manageGitRemote(
  body: {
    action: 'add' | 'set_url' | 'remove' | 'rename';
    name: string;
    url?: string;
    new_name?: string;
  },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/remotes', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function fetchGitRemote(
  body: { remote?: string; prune?: boolean; all?: boolean; tags?: boolean } = {},
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/fetch', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function pullGitRemote(
  body: { remote?: string; branch?: string; rebase?: boolean } = {},
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/pull', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function pushGitRemote(
  body: {
    remote?: string;
    branch?: string;
    set_upstream?: boolean;
    force_with_lease?: boolean;
    tags?: boolean;
    confirm?: boolean;
  } = {},
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/push', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function getGitTags(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitTagList> {
  return apiRequest<GitTagList>(
    withQuery('/api/git/tags', { project: project ?? undefined }),
    options,
  );
}

export function manageGitTag(
  body: {
    action: 'create' | 'delete' | 'push';
    name: string;
    target?: string;
    message?: string;
    remote?: string;
  },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/tags', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function mergeGitBranch(
  body: { branch?: string; action?: 'merge' | 'continue' | 'abort'; no_ff?: boolean; squash?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/merge', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function rebaseGitBranch(
  body: { branch?: string; action?: 'rebase' | 'continue' | 'abort' | 'skip' },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/rebase', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function cherryPickGitCommit(
  body: { sha?: string; action?: 'pick' | 'continue' | 'abort' | 'skip' },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/cherry-pick', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function revertGitCommit(
  sha: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/revert', { project: project ?? undefined }),
    { ...options, method: 'POST', body: { sha } },
  );
}

export function resetGitHead(
  body: { target: string; mode?: 'soft' | 'mixed' | 'hard'; confirm?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/reset', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function resolveGitConflict(
  body: { path: string; strategy: 'ours' | 'theirs' },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/resolve', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}
