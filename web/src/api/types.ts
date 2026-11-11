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
  | 'update'
  | 'internal';

export interface ApiErrorBody {
  error: { code: ErrorCode; message: string };
}

export interface Health {
  status: string;
  /** The build number as a string (`"107"`). See `BuildInfo`. */
  version: string;
  /** The same number as an integer. Absent on a server that predates build numbers. */
  build?: number;
  /** Short commit sha this binary was built from. */
  commit?: string;
  /** Commit date, RFC 3339. */
  commit_date?: string;
  /** Built from a working tree with uncommitted changes. */
  dirty?: boolean;
  uptime_secs: number;
  /**
   * The server's implicit workspace, or `null` on a fresh install: nothing is
   * chosen until the operator creates a project in the web UI.
   */
  workspace: string | null;
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
  /** Workbench result-pane width; absent means the product default. */
  result_panel_width?: number;
}

export interface ProjectListResponse {
  projects: Project[];
  /**
   * The server's implicit workspace (KARUCODE_WORKSPACE / --workspace), or `null`
   * when it was started without one — then the project list is the only source of
   * directories.
   */
  workspace: string | null;
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
  /**
   * The endpoint fronts several upstream channels, so a 401/403/404 means "this
   * channel's key is dead" and is worth another attempt.
   */
  aggregator: boolean;
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
  /** Retry 401/403/404: the next attempt lands on another upstream channel. */
  aggregator?: boolean;
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
  /** Model turns for one run. `0` - the default - means no ceiling. */
  max_iterations: number;
  max_run_secs: number;
  max_tool_output_bytes: number;
  max_command_output_bytes: number;
  command_timeout_secs: number;
  max_command_timeout_secs: number;
  max_file_read_bytes: number;
  search_max_results: number;
  context_max_chars: number;
  max_tool_calls_per_turn: number;
  /**
   * How many times a turn the provider gave up on is asked again, after a wait.
   * `0` disables the layer. Each retry is a fresh generation.
   */
  max_turn_retries: number;
  /** Wait before the first turn retry, in ms; doubles per retry, capped 8x. */
  turn_retry_delay_ms: number;
  /**
   * Run a turn's independent tool calls at the same time. Off runs every call
   * one at a time, in order. Not the same key as `model.parallel_tool_calls`,
   * which asks the endpoint for several calls in one turn.
   */
  parallel_tool_calls: boolean;
}

export type ApprovalMode = 'ask' | 'ask_commands' | 'auto' | 'read_only';

export interface ApprovalPolicy {
  mode: ApprovalMode;
  allow_commands: string[];
  auto_approve_paths: string[];
  timeout_secs: number;
}

