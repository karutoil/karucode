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
  | 'internal';

export interface ApiErrorBody {
  error: { code: ErrorCode; message: string };
}

export interface Health {
  status: string;
  version: string;
  uptime_secs: number;
  workspace: string;
  auth_required: boolean;
  provider_configured: boolean;
  db: string;
}

export interface PublicModelConfig {
  provider: string;
  base_url: string;
  model: string;
  temperature: number;
  max_tokens: number | null;
  top_p: number | null;
  request_timeout_secs: number;
  parallel_tool_calls: boolean;
  api_key_set: boolean;
}

export interface Limits {
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

export interface ConfigMcpSummary {
  name: string;
  transport: string;
  status: string;
  tools: string[];
  error: string | null;
}

export interface Config {
  workspace: string;
  model: PublicModelConfig;
  limits: Limits;
  approvals: ApprovalPolicy;
  subagents: SubagentConfig;
  plugins: ConfigPluginSummary[];
  mcp_servers: ConfigMcpSummary[];
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
  max_tokens?: number | null;
  top_p?: number | null;
  request_timeout_secs?: number;
  parallel_tool_calls?: boolean;
  /** Write-only. Never rendered back. */
  api_key?: string | null;
  clear_api_key?: boolean;
}

export interface ConfigPatch {
  model?: ModelConfigPatch;
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

export interface Session {
  id: string;
  title: string;
  workspace: string;
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
  created_at: number;
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

export interface WorkspaceFile {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
  binary: boolean;
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
}

/** Known server-sent events. Unknown event types are ignored by the reducer. */
export type KnownEvent = EventEnvelope &
  (
    | { type: 'session_created'; session_id: string; title: string }
    | { type: 'session_renamed'; session_id: string; title: string }
    | { type: 'session_deleted'; session_id: string }
    | { type: 'run_started'; run_id: string; prompt: string }
    | { type: 'run_finished'; run_id: string; status: RunStatus; error?: string | null }
    | { type: 'run_cancelled'; run_id: string }
    | { type: 'iteration_started'; iteration: number }
    | { type: 'text_delta'; delta: string }
    | { type: 'reasoning_delta'; delta: string }
    | { type: 'usage'; input_tokens: number; output_tokens: number }
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
    | { type: 'notice'; level: string; message: string }
    | { type: 'error'; message: string; kind?: string | null }
  );

export type KarucodeEvent = KnownEvent;

/** Parsed stream payload before we know whether the type is supported. */
export interface RawEvent extends EventEnvelope {
  type: string;
  [key: string]: unknown;
}

export type ConnectionState = 'idle' | 'connecting' | 'open' | 'reconnecting' | 'unauthorized';

export interface Toast {
  id: number;
  tone: 'info' | 'success' | 'error';
  message: string;
}