import type {
  GitFile,
  GitHubIssueSummary,
  GitHubPullSummary,
  GitRepoOperation,
  GitStatus,
} from '../../api/types';

/**
 * How a changed file is presented: the badge letter, the words next to it and
 * the tone carrying the colour. Pure functions with no Preact in them, so the
 * rules the manager applies can be tested without rendering anything.
 */

export type GitGroupId = 'conflicts' | 'staged' | 'changes' | 'untracked';

export interface GitGroup {
  id: GitGroupId;
  label: string;
  /** One line under the label: what acting on the group does. */
  hint: string;
  files: GitFile[];
}

export type GitTone = 'ok' | 'warn' | 'danger' | 'info';

/**
 * What the manager is showing in its detail region.
 *
 * `group` is carried for a file selection because the same path can appear in
 * both the staged and the changes group; the diff and the actions differ, and
 * "which row did the user click" cannot be recovered from the path alone.
 */
export type GitSelection =
  | { kind: 'file'; path: string; staged: boolean; group: GitGroupId }
  | { kind: 'commit'; sha: string; subject: string }
  | { kind: 'stash'; index: number; ref: string; subject: string };

/** The file a selection points at, for a caller that only needs that. */
export function selectedFile(
  selection: GitSelection | null,
): { path: string; staged: boolean } | null {
  if (!selection || selection.kind !== 'file') return null;
  return { path: selection.path, staged: selection.staged };
}

/** Untracked: `git status` prints `??` in both columns. */
export function isUntracked(file: GitFile): boolean {
  if (file.untracked !== undefined) return file.untracked;
  return file.status === '??' || file.status === '?';
}

/** One of git's seven unmerged states (`DD`, `AU`, `UD`, `UA`, `DU`, `AA`, `UU`). */
const CONFLICT_CODES = ['DD', 'AU', 'UD', 'UA', 'DU', 'AA', 'UU'];

export function isConflict(file: GitFile): boolean {
  if (file.conflict !== undefined) return file.conflict;
  return CONFLICT_CODES.includes(file.status);
}

/**
 * The working tree has changes for this path: it is untracked, or its worktree
 * column differs from the index. A file can be staged *and* have worktree
 * changes, which is why this is not simply `!file.staged`.
 */
export function hasUnstagedSide(file: GitFile): boolean {
  if (isUntracked(file)) return true;
  if (file.unstaged !== undefined) return file.unstaged;
  const worktree = file.worktree ?? '';
  if (worktree !== '') return worktree !== ' ';
  return !file.staged;
}

/**
 * The groups the manager renders, most urgent first.
 *
 * A file with both a staged and an unstaged side appears in *two* groups - that
 * is git's own model, and hiding it would make `git status` and the panel
 * disagree about the same tree.
 */
export function groupFiles(files: readonly GitFile[]): GitGroup[] {
  const conflicts: GitFile[] = [];
  const staged: GitFile[] = [];
  const changes: GitFile[] = [];
  const untracked: GitFile[] = [];
  for (const file of files) {
    if (isConflict(file)) {
      conflicts.push(file);
      continue;
    }
    if (isUntracked(file)) {
      untracked.push(file);
      continue;
    }
    if (file.staged) staged.push(file);
    if (hasUnstagedSide(file)) changes.push(file);
  }
  const groups: GitGroup[] = [
    {
      id: 'conflicts',
      label: 'Merge changes',
      hint: 'Resolve these in the file, then stage the result',
      files: conflicts,
    },
    {
      id: 'staged',
      label: 'Staged',
      hint: 'Included in the next commit',
      files: staged,
    },
    {
      id: 'changes',
      label: 'Changes',
      hint: 'Modified in the working tree',
      files: changes,
    },
    {
      id: 'untracked',
      label: 'Untracked',
      hint: 'Not in git yet',
      files: untracked,
    },
  ];
  return groups.filter((group) => group.files.length > 0);
}

/**
 * The one-letter code shown in the badge. The index side wins when the file is
 * staged, because that is the state the next commit will record.
 */
export function statusLetter(file: GitFile): string {
  const code = file.staged ? file.index : file.worktree;
  if (code && code !== ' ' && code !== '?') return code;
  const first = file.status.slice(0, 1);
  if (first && first !== ' ') return first;
  return '?';
}

/** Words for the letter; the colour is never the only signal. */
export function statusLabel(file: GitFile): string {
  switch (statusLetter(file)) {
    case 'M':
      return 'Modified';
    case 'A':
      return 'Added';
    case 'D':
      return 'Deleted';
    case 'R':
      return 'Renamed';
    case 'C':
      return 'Copied';
    case 'U':
      return 'Conflict';
    case 'T':
      return 'Type changed';
    default:
      return isUntracked(file) ? 'Untracked' : 'Changed';
  }
}

