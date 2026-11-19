import { apiRequest, withQuery } from '../client';
import type {
  PluginDiagnostics,
  PluginPanelData,
  PluginRescanResponse,
  PluginToolOutputRecord,
  PluginUiActionResult,
} from '../types/plugins';
import { adminHeaders, type CallOptions } from './common';

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
 * One plugin's tool-output budget: the value in force, the answer on record and
 * the audit trail behind it.
 */
export function getPluginToolOutput(
  name: string,
  options: CallOptions = {},
): Promise<PluginToolOutputRecord> {
  return apiRequest<PluginToolOutputRecord>(
    `/api/plugins/${encodeURIComponent(name)}/tool-output`,
    options,
  );
}

/**
 * Answer a plugin's tool-output request.
 *
 * This is the operator's half of the flow and the only call that creates a
 * grant: the agent files the request, a person answers it here. `bytes` may
 * differ from what was asked for (a grant can lower it); `reason` is recorded
 * with the answer.
 */
export function decidePluginToolOutput(
  name: string,
  decision: 'grant' | 'refuse',
  bytes: number | null,
  reason: string,
  adminToken?: string,
  options: CallOptions = {},
): Promise<PluginToolOutputRecord> {
  return apiRequest<PluginToolOutputRecord>(
    `/api/plugins/${encodeURIComponent(name)}/tool-output/decision`,
    {
      ...options,
      method: 'POST',
      headers: adminHeaders(adminToken, options),
      body: { decision, bytes, reason, channel: 'web' },
    },
  );
}

/** Forget an answer, so the plugin can be asked again (or asked differently). */
export function clearPluginToolOutput(
  name: string,
  adminToken?: string,
  options: CallOptions = {},
): Promise<PluginToolOutputRecord> {
  return apiRequest<PluginToolOutputRecord>(
    `/api/plugins/${encodeURIComponent(name)}/tool-output`,
    { ...options, method: 'DELETE', headers: adminHeaders(adminToken, options) },
  );
}

/**
 * What a plugin-install source offers, fetched before anything runs: the
 * review surface for choosing plugins to install.
 */
export interface PluginInstallPreview {
  source: string;
  kind: 'remote' | 'local';
  resolved?: string | null;
  plugins: Array<{
    name: string;
    description?: string | null;
    /** What the plugin's manifest asks to touch; shown before install. */
    permissions: string[];
    /** Whether installing will run a build. */
    builds: boolean;
    /** Whether an install of this name already exists (needs `force`). */
    already_installed: boolean;
  }>;
  /** Non-fatal findings (operator-only keys stripped, ...) to show up front. */
  notes: string[];
}

export interface PluginInstallJob {
  id: string;
  status: 'running' | 'completed' | 'failed' | 'cancelled';
  /** Progress lines in order, replayed from the poll. */
  events: Array<
    | { kind: 'downloading'; url: string }
    | { kind: 'note'; message: string }
    | { kind: 'building'; plugin: string; command: string }
    | { kind: 'installed'; plugin: string; path: string }
    | { kind: 'done'; installed: number }
  >;
  error?: string | null;
  result?: {
    installed: string[];
    paths: Array<{ name: string; path: string }>;
    /** The rescan the job ran; `added` names the new plugins. */
    rescan: PluginRescanResponse['report'];
  } | null;
  elapsed_ms: number;
}

export interface PluginInstallRequest {
  source: string;
  names?: string[];
  git_ref?: string;
  force?: boolean;
  no_build?: boolean;
  build_timeout_secs?: number;
}

/** Resolve a source and see what it offers, without installing anything. */
export function previewPluginInstall(
  request: PluginInstallRequest,
  options: CallOptions = {},
): Promise<PluginInstallPreview> {
  return apiRequest<PluginInstallPreview>('/api/plugins/install/preview', {
    ...options,
    method: 'POST',
    body: request,
  });
}

/**
 * Start an install job. The answer is the job id; poll it with
 * `getPluginInstallJob`. On success the job has already rescanned the host,
 * so the plugin is live by the time the poll says `completed`.
 */
export function startPluginInstall(
  request: PluginInstallRequest,
  options: CallOptions = {},
): Promise<{ id: string; status: string; poll: string }> {
  return apiRequest<{ id: string; status: string; poll: string }>('/api/plugins/install', {
    ...options,
    method: 'POST',
    body: request,
  });
}

/** The job's snapshot: status, progress lines, and the result when done. */
export function getPluginInstallJob(
  id: string,
  options: CallOptions = {},
): Promise<PluginInstallJob> {
  return apiRequest<PluginInstallJob>(`/api/plugins/install/${encodeURIComponent(id)}`, options);
}

/** Ask a running job to stop. Terminal jobs answer 409. */
export function cancelPluginInstall(
  id: string,
  options: CallOptions = {},
): Promise<{ id: string; status: string }> {
  return apiRequest<{ id: string; status: string }>(
    `/api/plugins/install/${encodeURIComponent(id)}/cancel`,
    { ...options, method: 'POST', body: {} },
  );
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

/**
 * Run one UI action a plugin declared.
 *
 * The action id — not the contribution id — names the handler, because two
 * buttons may share one. `project`/`session` scope the click exactly like a panel
 * read, so a plugin that acts on a workspace knows which one.
 *
 * The answer is already validated by the host: `message` is capped, `refresh`
 * names bare panel ids of this plugin, and `open_url` is only ever `http`/`https`.
 * The caller re-checks the URL anyway before opening a tab.
 */
export function runPluginUiAction(
  plugin: string,
  action: string,
  params: { project?: string | null; session?: string | null } = {},
  options: CallOptions = {},
): Promise<PluginUiActionResult> {
  return apiRequest<PluginUiActionResult>(
    withQuery(
      `/api/plugins/${encodeURIComponent(plugin)}/ui/actions/${encodeURIComponent(action)}`,
      { project: params.project ?? undefined, session: params.session ?? undefined },
    ),
    { ...options, method: 'POST', body: {} },
  );
}
