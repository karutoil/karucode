import type { JSX } from 'preact';
import { useCallback, useEffect, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import {
  commentGitHubIssue,
  createGitHubIssue,
  getGitHubIssue,
  listGitHubIssues,
  setGitHubIssueState,
} from '../../api/endpoints';
import type { GitHubIssueDetail, GitHubIssueSummary, GitHubStatus } from '../../api/types';
import { relativeTime } from '../../lib/format';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { Markdown } from '../Markdown';
import {
  ExternalIcon,
  IssueIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  SparkIcon,
} from '../icons';
import { Button, Callout, IconButton, Input, LinkButton, Select, Textarea } from '../../ui';
import { batchIssuesSessionSpec, issueSessionSpec } from './gitModel';
import type { ConfirmRequest } from './GitConfirmDialog';

export interface GitIssuesViewProps {
  projectId: string | null;
  githubStatus: GitHubStatus | null;
  busy: boolean;
  onLoadGitHubStatus: () => void;
  onOpenRemotes: () => void;
  onRequestConfirm: (request: ConfirmRequest) => void;
}

type IssueStateFilter = 'open' | 'closed' | 'all';

export function GitIssuesView({
  projectId,
  githubStatus,
  busy,
  onLoadGitHubStatus,
  onOpenRemotes,
  onRequestConfirm,
}: GitIssuesViewProps): JSX.Element {
  const [stateFilter, setStateFilter] = useState<IssueStateFilter>('open');
  const [search, setSearch] = useState('');
  const [issues, setIssues] = useState<GitHubIssueSummary[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [selectedNumber, setSelectedNumber] = useState<number | null>(null);
  const [detail, setDetail] = useState<GitHubIssueDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const [checkedNumbers, setCheckedNumbers] = useState<Set<number>>(new Set());
  const [creating, setCreating] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [newBody, setNewBody] = useState('');
  const [newLabels, setNewLabels] = useState('');

  const [commentText, setCommentText] = useState('');
  const [working, setWorking] = useState(false);
  const [launchedSessions, setLaunchedSessions] = useState<Record<number, string>>({});

  useEffect(() => {
    onLoadGitHubStatus();
  }, [onLoadGitHubStatus]);

  const ready = Boolean(
    githubStatus?.gh_installed && githubStatus?.authenticated && githubStatus?.remote_slug,
  );

  const fetchIssues = useCallback(() => {
    if (!ready) return;
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    listGitHubIssues(projectId, { state: stateFilter, limit: 50, signal: controller.signal })
      .then((res) => {
        if (!controller.signal.aborted) setIssues(res.issues);
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
    setIssues(null);
    setCheckedNumbers(new Set());
    if (ready) fetchIssues();
  }, [projectId, stateFilter, ready, fetchIssues]);

  useEffect(() => {
    if (selectedNumber === null || !ready) {
      setDetail(null);
      return;
    }
    const controller = new AbortController();
    setDetailLoading(true);
    getGitHubIssue(selectedNumber, projectId, { signal: controller.signal })
      .then((res) => {
        if (!controller.signal.aborted) setDetail(res.issue);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) store.pushToast(toErrorMessage(cause), 'error');
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoading(false);
      });
    return () => controller.abort();
  }, [selectedNumber, projectId, ready]);

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
        <p class="gitm-empty-title">GitHub issues unavailable</p>
        <p class="gitm-empty-text">
          {!githubStatus.gh_installed
            ? 'Install the GitHub CLI (`gh`) on this machine to view, create, and launch sessions from GitHub issues.'
            : !githubStatus.authenticated
              ? 'Sign in with `gh auth login` in the Terminal tab to access issues.'
              : 'This repository does not have a GitHub remote yet.'}
        </p>
        {githubStatus.authenticated && !githubStatus.remote_slug ? (
          <div>
            <Button
              variant="primary"
              testId="git-issues-open-remotes"
              onClick={onOpenRemotes}
            >
              Connect or create GitHub remote
            </Button>
          </div>
        ) : null}
      </div>
    );
  }

  const needle = search.trim().toLowerCase();
  const visible = (issues ?? []).filter((issue) => {
    if (!needle) return true;
    return (
      issue.title.toLowerCase().includes(needle) ||
      `#${issue.number}`.includes(needle) ||
      issue.labels.some((l) => l.name.toLowerCase().includes(needle)) ||
      (issue.author?.login ?? '').toLowerCase().includes(needle)
    );
  });

  const targetIssues =
    checkedNumbers.size > 0
      ? visible.filter((i) => checkedNumbers.has(i.number))
      : visible;

  const toggleChecked = (num: number): void => {
    setCheckedNumbers((prev) => {
      const next = new Set(prev);
      if (next.has(num)) next.delete(num);
      else next.add(num);
      return next;
    });
  };

  const startSingleSession = async (issue: GitHubIssueSummary): Promise<void> => {
    setWorking(true);
    const spec = issueSessionSpec(issue);
    const created = await actions.launchSessionWithPrompt({
      title: spec.title,
      prompt: spec.prompt,
      projectId,
      focus: true,
    });
    if (created) {
      setLaunchedSessions((prev) => ({ ...prev, [issue.number]: created.sessionId }));
    }
    setWorking(false);
  };

  const startCombinedSession = async (): Promise<void> => {
    if (targetIssues.length === 0) return;
    setWorking(true);
    const spec = batchIssuesSessionSpec(targetIssues);
    await actions.launchSessionWithPrompt({
      title: spec.title,
      prompt: spec.prompt,
      projectId,
      focus: true,
    });
    setWorking(false);
  };

  const startSeparateSessions = async (): Promise<void> => {
    if (targetIssues.length === 0) return;
    const launchAll = async (): Promise<void> => {
      setWorking(true);
      let count = 0;
      const nextMap: Record<number, string> = {};
      for (const issue of targetIssues) {
        const spec = issueSessionSpec(issue);
        const created = await actions.launchSessionWithPrompt({
          title: spec.title,
          prompt: spec.prompt,
          projectId,
          focus: false,
        });
        if (created) {
          count += 1;
          nextMap[issue.number] = created.sessionId;
        }
      }
      setLaunchedSessions((prev) => ({ ...prev, ...nextMap }));
      setWorking(false);
      if (count > 0) {
        store.pushToast(
          `Started ${count} karucode session${count === 1 ? '' : 's'} for issues.`,
          'success',
        );
      }
    };
    if (targetIssues.length > 3) {
      onRequestConfirm({
        title: `Start ${targetIssues.length} separate sessions?`,
        description: `This will create and run ${targetIssues.length} parallel karucode sessions (one per issue).`,
        confirmLabel: `Start ${targetIssues.length} sessions`,
        onConfirm: launchAll,
      });
    } else {
      await launchAll();
    }
  };

  const submitCreateIssue = async (event: Event): Promise<void> => {
    event.preventDefault();
    if (working || newTitle.trim() === '') return;
    setWorking(true);
    try {
      const labels = newLabels
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
      const res = await createGitHubIssue(
        {
          title: newTitle.trim(),
          body: newBody.trim() || undefined,
          labels: labels.length > 0 ? labels : undefined,
        },
        projectId,
      );
      store.pushToast(res.message, 'success');
      setCreating(false);
      setNewTitle('');
      setNewBody('');
      setNewLabels('');
      fetchIssues();
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
      await commentGitHubIssue(detail.number, commentText.trim(), projectId);
      setCommentText('');
      store.pushToast(`Commented on issue #${detail.number}`, 'success');
      const fresh = await getGitHubIssue(detail.number, projectId);
      setDetail(fresh.issue);
    } catch (cause: unknown) {
      store.pushToast(toErrorMessage(cause), 'error');
    } finally {
      setWorking(false);
    }
  };

  return (
    <div class="gitm-body">
      <aside class="gitm-side gitm-gh-side" aria-label="Issues list">
        <div class="gitm-filter-bar">
          <div class="gitm-filter-row">
            <Select
              class="gitm-select"
              aria-label="Issue state"
              value={stateFilter}
              testId="git-issues-state"
              options={[
                { value: 'open', label: 'Open' },
                { value: 'closed', label: 'Closed' },
                { value: 'all', label: 'All' },
              ]}
              onChange={(event) =>
                setStateFilter((event.target as HTMLSelectElement).value as IssueStateFilter)
              }
            />
            <Button
              variant="primary"
              size="sm"
              icon={<PlusIcon size={12} />}
              testId="git-issue-new"
              onClick={() => setCreating((v) => !v)}
            >
              New issue
            </Button>
            <IconButton
              size="sm"
              label="Refresh issues"
              title="Refresh issues"
              testId="git-issues-refresh"
              icon={<RefreshIcon size={13} class={loading ? 'spin' : undefined} />}
              disabled={loading}
              onClick={fetchIssues}
            />
          </div>

          <div class="gitm-search-wrap">
            <SearchIcon size={13} />
            <Input
              type="search"
              class="gitm-input"
              placeholder="Filter issues by title, #number, label…"
              aria-label="Filter issues"
              value={search}
              testId="git-issues-search"
              onInput={(event) => setSearch((event.target as HTMLInputElement).value)}
            />
          </div>

          {visible.length > 0 ? (
            <div class="gitm-batch-bar">
              <Button
                variant="primary"
                size="sm"
                icon={<SparkIcon size={12} />}
                testId="git-issues-session-combined"
                disabled={working || busy}
                onClick={() => void startCombinedSession()}
              >
                {checkedNumbers.size > 0
                  ? `Session for ${targetIssues.length} selected`
                  : `Session for all (${visible.length})`}
              </Button>
              {targetIssues.length > 1 ? (
                <Button
                  variant="secondary"
                  size="sm"
                  testId="git-issues-session-separate"
                  disabled={working || busy}
                  onClick={() => void startSeparateSessions()}
                >
                  1 per issue
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
          ) : loading && issues === null ? (
            <p class="gitm-note">Loading issues…</p>
          ) : visible.length === 0 ? (
            <p class="gitm-note">
              No {stateFilter === 'all' ? '' : `${stateFilter} `}issues found.
            </p>
          ) : (
            <ul class="gitm-rows">
              {visible.map((issue) => {
                const active = selectedNumber === issue.number;
                const checked = checkedNumbers.has(issue.number);
                const launchedId = launchedSessions[issue.number];
                return (
                  <li class={`gitm-row gitm-gh-row${active ? ' is-active' : ''}`} key={issue.number}>
                    {/* A bare row checkbox: `Checkbox` renders its label as
                        visible text, and this one's name is the row itself
                        (`aria-label`), inside the row's flex grid. */}
                    <input
                      type="checkbox"
                      class="gitm-row-check"
                      checked={checked}
                      aria-label={`Select issue #${issue.number}`}
                      onChange={() => toggleChecked(issue.number)}
                    />
                    <LinkButton
                      tone="muted"
                      class="gitm-row-main"
                      aria-pressed={active}
                      onClick={() => setSelectedNumber(active ? null : issue.number)}
                    >
                      <span
                        class={`gitm-badge ${issue.state === 'CLOSED' ? 'is-info' : 'is-ok'}`}
                        aria-hidden="true"
                      >
                        <IssueIcon size={11} />
                      </span>
                      <span class="gitm-name">
                        <span class="gitm-file">
                          #{issue.number} {issue.title}
                        </span>
                        <span class="gitm-dir">
                          {issue.author?.login ? `@${issue.author.login} · ` : ''}
                          {issue.labels.length > 0
                            ? `${issue.labels.map((l) => l.name).join(', ')} · `
                            : ''}
                          {relativeTime(Date.parse(issue.updatedAt))}
                        </span>
                      </span>
                    </LinkButton>
                    <div class="gitm-row-actions">
                      {launchedId ? (
                        <Button
                          variant="secondary"
                          size="sm"
                          title="Open launched session"
                          testId={`git-issue-open-session-${issue.number}`}
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
                          label={`Start session for issue #${issue.number}`}
                          title={`Start karucode session for issue #${issue.number}`}
                          testId={`git-issue-start-session-${issue.number}`}
                          icon={<SparkIcon size={13} />}
                          disabled={working || busy}
                          onClick={() => void startSingleSession(issue)}
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

      <section class="gitm-main" aria-label="Issue details">
        {creating ? (
          <form class="gitm-gh-create" onSubmit={(e) => void submitCreateIssue(e)}>
            <h3 class="gitm-card-title">Create issue</h3>
            <div class="gitm-field">
              <label class="gitm-label">Title</label>
              <Input
                type="text"
                class="gitm-input"
                placeholder="Issue summary"
                value={newTitle}
                disabled={working}
                testId="git-issue-new-title"
                onInput={(e) => setNewTitle((e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="gitm-field">
              <label class="gitm-label">Labels (comma-separated, optional)</label>
              <Input
                type="text"
                class="gitm-input"
                placeholder="bug, enhancement"
                value={newLabels}
                disabled={working}
                testId="git-issue-new-labels"
                onInput={(e) => setNewLabels((e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="gitm-field">
              <label class="gitm-label">Description</label>
              <Textarea
                class="gitm-message"
                rows={6}
                placeholder="Steps to reproduce, expected behavior, or requirements…"
                value={newBody}
                disabled={working}
                testId="git-issue-new-body"
                onInput={(e) => setNewBody((e.target as HTMLTextAreaElement).value)}
              />
            </div>
            <div class="gitm-setup-actions">
              <Button
                variant="secondary"
                testId="git-issue-new-cancel"
                disabled={working}
                onClick={() => setCreating(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                testId="git-issue-new-submit"
                disabled={working || newTitle.trim() === ''}
              >
                Create issue
              </Button>
            </div>
          </form>
        ) : selectedNumber === null ? (
          <div class="gitm-detail-empty">
            <p class="gitm-empty-title">Select an issue</p>
            <p class="gitm-empty-text">
              Inspect issue details and comments, or click Start session to have karucode
              investigate and fix one or all issues in this project.
            </p>
          </div>
        ) : detailLoading || !detail ? (
          <p class="gitm-note">Loading issue #{selectedNumber}…</p>
        ) : (
          <div class="gitm-detail">
            <header class="gitm-detail-head">
              <div class="gitm-detail-titles">
                <h3 class="gitm-detail-title">
                  #{detail.number} {detail.title}
                </h3>
                <p class="gitm-detail-sub">
                  <span class="gitm-chip">{detail.state}</span>
                  {detail.author?.login ? <span>opened by @{detail.author.login}</span> : null}
                  <span>{relativeTime(Date.parse(detail.createdAt))}</span>
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
                  testId="git-issue-detail-session"
                  disabled={working || busy}
                  onClick={() => void startSingleSession(detail)}
                >
                  Start session
                </Button>
                {detail.state === 'OPEN' ? (
                  <Button
                    variant="secondary"
                    size="sm"
                    testId="git-issue-close"
                    disabled={working || busy}
                    onClick={() =>
                      void setGitHubIssueState(detail.number, 'close', projectId).then(() => {
                        store.pushToast(`Closed issue #${detail.number}`, 'info');
                        fetchIssues();
                      })
                    }
                  >
                    Close issue
                  </Button>
                ) : (
                  <Button
                    variant="secondary"
                    size="sm"
                    testId="git-issue-reopen"
                    disabled={working || busy}
                    onClick={() =>
                      void setGitHubIssueState(detail.number, 'reopen', projectId).then(() => {
                        store.pushToast(`Reopened issue #${detail.number}`, 'info');
                        fetchIssues();
                      })
                    }
                  >
                    Reopen
                  </Button>
                )}
              </div>
            </header>

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
                  placeholder="Leave a comment on this issue…"
                  aria-label="Issue comment"
                  value={commentText}
                  disabled={working}
                  testId="git-issue-comment-body"
                  onInput={(e) => setCommentText((e.target as HTMLTextAreaElement).value)}
                />
                <div>
                  <Button
                    type="submit"
                    variant="primary"
                    size="sm"
                    testId="git-issue-comment-submit"
                    disabled={working || commentText.trim() === ''}
                  >
                    Comment
                  </Button>
                </div>
              </form>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
