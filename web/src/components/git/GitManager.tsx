import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { GitBranch, GitFile } from '../../api/types';
import * as store from '../../state/store';
import { AlertIcon, BranchIcon, ChevronDownIcon, RefreshIcon } from '../icons';
import { GitBranchDialog } from './GitBranchDialog';
import { GitChanges } from './GitChanges';
import { GitCommitBox } from './GitCommitBox';
import { GitConfirmDialog, type ConfirmRequest } from './GitConfirmDialog';
import { GitDiffPane } from './GitDiffPane';
import { GitHistory } from './GitHistory';
import {
  type GitGroup,
  type GitGroupId,
  type GitSelection,
  branchLabel,
  changeCounts,
  describeStatus,
  groupFiles,
  isUntracked,
} from './gitModel';
import { useGitRepo } from './useGitRepo';

export type GitManagerVariant = 'page' | 'panel';

export interface GitManagerProps {
  /**
   * `page` is the full-width workspace view (change list beside the patch);
   * `panel` is the inspector tab, one column with the patch inline.
   */
  variant?: GitManagerVariant;
  /**
   * Whether this manager is the view on screen.
   *
   * The full-width view stays mounted while another workspace tab is selected,
   * so without this it would show the tree from whenever it was last read - and
   * the agent edits files while you are looking at the transcript. Going
   * `false -> true` re-reads; the first render does not (it loads anyway).
   */
  active?: boolean;
}

/** The status row for a selected path, preferring the group the row came from. */
function findFile(
  selection: GitSelection | null,
  files: readonly GitFile[],
): GitFile | null {
  if (!selection || selection.kind !== 'file') return null;
  const matches = files.filter((file) => file.path === selection.path);
  if (matches.length === 0) return null;
  const wanted = selection.staged;
  return matches.find((file) => file.staged === wanted) ?? matches[0] ?? null;
}

/**
 * The source-control manager: the working tree, the commit box, branches and the
 * history in one panel.
 *
 * Everything it can *do* is local to this checkout (stage, unstage, discard,
 * commit, switch/create/delete a branch, stash). Fetch/pull/push are deliberately
 * absent: they need credentials and a prompt, which is the terminal's job, and a
 * web button that silently hangs on an SSH passphrase would be a worse experience
 * than not offering it.
 */
