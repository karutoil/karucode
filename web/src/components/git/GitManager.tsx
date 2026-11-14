import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { GitBranch, GitFile } from '../../api/types';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import {
  AlertIcon,
  BranchIcon,
  ChevronDownIcon,
  CommitIcon,
  DownloadIcon,
  ExternalIcon,
  GitIcon,
  GlobeIcon,
  IssueIcon,
  PullRequestIcon,
  RefreshIcon,
  UploadIcon,
} from '../icons';
import { GitBranchDialog } from './GitBranchDialog';
import { GitBranchesView } from './GitBranchesView';
import { GitChanges } from './GitChanges';
import { GitCommitBox } from './GitCommitBox';
import { GitConfirmDialog, type ConfirmRequest } from './GitConfirmDialog';
import { GitDiffPane } from './GitDiffPane';
import { GitHistory } from './GitHistory';
import { GitHistoryView } from './GitHistoryView';
import { GitInitView } from './GitInitView';
import { GitIssuesView } from './GitIssuesView';
import { GitPullRequestsView } from './GitPullRequestsView';
import { GitRemotesView } from './GitRemotesView';
import {
  type GitGroup,
  type GitGroupId,
  type GitSelection,
  branchLabel,
  changeCounts,
  describeStatus,
  groupFiles,
  isUntracked,
  operationLabel,
} from './gitModel';
import { useGitRepo } from './useGitRepo';

export type GitManagerVariant = 'page' | 'panel';
export type GitSection = 'changes' | 'history' | 'branches' | 'remotes' | 'pulls' | 'issues';

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
 * The source-control manager: working tree, commit box, history, branches,
 * tags, stashes, remotes, sync, and GitHub pull requests & issues.
 */