export interface SubagentConfig {
  /** Subagent loops allowed to run at once, across the whole install. */
  max_concurrent: number;
  /** Model turns for one subagent. `0` - the default - means no ceiling. */
  max_iterations: number;
  allow_recursive: boolean;
  max_depth: number;
  timeout_secs: number;
  /** Whether a spawn may detach and keep working after its tool call returns. */
  allow_background: boolean;
  /** Ceiling on live detached agents per session. */
  max_background: number;
  /** Default bound on `wait_for_agents`, in seconds. */
  wait_timeout_secs: number;
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

/**
 * One control a plugin contributes, as `GET /api/config` describes it.
 *
 * Described, never executed: `kind`, `slot`, `icon` and `target` are names from
 * closed sets the app owns, and `label`/`title` are plain text. Nothing here is
 * markup, a URL, a selector or code.
 */
export interface PluginUiContributionWire {
  plugin: string;
  version: string;
  /** `button`, `command` or `keybinding`. (`hide` entries are folded into `hidden`.) */
  kind: string;
  id: string;
  hosted_id: string;
  /** For a button: the slot it mounts in. */
  slot?: string | null;
  label: string;
  title?: string | null;
  icon?: string | null;
  /** The `ui/action` id to run. Exactly one of `action` / `message` is set. */
  action?: string | null;
  /** A user turn to send instead of running an action. */
  message?: string | null;
  /** For a keybinding: the canonical chord (`mod+shift+f`). */
  keys?: string | null;
}

/** A built-in control one or more plugins asked the app to hide. */
export interface PluginUiHiddenWire {
  target: string;
  /** Plugins that asked, so a missing button can name who took it. */
  plugins: string[];
}

/**
 * `GET /api/config`'s `plugin_ui`.
 *
 * `slots` and `hideable` are the server's catalogues: they travel with the
 * payload so a client can report a mismatch instead of hard-coding a mount point.
 */
export interface PluginUiConfig {
  contributions?: PluginUiContributionWire[];
  hidden?: PluginUiHiddenWire[];
  slots?: string[];
  hideable?: string[];
}

/** `POST /api/plugins/:name/ui/actions/:action` — the plugin's validated answer. */
export interface PluginUiActionResult {
  /** `false` renders the message as an error. */
  ok: boolean;
  /** The toast text. Already capped by the host. */
  message: string;
  /** Bare panel ids of this plugin the UI should re-read. */
  refresh: string[];
  /** An `http`/`https` URL to open in a new tab, or null. */
  open_url: string | null;
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
  /**
   * Server-side notification routing. Absent on an older server, which emits no
   * `notification` events at all.
   */
  notifications?: NotificationsConfig;
  plugins: ConfigPluginSummary[];
  /**
   * Inspector panels contributed by ready plugins, grouped per plugin.
   * Absent on an older server, which simply means "no plugin tabs".
   */
  plugin_panels?: PluginPanelGroup[];
  /**
   * What plugins contribute to the app's own UI: buttons in named slots, palette
   * commands, and built-in controls they asked to hide.
   *
   * Absent on an older server, which simply means "no plugin controls". The
   * frontend re-validates every entry (`components/uiContributions.ts`) instead
   * of trusting the shape.
   */
  plugin_ui?: PluginUiConfig;
  mcp_servers: ConfigMcpSummary[];
  /**
   * Whether the server re-reads plugin sources on a timer (`plugins_watch`), and
   * how often. Absent on an older server, which never hot-reloads on its own.
   */
  plugins_watch?: boolean;
  plugins_watch_secs?: number;
  /**
   * `[updates]`: whether the server looks for a newer build. Absent on an older
   * server, which never checks (and has no update routes at all).
   */
  updates?: UpdatesConfig;
  settings_locked: boolean;
  /** Provider ids this server can actually build. Drives the settings selector. */
  available_providers?: string[];
  /**
   * The web-UI access policy. Absent on an older server, which has no sign-in at
   * all beyond the bearer token.
   */
  access?: AccessConfig;
  version: string;
}

/** Which proxy sits in front of the server; it decides what forwarded headers count. */
export type TunnelKind = 'none' | 'cloudflare' | 'custom';

/**
 * `[access]` as `GET /api/config` reports it.
 *
 * Credentials are deliberately **not** here: a password hash and a passkey's
 * public key are served by `/api/auth/status` and `/api/auth/passkeys`, which
 * report only what the UI has to know (`password_set`, the passkey list).
 */
export interface AccessConfig {
  /** Require a password or passkey before `/api` answers. */
  enabled: boolean;
  /** `https://host[:port]` people type to reach this server, or `null`. */
  public_url: string | null;
  session_days: number;
  tunnel: TunnelKind;
  /** Extra proxy IPs/CIDRs whose forwarded headers are believed. */
  trusted_proxies: string[];
  /** Derived on the server: is any forwarded header read at all? */
  trusts_forwarded_headers: boolean;
  /** Derived: are Cloudflare's published ranges trusted? */
  cloudflare_ranges: boolean;
  /** `public_url` without its scheme, for display. */
  public_authority: string | null;
}

export interface AccessPatch {
  enabled?: boolean;
  /** An empty string clears it (meaning "use the browser's own origin"). */
  public_url?: string;
  session_days?: number;
  tunnel?: TunnelKind;
  trusted_proxies?: string[];
}

/** One registered passkey, as the settings screen lists it. */
export interface AuthPasskey {
  /** base64url credential id, also the handle used by PATCH/DELETE. */
  id: string;
  name: string;
  created_at: number;
  last_used_at: number | null;
}

/** One signed-in browser. `id` is a short, non-reversible handle. */
export interface AuthSession {
  id: string;
  created_at: number;
  expires_at: number;
  method: 'password' | 'passkey';
  label: string;
}

/**
 * `GET /api/auth/status`: everything the sign-in screen and the onboarding
 * wizard need, and the only auth payload available without a credential.
 */
export interface AuthStatus {
  enabled: boolean;
  sign_in_required: boolean;
  password_set: boolean;
  passkey_count: number;
  /** No credential exists yet: the UI has to offer "create", not "sign in". */
  needs_setup: boolean;
  signed_in: boolean;
  signed_in_with: string | null;
  token_required: boolean;
  session_days: number;
  public_url: string | null;
  tunnel: TunnelKind;
  trusted_proxies: string[];
  cloudflare_proxies: boolean;
  /** The origin this request is on, and the relying party a passkey would bind to. */
  origin: string | null;
  rp_id: string | null;
  min_password_len: number;
  max_password_len: number;
  passkey_challenge_ttl_secs: number;
}

/** What a passkey ceremony needs from the server before the browser can run it. */
export interface PasskeyRequestOptions {
  challenge_id: string;
  challenge: string;
  rp_id: string;
  timeout_ms: number;
  allow: { id: string; type: 'public-key' }[];
}

export interface PasskeyRegistrationOptions {
  challenge_id: string;
  challenge: string;
  rp: { id: string; name: string };
  user: { id: string; name: string; displayName: string };
  timeout_ms: number;
  exclude: { id: string; type: 'public-key' }[];
}

/** The browser's answer to `navigator.credentials.create`. */
export interface PasskeyRegistrationResult {
  credential_id: string;
  client_data_json: string;
  attestation_object: string;
}

/** The browser's answer to `navigator.credentials.get`. */
export interface PasskeyAssertionResult {
  credential_id: string;
  client_data_json: string;
  authenticator_data: string;
  signature: string;
}

/** `GET /api/onboarding`: whether the first-run wizard should run. */
export interface OnboardingState {
  needed: boolean;
  completed: boolean;
  provider_configured: boolean;
  has_sessions: boolean;
  project_count: number;
  has_credential: boolean;
  access: AccessSettingsSummary;
  workspace: string | null;
  version: string;
}

/** The Access tab's own payload (part of the onboarding state too). */
export interface AccessSettingsSummary {
  enabled: boolean;
  password_set: boolean;
  passkeys: AuthPasskey[];
  sessions: AuthSession[];
  public_url: string | null;
  session_days: number;
  tunnel: TunnelKind;
  trusted_proxies: string[];
  trusts_forwarded_headers: boolean;
  cloudflare_ranges: boolean;
  token_required: boolean;
  settings_locked: boolean;
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
  notifications?: NotificationsPatch;
  access?: AccessPatch;
  updates?: UpdatesPatch;
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
  /**
   * The newest run's status, or absent when the session has never run.
   *
   * `skip_serializing_if = "Option::is_none"` on the Rust side
   * (`kc-core/src/store.rs`), so this key really can be missing. Every consumer
   * compares it (`=== 'failed'`, `=== 'running'`), which is safe for `undefined`
   * as well as `null` - the type says so rather than pretending otherwise.
   */
  last_run_status?: RunStatus | null;
  running: boolean;
  /**
   * True for a control-plane chat: a conversation that acts on *other* projects
   * instead of running in one. Absent (or `false`) is the ordinary project
   * session — the server skips the key when it is false, so `undefined` and
   * `false` must behave the same here.
   */
  control?: boolean;
}

/**
 * A control chat as the control plane lists it: the session row plus what a
 * cross-project overview needs and a session detail cannot carry — how many of
 * its subagents are live right now, and the model it is pinned to.
 */
export interface ControlChatSummary extends SessionSummary {
  agents: { live: number; total: number };
  /**
   * Approval questions parked on this chat's run. Non-zero means the chat is
   * waiting on a person, which the rail says out loud rather than hiding behind a
   * colour.
   */
  pending_approvals?: number;
  /** Human-readable model pin (`Provider / model`), or `null` when it follows the default. */
  model_label?: string | null;
}

/**
 * `GET /api/control/overview`: one read for the whole control plane.
 *
 * The browser pane, the hero counters and the running/attention lists all come
 * from this object, so opening the control plane costs one request instead of
 * one per project. `control` is the control-chat list (the same rows
 * `GET /api/control/sessions` returns) and `live_agents` is the flat agent list
 * `agentsForControlChat` filters, so one snapshot never disagrees with itself.
 */
export interface ControlOverview {
  /** Server clock when the snapshot was taken, so the UI can say how fresh it is. */
  generated_at: number;
  sessions: { total: number; running: number; failed: number; control: number };
  agents: { live: number; paused: number; failed: number; total: number };
  /**
   * Approval questions parked right now. Each one is a run that cannot progress
   * until a person answers, which is why it is drawn ahead of everything else.
   */
  approvals: { pending: number };
  projects: {
    id: string;
    name: string;
    path: string;
    sessions: number;
    running: number;
  }[];
  control: ControlChatSummary[];
  running: SessionSummary[];
  attention: SessionSummary[];
  /** Project sessions stalled on an unanswered approval, newest activity first. */
  waiting: SessionSummary[];
  recent: SessionSummary[];
  live_agents: AgentSummary[];
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
  /**
   * The model's reasoning for this turn, when the endpoint streams analysis
   * separately from the answer (`reasoning_content`). The server stores it with
   * the assistant row, so a reloaded transcript can still show the thinking
   * block; absent on older rows and on models that do not think out loud.
   */
  reasoning_content?: string | null;
  /**
   * When set, a rewind hid this message. The server already filters hidden rows
   * out of every read, so this is only present on a row a caller fetched by id.
   */
  rewound_at?: number | null;
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
  /** Absent on the wire when the run did not fail (`skip_serializing_if`). */
  error?: string | null;
  started_at: number;
  /** Absent on the wire while the run is still in flight, or if it never settled. */
  finished_at?: number | null;
}

/**
 * A subagent's status.
 *
 * `paused` is live, not settled: the agent's loop is parked between iterations,
 * it still holds a subagent slot, and it can be resumed or killed. Anything that
 * asks "is this agent still around" has to treat `running | paused` as the answer.
 */
export type AgentStatus = 'running' | 'paused' | 'completed' | 'failed' | 'cancelled' | 'interrupted';

/** Statuses whose loop is still alive. */
export const LIVE_AGENT_STATUSES: readonly AgentStatus[] = ['running', 'paused'];

export function isAgentLive(status: AgentStatus): boolean {
  return LIVE_AGENT_STATUSES.includes(status);
}

/** `POST /api/agents/:id/:action`. */
export type AgentAction = 'pause' | 'resume' | 'cancel';

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
  /** True when the spawn detached and the tool call returned immediately. */
  background: boolean;
  /**
   * The agent's own last statement of what it is doing. Advisory, not measured.
   *
   * Optional because the server serialises it with
   * `skip_serializing_if = "String::is_empty"`: an agent that has never called
   * `report_status` sends no `note` key at all. The state layer fills in `''`
   * (`sessionAgentFromRecord`), but a row can reach a component unnormalised, so
   * readers must not assume it is a string.
   */
  note?: string;
  /**
   * The agent's claimed completion, 0..1. Absent when it has not said.
   *
   * `undefined` means the same as `null` here: the server skips a `None`
   * `progress`, so "has not said" arrives as a missing key, not a null one.
   */
  progress?: number | null;
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
  /**
   * Absent when the session has no project (`skip_serializing_if =
   * "Option::is_none"`). Only `openAgentSession` reads it, as a truthiness guard
   * before `switchProject`, so `undefined` and `null` behave the same.
   */
  project_id?: string | null;
}