export function GitManager({ variant = 'panel', active = true }: GitManagerProps): JSX.Element {
  const projectId = store.currentProjectId.value;
  const repo = useGitRepo(projectId);
  const [selection, setSelection] = useState<GitSelection | null>(null);
  const [branchesOpen, setBranchesOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  useEffect(() => {
    // Paths, SHAs and open confirmations from repository A are meaningless in B.
    setSelection(null);
    setBranchesOpen(false);
    setConfirm(null);
  }, [projectId]);

  // A manager that was hidden while the agent worked re-reads the tree when it
  // comes back, instead of showing what it last saw.
  const wasActive = useRef(active);
  useEffect(() => {
    if (wasActive.current === active) return;
    wasActive.current = active;
    if (active) repo.refresh();
  }, [active, repo.refresh]);

  const status = repo.status;
  const files = status?.files ?? [];
  const counts = changeCounts(files);
  const groups = groupFiles(files);
  const branch = branchLabel(status);
  const busy = repo.busy !== null;

  // The full-width view has room for the history, so it is already there when the
  // view opens; the inspector waits for the section to be expanded.
  useEffect(() => {
    if (variant !== 'page') return;
    repo.loadHistory();
    // `loadHistory` is rebuilt when the project changes, which is exactly when the
    // history has to be reloaded - so it is a real dependency.
  }, [variant, repo.loadHistory]);

  const requestDiscardFile = (file: GitFile, group: GitGroupId): void => {
    const untracked = group === 'untracked' || isUntracked(file);
    const staged = group === 'staged';
    setConfirm({
      title: untracked
        ? `Delete ${file.path}?`
        : staged
          ? `Discard the staged changes in ${file.path}?`
          : `Discard the changes in ${file.path}?`,
      description: untracked
        ? 'The file is not tracked by git, so there is no version to restore: it is deleted from disk.'
        : staged
          ? 'The index and the working tree are restored to the last commit for this file.'
          : 'The working tree is restored from the index. Anything staged for this file is kept.',
      confirmLabel: untracked ? 'Delete file' : 'Discard changes',
      note: 'This cannot be undone from this panel.',
      onConfirm: async () => {
        await repo.discard([file.path], staged);
      },
    });
  };

  const requestDiscardGroup = (group: GitGroup): void => {
    const paths = group.files.map((file) => file.path);
    setConfirm({
      title:
        group.id === 'untracked'
          ? `Delete ${paths.length} untracked file(s)?`
          : `Discard every change in ${group.label}?`,
      description:
        group.id === 'untracked'
          ? 'These files are not tracked by git, so there is no version to restore: they are deleted from disk.'
          : group.id === 'staged'
            ? 'The index and the working tree are restored to the last commit for these files.'
            : 'The working tree is restored from the index for these files. Staged versions are kept.',
      confirmLabel: group.id === 'untracked' ? 'Delete files' : 'Discard changes',
      note: 'This cannot be undone from this panel.',
      onConfirm: async () => {
        await repo.discard(paths, group.id === 'staged');
      },
    });
  };

  const requestDeleteBranch = (target: GitBranch): void => {
    setConfirm({
      title: `Delete branch ${target.name}?`,
      description: `The branch ref is removed. Commits that no other branch contains become unreachable.`,
      confirmLabel: 'Delete branch',
      note: 'Git refuses to delete an unmerged branch from this panel; delete it from the terminal with `git branch -D` if you are sure.',
      onConfirm: async () => {
        await repo.deleteBranch(target.name);
      },
    });
  };

  const requestDropStash = (index: number, ref: string): void => {
    setConfirm({
      title: `Drop ${ref}?`,
      description: 'The stash entry is deleted. The changes it held are not applied.',
      confirmLabel: 'Drop stash',
      note: 'This cannot be undone from this panel.',
      onConfirm: async () => {
        await repo.stash('drop', { index });
      },
    });
  };

  const onSelectFile = (file: GitFile, group: GitGroupId): void => {
    const staged = group === 'staged';
    const same =
      selection?.kind === 'file' &&
      selection.path === file.path &&
      selection.staged === staged &&
      selection.group === group;
    setSelection(
      same ? null : { kind: 'file', path: file.path, staged, group },
    );
  };

  const selectedFile = findFile(selection, files);

  const branchChip = (
    <button
      type="button"
      class="gitm-branch"
      onClick={() => setBranchesOpen(true)}
      aria-haspopup="dialog"
      title="Switch, create or delete a branch"
    >
      <span class="gitm-branch-glyph" aria-hidden="true">
        <BranchIcon size={13} />
      </span>
      <span class="gitm-branch-label">{branch ?? 'no branch'}</span>
      {status?.upstream ? (
        <span
          class="gitm-branch-track"
          title={`Tracking ${status.upstream}${
            counts.total === 0 ? ' — nothing to sync' : ''
          }`}
        >
          {status.ahead ? `↑${status.ahead}` : ''}
          {status.behind ? `↓${status.behind}` : ''}
          {!status.ahead && !status.behind ? 'synced' : ''}
        </span>
      ) : null}
      <span class="gitm-branch-chevron" aria-hidden="true">
        <ChevronDownIcon size={12} />
      </span>
    </button>
  );

  const refreshButton = (
    <button
      type="button"
      class="icon-button gitm-refresh"
      aria-label="Refresh git status"
      title="Refresh git status"
      disabled={repo.state === 'loading' || busy}
      onClick={repo.refresh}
    >
      <RefreshIcon size={14} class={repo.state === 'loading' ? 'spin' : undefined} />
    </button>
  );

  const notRepo =
    repo.state === 'ready' && status && !status.is_repo ? (
      <div class="empty-inspector" key="not-repo">
        <p class="empty-inspector-title">
          {status.error ? 'Git unavailable' : 'Not a git repository'}
        </p>
        <p>
          {status.error
            ? 'The git status probe failed, so there is no working tree to manage here.'
            : 'This workspace is not tracked by git, so there is nothing to stage, commit or review. `git init` in the project directory turns this panel on.'}
        </p>
        {status.error ? <p class="muted small">{status.error}</p> : null}
      </div>
    ) : null;

  const probeError =
    repo.state === 'error' ? (
      <div class="inline-error" role="alert" key="probe-error">
        <p>{repo.error}</p>
        <button type="button" class="button button-small" onClick={repo.refresh}>
          Retry
        </button>
      </div>
    ) : null;

  const statusWarning =
    status?.is_repo && status.error ? (
      <div class="inline-warning" role="status" key="status-warning">
        <p>
          <AlertIcon size={13} /> {status.error}
        </p>
      </div>
    ) : null;

  const actionError = repo.actionError ? (
    <div class="inline-error gitm-action-error" role="alert" key="action-error">
      <p>{repo.actionError}</p>
      <button type="button" class="button button-small" onClick={repo.clearActionError}>
        Dismiss
      </button>
    </div>
  ) : null;

  const loading =
    repo.state === 'loading' && status === null ? (
      <div class="gitm-loading" aria-busy="true" aria-label="Loading git status" key="loading">
        {[0, 1, 2, 3, 4].map((row) => (
          <div class="skeleton gitm-skeleton-row" key={row} />
        ))}
      </div>
    ) : null;

  const changesBlock =
    status?.is_repo === true ? (
      <GitChanges
        key="changes"
        variant={variant}
        groups={groups}
        selection={selection}
        busy={busy}
        onSelectFile={onSelectFile}
        onStage={(paths) => void repo.stage(paths)}
        onUnstage={(paths) => void repo.unstage(paths)}
        onDiscard={requestDiscardFile}
        onStageGroup={(group) => void repo.stage(group.files.map((file) => file.path))}
        onUnstageGroup={(group) => void repo.unstage(group.files.map((file) => file.path))}
        onDiscardGroup={requestDiscardGroup}
      />
    ) : null;

  const historyBlock = (
    <GitHistory
      key="history"
      variant={variant}
      commits={repo.commits}
      stashes={repo.stashes}
      status={status}
      selectedSha={selection?.kind === 'commit' ? selection.sha : null}
      busy={busy}
      defaultOpen={variant === 'page'}
      onLoadHistory={repo.loadHistory}
      onLoadStashes={repo.loadStashes}
      onSelectCommit={(commit) =>
        setSelection((current) =>
          current?.kind === 'commit' && current.sha === commit.id
            ? null
            : { kind: 'commit', sha: commit.id, subject: commit.subject },
        )
      }
      onStash={(action, includeUntracked) => void repo.stash(action, { includeUntracked })}
      onStashEntry={(action, stash) => {
        if (action === 'drop') requestDropStash(stash.index, stash.ref);
        else void repo.stash(action, { index: stash.index });
      }}
    />
  );

  const branchDialog = (
    <GitBranchDialog
      key="branch-dialog"
      open={branchesOpen}
      onClose={() => setBranchesOpen(false)}
      branches={repo.branches}
      current={status?.branch ?? null}
      detached={status?.detached ?? false}
      busy={busy}
      onCheckout={repo.checkout}
      onCreate={repo.createBranch}
      onRequestDelete={requestDeleteBranch}
    />
  );

  const commitBox = (
    <GitCommitBox
      key="commit-box"
      variant={variant}
      status={status}
      projectId={projectId}
      branch={status?.branch ?? null}
      busy={repo.busy}
      onCommit={(message, options) => repo.commit(message, options)}
    />
  );

  if (variant === 'page') {
    return (
      <div class="gitm gitm-page">
        <header class="gitm-head">
          <div class="gitm-head-titles">
            <p class="gitm-kicker">Source control</p>
            <div class="gitm-head-line">
              {branchChip}
              <h2 class="gitm-title">Working tree</h2>
            </div>
            <p class="gitm-desc">{describeStatus(status) ?? 'No repository to show'}</p>
          </div>
          <div class="gitm-head-actions">
            {status?.is_repo ? (
              // Only what exists: three chips reading "0" are noise next to a
              // sentence that already says the tree is clean.
              <span class="gitm-chips" aria-label="Change counts">
                {counts.staged > 0 ? (
                  <span class="gitm-chip is-staged" title="Staged files">
                    staged {counts.staged}
                  </span>
                ) : null}
                {counts.changes > 0 ? (
                  <span class="gitm-chip is-changes" title="Unstaged modified files">
                    modified {counts.changes}
                  </span>
                ) : null}
                {counts.untracked > 0 ? (
                  <span class="gitm-chip is-untracked" title="Untracked files">
                    untracked {counts.untracked}
                  </span>
                ) : null}
                {counts.conflicts > 0 ? (
                  <span class="gitm-chip is-conflict" title="Unresolved merge conflicts">
                    conflicts {counts.conflicts}
                  </span>
                ) : null}
                {counts.total === 0 ? <span class="gitm-chip is-clean">clean</span> : null}
              </span>
            ) : null}
            {refreshButton}
          </div>
        </header>

        {status?.is_repo ? (
          <div class="gitm-body" key="body">
            <aside class="gitm-side" aria-label="Source control actions">
              {commitBox}
              <div class="gitm-side-scroll">
                {statusWarning}
                {actionError}
                {changesBlock}
                {historyBlock}
              </div>
            </aside>
            <section class="gitm-main" aria-label="Diff">
              <GitDiffPane
                key="diff-pane"
                variant="page"
                selection={selection}
                projectId={projectId}
                reloadKey={repo.revision}
                busy={busy}
                file={selectedFile}
                onStage={(paths) => void repo.stage(paths)}
                onUnstage={(paths) => void repo.unstage(paths)}
                onDiscard={(file, staged) => {
                  const group: GitGroupId = staged ? 'staged' : isUntracked(file) ? 'untracked' : 'changes';
                  requestDiscardFile(file, group);
                }}
                onClear={() => setSelection(null)}
              />
            </section>
          </div>
        ) : (
          // One explanation, not a sidebar and a pane: there is no working tree to
          // put in them.
          <div class="gitm-empty-body" key="empty">
            {probeError}
            {notRepo}
            {loading}
          </div>
        )}

        {branchDialog}
        <GitConfirmDialog key="confirm" request={confirm} onClose={() => setConfirm(null)} />
      </div>
    );
  }

  return (
    <div class="gitm gitm-panel inspector-section">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Source control</span>
        <span class="gitm-kicker-actions">
          {busy ? <span class="gitm-busy">{repo.busy}</span> : null}
          {refreshButton}
        </span>
      </div>

      {status?.is_repo ? <div class="gitm-panel-branch">{branchChip}</div> : null}

      {probeError}
      {notRepo}
      {loading}
      {statusWarning}
      {actionError}

      {status?.is_repo ? (
        <>
          {commitBox}
          {changesBlock}
          {selection?.kind === 'file' || selection?.kind === 'commit' ? (
            <section class="gitm-panel-detail" aria-label="Selected change">
              <GitDiffPane
                variant="panel"
                selection={selection}
                projectId={projectId}
                reloadKey={repo.revision}
                busy={busy}
                file={selectedFile}
                onStage={(paths) => void repo.stage(paths)}
                onUnstage={(paths) => void repo.unstage(paths)}
                onDiscard={(file, staged) => {
                  const group: GitGroupId = staged ? 'staged' : isUntracked(file) ? 'untracked' : 'changes';
                  requestDiscardFile(file, group);
                }}
                onClear={() => setSelection(null)}
              />
            </section>
          ) : null}
          {historyBlock}
          <p class="inspector-note">
            <AlertIcon size={12} /> Local only: staging, commits, branches and stashes. Pull, push and
            fetch stay in the terminal.
          </p>
        </>
      ) : null}

      {branchDialog}
      <GitConfirmDialog key="confirm" request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}