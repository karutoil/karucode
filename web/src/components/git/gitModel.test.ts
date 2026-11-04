import { describe, expect, it } from 'vitest';
import type { GitFile } from '../../api/types';
import {
  branchLabel,
  branchNameProblem,
  canCommit,
  changeCounts,
  describeStatus,
  groupFiles,
  isConflict,
  isUntracked,
  porcelainCode,
  statusLabel,
  statusLetter,
  statusTone,
  type GitSelection,
  selectedFile,
  splitPath,
} from './gitModel';

function file(partial: Partial<GitFile> & { path: string }): GitFile {
  return {
    index: ' ',
    worktree: ' ',
    status: 'M',
    staged: false,
    ...partial,
  };
}

describe('gitModel — grouping', () => {
  it('separates staged, changes and untracked, and drops empty groups', () => {
    const groups = groupFiles([
      file({ path: 'a.ts', index: 'M', worktree: ' ', status: 'M', staged: true }),
      file({ path: 'b.ts', index: ' ', worktree: 'M', status: 'M' }),
      file({ path: 'c.ts', index: '?', worktree: '?', status: '??', untracked: true }),
    ]);
    expect(groups.map((group) => group.id)).toEqual(['staged', 'changes', 'untracked']);
    expect(groups[0]?.files.map((entry) => entry.path)).toEqual(['a.ts']);
    expect(groups[1]?.files.map((entry) => entry.path)).toEqual(['b.ts']);
    expect(groups[2]?.files.map((entry) => entry.path)).toEqual(['c.ts']);
  });

  it('lists a file with both a staged and a worktree side in both groups', () => {
    const groups = groupFiles([
      file({
        path: 'both.ts',
        index: 'M',
        worktree: 'M',
        status: 'MM',
        staged: true,
        unstaged: true,
      }),
    ]);
    expect(groups.map((group) => group.id)).toEqual(['staged', 'changes']);
    expect(groups[0]?.files).toHaveLength(1);
    expect(groups[1]?.files).toHaveLength(1);
  });

  it('puts an unmerged path in its own group and nowhere else', () => {
    const conflicted = file({
      path: 'clash.ts',
      index: 'U',
      worktree: 'U',
      status: 'UU',
      conflict: true,
      unstaged: true,
    });
    expect(isConflict(conflicted)).toBe(true);
    const groups = groupFiles([conflicted]);
    expect(groups.map((group) => group.id)).toEqual(['conflicts']);
  });

  it('understands the older status shape, which had no index/worktree letters', () => {
    const legacy = { path: 'old.ts', status: '??', staged: false } as unknown as GitFile;
    expect(isUntracked(legacy)).toBe(true);
    expect(groupFiles([legacy]).map((group) => group.id)).toEqual(['untracked']);
    const legacyModified = { path: 'mod.ts', status: 'M', staged: false } as unknown as GitFile;
    expect(groupFiles([legacyModified]).map((group) => group.id)).toEqual(['changes']);
  });
});

describe('gitModel — badge, label, tone', () => {
  it('prefers the index side for a staged file and the worktree side otherwise', () => {
    expect(statusLetter(file({ path: 'x', index: 'A', worktree: ' ', status: 'A', staged: true }))).toBe('A');
    expect(statusLetter(file({ path: 'x', index: 'M', worktree: 'D', status: 'MD', staged: true }))).toBe('M');
    expect(statusLetter(file({ path: 'x', index: ' ', worktree: 'D', status: 'D' }))).toBe('D');
    expect(
      statusLetter(file({ path: 'x', index: '?', worktree: '?', status: '??', untracked: true })),
    ).toBe('?');
  });

  it('spells the status out so colour is never the only signal', () => {
    expect(statusLabel(file({ path: 'a', index: 'A', status: 'A', staged: true }))).toBe('Added');
    expect(statusLabel(file({ path: 'a', index: ' ', worktree: 'D', status: 'D' }))).toBe('Deleted');
    expect(statusLabel(file({ path: 'a', index: 'R', worktree: ' ', status: 'R', staged: true }))).toBe(
      'Renamed',
    );
    expect(statusLabel(file({ path: 'a', status: '??', untracked: true }))).toBe('Untracked');
    expect(statusTone(file({ path: 'a', index: 'A', status: 'A', staged: true }))).toBe('ok');
    expect(statusTone(file({ path: 'a', index: ' ', worktree: 'D', status: 'D' }))).toBe('danger');
    expect(statusTone(file({ path: 'a', index: ' ', worktree: 'M', status: 'M' }))).toBe('warn');
  });

  it('renders the porcelain code for the tooltip', () => {
    expect(porcelainCode(file({ path: 'a', index: 'M', worktree: 'M', status: 'MM' }))).toBe('MM');
    expect(porcelainCode(file({ path: 'a', index: '?', worktree: '?', status: '??' }))).toBe('??');
    expect(porcelainCode(file({ path: 'a', index: ' ', worktree: 'M', status: 'M' }))).toBe(' M');
  });
});

