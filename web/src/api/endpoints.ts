import { apiRequest, withQuery } from './client';
import type {
  ApprovalDecision,
  ApprovalRecord,
  CatalogResponse,
  Config,
  ConfigPatch,
  GitDiff,
  GitStatus,
  Health,
  McpServer,
  PluginDiagnostics,
  Project,
  ProjectListResponse,
  ProviderTestRequest,
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

export function testProvider(
  body: ProviderTestRequest = {},
  options: CallOptions = {},
): Promise<ProviderTestResult> {
  return apiRequest<ProviderTestResult>('/api/provider/test', {
    ...options,
    method: 'POST',
    body,
  });
}

/** models.dev metadata: provider list, or one provider's models. */
export function getModelsCatalog(
  params: { provider?: string; refresh?: boolean } = {},
  options: CallOptions = {},
): Promise<CatalogResponse> {
  return apiRequest<CatalogResponse>(
    withQuery('/api/models/catalog', {
      provider: params.provider,
      refresh: params.refresh ? 'true' : undefined,
    }),
    options,
  );
}

export function getStats(options: CallOptions = {}): Promise<Stats> {
  return apiRequest<Stats>('/api/stats', options);
}

export function listSessions(
  projectId?: string | null,
  options: CallOptions = {},
): Promise<SessionSummary[]> {
  return apiRequest<{ sessions: SessionSummary[] }>(
    withQuery('/api/sessions', { project: projectId ?? undefined }),
    options,
  ).then((r) => r.sessions);
}

export function createSession(
  title?: string,
  projectId?: string | null,
  options: CallOptions = {},
): Promise<Session> {
  const body: Record<string, string> = {};
  if (title) body.title = title;
  if (projectId) body.project = projectId;
  return apiRequest<Session>('/api/sessions', {
    ...options,
    method: 'POST',
    body,
  });
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export function listProjects(options: CallOptions = {}): Promise<ProjectListResponse> {
  return apiRequest<ProjectListResponse>('/api/projects', options);
}

export function createProject(
  path: string,
  name?: string,
  options: CallOptions = {},
): Promise<Project> {
  return apiRequest<{ project: Project }>('/api/projects', {
    ...options,
    method: 'POST',
    body: name ? { path, name } : { path },
  }).then((r) => r.project);
}

export function renameProject(
  id: string,
  name: string,
  options: CallOptions = {},
): Promise<Project> {
  return apiRequest<{ project: Project }>(`/api/projects/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { name },
  }).then((r) => r.project);
}

export function deleteProject(
  id: string,
  force = false,
  options: CallOptions = {},
): Promise<{ deleted: boolean; deleted_sessions: number }> {
  return apiRequest<{ deleted: boolean; deleted_sessions: number }>(
    withQuery(`/api/projects/${encodeURIComponent(id)}`, { force: force ? 'true' : undefined }),
    { ...options, method: 'DELETE' },
  );
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
  project?: string | null,
  options: CallOptions = {},
): Promise<WorkspaceTree> {
  return apiRequest<WorkspaceTree>(
    withQuery('/api/workspace/tree', { path, depth, project: project ?? undefined }),
    options,
  );
}

export function getWorkspaceFile(
  path: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<WorkspaceFile> {
  return apiRequest<WorkspaceFile>(
    withQuery('/api/workspace/file', { path, project: project ?? undefined }),
    options,
  );
}

export function getGitStatus(
  project?: string | null,
  options: CallOptions = {},
): Promise<GitStatus> {
  return apiRequest<GitStatus>(
    withQuery('/api/git/status', { project: project ?? undefined }),
    options,
  );
}

export function getGitDiff(
  path: string,
  staged: boolean,
  base?: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<GitDiff> {
  return apiRequest<GitDiff>(
    withQuery('/api/git/diff', {
      path,
      staged,
      base: base ?? undefined,
      project: project ?? undefined,
    }),
    options,
  );
}

export function listPlugins(options: CallOptions = {}): Promise<PluginDiagnostics[]> {
  return apiRequest<{ plugins: PluginDiagnostics[] }>('/api/plugins', options).then((r) => r.plugins);
}

export function listMcpServers(options: CallOptions = {}): Promise<McpServer[]> {
  return apiRequest<{ servers: McpServer[] }>('/api/mcp', options).then((r) => r.servers);
}