import { apiRequest, withQuery } from '../client';
import type {
  GitDiff,
  GitHubIssueDetail,
  GitHubIssueSummary,
  GitHubPullDetail,
  GitHubPullSummary,
  GitHubStatus,
  GitMutationResult,
} from '../types/git';
import type { CallOptions } from './common';

export function getGitHubStatus(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitHubStatus> {
  return apiRequest<GitHubStatus>(
    withQuery('/api/git/github/status', { project: project ?? undefined }),
    options,
  );
}

export function createGitHubRepo(
  body: {
    name: string;
    description?: string;
    visibility?: 'private' | 'public' | 'internal';
    remote?: string;
    push?: boolean;
  },
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery('/api/git/github/repo', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function listGitHubPulls(
  project?: string | null,
  options: CallOptions & { state?: 'open' | 'closed' | 'merged' | 'all'; limit?: number } = {},
): Promise<{ pulls: GitHubPullSummary[] }> {
  return apiRequest<{ pulls: GitHubPullSummary[] }>(
    withQuery('/api/git/github/pulls', {
      project: project ?? undefined,
      state: options.state,
      limit: options.limit,
    }),
    options,
  );
}

export function getGitHubPull(
  number: number,
  project?: string | null,
  options: CallOptions = {},
): Promise<{ pull: GitHubPullDetail }> {
  return apiRequest<{ pull: GitHubPullDetail }>(
    withQuery(`/api/git/github/pulls/${number}`, { project: project ?? undefined }),
    options,
  );
}

export function getGitHubPullDiff(
  number: number,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitDiff> {
  return apiRequest<GitDiff>(
    withQuery(`/api/git/github/pulls/${number}/diff`, { project: project ?? undefined }),
    options,
  );
}

export function createGitHubPull(
  body: { title: string; body?: string; base?: string; head?: string; draft?: boolean },
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string; url?: string | null }> {
  return apiRequest<{ ok: boolean; message: string; url?: string | null }>(
    withQuery('/api/git/github/pulls', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function checkoutGitHubPull(
  number: number,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitMutationResult> {
  return apiRequest<GitMutationResult>(
    withQuery(`/api/git/github/pulls/${number}/checkout`, { project: project ?? undefined }),
    { ...options, method: 'POST', body: {} },
  );
}

export function mergeGitHubPull(
  number: number,
  body: { method?: 'merge' | 'squash' | 'rebase'; delete_branch?: boolean; confirm: true },
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string }> {
  return apiRequest<{ ok: boolean; message: string }>(
    withQuery(`/api/git/github/pulls/${number}/merge`, { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function setGitHubPullState(
  number: number,
  action: 'close' | 'reopen',
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string }> {
  return apiRequest<{ ok: boolean; message: string }>(
    withQuery(`/api/git/github/pulls/${number}/state`, { project: project ?? undefined }),
    { ...options, method: 'POST', body: { action } },
  );
}

export function commentGitHubPull(
  number: number,
  body: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string }> {
  return apiRequest<{ ok: boolean; message: string }>(
    withQuery(`/api/git/github/pulls/${number}/comment`, { project: project ?? undefined }),
    { ...options, method: 'POST', body: { body } },
  );
}

export function listGitHubIssues(
  project?: string | null,
  options: CallOptions & { state?: 'open' | 'closed' | 'all'; limit?: number } = {},
): Promise<{ issues: GitHubIssueSummary[] }> {
  return apiRequest<{ issues: GitHubIssueSummary[] }>(
    withQuery('/api/git/github/issues', {
      project: project ?? undefined,
      state: options.state,
      limit: options.limit,
    }),
    options,
  );
}

export function getGitHubIssue(
  number: number,
  project?: string | null,
  options: CallOptions = {},
): Promise<{ issue: GitHubIssueDetail }> {
  return apiRequest<{ issue: GitHubIssueDetail }>(
    withQuery(`/api/git/github/issues/${number}`, { project: project ?? undefined }),
    options,
  );
}

export function createGitHubIssue(
  body: { title: string; body?: string; labels?: string[] },
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string; url?: string | null }> {
  return apiRequest<{ ok: boolean; message: string; url?: string | null }>(
    withQuery('/api/git/github/issues', { project: project ?? undefined }),
    { ...options, method: 'POST', body },
  );
}

export function setGitHubIssueState(
  number: number,
  action: 'close' | 'reopen',
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string }> {
  return apiRequest<{ ok: boolean; message: string }>(
    withQuery(`/api/git/github/issues/${number}/state`, { project: project ?? undefined }),
    { ...options, method: 'POST', body: { action } },
  );
}

export function commentGitHubIssue(
  number: number,
  body: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<{ ok: boolean; message: string }> {
  return apiRequest<{ ok: boolean; message: string }>(
    withQuery(`/api/git/github/issues/${number}/comment`, { project: project ?? undefined }),
    { ...options, method: 'POST', body: { body } },
  );
}