/** `GET /api/agents/:id`: the agent and the transcript it wrote. */
export interface AgentDetail {
  agent: AgentSummary;
  messages: StoredMessage[];
  messages_truncated: boolean;
  messages_before?: number | null;
  messages_limit: number;
}

/**
 * Token totals from `GET /api/sessions/:id/info`.
 *
 * `cache_hit_ratio` is `null` when no call reported a prefix-cache figure —
 * *unknown*, which is not the same as a 0% hit. `cached_known_calls` is the
 * denominator, so the UI can say "no cache data" instead of drawing an empty bar.
 */
export interface UsageTotals {
  /** Provider calls recorded in this scope. */
  calls: number;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  /** How many of `calls` reported a cache figure at all. */
  cached_known_calls: number;
  /** 0..1, or `null` when no call reported one. */
  cache_hit_ratio: number | null;
  /** The newest call's prompt size: the context-window fill, not a sum. */
  last_input_tokens: number;
}

/** One subagent's share of a session, from `GET /api/sessions/:id/info`. */
export interface SessionInfoAgent {
  id: string;
  name: string;
  role: string;
  status: AgentStatus;
  model: string;
  iterations: number;
  /** `null` while the agent is still running. */
  duration_ms: number | null;
  created_at: number;
  finished_at: number | null;
  /** Messages in this agent's own transcript. */
  messages: number;
  usage: UsageTotals;
}