describe('gitModel — counts and sentences', () => {
  it('counts paths, not group memberships', () => {
    const counts = changeCounts([
      file({ path: 'a', index: 'M', worktree: 'M', status: 'MM', staged: true, unstaged: true }),
      file({ path: 'b', status: '??', untracked: true }),
      file({ path: 'c', index: 'U', worktree: 'U', status: 'UU', conflict: true }),
    ]);
    expect(counts).toEqual({ total: 3, staged: 1, changes: 1, untracked: 1, conflicts: 1 });
  });

  it('describes the tree, and says nothing for a non-repository', () => {
    expect(describeStatus(null)).toBeNull();
    expect(describeStatus({ is_repo: false, branch: null, error: null, files: [] })).toBeNull();
    expect(
      describeStatus({ is_repo: true, branch: 'main', error: null, files: [] }),
    ).toBe('Working tree clean');
    expect(
      describeStatus({
        is_repo: true,
        branch: 'main',
        error: null,
        files: [
          file({ path: 'a', index: 'M', status: 'M', staged: true }),
          file({ path: 'b', status: '??', untracked: true }),
        ],
      }),
    ).toBe('2 changes · 1 staged · 1 untracked');
  });

  it('names the branch, including a detached HEAD', () => {
    expect(branchLabel(null)).toBeNull();
    expect(branchLabel({ is_repo: false, branch: null, error: null, files: [] })).toBeNull();
    expect(branchLabel({ is_repo: true, branch: 'main', error: null, files: [] })).toBe('main');
    expect(
      branchLabel({ is_repo: true, branch: null, detached: true, error: null, files: [] }),
    ).toBe('detached HEAD');
  });
});

describe('gitModel — commit gate', () => {
  const dirty = {
    is_repo: true,
    branch: 'main',
    error: null,
    files: [file({ path: 'a', status: 'M' })],
  };

  it('needs a message and something to commit', () => {
    expect(canCommit('', dirty, { amend: false }).enabled).toBe(false);
    expect(canCommit('   ', dirty, { amend: false }).reason).toBe('Write a commit message first');
    expect(canCommit('msg', { ...dirty, files: [] }, { amend: false }).reason).toBe('Nothing to commit');
    expect(canCommit('msg', dirty, { amend: false }).enabled).toBe(true);
    expect(canCommit('msg', null, { amend: false }).reason).toBe('Waiting for the git status…');
    expect(
      canCommit('msg', { is_repo: false, branch: null, error: null, files: [] }, { amend: false }).reason,
    ).toBe('Not a git repository');
  });

  it('allows an amend with no message (git keeps the previous one)', () => {
    expect(canCommit('', dirty, { amend: true }).enabled).toBe(true);
  });

  it('refuses to commit an unmerged path before asking git', () => {
    const conflicted = {
      is_repo: true,
      branch: 'main',
      error: null,
      files: [file({ path: 'a', status: 'UU', conflict: true })],
    };
    expect(canCommit('msg', conflicted, { amend: false }).reason).toBe(
      'Resolve the merge conflicts first',
    );
  });
});

describe('gitModel — paths, branch names, selection', () => {
  it('splits a path into name and directory', () => {
    expect(splitPath('src/plugins/loader.rs')).toEqual({ name: 'loader.rs', dir: 'src/plugins' });
    expect(splitPath('README.md')).toEqual({ name: 'README.md', dir: '' });
  });

  it('mirrors the server branch-name rules for immediate feedback', () => {
    expect(branchNameProblem('feat/x-1')).toBeNull();
    expect(branchNameProblem('')).not.toBeNull();
    expect(branchNameProblem('-dash')).not.toBeNull();
    expect(branchNameProblem('a b')).not.toBeNull();
    expect(branchNameProblem('a..b')).not.toBeNull();
    expect(branchNameProblem('at@{1}')).not.toBeNull();
    expect(branchNameProblem('caret^')).not.toBeNull();
    expect(branchNameProblem('x.lock')).not.toBeNull();
  });

  it('only returns a file for a file selection', () => {
    const fileSelection: GitSelection = { kind: 'file', path: 'a.ts', staged: true, group: 'staged' };
    expect(selectedFile(fileSelection)).toEqual({ path: 'a.ts', staged: true });
    expect(selectedFile({ kind: 'commit', sha: 'abc', subject: 'x' })).toBeNull();
    expect(selectedFile(null)).toBeNull();
  });
});