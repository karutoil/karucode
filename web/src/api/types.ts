// Types mirroring docs/API.md. These are the contract with kc-server; keep field
// names exactly as documented so we notice drift early.

export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export type ErrorCode =
  | 'bad_request'
  | 'not_found'
  | 'permission_denied'
  | 'unauthorized'
  | 'config'
  | 'provider'
  | 'tool'
  | 'store'
  | 'plugin'
  | 'timeout'
  | 'path_escape'
  | 'payload_too_large'
  | 'internal';

export interface ApiErrorBody {
  error: { code: ErrorCode; message: string };
}

export interface Health {
  status: string;
  version: string;
  uptime_secs: number;
  workspace: string;
  /** Every project the server can serve, newest counts included. */
  projects: Project[];
  auth_required: boolean;
  provider_configured: boolean;
  db: string;
}

/** A workspace the agent may work in; one entry in the project rail. */
export interface Project {
  id: string;
  name: string;
  /** Canonical absolute directory. */
  path: string;
  created_at: number;
  updated_at: number;
  session_count: number;
}

export interface ProjectListResponse {
  projects: Project[];
  /** The server's default workspace (KARUCODE_WORKSPACE / --workspace). */
  workspace: string;
}

/** One model the user keeps available on a provider profile. */
export interface ModelEntry {
  id: string;
  name?: string | null;
  context_window?: number | null;
  max_output_tokens?: number | null;
  tool_call?: boolean | null;
  reasoning?: boolean | null;
  /**
   * Whether the model accepts **image** input, so `read_file` may show it a
   * PNG/JPEG/GIF/WebP. `null`/absent means "not declared": the install-wide
   * `model.supports_images` default applies. Prefilled from models.dev
   * (`modalities.input` containing `image`) and editable, because a gateway may
   * serve a vision model under a text-only id or the other way round.
   */
  vision?: boolean | null;
  cost_input?: number | null;
  cost_output?: number | null;
  /** models.dev provider the metadata came from, when prefilled. */
  catalog_provider?: string | null;
}

/** A named OpenAI-compatible endpoint plus its curated model list. */
export interface ProviderProfile {
  id: string;
  name: string;
  provider: string;
  base_url: string;
  models: ModelEntry[];
  request_timeout_secs: number;
  /** The key itself is write-only and never sent to the browser. */
  api_key_set: boolean;
}

export interface ActiveSelection {
  provider: string | null;
  model: string | null;
}

export interface ProviderPatch {
  id: string;
  name?: string;
  provider?: string;
  base_url?: string;
  /** Write-only; empty strings are ignored. */
  api_key?: string;
  clear_api_key?: boolean;
  /** Replaces the whole list when present. */
  models?: ModelEntry[];
  request_timeout_secs?: number;
}

export interface ProvidersPatch {
  upsert?: ProviderPatch[];
  delete?: string[];
}

export interface ActivePatch {
  provider?: string;
  model?: string;
}

/** One models.dev model, normalized by the server for prefilling. */
export interface CatalogModel {
  id: string;
  name: string;
  context_window?: number;
  max_output_tokens?: number;
  tool_call?: boolean;
  reasoning?: boolean;
  /**
   * Whether models.dev says this model takes **image** input. Present only when
   * the document answers: a model whose inputs are `audio` or `pdf` alone is
   * `false` even though it is an "attachment" model, because handing it a PNG
   * fails the whole turn.
   */
  vision?: boolean;
  cost_input?: number;
  cost_output?: number;
}

export interface CatalogProviderSummary {
  id: string;
  name: string;
  model_count: number;
}

export interface CatalogResponse {
  ok: boolean;
  providers: CatalogProviderSummary[];
  provider?: CatalogProviderSummary | null;
  models: CatalogModel[];
  fetched_at_ms: number;
  /** True when an unreachable catalog served a cached copy. */
  stale: boolean;
  error: string | null;
}

/** The best catalog entry for an arbitrary (possibly gateway-prefixed) id. */
export interface CatalogLookupMatch {
  /** models.dev provider id the metadata came from. */
  provider: string;
  /** The suffix of the queried id that matched; equals `model.id`. */
  matched_id: string;
  model: CatalogModel;
}

export interface CatalogLookupResponse {
  ok: boolean;
  /** Null when the catalog does not carry the model (an ordinary miss). */
  match: CatalogLookupMatch | null;
  fetched_at_ms: number;
  stale: boolean;
  error: string | null;
}

