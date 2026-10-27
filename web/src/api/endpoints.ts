import { apiRequest, withQuery } from './client';
import type {
  ApprovalDecision,
  ApprovalRecord,
  Config,
  ConfigPatch,
  GitDiff,
  GitStatus,
  Health,
  McpServer,
  PluginDiagnostics,
  ProviderTestResult,
  Session,
  SessionDetail,
  SessionSummary,
  Stats,
  WorkspaceFile,
  WorkspaceTree,
} from './types';

export type FetchLike = typeof fetch;
export interface CallOptions {
  signal?: AbortSignal;
  fetchImpl?: FetchLike;
  /** Override the stored bearer token (used by the auth screen). */
  token?: string | null;
}

export function getHealth(options: CallOptions = {}): Promise<Health> {
  return apiRequest<Health>('/api/health', options);
}

export function getConfig(options: CallOptions = {}): Promise<Config> {
  return apiRequest<Config>('/api/config', options);
}

export function patchConfig(patch: ConfigPatch, options: CallOptions = {}): Promise<Config> {
  return apiRequest<Config>('/api/config', { ...options, method: 'PATCH', body: patch });
}

export function testProvider(options: CallOptions = {}): Promise<ProviderTestResult> {
  return apiRequest<ProviderTestResult>('/api/provider/test', {
    ...options,
    method: 'POST',
    body: {},
  });
}

export function getStats(options: CallOptions = {}): Promise<Stats> {
  return apiRequest<Stats>('/api/stats', options);
}

export function listSessions(options: CallOptions = {}): Promise<SessionSummary[]> {
  return apiRequest<{ sessions: SessionSummary[] }>('/api/sessions', options).then((r) => r.sessions);
}

export function createSession(title?: string, options: CallOptions = {}): Promise<Session> {
  return apiRequest<Session>('/api/sessions', {
    ...options,
    method: 'POST',
    body: title ? { title } : {},
  });
}

export function getSession(id: string, options: CallOptions = {}): Promise<SessionDetail> {
  return apiRequest<SessionDetail>(`/api/sessions/${encodeURIComponent(id)}`, options);
}

export function renameSession(id: string, title: string, options: CallOptions = {}): Promise<Session> {
  return apiRequest<Session>(`/api/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { title },
  });
}

export function deleteSession(id: string, options: CallOptions = {}): Promise<{ deleted: boolean }> {
  return apiRequest<{ deleted: boolean }>(`/api/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}

export function sendMessage(
  sessionId: string,
  content: string,
  options: CallOptions = {},
): Promise<{ run_id: string; session_id: string }> {
  return apiRequest<{ run_id: string; session_id: string }>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    { ...options, method: 'POST', body: { content } },
  );
}

export function cancelRun(sessionId: string, options: CallOptions = {}): Promise<{ cancelled: boolean }> {
  return apiRequest<{ cancelled: boolean }>(
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { ...options, method: 'POST', body: {} },
  );
}

export function listApprovals(sessionId: string, options: CallOptions = {}): Promise<ApprovalRecord[]> {
  return apiRequest<{ approvals: ApprovalRecord[] }>(
    withQuery('/api/approvals', { session: sessionId }),
    options,
  ).then((r) => r.approvals);
}

export function resolveApproval(
  approvalId: string,
  decision: Exclude<ApprovalDecision, 'expired'>,
  options: CallOptions = {},
): Promise<{ resolved: boolean; decision: ApprovalDecision }> {
  return apiRequest<{ resolved: boolean; decision: ApprovalDecision }>(
    `/api/approvals/${encodeURIComponent(approvalId)}`,
    { ...options, method: 'POST', body: { decision } },
  );
}

export function getWorkspaceTree(
  path = '.',
  depth = 1,
  options: CallOptions = {},
): Promise<WorkspaceTree> {
  return apiRequest<WorkspaceTree>(withQuery('/api/workspace/tree', { path, depth }), options);
}

export function getWorkspaceFile(path: string, options: CallOptions = {}): Promise<WorkspaceFile> {
  return apiRequest<WorkspaceFile>(withQuery('/api/workspace/file', { path }), options);
}

export function getGitStatus(options: CallOptions = {}): Promise<GitStatus> {
  return apiRequest<GitStatus>('/api/git/status', options);
}

export function getGitDiff(
  path: string,
  staged: boolean,
  base?: string,
  options: CallOptions = {},
): Promise<GitDiff> {
  return apiRequest<GitDiff>(
    withQuery('/api/git/diff', { path, staged, base: base ?? undefined }),
    options,
  );
}

export function listPlugins(options: CallOptions = {}): Promise<PluginDiagnostics[]> {
  return apiRequest<{ plugins: PluginDiagnostics[] }>('/api/plugins', options).then((r) => r.plugins);
}

export function listMcpServers(options: CallOptions = {}): Promise<McpServer[]> {
  return apiRequest<{ servers: McpServer[] }>('/api/mcp', options).then((r) => r.servers);
}