import type { StoredMessage } from './messages';

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
