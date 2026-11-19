import type { AgentSummary } from './agents';
import type { SessionSummary } from './sessions';

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