export interface ProviderTestRequest {
  profile?: string;
  base_url?: string;
  api_key?: string;
  model?: string;
  request_timeout_secs?: number;
}

export interface PublicModelConfig {
  provider: string;
  base_url: string;
  model: string;
  temperature: number;
  /** Whether a temperature is sent at all (off by default). */
  send_temperature: boolean;
  max_tokens: number | null;
  top_p: number | null;
  request_timeout_secs: number;
  parallel_tool_calls: boolean;
  /**
   * Install-wide default: may `read_file` show the model an image? A
   * `[[providers.models]] vision` entry overrides it per model.
   */
  supports_images: boolean;
  api_key_set: boolean;
}

export interface Limits {
  max_iterations: number;
  max_run_secs: number;
  auto_continue: boolean;
  max_continues: number;
  max_tool_output_bytes: number;
  max_command_output_bytes: number;
  command_timeout_secs: number;
  max_command_timeout_secs: number;
  max_file_read_bytes: number;
  search_max_results: number;
  context_max_chars: number;
  max_tool_calls_per_turn: number;
}

export type ApprovalMode = 'ask' | 'ask_commands' | 'auto' | 'read_only';

export interface ApprovalPolicy {
  mode: ApprovalMode;
  allow_commands: string[];
  auto_approve_paths: string[];
  timeout_secs: number;
}

export interface SubagentConfig {
  max_concurrent: number;
  max_iterations: number;
  max_continues: number;
  allow_recursive: boolean;
  max_depth: number;
  timeout_secs: number;
}

export interface ConfigPluginSummary {
  name: string;
  version: string;
  status: string;
  tools: string[];
  error: string | null;
  permissions: string[];
}

/**
 * One inspector panel a plugin contributes.
 *
 * `data` is not part of this: it is fetched per panel from
 * `GET /api/plugins/:name/panels/:panel` and is opaque JSON, so it is typed as
 * `unknown` at that boundary rather than here.
 */
export interface PluginPanelSummary {
  id: string;
  /** `plugin__<plugin>__<id>`: the tab id and the key for a renderer. */
  hosted_id: string;
  title: string;
  icon: string | null;
  /** `tool` or `static`. Both are read through `panels/read`. */
  source: string;
  /** Suggested poll interval in ms; 0 means "only on explicit refresh". */
  refresh_ms: number;
}

/** Panels contributed by one plugin, in manifest order. */
export interface PluginPanelGroup {
  plugin: string;
  version: string;
  panels: PluginPanelSummary[];
}

/** `GET /api/plugins/:name/panels/:panel` — the payload the UI renders. */
export interface PluginPanelData {
  /** Title the plugin wants shown (its manifest's title when it omits one). */
  title: string;
  /** Opaque JSON. Untrusted: validate before reading any field. */
  data: unknown;
  /** Cache key; changes whenever a fresh read happened. */
  version: number;
  fetched_ms: number;
}

export interface ConfigMcpSummary {
  name: string;
  transport: string;
  status: string;
  tools: string[];
  error: string | null;
}

export interface Config {
  workspace: string;
  /** Live/effective model connection (mirrors the active provider profile). */
  model: PublicModelConfig;
  /** Named endpoints and their model libraries. */
  providers: ProviderProfile[];
  active: ActiveSelection;
  limits: Limits;
  approvals: ApprovalPolicy;
  subagents: SubagentConfig;
  plugins: ConfigPluginSummary[];
  /**
   * Inspector panels contributed by ready plugins, grouped per plugin.
   * Absent on an older server, which simply means "no plugin tabs".
   */
  plugin_panels?: PluginPanelGroup[];
  mcp_servers: ConfigMcpSummary[];
  /**
   * Whether the server re-reads plugin sources on a timer (`plugins_watch`), and
   * how often. Absent on an older server, which never hot-reloads on its own.
   */
  plugins_watch?: boolean;
  plugins_watch_secs?: number;
  settings_locked: boolean;
  /** Provider ids this server can actually build. Drives the settings selector. */
  available_providers?: string[];
  version: string;
}

export interface ModelConfigPatch {
  provider?: string;
  base_url?: string;
  model?: string;
  temperature?: number;
  send_temperature?: boolean;
  max_tokens?: number | null;
  top_p?: number | null;
  request_timeout_secs?: number;
  parallel_tool_calls?: boolean;
  /** Install-wide default for image input; a per-model `vision` wins. */
  supports_images?: boolean;
  /** Write-only. Never rendered back. */
  api_key?: string | null;
  clear_api_key?: boolean;
}