export function GitManager({ variant = 'panel', active = true }: GitManagerProps): JSX.Element {
  const projectId = store.currentProjectId.value;
  const project = store.currentProject.value;
  const repo = useGitRepo(projectId);
  const [section, setSection] = useState<GitSection>('changes');
  const [selection, setSelection] = useState<GitSelection | null>(null);
  const [branchesOpen, setBranchesOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [idName, setIdName] = useState('');
  const [idEmail, setIdEmail] = useState('');
  const [showIdentityForm, setShowIdentityForm] = useState(false);

  useEffect(() => {
    // Paths, SHAs and open confirmations from repository A are meaningless in B.
    setSelection(null);
    setBranchesOpen(false);
    setConfirm(null);
    setSection('changes');
    setShowIdentityForm(false);
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
  const activeOp = operationLabel(status?.operation);
  const remotesCount = status?.remotes_count ?? repo.remotes?.length ?? 0;

  // The full-width view has room for the history, so it is already there when the
  // view opens; the inspector waits for the section to be expanded.
  useEffect(() => {
    if (variant !== 'page') return;
    repo.loadHistory();
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
      note: 'Git refuses to delete an unmerged branch from this dialog; use the Branches tab to force delete if you are sure.',
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
      <GitInitView
        error={status.error}
        busy={busy}
        onInit={(opts) => repo.initRepo(opts)}
      />
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

  const operationBanner =
    status?.is_repo && activeOp ? (
      <div class="gitm-op-banner" role="status">
        <span class="gitm-op-label">
          <AlertIcon size={13} /> {activeOp}
        </span>
        <div class="gitm-op-actions">
          {status.operation === 'merge' ? (
            <>
              <button
                type="button"
                class="button button-small button-primary"
                disabled={busy || counts.conflicts > 0}
                onClick={() => void repo.merge({ action: 'continue' })}
              >
                Continue merge
              </button>
              <button
                type="button"
                class="button button-small button-danger"
                disabled={busy}
                onClick={() => void repo.merge({ action: 'abort' })}
              >
                Abort merge
              </button>
            </>
          ) : status.operation === 'rebase' ? (
            <>
              <button
                type="button"
                class="button button-small button-primary"
                disabled={busy || counts.conflicts > 0}
                onClick={() => void repo.rebase({ action: 'continue' })}
              >
                Continue
              </button>
              <button
                type="button"
                class="button button-small"
                disabled={busy}
                onClick={() => void repo.rebase({ action: 'skip' })}
              >
                Skip
              </button>
              <button
                type="button"
                class="button button-small button-danger"
                disabled={busy}
                onClick={() => void repo.rebase({ action: 'abort' })}
              >
                Abort rebase
              </button>
            </>
          ) : status.operation === 'cherry-pick' ? (
            <>
              <button
                type="button"
                class="button button-small button-primary"
                disabled={busy || counts.conflicts > 0}
                onClick={() => void repo.cherryPick({ action: 'continue' })}
              >
                Continue
              </button>
              <button
                type="button"
                class="button button-small"
                disabled={busy}
                onClick={() => void repo.cherryPick({ action: 'skip' })}
              >
                Skip
              </button>
              <button
                type="button"
                class="button button-small button-danger"
                disabled={busy}
                onClick={() => void repo.cherryPick({ action: 'abort' })}
              >
                Abort
              </button>
            </>
          ) : null}
        </div>
      </div>
    ) : null;

  const missingIdentity =
    status?.is_repo &&
    status.identity !== undefined &&
    (!status.identity.name || !status.identity.email);

  const identityBanner = missingIdentity ? (
    <div class="gitm-id-banner">
      <div class="gitm-id-row">
        <span>
          <AlertIcon size={13} /> Git author identity (<code>user.name</code> /{' '}
          <code>user.email</code>) is not configured.
        </span>
        <button
          type="button"
          class="tiny-link"
          onClick={() => setShowIdentityForm((v) => !v)}
        >
          {showIdentityForm ? 'Hide' : 'Configure'}
        </button>
      </div>
      {showIdentityForm ? (
        <form
          class="gitm-id-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (busy) return;
            void repo.setIdentity(idName, idEmail).then((ok) => {
              if (ok) setShowIdentityForm(false);
            });
          }}
        >
          <input
            type="text"
            class="gitm-input"
            placeholder="Your Name"
            aria-label="Git user.name"
            value={idName}
            onInput={(e) => setIdName((e.target as HTMLInputElement).value)}
          />
          <input
            type="email"
            class="gitm-input"
            placeholder="you@example.com"
            aria-label="Git user.email"
            value={idEmail}
            onInput={(e) => setIdEmail((e.target as HTMLInputElement).value)}
          />
          <button
            type="submit"
            class="button button-small button-primary"
            disabled={busy || (idName.trim() === '' && idEmail.trim() === '')}
          >
            Save identity
          </button>
        </form>
      ) : null}
    </div>
  ) : null;

  const pageSections: Array<{
    id: GitSection;
    label: string;
    icon: JSX.Element;
    count?: number;
  }> = [
    {
      id: 'changes',
      label: 'Changes',
      icon: <GitIcon size={13} />,
      count: counts.total > 0 ? counts.total : undefined,
    },
    {
      id: 'history',
      label: 'History',
      icon: <CommitIcon size={13} />,
    },
    {
      id: 'branches',
      label: 'Branches',
      icon: <BranchIcon size={13} />,
      count: repo.branches.length > 0 ? repo.branches.length : undefined,
    },
    {
      id: 'remotes',
      label: 'Remotes',
      icon: <GlobeIcon size={13} />,
      count: remotesCount > 0 ? remotesCount : undefined,
    },
    {
      id: 'pulls',
      label: 'Pull requests',
      icon: <PullRequestIcon size={13} />,
    },
    {
      id: 'issues',
      label: 'Issues',
      icon: <IssueIcon size={13} />,
    },
  ];

  const defaultRepoName = (project?.name ?? 'repository')
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'repository';

  if (variant === 'page') {
    return (
      <div class="gitm gitm-page">
        <header class="gitm-head">
          <div class="gitm-head-titles">
            <p class="gitm-kicker">Source control</p>
            <div class="gitm-head-line">
              {status?.is_repo ? branchChip : null}
              <h2 class="gitm-title">
                {status === null || status.is_repo ? 'Working tree' : 'Repository setup'}
              </h2>
              {repo.githubStatus?.repo?.url ? (
                <a
                  class="gitm-gh-pill"
                  href={repo.githubStatus.repo.url}
                  target="_blank"
                  rel="noreferrer"
                  title="Open repository on GitHub"
                >
                  <GlobeIcon size={12} />
                  <span>{repo.githubStatus.repo.nameWithOwner}</span>
                  <ExternalIcon size={11} />
                </a>
              ) : null}
            </div>
            <p class="gitm-desc">
              {status?.is_repo
                ? (describeStatus(status) ?? 'No repository to show')
                : 'Initialize a git repository or connect a remote'}
            </p>
          </div>

          <div class="gitm-head-actions">
            {status?.is_repo ? (
              <>
                <div class="gitm-sync-bar" role="group" aria-label="Remote sync">
                  {remotesCount === 0 ? (
                    <button
                      type="button"
                      class="button button-small"
                      disabled={busy}
                      onClick={() => setSection('remotes')}
                    >
                      <GlobeIcon size={12} /> Set remote
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        class="button button-small"
                        disabled={busy}
                        title="Fetch updates from all remotes"
                        onClick={() => void repo.fetch({ all: true, prune: true })}
                      >
                        Fetch
                      </button>
                      <button
                        type="button"
                        class="button button-small"
                        disabled={busy || !status.branch}
                        title="Pull from remote"
                        onClick={() => void repo.pull()}
                      >
                        <DownloadIcon size={12} /> Pull
                        {status.behind ? ` ↓${status.behind}` : ''}
                      </button>
                      <button
                        type="button"
                        class="button button-small button-primary"
                        disabled={busy || !status.branch}
                        title={
                          status.upstream
                            ? `Push to ${status.upstream}`
                            : 'Publish branch to origin'
                        }
                        onClick={() =>
                          void repo.push({
                            set_upstream: !status.upstream,
                          })
                        }
                      >
                        <UploadIcon size={12} />{' '}
                        {status.upstream
                          ? `Push${status.ahead ? ` ↑${status.ahead}` : ''}`
                          : 'Publish'}
                      </button>
                    </>
                  )}
                </div>

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
              </>
            ) : null}
            {refreshButton}
          </div>
        </header>

        {status?.is_repo ? (
          <nav class="gitm-nav" role="tablist" aria-label="Source control sections">
            {pageSections.map((item) => {
              const selected = section === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  class={`gitm-nav-tab${selected ? ' is-active' : ''}`}
                  onClick={() => setSection(item.id)}
                >
                  {item.icon}
                  <span>{item.label}</span>
                  {item.count !== undefined ? (
                    <span class="gitm-nav-count">{item.count}</span>
                  ) : null}
                </button>
              );
            })}
          </nav>
        ) : null}

        {operationBanner}
        {identityBanner}

        {status?.is_repo ? (
          section === 'changes' ? (
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
                    const group: GitGroupId = staged
                      ? 'staged'
                      : isUntracked(file)
                        ? 'untracked'
                        : 'changes';
                    requestDiscardFile(file, group);
                  }}
                  onResolveConflict={(path, strategy) =>
                    void repo.resolveConflict(path, strategy)
                  }
                  onOpenFile={(path) => actions.openFileInTab(path)}
                  onClear={() => setSelection(null)}
                />
              </section>
            </div>
          ) : section === 'history' ? (
            <GitHistoryView
              commits={repo.commits}
              branches={repo.branches}
              projectId={projectId}
              revision={repo.revision}
              busy={busy}
              onLoadHistory={repo.loadHistory}
              onCheckoutDetached={(sha) => repo.checkout(sha, { detach: true })}
              onCreateBranchAt={(name, start) => repo.createBranch(name, { start, checkout: true })}
              onCreateTagAt={(name, target) => repo.manageTag({ action: 'create', name, target })}
              onCherryPick={(sha) => repo.cherryPick({ sha, action: 'pick' })}
              onRevert={(sha) => repo.revertCommit(sha)}
              onReset={(target, mode, confirmReset) =>
                repo.resetHead({ target, mode, confirm: confirmReset })
              }
              onRequestConfirm={setConfirm}
            />
          ) : section === 'branches' ? (
            <div class="gitm-branches-layout">
              <GitBranchesView
                status={status}
                branches={repo.branches}
                remoteBranches={repo.remoteBranches}
                tags={repo.tags}
                stashes={repo.stashes}
                busy={busy}
                onLoadTags={repo.loadTags}
                onLoadStashes={repo.loadStashes}
                onCheckout={repo.checkout}
                onCreateBranch={repo.createBranch}
                onRenameBranch={repo.renameBranch}
                onDeleteBranch={repo.deleteBranch}
                onMerge={(b) => repo.merge({ branch: b })}
                onRebase={(b) => repo.rebase({ branch: b })}
                onManageTag={repo.manageTag}
                onStash={repo.stash}
                onInspectStash={(entry) =>
                  setSelection({
                    kind: 'stash',
                    index: entry.index,
                    ref: entry.ref,
                    subject: entry.subject,
                  })
                }
                onRequestConfirm={setConfirm}
              />
              {selection?.kind === 'stash' ? (
                <section class="gitm-stash-preview" aria-label="Stash diff">
                  <GitDiffPane
                    variant="panel"
                    selection={selection}
                    projectId={projectId}
                    reloadKey={repo.revision}
                    busy={busy}
                    file={null}
                    onStage={() => {}}
                    onUnstage={() => {}}
                    onDiscard={() => {}}
                    onClear={() => setSelection(null)}
                  />
                </section>
              ) : null}
            </div>
          ) : section === 'remotes' ? (
            <GitRemotesView
              status={status}
              remotes={repo.remotes}
              githubStatus={repo.githubStatus}
              defaultRepoName={defaultRepoName}
              busy={busy}
              onLoadRemotes={repo.loadRemotes}
              onLoadGitHubStatus={repo.loadGitHubStatus}
              onManageRemote={repo.manageRemote}
              onFetch={repo.fetch}
              onPull={repo.pull}
              onPush={repo.push}
              onCreateGitHubRepo={repo.createGitHubRepo}
              onRequestConfirm={setConfirm}
            />
          ) : section === 'pulls' ? (
            <GitPullRequestsView
              projectId={projectId}
              currentBranch={status.branch}
              branches={repo.branches}
              githubStatus={repo.githubStatus}
              busy={busy}
              onLoadGitHubStatus={repo.loadGitHubStatus}
              onCheckoutPull={repo.checkoutPull}
              onOpenRemotes={() => setSection('remotes')}
              onRequestConfirm={setConfirm}
            />
          ) : (
            <GitIssuesView
              projectId={projectId}
              githubStatus={repo.githubStatus}
              busy={busy}
              onLoadGitHubStatus={repo.loadGitHubStatus}
              onOpenRemotes={() => setSection('remotes')}
              onRequestConfirm={setConfirm}
            />
          )
        ) : (
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
      {operationBanner}

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
                  const group: GitGroupId = staged
                    ? 'staged'
                    : isUntracked(file)
                      ? 'untracked'
                      : 'changes';
                  requestDiscardFile(file, group);
                }}
                onResolveConflict={(path, strategy) =>
                  void repo.resolveConflict(path, strategy)
                }
                onOpenFile={(path) => actions.openFileInTab(path)}
                onClear={() => setSelection(null)}
              />
            </section>
          ) : null}
          {historyBlock}
        </>
      ) : null}

      {branchDialog}
      <GitConfirmDialog key="confirm" request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