export function statusTone(file: GitFile): GitTone {
  switch (statusLetter(file)) {
    case 'A':
      return 'ok';
    case 'D':
    case 'U':
      return 'danger';
    case 'R':
    case 'C':
      return 'info';
    default:
      return 'warn';
  }
}

/** The two-letter porcelain code, for the tooltip that explains the badge. */
export function porcelainCode(file: GitFile): string {
  const index = file.index ?? ' ';
  const worktree = file.worktree ?? ' ';
  const code = `${index === '' ? ' ' : index}${worktree === '' ? ' ' : worktree}`;
  return code.trim() === '' ? file.status : code;
}

export interface ChangeCounts {
  total: number;
  staged: number;
  changes: number;
  untracked: number;
  conflicts: number;
}

/** Counts for the header chips. `total` counts paths, not group memberships. */
export function changeCounts(files: readonly GitFile[]): ChangeCounts {
  const counts: ChangeCounts = { total: files.length, staged: 0, changes: 0, untracked: 0, conflicts: 0 };
  for (const file of files) {
    // An unmerged path gets its own bucket: it is neither "staged" nor an
    // ordinary working-tree change until somebody resolves it.
    if (isConflict(file)) {
      counts.conflicts += 1;
      continue;
    }
    // Untracked paths are their own bucket: counting them as "modified" too made
    // one new file read as "1 unstaged · 1 untracked".
    if (isUntracked(file)) {
      counts.untracked += 1;
      continue;
    }
    if (file.staged) counts.staged += 1;
    if (hasUnstagedSide(file)) counts.changes += 1;
  }
  return counts;
}

/** `src/plugins/loader.rs` → `{ name: 'loader.rs', dir: 'src/plugins' }`. */
export function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf('/');
  if (slash === -1) return { name: path, dir: '' };
  return { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}

/**
 * What is wrong with a branch name, or `null` when it looks usable.
 *
 * A mirror of `kc_tools::git::validate_branch_name` for immediate feedback in the
 * form; the server re-validates and a refusal from it always wins, so this is
 * never a security boundary - just a way to say "no spaces" before a round trip.
 */