export interface ConfigPatch {
  model?: ModelConfigPatch;
  providers?: ProvidersPatch;
  active?: ActivePatch;
  limits?: Partial<Limits>;
  approvals?: Partial<ApprovalPolicy>;
  subagents?: Partial<SubagentConfig>;
}

export interface ProviderTestResult {
  ok: boolean;
  models?: string[];
  error: string | null;
  latency_ms: number;
}

export interface Stats {
  uptime_secs: number;
  rss_kb: number;
  sessions: number;
  active_runs: number;
  events_emitted: number;
  db_path: string;
}

export type RunStatus =
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'interrupted'
  | 'limit_reached';

/** Per-session model pin: provider + model, optionally a reasoning effort. */
export interface SessionModelSelection {
  provider: string;
  model: string;
  /** `none|minimal|low|medium|high|max`; absent follows the endpoint default. */
  reasoning_effort?: string | null;
}

export interface Session {
  id: string;
  title: string;
  workspace: string;
  /** Per-session model pin; unset follows the install-wide default. */
  model?: SessionModelSelection | null;
  /** Project the session runs in (null for a session created without one). */
  project_id: string | null;
  created_at: number;
  updated_at: number;
}

export interface SessionSummary extends Session {
  message_count: number;
  last_run_status: RunStatus | null;
  running: boolean;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
}

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export interface StoredMessage {
  id: number;
  session_id: string;
  run_id: string | null;
  agent_id: string | null;
  role: MessageRole;
  content: string;
  tool_calls: ToolCall[] | null;
  tool_call_id: string | null;
  name: string | null;
  /** Images the tool returned for a vision model (base64, no `data:` prefix). */
  images?: ImageAttachment[] | null;
  created_at: number;
}

/**
 * An image a tool handed the model, exactly as the server stores it. `data` is
 * standard base64; the UI turns it into a data URL for `<img src>`. Only the
 * media types the vision APIs accept are ever produced (`image/png`, `jpeg`,
 * `gif`, `webp`).
 */
export interface ImageAttachment {
  media_type: string;
  data: string;
  bytes: number;
  name?: string | null;
}

export interface RunRecord {
  id: string;
  session_id: string;
  status: RunStatus;
  model: string;
  prompt: string;
  error: string | null;
  started_at: number;
  finished_at: number | null;
}

export type AgentStatus = 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';

export interface AgentRecord {
  id: string;
  session_id: string;
  parent_run_id: string | null;
  parent_agent_id: string | null;
  name: string;
  role: string;
  task: string;
  status: AgentStatus;
  model: string;
  iterations: number;
  summary: string;
  error: string | null;
  owned_paths: string[];
  created_at: number;
  updated_at: number;
  finished_at: number | null;
}

/**
 * An agent plus the session context the workspace-wide Agents view needs.
 *
 * The server flattens the agent fields into the same object, so this extends
 * `AgentRecord`; `session_title` and `workspace` are what a global list cannot
 * derive on the client without one request per session.
 */
export interface AgentSummary extends AgentRecord {
  session_title: string;
  workspace: string;
  project_id: string | null;
}

/** `GET /api/agents/:id`: the agent and the transcript it wrote. */
export interface AgentDetail {
  agent: AgentSummary;
  messages: StoredMessage[];
  messages_truncated: boolean;
  messages_limit: number;
}

export type ApprovalKind =
  | 'shell_command'
  | 'file_write'
  | 'file_edit'
  | 'delete_file'
  | 'network_tool'
  | 'plugin'
  | 'mcp'
  | 'other';

export type ApprovalStatus = 'pending' | 'resolved' | 'expired';
export type ApprovalDecision = 'allow_once' | 'allow_session' | 'deny' | 'expired';

/** Approval request as delivered by an `approval_requested` event. */
export interface ApprovalRequest {
  id: string;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  session_id: string;
  run_id: string;
  agent_id?: string | null;
}

export interface ApprovalRecord {
  id: string;
  session_id: string;
  run_id: string;
  agent_id: string | null;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  status: ApprovalStatus;
  decision: ApprovalDecision | null;
  created_at: number;
  resolved_at: number | null;
}

export interface ClaimRecord {
  path: string;
  session_id: string;
  agent_id: string | null;
  mode: string;
  created_at: number;
}