/** One row of the tool-call breakdown. */
export interface SessionInfoTool {
  name: string;
  count: number;
}

/** `GET /api/sessions/:id/info` — counts and totals, never message bodies. */
export interface SessionInfo {
  session: Session;
  running: boolean;
  stats: {
    messages_total: number;
    messages_main: number;
    messages_subagent: number;
    messages_by_role: Record<string, number>;
    messages_main_by_role: Record<string, number>;
    tool_calls_total: number;
    tool_calls: Record<string, number>;
    runs_by_status: Record<string, number>;
    runs_total: number;
    /** Wall clock during which a run was in flight; idle gaps excluded. */
    working_ms: number;
  };
  usage: {
    main: UsageTotals;
    /** Each subagent's totals, keyed by agent id. */
    by_agent: Record<string, UsageTotals>;
    /** The whole session: main plus every subagent. */
    total: UsageTotals;
  };
  context: {
    /** The model this session resolves to right now. */
    model: string;
    /** The model entry's `context_window`, or `null` when it declares none. */
    window: number | null;
    /** The newest main-agent call's prompt size (the window fill). */
    used: number;
  };
  agents: SessionInfoAgent[];
  messages: {
    total: number;
    main: number;
    subagent: number;
    by_role: Record<string, number>;
    main_by_role: Record<string, number>;
  };
  tools: {
    total: number;
    /** Capped server-side; `truncated` says more names exist. */
    by_name: SessionInfoTool[];
    truncated: boolean;
  };
  runs: {
    total: number;
    by_status: Record<string, number>;
    working_ms: number;
    /** The newest few, for a timeline. */
    recent: RunRecord[];
  };
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
  /** Absent when a top-level run raised the approval (`skip_serializing_if`). */
  agent_id?: string | null;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  status: ApprovalStatus;
  /** Absent while pending. Read through `??`/truthiness everywhere. */
  decision?: ApprovalDecision | null;
  created_at: number;
  /** Absent while pending. Nothing in the UI reads it; safe either way. */
  resolved_at?: number | null;
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
  /** Exclusive cursor for the next older page, when available. */
  messages_before?: number | null;
  /** The maximum row count, not necessarily the number returned (byte limits also apply). */
  messages_limit?: number;
  runs: RunRecord[];
  agents: AgentRecord[];
  approvals: ApprovalRecord[];
  claims: ClaimRecord[];
  running: boolean;
  /**
   * Messages the user queued while a run was in flight, oldest first.
   *
   * They are not transcript rows: they reach the conversation only when the
   * runtime folds a steer in, or starts the next run for a follow-up. Carried on
   * the snapshot so a reload (or a second browser) shows the same pending work
   * instead of silently losing it.
   */
  queued_steers?: QueuedMessage[];
  queued_follow_ups?: QueuedMessage[];
  /** When this session's transcript was last rewound, if it ever was. */
  last_rewind_at?: number | null;
}

