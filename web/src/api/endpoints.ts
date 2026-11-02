import { ApiError, apiRequest, apiRequestBlob, withQuery } from './client';
import type {
  AgentAction,
  AgentDetail,
  AgentStatus,
  AgentSummary,
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
  PluginPanelData,
  PluginRescanResponse,
  Project,
  ProjectListResponse,
  ProviderTestRequest,
  ProviderTestResult,
  QueuedAccepted,
  QueuedAttachment,
  QueuedKind,
  QueuesResponse,
  RecallResponse,
  RewindPreviewResponse,
  RewindRequest,
  RewindResponse,
  Session,
  SessionDetail,
  SessionInfo,
  SessionModelSelection,
  SessionSummary,
  Stats,
  Terminal,
  TerminalCreateRequest,
  TerminalList,
  WorkspaceFile,
  WorkspaceFileWrite,
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

/** How large an icon the rail is willing to draw. */
export const MAX_PROJECT_ICON_BYTES = 512 * 1024;

/**
 * The icon bytes for a project, or `null` when it ships none.
 *
 * The bytes are fetched through the shared client rather than drawn from an
 * `<img src>`: an `<img>` cannot send the `Authorization` header, so it would
 * break on a deployment with `KARUCODE_AUTH_TOKEN` set (only `/api/events`
 * accepts the token in the query string). A `null` result is the normal case —
 * most projects have no favicon — and is not an error.
 */
export async function getProjectIcon(
  id: string,
  options: CallOptions = {},
): Promise<Blob | null> {
  try {
    return await apiRequestBlob(
      withQuery(`/api/projects/${encodeURIComponent(id)}/favicon`, {
        max_bytes: MAX_PROJECT_ICON_BYTES,
      }),
      options,
    );
  } catch (error) {
    // `204 No Content` is the server's quiet "this project ships no icon" — the
    // normal case, which the rail answers with initials. A 404 is accepted too so
    // an older server keeps working.
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
}

export function getSession(id: string, options: CallOptions = {}): Promise<SessionDetail> {
  return apiRequest<SessionDetail>(`/api/sessions/${encodeURIComponent(id)}`, options);
}

/**
 * A session's counts and token totals, without its transcript.
 *
 * Cheap enough for the inspector's Session tab to poll while a run is in
 * flight: it carries no message bodies, unlike `getSession`.
 */
export function getSessionInfo(id: string, options: CallOptions = {}): Promise<SessionInfo> {
  return apiRequest<SessionInfo>(`/api/sessions/${encodeURIComponent(id)}/info`, options);
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

/**
 * Queue a message against the run that is already working.
 *
 * `content` is what the model will see (the draft with one block per attachment,
 * exactly as `sendMessage` would have sent it), and `draft`/`attachments` are what
 * a recall puts back in the composer. The server answers `202` with the entry, so
 * the pending panel can draw it without waiting for the `queued_messages` frame.
 *
 * Both kinds are refused with `409` when no run is in flight: a message with no
 * run to steer *is* a plain send.
 */
export function sendQueuedMessage(
  sessionId: string,
  mode: QueuedKind,
  message: { content: string; draft?: string; attachments?: QueuedAttachment[] },
  options: CallOptions = {},
): Promise<QueuedAccepted> {
  return apiRequest<QueuedAccepted>(
    withQuery(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, { mode }),
    {
      ...options,
      method: 'POST',
      body: {
        content: message.content,
        ...(message.draft !== undefined ? { draft: message.draft } : {}),
        ...(message.attachments && message.attachments.length > 0
          ? { attachments: message.attachments }
          : {}),
      },
    },
  );
}

/** The pending messages for a session, without loading its transcript. */
export function getQueues(sessionId: string, options: CallOptions = {}): Promise<QueuesResponse> {
  return apiRequest<QueuesResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/queues`,
    options,
  );
}

/**
 * Take a queued message back out, so it never reaches the model.
 *
 * The response carries the entry itself: the draft goes back in the composer and
 * the chips are redrawn. A message that already landed is `404` - the caller then
 * refreshes rather than reviving something the run has applied.
 */
export function recallQueued(
  sessionId: string,
  queuedId: string,
  options: CallOptions = {},
): Promise<RecallResponse> {
  return apiRequest<RecallResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/queues/${encodeURIComponent(queuedId)}`,
    { ...options, method: 'DELETE' },
  );
}

export function cancelRun(sessionId: string, options: CallOptions = {}): Promise<{ cancelled: boolean }> {
  return apiRequest<{ cancelled: boolean }>(
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { ...options, method: 'POST', body: {} },
  );
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/**
 * What a rewind to `messageId` would do. Safe to call on hover: it hides
 * nothing and writes nothing.
 */
export function previewRewind(
  sessionId: string,
  messageId: number,
  options: CallOptions = {},
): Promise<RewindPreviewResponse> {
  return apiRequest<RewindPreviewResponse>(
    withQuery(`/api/sessions/${encodeURIComponent(sessionId)}/rewind`, {
      message: String(messageId),
    }),
    options,
  );
}

/**
 * Rewind a session to `messageId`.
 *
 * `mode: 'none'` hides the tail and stops (the caller then edits the prompt in
 * the composer); `retry` re-sends the original prompt; `note` sends `note`
 * instead. Both of the latter hide the chosen prompt as well, so the
 * replacement does not follow the request it replaces.
 */
export function rewindSession(
  sessionId: string,
  request: RewindRequest,
  options: CallOptions = {},
): Promise<RewindResponse> {
  return apiRequest<RewindResponse>(
    `/api/sessions/${encodeURIComponent(sessionId)}/rewind`,
    { ...options, method: 'POST', body: request },
  );
}

// ---------------------------------------------------------------------------
// Subagents
// ---------------------------------------------------------------------------

/**
 * Every agent the server knows about, newest first.
 *
 * `project` scopes the list to one workspace; `session` and `statuses` narrow
 * it further. This is what lets the Agents view show a subagent running in a
 * session the user is not currently looking at.
 */
export function listAgents(
  params: { project?: string | null; session?: string | null; statuses?: AgentStatus[] } = {},
  options: CallOptions = {},
): Promise<AgentSummary[]> {
  return apiRequest<{ agents: AgentSummary[] }>(
    withQuery('/api/agents', {
      project: params.project ?? undefined,
      session: params.session ?? undefined,
      status: params.statuses && params.statuses.length > 0 ? params.statuses.join(',') : undefined,
    }),
    options,
  ).then((r) => r.agents);
}

/** One agent plus the transcript it wrote (its own messages only). */
export function getAgent(
  id: string,
  params: { messages?: number } = {},
  options: CallOptions = {},
): Promise<AgentDetail> {
  return apiRequest<AgentDetail>(
    withQuery(`/api/agents/${encodeURIComponent(id)}`, { messages: params.messages }),
    options,
  );
}

/**
 * Pause, resume or cancel one live subagent.
 *
 * Only meaningful while the agent's loop exists: an id that already finished
 * answers 404, which the caller shows as "it finished" rather than an error the
 * user has to act on.
 */
export function controlAgent(
  id: string,
  action: AgentAction,
  options: CallOptions = {},
): Promise<{ agent_id: string; action: AgentAction; status: AgentStatus }> {
  return apiRequest(
    `/api/agents/${encodeURIComponent(id)}/${action}`,
    { method: 'POST', ...options },
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

/**
 * Save a workspace file from the viewer's editor.
 *
 * `PUT /api/workspace/file?path=<file>[&project=][&create=][&force=]` takes the
 * **whole** new contents as JSON. `expectedSize` is optimistically checked by the
 * server: when the file changed on disk since it was loaded the save is refused
 * with `409` and nothing is written, unless `force` is set — which is the user's
 * answer to that prompt, so only pass it after asking.
 */
export function saveWorkspaceFile(
  path: string,
  content: string,
  params: {
    project?: string | null;
    /** Size the editor loaded; omit to save without a staleness check. */
    expectedSize?: number;
    /** Create the file if it does not exist (the viewer's "new file" path). */
    create?: boolean;
    /** Overwrite even when the file changed on disk. */
    force?: boolean;
  } = {},
  options: CallOptions = {},
): Promise<WorkspaceFileWrite> {
  return apiRequest<WorkspaceFileWrite>(
    withQuery('/api/workspace/file', {
      path,
      project: params.project ?? undefined,
      create: params.create ? 'true' : undefined,
      force: params.force ? 'true' : undefined,
    }),
    { ...options, method: 'PUT', body: { content, expected_size: params.expectedSize } },
  );
}

/**
 * URL for one workspace file's raw bytes (`GET /api/workspace/raw`).
 *
 * The text endpoint answers JSON, which an `<img>` or `<iframe>` cannot render,
 * so the viewer points those elements at this URL. It is read-only and confined
 * like every other workspace path.
 */
export function workspaceRawUrl(path: string, project?: string | null): string {
  const query = new URLSearchParams({ path });
  if (project) query.set('project', project);
  return `/api/workspace/raw?${query.toString()}`;
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

/**
 * Re-read every plugin source and apply it: new plugins are started, removed
 * ones are dropped and changed ones are replaced, without restarting the server.
 *
 * The body carries the report plus the live diagnostics, so one call is enough
 * to update a list.
 */
export function rescanPlugins(options: CallOptions = {}): Promise<PluginRescanResponse> {
  return apiRequest<PluginRescanResponse>('/api/plugins/reload', {
    ...options,
    method: 'POST',
    body: {},
  });
}

/** Replace one plugin's process (the "I rebuilt it" path). */
export function reloadPlugin(
  name: string,
  options: CallOptions = {},
): Promise<PluginDiagnostics> {
  return apiRequest<PluginDiagnostics>(`/api/plugins/${encodeURIComponent(name)}/reload`, {
    ...options,
    method: 'POST',
    body: {},
  });
}

/** Start or stop one plugin's process without restarting the server. */
export function setPluginEnabled(
  name: string,
  enabled: boolean,
  options: CallOptions = {},
): Promise<PluginDiagnostics> {
  return apiRequest<PluginDiagnostics>(`/api/plugins/${encodeURIComponent(name)}/enable`, {
    ...options,
    method: 'POST',
    body: { enabled },
  });
}

/**
 * Read one plugin panel's payload.
 *
 * The payload is opaque JSON declared by a plugin: render it with a registered
 * component (see `components/pluginPanels.ts`) or as read-only JSON, never as
 * HTML. `project`/`session` scope the read exactly like the other project-scoped
 * routes, and an unknown id is a 404 rather than an empty panel.
 */
export function getPluginPanel(
  plugin: string,
  panel: string,
  params: { project?: string | null; session?: string | null } = {},
  options: CallOptions = {},
): Promise<PluginPanelData> {
  return apiRequest<PluginPanelData>(
    withQuery(
      `/api/plugins/${encodeURIComponent(plugin)}/panels/${encodeURIComponent(panel)}`,
      { project: params.project ?? undefined, session: params.session ?? undefined },
    ),
    options,
  );
}

export function listMcpServers(options: CallOptions = {}): Promise<McpServer[]> {
  return apiRequest<{ servers: McpServer[] }>('/api/mcp', options).then((r) => r.servers);
}

// ---- terminals ------------------------------------------------------------
//
// The durable half of the terminal tab: the rows that make the same tabs appear
// in every browser and after a restart. Attaching to a shell is a websocket,
// which `state/terminalSocket.ts` owns.

export function listTerminals(options: CallOptions = {}): Promise<TerminalList> {
  return apiRequest<TerminalList>('/api/terminals', options);
}

export function createTerminal(
  body: TerminalCreateRequest = {},
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>('/api/terminals', {
    ...options,
    method: 'POST',
    body,
  });
}

/** Rename a terminal. `null` restores the derived default title. */
export function renameTerminal(
  id: string,
  title: string | null,
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>(`/api/terminals/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { title },
  });
}

export function deleteTerminal(id: string, options: CallOptions = {}): Promise<{ deleted: string }> {
  return apiRequest<{ deleted: string }>(`/api/terminals/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}

/** Kill the shell and start a new one, keeping the id, title and position. */
export function restartTerminal(
  id: string,
  params: { cols?: number; rows?: number } = {},
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>(
    withQuery(`/api/terminals/${encodeURIComponent(id)}/restart`, {
      cols: params.cols,
      rows: params.rows,
    }),
    { ...options, method: 'POST', body: {} },
  );
}
