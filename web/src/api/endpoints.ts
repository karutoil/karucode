import { apiRequest, withQuery } from './client';
import type {
  ApprovalDecision,
  ApprovalRecord,
  AttachmentUploadRequest,
  AttachmentUploadResponse,
  CatalogLookupResponse,
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
  SessionModelSelection,
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

/**
 * models.dev metadata for one model id. Gateway routing prefixes
 * (`a6api/model`, `a6api/upstream/model`) are stripped server-side until a
 * suffix matches; `match` is null when the catalog does not carry the model.
 */
export function lookupModelMetadata(
  model: string,
  options: CallOptions = {},
): Promise<CatalogLookupResponse> {
  return apiRequest<CatalogLookupResponse>(
    withQuery('/api/models/lookup', { model }),
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

/**
 * Pin this session to a provider + model (optionally with a reasoning effort),
 * or pass null to follow the install-wide default again.
 */
export function setSessionModel(
  id: string,
  selection: SessionModelSelection | null,
  options: CallOptions = {},
): Promise<Session> {
  return apiRequest<Session>(`/api/sessions/${encodeURIComponent(id)}/model`, {
    ...options,
    method: 'PATCH',
    body: selection ?? {},
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

/** Scope for an upload: the session owns the directory, the project the root. */
export interface UploadAttachmentParams {
  session: string;
  project?: string | null;
}

/**
 * Store a local file in the workspace so the agent can open it.
 *
 * `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]` takes
 * the raw bytes as the request body and answers `201` with the workspace-relative
 * path under `.karucode/attachments/<session>/` that the message must reference.
 * Names are never overwritten, so the response may report `report-1.pdf` where
 * `report.pdf` was sent: always compose the reference from the RESPONSE, never
 * from the local `File.name`.
 */
export function uploadAttachment(
  file: AttachmentUploadRequest,
  params: UploadAttachmentParams,
  options: CallOptions = {},
): Promise<AttachmentUploadResponse> {
  return apiRequest<AttachmentUploadResponse>(
    withQuery('/api/workspace/upload', {
      session: params.session,
      name: file.name,
      project: params.project ?? undefined,
    }),
    {
      ...options,
      method: 'POST',
      rawBody: file.data,
      contentType: file.contentType,
    },
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