// ---------------------------------------------------------------------------
// Queued messages (steer / follow-up / recall)
// ---------------------------------------------------------------------------

/** How a queued message relates to the run it was accepted during. */
export type QueuedKind = 'steer' | 'follow_up';

/**
 * One file attached to a queued message.
 *
 * Only the chip: the bytes are already inside the message's `content`, which is
 * what the model sees. `path` is where an upload landed (or the inline file's
 * own path).
 */
export interface QueuedAttachment {
  name: string;
  path: string;
  mode: 'inline' | 'upload';
  /** Bytes; `0` means the server did not record a size. */
  size: number;
  /**
   * The inlined text of an `inline` attachment.
   *
   * Sent back so a recall can rebuild the chip the composer drew: the browser
   * read this from a local file the server never saw, so it is the only copy
   * that can be inlined again. An upload has none - the file is on disk at
   * `path`.
   */
  content?: string;
}

/** One message waiting for a run to pick it up. */
export interface QueuedMessage {
  /** `steer_…` or `followup_…`; what a recall names. */
  id: string;
  kind: QueuedKind;
  /** The exact text the model will see (draft + one block per attachment). */
  content: string;
  /**
   * The draft to restore on recall; the content when nothing was attached.
   *
   * **Optional on the wire.** Rust declares it `Option<String>` with
   * `skip_serializing_if = "Option::is_none"` (`kc-core/src/queue.rs`), so it is
   * absent for the normal case - a message with no attachments, where the draft
   * and the content are the same string. `GET /api/sessions/:id/queues` hides
   * that behind `public_view()`, but the `queued_messages` SSE frame serialises
   * the raw struct, so a required field here was a lie that only survived because
   * every consumer happened to write `entry.draft || entry.content`.
   *
   * It is normalised once, at the state boundary (`queuedLists` in `chat.ts`), so
   * everything downstream can read it as a string. Making it a required field on
   * the type is the wrong fix: it would break the moment a frame omitted it, and
   * the *server* is what should change (see the follow-up note in `chat.ts`).
   */
  draft?: string;
  attachments?: QueuedAttachment[];
  accepted_at: number;
}

/** `GET /api/sessions/:id/queues` — both queues, without the transcript. */
export interface QueuesResponse {
  session_id: string;
  running: boolean;
  steers: QueuedMessage[];
  follow_ups: QueuedMessage[];
}

/** `POST /api/sessions/:id/messages?mode=steer|follow_up` → `202`. */
export interface QueuedAccepted {
  mode: QueuedKind;
  session_id: string;
  /** Present for `steer`. */
  steer_id?: string;
  /** Present for `follow_up`. */
  follow_up_id?: string;
  queued: true;
  /** The entry as it was queued, so the panel can draw it immediately. */
  queued_message?: QueuedMessage;
}