export function branchNameProblem(name: string): string | null {
  const value = name.trim();
  if (value === '') return 'Enter a branch name';
  if (value.length > 120) return 'A branch name may be at most 120 characters';
  if (value.startsWith('-')) return 'A branch name may not start with `-`';
  if (value.startsWith('/') || value.endsWith('/') || value.includes('//')) {
    return 'A branch name may not begin, end or double a `/`';
  }
  if (/[\s~^:?*[\\]/.test(value)) return 'A branch name may not contain spaces or ~ ^ : ? * [ \\';
  if (value.includes('..')) return 'A branch name may not contain `..`';
  if (value.includes('@{')) return 'A branch name may not contain `@{`';
  if (value.endsWith('.') || value.endsWith('.lock')) return 'A branch name may not end with `.` or `.lock`';
  return null;
}

/**
 * The sentence under the title: what the working tree holds and where.
 *
 * Returns `null` for "there is nothing to describe" (no repository), so the
 * caller can print its own explanation instead of a fabricated "0 changes".
 */
export function describeStatus(status: GitStatus | null): string | null {
  if (!status || !status.is_repo) return null;
  const counts = changeCounts(status.files);
  if (counts.total === 0) return 'Working tree clean';
  const parts = [`${counts.total} ${counts.total === 1 ? 'change' : 'changes'}`];
  if (counts.staged > 0) parts.push(`${counts.staged} staged`);
  if (counts.changes > 0) parts.push(`${counts.changes} unstaged`);
  if (counts.untracked > 0) parts.push(`${counts.untracked} untracked`);
  if (counts.conflicts > 0) parts.push(`${counts.conflicts} conflicted`);
  return parts.join(' · ');
}

/** Where the HEAD is, for the branch chip: a branch name or a detached id. */
export function branchLabel(status: GitStatus | null): string | null {
  if (!status || !status.is_repo) return null;
  if (status.branch) return status.branch;
  return status.detached ? 'detached HEAD' : null;
}

/**
 * Whether the commit button may fire, and why not when it may not.
 *
 * `amend` is allowed with an empty message (the server then uses `--no-edit`, so
 * the previous message is kept); everything else needs a message to record.
 */
export function canCommit(
  message: string,
  status: GitStatus | null,
  options: { amend: boolean },
): { enabled: boolean; reason: string | null } {
  if (!status) return { enabled: false, reason: 'Waiting for the git status…' };
  if (!status.is_repo) return { enabled: false, reason: 'Not a git repository' };
  const counts = changeCounts(status.files);
  if (counts.conflicts > 0) {
    // `git commit` would refuse anyway (an unmerged path cannot be committed);
    // saying so here is faster than a round trip that fails.
    return { enabled: false, reason: 'Resolve the merge conflicts first' };
  }
  if (options.amend) return { enabled: true, reason: null };
  if (message.trim() === '') return { enabled: false, reason: 'Write a commit message first' };
  if (counts.total === 0) return { enabled: false, reason: 'Nothing to commit' };
  return { enabled: true, reason: null };
}

export function operationLabel(operation: GitRepoOperation | null | undefined): string | null {
  switch (operation) {
    case 'merge':
      return 'Merge in progress';
    case 'rebase':
      return 'Rebase in progress';
    case 'cherry-pick':
      return 'Cherry-pick in progress';
    case 'revert':
      return 'Revert in progress';
    default:
      return null;
  }
}

export function remoteNameProblem(name: string): string | null {
  const value = name.trim();
  if (value === '') return 'Enter a remote name';
  if (value.length > 100) return 'Remote name may be at most 100 characters';
  if (value.startsWith('-')) return 'Remote name may not start with `-`';
  if (!/^[A-Za-z0-9._-]+$/.test(value)) {
    return 'Remote name may only contain letters, digits, `.`, `_`, or `-`';
  }
  return null;
}

export function issueSessionSpec(issue: GitHubIssueSummary): { title: string; prompt: string } {
  const labels = issue.labels.map((l) => l.name).filter(Boolean).join(', ');
  const body = (issue.body ?? '').trim();
  const truncatedBody = body.length > 8000 ? `${body.slice(0, 8000)}\n\n…(truncated)` : body;
  const lines = [
    `Work on GitHub Issue #${issue.number}: ${issue.title}`,
    `URL: ${issue.url}`,
    issue.author?.login ? `Author: @${issue.author.login}` : null,
    labels ? `Labels: ${labels}` : null,
    '',
    truncatedBody ? `Description:\n${truncatedBody}` : 'No description provided.',
    '',
    'Inspect the codebase, diagnose the root cause or requirements, implement the fix/feature, and run the project tests to verify.',
  ].filter((line): line is string => line !== null);
  return {
    title: `Issue #${issue.number}: ${issue.title}`.slice(0, 120),
    prompt: lines.join('\n'),
  };
}

export function pullSessionSpec(pull: GitHubPullSummary): { title: string; prompt: string } {
  const labels = pull.labels.map((l) => l.name).filter(Boolean).join(', ');
  const body = (pull.body ?? '').trim();
  const truncatedBody = body.length > 8000 ? `${body.slice(0, 8000)}\n\n…(truncated)` : body;
  const lines = [
    `Work on GitHub Pull Request #${pull.number}: ${pull.title}`,
    `Branch: ${pull.headRefName} -> ${pull.baseRefName}`,
    `URL: ${pull.url}`,
    pull.author?.login ? `Author: @${pull.author.login}` : null,
    labels ? `Labels: ${labels}` : null,
    pull.mergeable ? `Mergeable: ${pull.mergeable}` : null,
    '',
    truncatedBody ? `Description:\n${truncatedBody}` : 'No description provided.',
    '',
    'Review the PR changes and any failing checks or feedback, inspect the relevant files in this repository, and propose or implement the needed updates.',
  ].filter((line): line is string => line !== null);
  return {
    title: `PR #${pull.number}: ${pull.title}`.slice(0, 120),
    prompt: lines.join('\n'),
  };
}

export function batchIssuesSessionSpec(issues: readonly GitHubIssueSummary[]): { title: string; prompt: string } {
  const list = issues
    .map((issue) => {
      const summary = (issue.body ?? '').trim().split('\n')[0]?.slice(0, 180) ?? '';
      return `- #${issue.number}: ${issue.title} (${issue.url})${summary ? `\n  ${summary}` : ''}`;
    })
    .join('\n');
  return {
    title: `Issues (${issues.length}): ${issues.map((i) => `#${i.number}`).join(', ')}`.slice(0, 120),
    prompt: [
      `Triage and address the following ${issues.length} GitHub issue(s) in this project:`,
      '',
      list,
      '',
      'Inspect the codebase, prioritize and address each issue, and verify with the project test suite.',
    ].join('\n'),
  };
}

export function batchPullsSessionSpec(pulls: readonly GitHubPullSummary[]): { title: string; prompt: string } {
  const list = pulls
    .map(
      (pull) =>
        `- #${pull.number}: ${pull.title} [${pull.headRefName} -> ${pull.baseRefName}] (${pull.url})`,
    )
    .join('\n');
  return {
    title: `PRs (${pulls.length}): ${pulls.map((p) => `#${p.number}`).join(', ')}`.slice(0, 120),
    prompt: [
      `Review and work through the following ${pulls.length} GitHub pull request(s) in this project:`,
      '',
      list,
      '',
      'Inspect each PR’s branch/changes, check for conflicts or failing checks, and summarize or fix what is needed.',
    ].join('\n'),
  };
}