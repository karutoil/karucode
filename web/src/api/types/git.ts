/**
 * One entry of `GET /api/git/status`.
 *
 * `index`/`worktree` are the two porcelain columns: a file can be staged *and*
 * modified at once (`MM`), which is why both flags are reported instead of a
 * single `staged` bit. `status` keeps the older trimmed form (`M`, `MM`, `??`)
 * for anything that only wants one label.
 */
export interface GitFile {
  path: string;
  /** Source path of a rename/copy, when git reports one. */
  orig_path?: string | null;
  /** Index column: `M`, `A`, `D`, `R`, `C`, `?`, or a space. */
  index: string;
  /** Worktree column, same alphabet. */
  worktree: string;
  status: string;
  /** There is something in the index for this path. */
  staged: boolean;
  /** The working tree differs from the index (or the file is untracked). */
  unstaged?: boolean;
  untracked?: boolean;
  /** An unresolved merge conflict. */
  conflict?: boolean;
}

export type GitRepoOperation = 'merge' | 'rebase' | 'cherry-pick' | 'revert';

export interface GitIdentity {
  name: string | null;
  email: string | null;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  /** A detached HEAD has no branch name. */
  detached?: boolean;
  /** `origin/main` when the branch tracks an upstream. */
  upstream?: string | null;
  ahead?: number;
  behind?: number;
  operation?: GitRepoOperation | null;
  has_head?: boolean;
  identity?: GitIdentity;
  remotes_count?: number;
  error: string | null;
  files: GitFile[];
  /** The server cut the listing at its output limit. */
  truncated?: boolean;
}

export interface GitDiff {
  diff: string;
  truncated: boolean;
}

/** One local branch from `GET /api/git/branches`. */
export interface GitBranch {
  name: string;
  current: boolean;
  commit: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  /** Committer date, seconds since the epoch. */
  timestamp: number;
  subject: string;
}

export interface GitRemoteBranch {
  name: string;
  remote: string;
  branch: string;
  commit: string;
  timestamp: number;
  subject: string;
}

export interface GitBranchList {
  is_repo: boolean;
  branch: string | null;
  detached: boolean;
  branches: GitBranch[];
  remote_branches?: GitRemoteBranch[];
  truncated: boolean;
}

export interface GitTag {
  name: string;
  commit: string;
  annotated: boolean;
  timestamp: number;
  subject: string;
}

export interface GitTagList {
  is_repo: boolean;
  tags: GitTag[];
  truncated: boolean;
}

export interface GitRemote {
  name: string;
  fetch_url: string | null;
  push_url: string | null;
  github: { owner: string; repo: string } | null;
}

export interface GitRemoteList {
  is_repo: boolean;
  remotes: GitRemote[];
}

export interface GitStashDetail {
  ref: string;
  index: number;
  files: GitCommitFile[];
  diff: string;
  truncated: boolean;
}

export interface GitHubStatus {
  gh_installed: boolean;
  authenticated: boolean;
  username: string | null;
  remote_slug: string | null;
  repo: {
    nameWithOwner?: string;
    name?: string;
    url?: string;
    sshUrl?: string;
    defaultBranchRef?: { name: string } | null;
    isPrivate?: boolean;
    description?: string | null;
    stargazerCount?: number;
    forkCount?: number;
  } | null;
  error: string | null;
}

export interface GitHubLabel {
  id?: string;
  name: string;
  color?: string;
  description?: string;
}

export interface GitHubAuthor {
  login: string;
  is_bot?: boolean;
}

export interface GitHubStatusCheck {
  __typename?: string;
  name?: string;
  context?: string;
  status?: string;
  state?: string;
  conclusion?: string | null;
  detailsUrl?: string;
  targetUrl?: string;
}

export interface GitHubPullSummary {
  number: number;
  title: string;
  state: 'OPEN' | 'CLOSED' | 'MERGED' | string;
  isDraft: boolean;
  author: GitHubAuthor | null;
  headRefName: string;
  baseRefName: string;
  url: string;
  createdAt: string;
  updatedAt: string;
  labels: GitHubLabel[];
  reviewDecision?: string | null;
  mergeable?: string | null;
  additions?: number;
  deletions?: number;
  changedFiles?: number;
  statusCheckRollup?: GitHubStatusCheck[] | null;
  body?: string;
}

export interface GitHubComment {
  id?: string;
  author: GitHubAuthor | null;
  body: string;
  createdAt: string;
}

export interface GitHubPullDetail extends GitHubPullSummary {
  comments?: GitHubComment[];
  reviews?: Array<{
    author: GitHubAuthor | null;
    state: string;
    body: string;
    submittedAt?: string;
  }>;
  files?: Array<{
    path: string;
    additions: number;
    deletions: number;
  }>;
  commits?: Array<{
    oid: string;
    messageHeadline: string;
    authors?: Array<{ login?: string; name?: string }>;
  }>;
}

export interface GitHubIssueSummary {
  number: number;
  title: string;
  state: 'OPEN' | 'CLOSED' | string;
  author: GitHubAuthor | null;
  url: string;
  createdAt: string;
  updatedAt: string;
  labels: GitHubLabel[];
  assignees?: GitHubAuthor[];
  comments?: GitHubComment[] | number;
  body?: string;
}

export interface GitHubIssueDetail extends Omit<GitHubIssueSummary, 'comments'> {
  comments?: GitHubComment[];
}

/** One commit from `GET /api/git/log`. */
export interface GitCommit {
  id: string;
  short_id: string;
  author: string;
  email: string;
  /** Author date, seconds since the epoch. */
  timestamp: number;
  parents: string[];
  /** `HEAD -> main, origin/main` as git decorates it. */
  refs: string;
  subject: string;
}

export interface GitCommitList {
  is_repo: boolean;
  commits: GitCommit[];
  truncated?: boolean;
}

/** One entry of a commit's file list (`--numstat`). */
export interface GitCommitFile {
  path: string;
  /** `null` for a binary file, where git prints `-`. */
  additions: number | null;
  deletions: number | null;
  binary: boolean;
}

export interface GitCommitDetail {
  id: string;
  short_id: string;
  author: string;
  email: string;
  timestamp: number;
  parents: string[];
  refs: string;
  subject: string;
  body: string;
  files: GitCommitFile[];
  diff: string;
  truncated: boolean;
}

export interface GitStash {
  ref: string;
  index: number;
  commit: string;
  timestamp: number;
  subject: string;
}

export interface GitStashList {
  is_repo: boolean;
  stashes: GitStash[];
  truncated?: boolean;
}

/**
 * Every mutating git route answers with the same envelope: the action's own
 * message, a freshly read status (so the panel never shows a stale tree) and,
 * for `POST /api/git/commit`, the new revision.
 */
export interface GitMutationResult {
  ok: boolean;
  message: string | null;
  status: GitStatus;
  commit: { id: string; short_id: string; subject: string } | null;
  remotes?: GitRemote[];
  url?: string | null;
}
