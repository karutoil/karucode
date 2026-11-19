import type { AgentStatus } from './agents';
import type { ApprovalDecision, ApprovalRequest } from './approvals';
import type { JsonValue } from './common';
import type { ImageAttachment, QueuedMessage } from './messages';
import type { NotifyPayload } from './notifications';
import type { RunStatus, SessionModelSelection } from './sessions';

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
         * A slash command answered. `message_id` is the transcript row the result
         * was written to, so a client that already has the row can replace it
         * instead of appending a second copy.
         */
        type: 'command_result';
        command: string;
        message_id: number;
        content: string;
        state: 'ok' | 'error' | string;
      }
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