export interface SessionDetail {
  session: Session;
  messages: StoredMessage[];
  /**
   * True when the server returned only the newest `messages_limit` messages.
   * The window is the *tail*: without this the transcript would silently start
   * mid-conversation on a long session.
   */
  messages_truncated?: boolean;
  /** The window size the server applied (default 5 000). */
  messages_limit?: number;
  runs: RunRecord[];
  agents: AgentRecord[];
  approvals: ApprovalRecord[];
  claims: ClaimRecord[];
  running: boolean;
}

export interface WorkspaceEntry {
  name: string;
  path: string;
  kind: 'dir' | 'file' | string;
  size: number;
}

export interface WorkspaceTree {
  path: string;
  entries: WorkspaceEntry[];
}

/**
 * `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]`.
 *
 * The request body is the raw file bytes (no base64, no multipart) with
 * `Content-Type: application/octet-stream`; the server stores the bytes
 * verbatim under `<project>/.karucode/attachments/<session>/` so the
 * workspace-confined file tools can read them back.
 */
export interface AttachmentUploadRequest {
  /** Original file name, without directories (`<=160` bytes). */
  name: string;
  /** Raw bytes, at most 8 MiB. */
  data: Blob | ArrayBuffer | Uint8Array;
  /** MIME type; omit for the contract's `application/octet-stream`. */
  contentType?: string;
}

/**
 * `201` response of `POST /api/workspace/upload`.
 *
 * Only these three fields are guaranteed by the contract. The server may send
 * more (`session`, `content_type`, `workspace`, `deduplicated`) — read them only
 * as hints.
 */
export interface AttachmentUploadResponse {
  /** Workspace-relative path — usable in an @mention or a tool argument. */
  path: string;
  /**
   * Name actually written, which can differ from the requested one on a
   * collision (`report.pdf` → `report-1.pdf`). Compose references from `path`.
   */
  name: string;
  /** Bytes written. */
  size: number;
  session?: string;
  content_type?: string;
  /** Absolute project root the file was written under (hint). */
  workspace?: string;
  /** True when the requested name was already taken (hint). */
  deduplicated?: boolean;
}

export interface WorkspaceFile {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
  binary: boolean;
  language: string | null;
}

/**
 * `200` response of `PUT /api/workspace/file` — the editor's save.
 *
 * The body sent is `{ content, expected_size? }`; `created` says the save made a
 * new file rather than replacing one.
 */
export interface WorkspaceFileWrite {
  path: string;
  /** Bytes written (`content.length` in UTF-8). */
  size: number;
  /** True when the file did not exist before this save. */
  created: boolean;
  language: string | null;
}

export interface GitFile {
  path: string;
  status: string;
  staged: boolean;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  error: string | null;
  files: GitFile[];
}

export interface GitDiff {
  diff: string;
  truncated: boolean;
}

export interface PluginToolSpec {
  name: string;
  description: string;
  parameters: JsonValue;
}

export interface PluginManifest {
  name: string;
  version: string;
  protocol?: string;
  capabilities?: string[];
  permissions?: string[];
  hooks?: string[];
}

export interface PluginDiagnostics {
  name: string;
  status: string;
  manifest: PluginManifest | null;
  tools: PluginToolSpec[];
  error: string | null;
  pid: number | null;
  restart_count: number;
}

/** What one plugin rescan changed. Empty arrays mean "nothing to do". */
export interface PluginRescanReport {
  added: string[];
  removed: string[];
  restarted: string[];
  unchanged: string[];
}

export interface PluginRescanResponse {
  report: PluginRescanReport;
  plugins: PluginDiagnostics[];
}

export interface McpServer {
  name: string;
  transport: string;
  status: string;
  tools: string[];
  error: string | null;
}

export interface EventEnvelope {
  seq: number;
  ts_ms: number;
  session_id?: string | null;
  run_id?: string | null;
  agent_id?: string | null;
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  /**
   * Prompt tokens the provider served from its prefix cache. Absent when the
   * endpoint does not report them - which is different from a 0% hit rate.
   */
  cached_tokens?: number;
}

