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
            <button type="button" class="button button-primary" onClick={onOpenRemotes}>
              Connect or create GitHub remote
            </button>
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
            <select
              class="gitm-select"
              aria-label="Pull request state"
              value={stateFilter}
              onChange={(event) => setStateFilter((event.target as HTMLSelectElement).value as PrStateFilter)}
            >
              <option value="open">Open</option>
              <option value="closed">Closed</option>
              <option value="merged">Merged</option>
              <option value="all">All</option>
            </select>
            <button
              type="button"
              class="button button-small button-primary"
              onClick={() => setCreating((v) => !v)}
            >
              <PlusIcon size={12} /> New PR
            </button>
            <button
              type="button"
              class="icon-button"
              aria-label="Refresh pull requests"
              title="Refresh pull requests"
              disabled={loading}
              onClick={fetchPulls}
            >
              <RefreshIcon size={13} class={loading ? 'spin' : undefined} />
            </button>
          </div>
          <div class="gitm-search-wrap">
            <SearchIcon size={13} />
            <input
              type="search"
              class="gitm-input"
              placeholder="Filter PRs by title, #number, branch…"
              aria-label="Filter pull requests"
              value={search}
              onInput={(event) => setSearch((event.target as HTMLInputElement).value)}
            />
          </div>

          {visible.length > 0 ? (
            <div class="gitm-batch-bar">
              <button
                type="button"
                class="button button-small button-primary"
                disabled={working || busy}
                onClick={() => void startCombinedSession()}
              >
                <SparkIcon size={12} /> Session for all ({visible.length})
              </button>
              {visible.length > 1 ? (
                <button
                  type="button"
                  class="button button-small"
                  disabled={working || busy}
                  onClick={() => void startSeparateSessions()}
                >
                  1 per PR
                </button>
              ) : null}
            </div>
          ) : null}
        </div>

        <div class="gitm-side-scroll">
          {error ? (
            <div class="inline-error gitm-action-error" role="alert">
              <p>{error}</p>
            </div>
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
                    <button
                      type="button"
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
                    </button>
                    <div class="gitm-row-actions">
                      {launchedId ? (
                        <button
                          type="button"
                          class="button button-small"
                          title="Open launched session"
                          onClick={() => {
                            void actions.openSessionInProject(projectId, launchedId);
                          }}
                        >
                          Open
                        </button>
                      ) : (
                        <button
                          type="button"
                          class="gitm-act"
                          title={`Start karucode session for PR #${pr.number}`}
                          aria-label={`Start session for PR #${pr.number}`}
                          disabled={working || busy}
                          onClick={() => void startSingleSession(pr)}
                        >
                          <SparkIcon size={13} />
                        </button>
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
              <input
                type="text"
                class="gitm-input"
                placeholder="feat: describe your change"
                value={newTitle}
                disabled={working}
                onInput={(e) => setNewTitle((e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="gitm-field-grid">
              <div class="gitm-field">
                <label class="gitm-label">Base branch</label>
                <select
                  class="gitm-select"
                  value={newBase}
                  disabled={working}
                  onChange={(e) => setNewBase((e.target as HTMLSelectElement).value)}
                >
                  {branches.map((b) => (
                    <option key={b.name} value={b.name}>
                      {b.name}
                    </option>
                  ))}
                  {!branches.some((b) => b.name === newBase) ? (
                    <option value={newBase}>{newBase}</option>
                  ) : null}
                </select>
              </div>
              <div class="gitm-field">
                <label class="gitm-label">Head branch</label>
                <input type="text" class="gitm-input" value={currentBranch ?? ''} disabled />
              </div>
            </div>
            <div class="gitm-field">
              <label class="gitm-label">Description</label>
              <textarea
                class="gitm-message"
                rows={5}
                placeholder="What changed and how was it verified?"
                value={newBody}
                disabled={working}
                onInput={(e) => setNewBody((e.target as HTMLTextAreaElement).value)}
              />
            </div>
            <label class="gitm-check">
              <input
                type="checkbox"
                checked={newDraft}
                disabled={working}
                onChange={(e) => setNewDraft((e.target as HTMLInputElement).checked)}
              />
              <span>Create as draft</span>
            </label>
            <div class="gitm-setup-actions">
              <button
                type="button"
                class="button"
                disabled={working}
                onClick={() => setCreating(false)}
              >
                Cancel
              </button>
              <button
                type="submit"
                class="button button-primary"
                disabled={working || newTitle.trim() === ''}
              >
                Create pull request
              </button>
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
                <button
                  type="button"
                  class="button button-small button-primary"
                  disabled={working || busy}
                  onClick={() => void startSingleSession(detail)}
                >
                  <SparkIcon size={12} /> Start session
                </button>
                <button
                  type="button"
                  class="button button-small"
                  disabled={working || busy}
                  onClick={() => void onCheckoutPull(detail.number)}
                >
                  Checkout
                </button>
                {detail.state === 'OPEN' ? (
                  <>
                    <button
                      type="button"
                      class="button button-small"
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
                    </button>
                    <button
                      type="button"
                      class="button button-small button-danger"
                      disabled={working || busy}
                      onClick={() =>
                        void setGitHubPullState(detail.number, 'close', projectId).then(() => {
                          store.pushToast(`Closed PR #${detail.number}`, 'info');
                          fetchPulls();
                        })
                      }
                    >
                      Close
                    </button>
                  </>
                ) : detail.state === 'CLOSED' ? (
                  <button
                    type="button"
                    class="button button-small"
                    disabled={working || busy}
                    onClick={() =>
                      void setGitHubPullState(detail.number, 'reopen', projectId).then(() => {
                        store.pushToast(`Reopened PR #${detail.number}`, 'info');
                        fetchPulls();
                      })
                    }
                  >
                    Reopen
                  </button>
                ) : null}
              </div>
            </header>

            <div class="gitm-subtabs">
              <button
                type="button"
                class={`button button-small${detailTab === 'overview' ? ' button-primary' : ''}`}
                onClick={() => setDetailTab('overview')}
              >
                Overview
              </button>
              <button
                type="button"
                class={`button button-small${detailTab === 'diff' ? ' button-primary' : ''}`}
                onClick={() => loadPullDiff(detail.number)}
              >
                Files diff ({detail.changedFiles ?? 0})
              </button>
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
                  <textarea
                    class="gitm-message"
                    rows={3}
                    placeholder="Leave a comment on this pull request…"
                    aria-label="Pull request comment"
                    value={commentText}
                    disabled={working}
                    onInput={(e) => setCommentText((e.target as HTMLTextAreaElement).value)}
                  />
                  <div>
                    <button
                      type="submit"
                      class="button button-small button-primary"
                      disabled={working || commentText.trim() === ''}
                    >
                      Comment
                    </button>
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
