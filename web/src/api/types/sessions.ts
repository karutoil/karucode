import type { AgentRecord, AgentStatus, UsageTotals } from './agents';
import type { ApprovalRecord } from './approvals';
import type { QueuedMessage, StoredMessage } from './messages';

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
  /**
   * Per-session override for subagent git worktrees: `true` isolates this
   * session's children, `false` keeps them in the shared tree, and absent (which
   * the server skips entirely) follows `config.subagents.worktrees`.
   *
   * `false` is an override, not "unset" - a session can opt out of an install
   * that turned isolation on.
   */
  worktrees?: boolean | null;
  /** Project the session runs in (null for a session created without one). */
  project_id: string | null;
  /**
   * True for a temporary session: never listed anywhere in the UI, and deleted
   * by the server once its work is over. Sent as `false` for ordinary sessions
   * and absent from payloads written before the field existed.
   */
  ephemeral?: boolean;
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
 * How a plugin asked the rail to draw one session (§R4 of the extensibility ADR).
 *
 * Display only: the server merges these by id from `before_session_list` answers
 * and drops ids it did not list, so an annotation can never hide or invent a row.
 * A lower `group` sorts first and `sort_key` breaks ties within it — which is how
 * a workflow plugin keeps open work above finished work.
 */
export interface SessionAnnotation {
  id: string;
  group?: number;
  sort_key?: number;
  badges?: { text: string; tone: string }[];
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