/** Known server-sent events. Unknown event types are ignored by the reducer. */
export type KnownEvent = EventEnvelope &
  (
    | { type: 'session_created'; session_id: string; title: string }
    | { type: 'session_renamed'; session_id: string; title: string }
    | { type: 'session_model_changed'; session_id: string; model: SessionModelSelection | null }
    | { type: 'session_deleted'; session_id: string }
    | { type: 'steer_applied'; steer_id: string; content: string }
    | { type: 'run_started'; run_id: string; prompt: string }
    | { type: 'run_finished'; run_id: string; status: RunStatus; error?: string | null }
    | { type: 'run_cancelled'; run_id: string }
    | { type: 'iteration_started'; iteration: number }
    | { type: 'text_delta'; delta: string }
    | { type: 'reasoning_delta'; delta: string }
    | { type: 'usage'; input_tokens: number; output_tokens: number; cached_tokens?: number }
    | {
        type: 'tool_call_started';
        call_id: string;
        name: string;
        arguments: JsonValue;
      }
    | {
        type: 'tool_call_finished';
        call_id: string;
        name: string;
        ok: boolean;
        output: string;
        truncated: boolean;
        duration_ms: number;
        /** Images the tool attached for a vision model; absent when it refused. */
        images?: ImageAttachment[];
      }
    | { type: 'command_started'; call_id: string; command: string; cwd: string }
    | {
        type: 'command_output';
        call_id: string;
        stream: 'stdout' | 'stderr' | string;
        chunk: string;
      }
    | {
        type: 'command_finished';
        call_id: string;
        exit_code: number | null;
        duration_ms: number;
        truncated: boolean;
      }
    | { type: 'approval_requested'; approval: ApprovalRequest }
    | { type: 'approval_resolved'; approval_id: string; decision: ApprovalDecision }
    | {
        type: 'agent_spawned';
        agent_id: string;
        name: string;
        role: string;
        task: string;
        model: string;
        parent_run_id: string;
      }
    | { type: 'agent_progress'; agent_id: string; iterations: number; note: string }
    | {
        type: 'agent_finished';
        agent_id: string;
        status: AgentStatus;
        summary: string;
        error?: string | null;
      }
    | { type: 'stream_retry'; attempt: number; max_attempts: number; reason: string }
    | { type: 'notice'; level: string; message: string }
    // Transport marker, not a session event: the SSE pump dropped frames for this
    // client. Exempt from the `seq` dedupe (see api/events.ts).
    | { type: 'stream_lagged'; skipped: number }
    | { type: 'error'; message: string; kind?: string | null }
  );

export type KarucodeEvent = KnownEvent;

/** Parsed stream payload before we know whether the type is supported. */
export interface RawEvent extends EventEnvelope {
  type: string;
  [key: string]: unknown;
}

export type ConnectionState =
  | 'idle'
  | 'connecting'
  | 'open'
  | 'reconnecting'
  | 'unauthorized'
  /** The server refused this origin (Host allowlist): retrying cannot help. */
  | 'blocked';

export interface Toast {
  id: number;
  tone: 'info' | 'success' | 'error';
  message: string;
}

/**
 * One browser terminal.
 *
 * `id`, `title`, `cwd` and `position` are the *durable* half, stored in SQLite:
 * they come back in every browser and after every server restart. `running`,
 * `pid` and `attached` are the *process* half and describe the shell this
 * server has right now - a terminal with `running: false` is a tab that will
 * start a fresh shell when opened.
 */
export interface Terminal {
  id: string;
  title: string;
  /** Absolute directory the shell was started in. */
  cwd: string;
  /** The project it was created in, as a label only. */
  project: string | null;
  /** Sort key for the tab strip; ties break on `created_at`. */
  position: number;
  created_at: number;
  updated_at: number;
  last_used_at: number;
  /** A live shell exists for this tab right now. */
  running: boolean;
  pid: number;
  /** How many browsers are attached (0 or 1 in practice). */
  attached: number;
}

export interface TerminalList {
  /** False when the server has the terminal disabled; the tab is hidden then. */
  enabled: boolean;
  terminals: Terminal[];
  /** `terminals.max_terminals`, so the UI can disable **New** at the cap. */
  max_terminals?: number;
  /** The resolved shell program, shown as a hint. */
  shell?: string;
  scrollback_bytes?: number;
}

export interface TerminalCreateRequest {
  title?: string;
  cwd?: string;
  project?: string | null;
  cols?: number;
  rows?: number;
}

/**
 * A control frame from the terminal websocket. Binary frames are raw PTY bytes;
 * only text frames are JSON, so a program printing arbitrary bytes cannot forge
 * one of these.
 */
export type TerminalControlFrame =
  | { type: 'ready'; replay: boolean; replay_bytes: number; pid: number; running: boolean }
  | { type: 'exit'; code: number | null }
  | { type: 'lagged'; missed: number }
  | { type: 'detached' }
  | { type: 'error'; message: string }
  | { type: 'pong' };