import type { JSX } from 'preact';
import { useCallback, useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import {
  commentGitHubPull,
  createGitHubPull,
  getGitHubPull,
  getGitHubPullDiff,
  listGitHubPulls,
  mergeGitHubPull,
  setGitHubPullState,
} from '../../api/endpoints';
import type {
  GitBranch,
  GitDiff,
  GitHubPullDetail,
  GitHubPullSummary,
  GitHubStatus,
} from '../../api/types';
import { relativeTime } from '../../lib/format';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { DiffView } from '../DiffView';
import { Markdown } from '../Markdown';
import {
  BranchIcon,
  ExternalIcon,
  PlusIcon,
  PullRequestIcon,
  RefreshIcon,
  SearchIcon,
  SparkIcon,
} from '../icons';
import { Button, Callout, Checkbox, IconButton, Input, LinkButton, Select, Textarea } from '../../ui';
import { batchPullsSessionSpec, pullSessionSpec } from './gitModel';
import type { ConfirmRequest } from './GitConfirmDialog';

export interface GitPullRequestsViewProps {
  projectId: string | null;
  currentBranch: string | null;
  branches: GitBranch[];
  githubStatus: GitHubStatus | null;
  busy: boolean;
  onLoadGitHubStatus: () => void;
  onCheckoutPull: (number: number) => Promise<boolean>;
  onOpenRemotes: () => void;
  onRequestConfirm: (request: ConfirmRequest) => void;
}

type PrStateFilter = 'open' | 'closed' | 'merged' | 'all';

function checkSummary(pull: GitHubPullSummary): { label: string; tone: 'ok' | 'danger' | 'warn' } | null {
  const checks = pull.statusCheckRollup ?? [];
  if (checks.length === 0) return null;
  let failed = 0;
  let pending = 0;
  for (const c of checks) {
    const conclusion = (c.conclusion ?? '').toUpperCase();
    const status = (c.status ?? c.state ?? '').toUpperCase();
    if (conclusion === 'FAILURE' || conclusion === 'ERROR' || conclusion === 'TIMED_OUT' || status === 'FAILURE' || status === 'ERROR') {
      failed += 1;
    } else if (status === 'IN_PROGRESS' || status === 'QUEUED' || status === 'PENDING' || (!conclusion && status !== 'COMPLETED')) {
      pending += 1;
    }
  }
  if (failed > 0) return { label: `${failed} failed`, tone: 'danger' };
  if (pending > 0) return { label: `${pending} pending`, tone: 'warn' };
  return { label: `${checks.length} passed`, tone: 'ok' };
}

export function GitPullRequestsView({
  projectId,
  currentBranch,
  branches,
  githubStatus,
  busy,
  onLoadGitHubStatus,
  onCheckoutPull,
  onOpenRemotes,
  onRequestConfirm,
}: GitPullRequestsViewProps): JSX.Element {
  const [stateFilter, setStateFilter] = useState<PrStateFilter>('open');
  const [search, setSearch] = useState('');
  const [pulls, setPulls] = useState<GitHubPullSummary[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [selectedNumber, setSelectedNumber] = useState<number | null>(null);
  const [detail, setDetail] = useState<GitHubPullDetail | null>(null);
  const [detailTab, setDetailTab] = useState<'overview' | 'diff'>('overview');
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [newBody, setNewBody] = useState('');
  const [newBase, setNewBase] = useState(
    githubStatus?.repo?.defaultBranchRef?.name ?? 'main',
  );
  const [newDraft, setNewDraft] = useState(false);

  const [commentText, setCommentText] = useState('');
  const [working, setWorking] = useState(false);
  const [launchedSessions, setLaunchedSessions] = useState<Record<number, string>>({});

  useEffect(() => {
    onLoadGitHubStatus();
  }, [onLoadGitHubStatus]);

  useEffect(() => {
    if (githubStatus?.repo?.defaultBranchRef?.name) {
      setNewBase(githubStatus.repo.defaultBranchRef.name);
    }
  }, [githubStatus?.repo?.defaultBranchRef?.name]);

  const ready = Boolean(
    githubStatus?.gh_installed && githubStatus?.authenticated && githubStatus?.remote_slug,
  );

  const fetchPulls = useCallback(() => {
    if (!ready) return;
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    listGitHubPulls(projectId, { state: stateFilter, limit: 40, signal: controller.signal })
      .then((res) => {
        if (!controller.signal.aborted) setPulls(res.pulls);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setError(toErrorMessage(cause));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
  }, [projectId, stateFilter, ready]);

  useEffect(() => {
    setSelectedNumber(null);
    setDetail(null);
    setDiff(null);
    setPulls(null);
    if (ready) fetchPulls();
  }, [projectId, stateFilter, ready, fetchPulls]);

  useEffect(() => {
    if (selectedNumber === null || !ready) {
      setDetail(null);
      setDiff(null);
      return;
    }
    const controller = new AbortController();
    setDetailLoading(true);
    setDetailTab('overview');
    setDiff(null);
    getGitHubPull(selectedNumber, projectId, { signal: controller.signal })
      .then((res) => {
        if (!controller.signal.aborted) setDetail(res.pull);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) store.pushToast(toErrorMessage(cause), 'error');
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoading(false);
      });
    return () => controller.abort();
  }, [selectedNumber, projectId, ready]);

  const loadPullDiff = (num: number): void => {
    setDetailTab('diff');
    if (diff !== null) return;
    getGitHubPullDiff(num, projectId)
      .then(setDiff)
      .catch((cause: unknown) => store.pushToast(toErrorMessage(cause), 'error'));
  };

  if (githubStatus === null) {
    return (
      <div class="gitm-empty">
        <p class="gitm-empty-title">Checking GitHub status…</p>
      </div>
    );
  }

  if (!githubStatus.gh_installed || !githubStatus.authenticated || !githubStatus.remote_slug) {
    return (
      <div class="gitm-empty">
        <p class="gitm-empty-title">GitHub pull requests unavailable</p>
        <p class="gitm-empty-text">
          {!githubStatus.gh_installed
            ? 'Install the GitHub CLI (`gh`) on this machine to browse, review, and work on pull requests.'
            : !githubStatus.authenticated
              ? 'Sign in with `gh auth login` in the Terminal tab to access pull requests.'
              : 'This repository does not have a GitHub remote yet.'}
        </p>
        {githubStatus.authenticated && !githubStatus.remote_slug ? (
          <div>
            <Button variant="primary" testId="git-prs-open-remotes" onClick={onOpenRemotes}>
              Connect or create GitHub remote
            </Button>
          </div>
        ) : null}
      </div>
    );
  }

  const needle = search.trim().toLowerCase();
  const visible = (pulls ?? []).filter((pr) => {
    if (!needle) return true;
    return (
      pr.title.toLowerCase().includes(needle) ||
      `#${pr.number}`.includes(needle) ||
      pr.headRefName.toLowerCase().includes(needle) ||
      (pr.author?.login ?? '').toLowerCase().includes(needle)
    );
  });

  const startSingleSession = async (pr: GitHubPullSummary): Promise<void> => {
    setWorking(true);
    const spec = pullSessionSpec(pr);
    const created = await actions.launchSessionWithPrompt({
      title: spec.title,
      prompt: spec.prompt,
      projectId,
      focus: true,
    });
    if (created) {
      setLaunchedSessions((prev) => ({ ...prev, [pr.number]: created.sessionId }));
    }
    setWorking(false);
  };

  const startCombinedSession = async (): Promise<void> => {
    if (visible.length === 0) return;
    setWorking(true);
    const spec = batchPullsSessionSpec(visible);
    await actions.launchSessionWithPrompt({
      title: spec.title,
      prompt: spec.prompt,
      projectId,
      focus: true,
    });
    setWorking(false);
  };

  const startSeparateSessions = async (): Promise<void> => {
    if (visible.length === 0) return;
    const launchAll = async (): Promise<void> => {
      setWorking(true);
      let count = 0;
      const nextMap: Record<number, string> = {};
      for (const pr of visible) {
        const spec = pullSessionSpec(pr);
        const created = await actions.launchSessionWithPrompt({
          title: spec.title,
          prompt: spec.prompt,
          projectId,
          focus: false,
        });
        if (created) {
          count += 1;
          nextMap[pr.number] = created.sessionId;
        }
      }
      setLaunchedSessions((prev) => ({ ...prev, ...nextMap }));
      setWorking(false);
      if (count > 0) {
        store.pushToast(`Started ${count} karucode session${count === 1 ? '' : 's'} for pull requests.`, 'success');
      }
    };
    if (visible.length > 3) {
      onRequestConfirm({
        title: `Start ${visible.length} separate sessions?`,
        description: `This will create and run ${visible.length} parallel karucode sessions (one per pull request).`,
        confirmLabel: `Start ${visible.length} sessions`,
        onConfirm: launchAll,
      });
    } else {
      await launchAll();
    }
  };

  const submitCreatePr = async (event: Event): Promise<void> => {
    event.preventDefault();
    if (working || newTitle.trim() === '') return;
    setWorking(true);
    try {
      const res = await createGitHubPull(
        {
          title: newTitle.trim(),
          body: newBody.trim() || undefined,
          base: newBase.trim() || undefined,
          head: currentBranch ?? undefined,
          draft: newDraft,
        },
        projectId,
      );
      store.pushToast(res.message, 'success');
      setCreating(false);
      setNewTitle('');
      setNewBody('');
      setNewDraft(false);
      fetchPulls();
    } catch (cause: unknown) {
      store.pushToast(toErrorMessage(cause), 'error');
    } finally {
      setWorking(false);
    }
  };

  const submitComment = async (event: Event): Promise<void> => {
    event.preventDefault();
    if (!detail || working || commentText.trim() === '') return;
    setWorking(true);
    try {
      await commentGitHubPull(detail.number, commentText.trim(), projectId);
      setCommentText('');
      store.pushToast(`Commented on PR #${detail.number}`, 'success');
      const fresh = await getGitHubPull(detail.number, projectId);
      setDetail(fresh.pull);
    } catch (cause: unknown) {
      store.pushToast(toErrorMessage(cause), 'error');
    } finally {
      setWorking(false);
    }
  };

  return (
    <div class="gitm-body">
      <aside class="gitm-side gitm-gh-side" aria-label="Pull requests list">
        <div class="gitm-filter-bar">
          <div class="gitm-filter-row">
            <Select
              class="gitm-select"
              aria-label="Pull request state"
              value={stateFilter}
              testId="git-prs-state"
              options={[
                { value: 'open', label: 'Open' },
                { value: 'closed', label: 'Closed' },
                { value: 'merged', label: 'Merged' },
                { value: 'all', label: 'All' },
              ]}
              onChange={(event) => setStateFilter((event.target as HTMLSelectElement).value as PrStateFilter)}
            />
            <Button
              variant="primary"
              size="sm"
              icon={<PlusIcon size={12} />}
              testId="git-pr-new"
              onClick={() => setCreating((v) => !v)}
            >
              New PR
            </Button>
            <IconButton
              size="sm"
              label="Refresh pull requests"
              title="Refresh pull requests"
              testId="git-prs-refresh"
              icon={<RefreshIcon size={13} class={loading ? 'spin' : undefined} />}
              disabled={loading}
              onClick={fetchPulls}
            />
          </div>
          <div class="gitm-search-wrap">
            <SearchIcon size={13} />
            <Input
              type="search"
              class="gitm-input"
              placeholder="Filter PRs by title, #number, branch…"
              aria-label="Filter pull requests"
              value={search}
              testId="git-prs-search"
              onInput={(event) => setSearch((event.target as HTMLInputElement).value)}
            />
          </div>

          {visible.length > 0 ? (
            <div class="gitm-batch-bar">
              <Button
                variant="primary"
                size="sm"
                icon={<SparkIcon size={12} />}
                testId="git-prs-session-combined"
                disabled={working || busy}
                onClick={() => void startCombinedSession()}
              >
                Session for all ({visible.length})
              </Button>
              {visible.length > 1 ? (
                <Button
                  variant="secondary"
                  size="sm"
                  testId="git-prs-session-separate"
                  disabled={working || busy}
                  onClick={() => void startSeparateSessions()}
                >
                  1 per PR
                </Button>
              ) : null}
            </div>
          ) : null}
        </div>

        <div class="gitm-side-scroll">
          {error ? (
            <Callout tone="danger" class="gitm-action-error">
              {error}
            </Callout>
          ) : loading && pulls === null ? (
            <p class="gitm-note">Loading pull requests…</p>
          ) : visible.length === 0 ? (
            <p class="gitm-note">No {stateFilter === 'all' ? '' : `${stateFilter} `}pull requests found.</p>
          ) : (
            <ul class="gitm-rows">
              {visible.map((pr) => {
                const active = selectedNumber === pr.number;
                const checks = checkSummary(pr);
                const launchedId = launchedSessions[pr.number];
                return (
                  <li class={`gitm-row gitm-gh-row${active ? ' is-active' : ''}`} key={pr.number}>
                    <LinkButton
                      tone="muted"
                      class="gitm-row-main"
                      aria-pressed={active}
                      onClick={() => setSelectedNumber(active ? null : pr.number)}
                    >
                      <span
                        class={`gitm-badge ${
                          pr.state === 'MERGED'
                            ? 'is-info'
                            : pr.state === 'CLOSED'
                              ? 'is-danger'
                              : 'is-ok'
                        }`}
                        aria-hidden="true"
                      >
                        <PullRequestIcon size={11} />
                      </span>
                      <span class="gitm-name">
                        <span class="gitm-file">
                          #{pr.number} {pr.title}
                        </span>
                        <span class="gitm-dir">
                          {pr.headRefName} → {pr.baseRefName}
                          {pr.isDraft ? ' · Draft' : ''}
                          {checks ? ` · ${checks.label}` : ''}
                          {' · '}
                          {relativeTime(Date.parse(pr.updatedAt))}
                        </span>
                      </span>
                    </LinkButton>
                    <div class="gitm-row-actions">
                      {launchedId ? (
                        <Button
                          variant="secondary"
                          size="sm"
                          title="Open launched session"
                          testId={`git-pr-open-session-${pr.number}`}
                          onClick={() => {
                            void actions.openSessionInProject(projectId, launchedId);
                          }}
                        >
                          Open
                        </Button>
                      ) : (
                        <IconButton
                          size="sm"
                          class="gitm-act"
                          label={`Start session for PR #${pr.number}`}
                          title={`Start karucode session for PR #${pr.number}`}
                          testId={`git-pr-start-session-${pr.number}`}
                          icon={<SparkIcon size={13} />}
                          disabled={working || busy}
                          onClick={() => void startSingleSession(pr)}
                        />
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </aside>

      <section class="gitm-main" aria-label="Pull request details">
        {creating ? (
          <form class="gitm-gh-create" onSubmit={(e) => void submitCreatePr(e)}>
            <h3 class="gitm-card-title">Create pull request</h3>
            <div class="gitm-field">
              <label class="gitm-label">Title</label>
              <Input
                type="text"
                class="gitm-input"
                placeholder="feat: describe your change"
                value={newTitle}
                disabled={working}
                testId="git-pr-new-title"
                onInput={(e) => setNewTitle((e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="gitm-field-grid">
              <div class="gitm-field">
                <label class="gitm-label">Base branch</label>
                <Select
                  class="gitm-select"
                  aria-label="Base branch"
                  value={newBase}
                  disabled={working}
                  testId="git-pr-new-base"
                  options={[
                    ...branches.map((b) => ({ value: b.name, label: b.name })),
                    ...(branches.some((b) => b.name === newBase)
                      ? []
                      : [{ value: newBase, label: newBase }]),
                  ]}
                  onChange={(e) => setNewBase((e.target as HTMLSelectElement).value)}
                />
              </div>
              <div class="gitm-field">
                <label class="gitm-label">Head branch</label>
                <Input
                  type="text"
                  class="gitm-input"
                  aria-label="Head branch"
                  value={currentBranch ?? ''}
                  disabled
                  testId="git-pr-new-head"
                />
              </div>
            </div>
            <div class="gitm-field">
              <label class="gitm-label">Description</label>
              <Textarea
                class="gitm-message"
                rows={5}
                placeholder="What changed and how was it verified?"
                value={newBody}
                disabled={working}
                testId="git-pr-new-body"
                onInput={(e) => setNewBody((e.target as HTMLTextAreaElement).value)}
              />
            </div>
            <Checkbox
              class="gitm-check"
              checked={newDraft}
              disabled={working}
              label="Create as draft"
              onChange={(checked) => setNewDraft(checked)}
            />
            <div class="gitm-setup-actions">
              <Button
                variant="secondary"
                testId="git-pr-new-cancel"
                disabled={working}
                onClick={() => setCreating(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                testId="git-pr-new-submit"
                disabled={working || newTitle.trim() === ''}
              >
                Create pull request
              </Button>
            </div>
          </form>
        ) : selectedNumber === null ? (
          <div class="gitm-detail-empty">
            <p class="gitm-empty-title">Select a pull request</p>
            <p class="gitm-empty-text">
              Inspect its description, CI checks, and diff, check out its branch, or launch a
              karucode session for one or all pull requests.
            </p>
          </div>
        ) : detailLoading || !detail ? (
          <p class="gitm-note">Loading PR #{selectedNumber}…</p>
        ) : (
          <div class="gitm-detail">
            <header class="gitm-detail-head">
              <div class="gitm-detail-titles">
                <h3 class="gitm-detail-title">
                  #{detail.number} {detail.title}
                </h3>
                <p class="gitm-detail-sub">
                  <span class="gitm-chip">{detail.isDraft ? 'Draft' : detail.state}</span>
                  <span>
                    <BranchIcon size={12} /> {detail.headRefName} → {detail.baseRefName}
                  </span>
                  {detail.author?.login ? <span>by @{detail.author.login}</span> : null}
                  {detail.additions !== undefined ? (
                    <span>
                      +{detail.additions} / −{detail.deletions ?? 0} ({detail.changedFiles ?? 0}{' '}
                      files)
                    </span>
                  ) : null}
                  <a class="tiny-link" href={detail.url} target="_blank" rel="noreferrer">
                    GitHub <ExternalIcon size={11} />
                  </a>
                </p>
              </div>
              <div class="gitm-detail-actions">
                <Button
                  variant="primary"
                  size="sm"
                  icon={<SparkIcon size={12} />}
                  testId="git-pr-detail-session"
                  disabled={working || busy}
                  onClick={() => void startSingleSession(detail)}
                >
                  Start session
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  testId="git-pr-checkout"
                  disabled={working || busy}
                  onClick={() => void onCheckoutPull(detail.number)}
                >
                  Checkout
                </Button>
                {detail.state === 'OPEN' ? (
                  <>
                    <Button
                      variant="secondary"
                      size="sm"
                      testId="git-pr-merge"
                      disabled={working || busy}
                      onClick={() =>
                        onRequestConfirm({
                          title: `Merge PR #${detail.number}?`,
                          description: `Squash-merges ${detail.headRefName} into ${detail.baseRefName} on GitHub.`,
                          confirmLabel: 'Squash & merge',
                          onConfirm: async () => {
                            await mergeGitHubPull(
                              detail.number,
                              { method: 'squash', confirm: true },
                              projectId,
                            );
                            store.pushToast(`Merged PR #${detail.number}`, 'success');
                            fetchPulls();
                            const fresh = await getGitHubPull(detail.number, projectId);
                            setDetail(fresh.pull);
                          },
                        })
                      }
                    >
                      Merge
                    </Button>
                    <Button
                      variant="danger"
                      size="sm"
                      testId="git-pr-close"
                      disabled={working || busy}
                      onClick={() =>
                        void setGitHubPullState(detail.number, 'close', projectId).then(() => {
                          store.pushToast(`Closed PR #${detail.number}`, 'info');
                          fetchPulls();
                        })
                      }
                    >
                      Close
                    </Button>
                  </>
                ) : detail.state === 'CLOSED' ? (
                  <Button
                    variant="secondary"
                    size="sm"
                    testId="git-pr-reopen"
                    disabled={working || busy}
                    onClick={() =>
                      void setGitHubPullState(detail.number, 'reopen', projectId).then(() => {
                        store.pushToast(`Reopened PR #${detail.number}`, 'info');
                        fetchPulls();
                      })
                    }
                  >
                    Reopen
                  </Button>
                ) : null}
              </div>
            </header>

            <div class="gitm-subtabs">
              <Button
                variant={detailTab === 'overview' ? 'primary' : 'secondary'}
                size="sm"
                aria-pressed={detailTab === 'overview'}
                testId="git-pr-tab-overview"
                onClick={() => setDetailTab('overview')}
              >
                Overview
              </Button>
              <Button
                variant={detailTab === 'diff' ? 'primary' : 'secondary'}
                size="sm"
                aria-pressed={detailTab === 'diff'}
                testId="git-pr-tab-diff"
                onClick={() => loadPullDiff(detail.number)}
              >
                Files diff ({detail.changedFiles ?? 0})
              </Button>
            </div>

            {detailTab === 'diff' ? (
              diff === null ? (
                <p class="gitm-note">Loading pull request diff…</p>
              ) : (
                <DiffView diff={diff.diff} truncated={diff.truncated} />
              )
            ) : (
              <div class="gitm-gh-body">
                {detail.labels.length > 0 ? (
                  <div class="gitm-label-row">
                    {detail.labels.map((l) => (
                      <span class="gitm-chip" key={l.name}>
                        {l.name}
                      </span>
                    ))}
                  </div>
                ) : null}

                {detail.statusCheckRollup && detail.statusCheckRollup.length > 0 ? (
                  <div class="gitm-checks-box">
                    <h4 class="gitm-label">Checks ({detail.statusCheckRollup.length})</h4>
                    <ul class="gitm-checks-list">
                      {detail.statusCheckRollup.map((c, idx) => (
                        <li key={idx} class="gitm-check-item">
                          <span class="gitm-check-name">{c.name ?? c.context ?? 'Check'}</span>
                          <span class="gitm-chip">{c.conclusion ?? c.status ?? c.state ?? '—'}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}

                <div class="gitm-gh-markdown">
                  {detail.body?.trim() ? (
                    <Markdown text={detail.body} />
                  ) : (
                    <p class="gitm-hint">No description provided.</p>
                  )}
                </div>

                {detail.comments && detail.comments.length > 0 ? (
                  <div class="gitm-comments">
                    <h4 class="gitm-label">Comments ({detail.comments.length})</h4>
                    {detail.comments.map((c, idx) => (
                      <div class="gitm-comment" key={c.id ?? idx}>
                        <div class="gitm-comment-head">
                          <strong>@{c.author?.login ?? 'user'}</strong>
                          <span>{relativeTime(Date.parse(c.createdAt))}</span>
                        </div>
                        <Markdown text={c.body} />
                      </div>
                    ))}
                  </div>
                ) : null}

                <form class="gitm-comment-form" onSubmit={(e) => void submitComment(e)}>
                  <Textarea
                    class="gitm-message"
                    rows={3}
                    placeholder="Leave a comment on this pull request…"
                    aria-label="Pull request comment"
                    value={commentText}
                    disabled={working}
                    testId="git-pr-comment-body"
                    onInput={(e) => setCommentText((e.target as HTMLTextAreaElement).value)}
                  />
                  <div>
                    <Button
                      type="submit"
                      variant="primary"
                      size="sm"
                      testId="git-pr-comment-submit"
                      disabled={working || commentText.trim() === ''}
                    >
                      Comment
                    </Button>
                  </div>
                </form>
              </div>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