/** `DELETE /api/sessions/:id/queues/:id` — the message, handed back. */
export interface RecallResponse {
  session_id: string;
  recalled: QueuedMessage;
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/**
 * Where a rewind cuts and what it hides.
 *
 * `keep_through` is the id of the last *visible* message. `before_prompt` is the
 * id in front of it, used by the retry and note modes, which hide the chosen
 * prompt as well so the replacement turn does not follow it directly.
 */
export interface RewindPoint {
  session_id: string;
  user_message_id: number;
  prompt: string;
  keep_through: number;
  before_prompt: number;
  messages_removed: number;
}

/** One file a rewind will touch. */
export interface RewindFile {
  path: string;
  /** The bytes restored, or `null` when the file is removed instead. */
  restore?: string | null;
  /** True when the rewind deletes the file (the agent created it). */
  removes_file: boolean;
  /** How many journal rows in the region changed this path. */
  changes: number;
  /** Tools that touched it, in the order they ran. */
  tools: string[];
  /** True when a before-image is missing, so the file cannot be fully restored. */
  incomplete: boolean;
}

/** What a rewind would do, computed by the server from the journal it will replay. */
export interface RewindPreview {
  session_id: string;
  message_id: number;
  messages_removed: number;
  files: RewindFile[];
  incomplete: boolean;
  /**
   * Tools the rewound turn called that cannot be undone — `run_command`, a
   * plugin, an MCP call. Their effects stay; the dialog says so rather than
   * implying a clean revert.
   */
  opaque_tools: string[];
}

/** `GET /api/sessions/:id/rewind?message=N`. Mutates nothing. */
export interface RewindPreviewResponse {
  session_id: string;
  point: RewindPoint;
  preview: RewindPreview;
  running: boolean;
}

export type RewindUndoOutcome = 'restored' | 'removed' | 'not_captured' | 'failed';

/** How a rewind relates to the turn it replaces. */
export type RewindMode = 'none' | 'retry' | 'note';

export interface RewindRequest {
  message_id: number;
  /** Undo the file changes made after the message. Off by default. */
  revert?: boolean;
  /**
   * `none` hides the tail and stops, `retry` re-sends the original prompt,
   * `note` sends `note` instead.
   */
  mode?: RewindMode;
  /** The replacement prompt for `mode: 'note'`. */
  note?: string;
  /** Whether the chosen prompt survives. Defaults per mode on the server. */
  keep_prompt?: boolean;
}

/** `POST /api/sessions/:id/rewind`. */
export interface RewindResponse {
  session_id: string;
  point: RewindPoint;
  preview: RewindPreview;
  /** One entry per journal row, newest first. */
  outcomes: RewindUndoOutcome[];
  /** The run started by `retry` or `note`; `null` for `none`. */
  run_id: string | null;
  mode: RewindMode;
  files_restored: number;
  files_skipped: number;
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
  /** Optional for servers predating bounded tree collection. */
  truncated?: boolean;
  /** Maximum entries in this response, separate from the traversal work cap. */
  limit?: number;
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
  /**
   * SHA-256 of the complete on-disk bytes, as `sha256:<64 lowercase hex>`.
   * Optional only for compatibility with older servers.
   */
  digest?: string;
  truncated: boolean;
  binary: boolean;
  language: string | null;
}

/**
 * `200` response of `PUT /api/workspace/file` — the editor's save.
 *
 * The body sent is `{ content, expected_digest?, expected_size? }`; the digest is
 * the precise optimistic-concurrency check and size remains for older servers.
 * `created` says the save made a new file rather than replacing one.
 */
export interface WorkspaceFileWrite {
  path: string;
  /** Bytes written (`content.length` in UTF-8). */
  size: number;
  /** Fresh digest of the written bytes on servers that support digest checks. */
  digest?: string;
  /** True when the file did not exist before this save. */
  created: boolean;
  language: string | null;
}

/**
 * One entry of `GET /api/git/status`.
 *
 * `index`/`worktree` are the two porcelain columns: a file can be staged *and*
 * modified at once (`MM`), which is why both flags are reported instead of a
 * single `staged` bit. `status` keeps the older trimmed form (`M`, `MM`, `??`)
 * for anything that only wants one label.
 */
export interface GitFile {
  path: string;
  /** Source path of a rename/copy, when git reports one. */
  orig_path?: string | null;
  /** Index column: `M`, `A`, `D`, `R`, `C`, `?`, or a space. */
  index: string;
  /** Worktree column, same alphabet. */
  worktree: string;
  status: string;
  /** There is something in the index for this path. */
  staged: boolean;
  /** The working tree differs from the index (or the file is untracked). */
  unstaged?: boolean;
  untracked?: boolean;
  /** An unresolved merge conflict. */
  conflict?: boolean;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  /** A detached HEAD has no branch name. */
  detached?: boolean;
  /** `origin/main` when the branch tracks an upstream. */
  upstream?: string | null;
  ahead?: number;
  behind?: number;
  error: string | null;
  files: GitFile[];
  /** The server cut the listing at its output limit. */
  truncated?: boolean;
}

export interface GitDiff {
  diff: string;
  truncated: boolean;
}

/** One local branch from `GET /api/git/branches`. */
export interface GitBranch {
  name: string;
  current: boolean;
  commit: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  /** Committer date, seconds since the epoch. */
  timestamp: number;
  subject: string;
}

export interface GitBranchList {
  is_repo: boolean;
  branch: string | null;
  detached: boolean;
  branches: GitBranch[];
  truncated: boolean;
}

/** One commit from `GET /api/git/log`. */
export interface GitCommit {
  id: string;
  short_id: string;
  author: string;
  email: string;
  /** Author date, seconds since the epoch. */
  timestamp: number;
  parents: string[];
  /** `HEAD -> main, origin/main` as git decorates it. */
  refs: string;
  subject: string;
}

export interface GitCommitList {
  is_repo: boolean;
  commits: GitCommit[];
  truncated?: boolean;
}

/** One entry of a commit's file list (`--numstat`). */
export interface GitCommitFile {
  path: string;
  /** `null` for a binary file, where git prints `-`. */
  additions: number | null;
  deletions: number | null;
  binary: boolean;
}

export interface GitCommitDetail {
  id: string;
  short_id: string;
  author: string;
  email: string;
  timestamp: number;
  parents: string[];
  refs: string;
  subject: string;
  body: string;
  files: GitCommitFile[];
  diff: string;
  truncated: boolean;
}

export interface GitStash {
  ref: string;
  index: number;
  commit: string;
  timestamp: number;
  subject: string;
}

export interface GitStashList {
  is_repo: boolean;
  stashes: GitStash[];
  truncated?: boolean;
}

/**
 * Every mutating git route answers with the same envelope: the action's own
 * message, a freshly read status (so the panel never shows a stale tree) and,
 * for `POST /api/git/commit`, the new revision.
 */
export interface GitMutationResult {
  ok: boolean;
  message: string | null;
  status: GitStatus;
  commit: { id: string; short_id: string; subject: string } | null;
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
  /** Absent when the plugin failed before its manifest was read. Read as `?.`. */
  manifest?: PluginManifest | null;
  tools: PluginToolSpec[];
  /** Absent when there is nothing wrong. Read as a truthiness test, so safe. */
  error?: string | null;
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
    | {
        type: 'session_rewound';
        session_id: string;
        /** The prompt the rewind normalised to. */
        message_id: number;
        /** How many messages the rewind hid; `0` is a no-op (a retry of the newest prompt). */
        messages_removed: number;
      }
    | { type: 'steer_applied'; steer_id: string; content: string }
    | {
        /**
         * A session's queues changed. Carries both lists in full: the newest frame
         * is the truth, so a missed one cannot leave a stale pending panel.
         */
        type: 'queued_messages';
        session_id: string;
        steers: QueuedMessage[];
        follow_ups: QueuedMessage[];
      }
    | { type: 'run_started'; run_id: string; prompt: string }
    | { type: 'run_finished'; run_id: string; status: RunStatus; error?: string | null }
    | { type: 'run_cancelled'; run_id: string }
    | { type: 'iteration_started'; iteration: number }
    | { type: 'text_delta'; delta: string }
    /** Full final text when server-side normalization changed the streamed deltas. */
    | { type: 'text_replaced'; run_id: string; content: string }
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
        /** True when the spawn detached instead of blocking its caller. */
        background?: boolean;
      }
    | { type: 'agent_progress'; agent_id: string; iterations: number; note: string }
    | { type: 'agent_paused'; agent_id: string; paused: boolean; by: string }
    | {
        /**
         * A message reached (or was queued for) an agent. `to` is an agent id, or
         * `run:<id>` when the recipient is the session's main agent.
         */
        type: 'agent_message';
        from_agent_id: string | null;
        to: string;
        state: string;
      }
    | {
        /** A subagent's own report of what it is doing. Advisory, not measured. */
        type: 'agent_note';
        agent_id: string;
        note: string;
        progress?: number | null;
      }
    | {
        type: 'agent_finished';
        agent_id: string;
        status: AgentStatus;
        summary: string;
        error?: string | null;
      }
    | { type: 'stream_retry'; attempt: number; max_attempts: number; reason: string }
    | { type: 'notice'; level: string; message: string }
    // A human-facing notification: the run settled, something waits for an answer,
    // or a plugin asked for attention. The server sends none when
    // `[notifications] enabled = false`, so the client never has to filter one out.
    | { type: 'notification'; notification: NotifyPayload }
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

/** Why a human is being interrupted. Mirrors `kc_core::notify::NotifyKind`. */
export type NotifyKind = 'run_finished' | 'needs_input' | 'error' | 'info';

/**
 * `config.notifications` from `GET /api/config`.
 *
 * The switches are server-side on purpose: `enabled = false` means the server sends
 * no `notification` event at all, so a client cannot accidentally ignore it. The
 * settings screen edits these through `PATCH /api/config`.
 */
export interface NotificationsConfig {
  enabled: boolean;
  run_finished: boolean;
  needs_input: boolean;
  error: boolean;
  info: boolean;
  /** Whether plugins may raise notifications at all. */
  plugin: boolean;
  /** Skip notifications for runs shorter than this. */
  min_run_secs: number;
  /** Include the first line of the answer as the notification body. */
  say_body: boolean;
  /** The documented ceiling, sent by the server so the UI need not hard-code it. */
  max_min_run_secs: number;
  /** The kinds this server can emit. */
  kinds: NotifyKind[];
}

/** `[notifications]` section of `PATCH /api/config`. */
export interface NotificationsPatch {
  enabled?: boolean;
  run_finished?: boolean;
  needs_input?: boolean;
  error?: boolean;
  info?: boolean;
  plugin?: boolean;
  min_run_secs?: number;
  say_body?: boolean;
}

/**
 * One notification from `GET /api/events` (`type: "notification"`).
 *
 * Everything is plain text, capped by the server, and `source` is either
 * `"runtime"` or the name of the plugin that raised it. A client that does not
 * recognise a `kind` should render it as `info` rather than dropping it: the server
 * may learn kinds before the browser does.
 */
export interface NotifyPayload {
  /** Stable per source moment (`run:<id>`, `approval:<id>`, `plugin:<name>:<title>`). */
  id: string;
  kind: NotifyKind;
  title: string;
  body: string;
  session_id: string;
  /** The session's title at emit time, so the notification is readable on its own. */
  session_title: string;
  /**
   * The project the session belongs to, when the server could read it.
   *
   * Absent on a server that predates the field, and for a notice that names no
   * session at all. This is what lets the rail count unread news per project
   * without knowing which project owns a session it has never listed.
   */
  project_id?: string;
  /** `runtime`, or the plugin's name. */
  source: string;
  /** A cue override; absent means "the kind's default". */
  sound?: string | null;
  /** Keep the desktop notification until it is dismissed. */
  sticky: boolean;
}

export interface Toast {
  id: number;
  tone: 'info' | 'success' | 'error';
  message: string;
  /**
   * A notification about a session, rather than transient feedback. Such a toast
   * offers "Open" instead of only a dismiss button, and is what the unread badge is
   * cleared by.
   */
  sessionId?: string;
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

/**
 * `POST /api/artifacts` — publish one chat artifact's source.
 *
 * `html` is the artifact body with its fence metadata already stripped; a
 * fragment is wrapped into a document server-side. `theme` only picks which way
 * the wrapper's own background/foreground defaults go (`light` when omitted).
 */
export interface ArtifactCreateRequest {
  html: string;
  theme?: 'light' | 'dark';
}

/**
 * `201` response of `POST /api/artifacts`.
 *
 * `id` is the content hash the document is stored under, and `url` is what an
 * `<iframe>` loads. Both are stable for the same source and theme, so a client
 * can post freely without tracking what it has already published.
 */
export interface ArtifactCreateResponse {
  id: string;
  url: string;
  /** Bytes of the wrapped document that will be served. */
  bytes: number;
  theme: 'light' | 'dark';
}

// ---------------------------------------------------------------------------
// Versioning and updates (`GET /api/update`)
// ---------------------------------------------------------------------------

/**
 * Which build this is.
 *
 * The version is the git commit count — build 107 is the 107th commit — because
 * karucode has no release trains: every commit on `main` is a release. Never
 * parse `version` as a float; use `build` to compare.
 */
export interface BuildInfo {
  /** The build number as a string, exactly as `--version` prints it. */
  version: string;
  build: number;
  commit: string;
  commit_date: string;
  /** The binary was built from a tree with uncommitted changes. */
  dirty: boolean;
  /** Release channel label (`stable` unless the build said otherwise). */
  channel: string;
  /** Target triple the binary was built for. */
  target: string;
}

/** How the running binary is installed, and whether this process may replace it. */
export interface UpdateInstallInfo {
  kind: 'managed' | 'source' | 'container';
  path: string;
  /** `false` on a source tree, a container, or a binary owned by another user. */
  writable: boolean;
  /** One sentence saying what an update would do here. */
  detail: string;
}

/** State of update checking itself. */
export interface UpdateCheckInfo {
  enabled: boolean;
  manifest_url: string | null;
  interval_secs: number;
  /** Unix milliseconds of the last attempt, successful or not. */
  checked_at: number | null;
  /** Why the last attempt failed; cleared by a successful one. */
  error: string | null;
}

/** The download for this target that `latest` names. */
export interface UpdateArtifactInfo {
  url: string;
  sha256: string;
  size: number | null;
  tarball: string | null;
}

/** The newest release the last check found. */
export interface UpdateLatest {
  build: number;
  version: string;
  commit: string | null;
  commit_date: string | null;
  released_at: string | null;
  /** Release notes, plain text, capped by the server. */
  notes: string | null;
  /** The human-readable release page. */
  url: string | null;
  artifact: UpdateArtifactInfo | null;
  /** `false` means "a newer build exists, but not for this platform". */
  has_target_for_this_build: boolean;
}

/** A verified download waiting to be installed. */
export interface StagedUpdate {
  build: number;
  version: string;
  commit: string | null;
  path: string;
  sha256: string;
  size: number;
  /** Unix milliseconds. */
  staged_at: number;
}

/** The last build this install put in place. */
export interface AppliedUpdate {
  from: string;
  to: string;
  path: string;
  /** Unix milliseconds. */
  applied_at: number;
}

/** Whether a restart is even possible, and the command that installs by hand. */
export interface UpdateRestartInfo {
  /** A supervisor (systemd) would start the new binary after an exit. */
  supervised: boolean;
  /** e.g. `sudo /usr/local/bin/karucode update --apply --data-dir /var/lib/karucode`. */
  apply_command: string;
}

/** `GET /api/update` — everything the Updates panel draws. */
export interface UpdateStatus {
  current: BuildInfo;
  install: UpdateInstallInfo;
  check: UpdateCheckInfo;
  latest: UpdateLatest | null;
  update_available: boolean;
  phase: 'idle' | 'checking' | 'downloading' | 'applying';
  staged: StagedUpdate | null;
  last_applied: AppliedUpdate | null;
  restart: UpdateRestartInfo;
}

/** What an apply did. */
export interface UpdateApplyResult {
  installed: boolean;
  /** The running process still holds the old binary: restart to run the new one. */
  restart_required: boolean;
  /** The server asked its supervisor to restart it. */
  restart_requested: boolean;
  from: string;
  to: string;
  path: string;
  /** This process could not write the binary; run `command` with privilege. */
  needs_privilege: boolean;
  command: string | null;
  message: string;
}

/** `POST /api/update/apply` */
export interface UpdateApplyResponse {
  update: UpdateStatus;
  result: UpdateApplyResult;
}

/** `[updates]` as `GET /api/config` reports it. */
export interface UpdatesConfig {
  check: boolean;
  /** Clamped by the server to `min_check_secs..=max_check_secs`. */
  check_secs: number;
  manifest_url: string;
  auto_apply: boolean;
  /** The documented bounds, so the form validates with the server's numbers. */
  min_check_secs: number;
  max_check_secs: number;
  default_manifest_url: string;
}

/** `[updates]` section of `PATCH /api/config`. */
export interface UpdatesPatch {
  check?: boolean;
  check_secs?: number;
  manifest_url?: string;
  auto_apply?: boolean;
}
