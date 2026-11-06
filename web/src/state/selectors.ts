import type { AgentStatus, RunStatus, SessionDetail, SessionSummary } from '../api/types';
import type { ApprovalView, ChatState, CommandItem, MessageItem, ToolItem } from './chat';

export function pendingApprovals(state: ChatState): ApprovalView[] {
  return state.items
    .filter((item) => item.kind === 'approval' && item.approval.status === 'pending')
    .map((item) => (item as { approval: ApprovalView }).approval);
}

export type ActivityKind = 'idle' | 'thinking' | 'tool' | 'streaming' | 'approval' | 'error' | 'interrupted' | 'cancelled';

export interface Activity {
  kind: ActivityKind;
  label: string;
}

/** The composer's status line: names what the agent is doing right now. */
export function agentActivity(state: ChatState): Activity {
  const pending = pendingApprovals(state);
  if (pending.length > 0) {
    return {
      kind: 'approval',
      label: pending.length === 1 ? 'Waiting for approval' : `Waiting for ${pending.length} approvals`,
    };
  }

  if (state.running) {
    const active = [...state.items]
      .reverse()
      .find(
        (item): item is ToolItem | CommandItem =>
          (item.kind === 'tool' || item.kind === 'command') && item.status === 'running',
      );
    if (active) {
      return {
        kind: 'tool',
        label: active.kind === 'command' ? 'Running command' : `Running ${active.name}`,
      };
    }
    const streaming = [...state.items]
      .reverse()
      .find(
        (item): item is MessageItem =>
          item.kind === 'message' && item.role === 'assistant' && item.streaming && item.content !== '',
      );
    if (streaming) return { kind: 'streaming', label: 'Streaming' };
    return { kind: 'thinking', label: 'Thinking' };
  }

  if (state.runStatus === 'interrupted') return { kind: 'interrupted', label: 'Interrupted' };
  if (state.runStatus === 'cancelled') return { kind: 'cancelled', label: 'Stopped' };
  if (state.runStatus === 'failed' || state.runError) return { kind: 'error', label: 'Run failed' };
  return { kind: 'idle', label: 'Ready' };
}

/**
 * A run status as the word the UI says: `limit_reached` reads `Limit reached`,
 * and an absent or unknown status reads `Idle` rather than nothing.
 *
 * The live announcer, the context drawer and {@link sessionStatusView} all read
 * this, so a status is spelled one way everywhere it appears.
 */
export function runStatusLabel(status: RunStatus | null): string {
  switch (status) {
    case 'running':
      return 'Running';
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
    case 'interrupted':
      return 'Interrupted';
    case 'limit_reached':
      return 'Limit reached';
    default:
      return 'Idle';
  }
}

// ---------------------------------------------------------------------------
// Status vocabulary
// ---------------------------------------------------------------------------
//
// One definition per status word and per status colour, shared by the agent
// roster, the session rails and the control plane. This file owns them rather
// than `lib/format.ts` because it already owns the rest of the vocabulary
// (`runStatusLabel`, `toolStatusLabel`, `agentStatusTone`) and already imports
// the wire types the words are keyed on; `lib/format.ts` formats values
// (durations, bytes, counts, times) and imports nothing at all. A status is not
// a value format - it is a fact about a row - so the mapping lives with the
// other status facts.

/**
 * The tone names the shared stylesheet paints: `status-dot status-<tone>`,
 * `session-state-<tone>`, `badge-<tone>`, `online-ind status-<tone>`.
 *
 * `running` is a tone of its own rather than a flavour of `ok`, because a live
 * row is animated in CSS where a settled one is not.
 */
export type StatusTone = 'ok' | 'warn' | 'danger' | 'muted' | 'running';

/**
 * Every agent status as the one word the UI says about it: `completed` reads
 * as `done` everywhere, never `Completed` in one list and `finished` in another.
 *
 * The live label is *not* here — an agent that is still running says what it is
 * running (`running edit_file`, `waiting for approval`, `thinking`), which
 * `AgentsTab.tsx`'s `liveLabel` derives from the agent's own feed. This record is
 * the settled vocabulary only, so it cannot go stale as the live label evolves.
 * Read it through {@link agentStatusLabel} unless you need to iterate it.
 */
export const AGENT_STATUS_LABEL: Readonly<Record<AgentStatus, string>> = {
  running: 'running',
  paused: 'paused',
  completed: 'done',
  failed: 'failed',
  cancelled: 'cancelled',
  interrupted: 'interrupted',
};

/**
 * The word for one agent status, tolerant of a status this build has never seen.
 *
 * The server can be newer than the bundle: an unknown status is echoed back as
 * it arrived rather than dropped or rendered `undefined`, which is the same
 * contract {@link agentStatusTone} keeps by returning `muted`.
 */
export function agentStatusLabel(status: AgentStatus | string): string {
  return Object.hasOwn(AGENT_STATUS_LABEL, status)
    ? AGENT_STATUS_LABEL[status as AgentStatus]
    : String(status);
}

export function toolStatusLabel(status: ToolItem['status'] | CommandItem['status']): string {
  switch (status) {
    case 'running':
      return 'Running';
    case 'ok':
      return 'Done';
    case 'error':
      return 'Failed';
    case 'stopped':
      return 'Stopped';
    default:
      return '';
  }
}

/**
 * The tone a status is painted with: agents, runs and sessions share the scale,
 * because the stylesheet's `status-<tone>` classes are shared too.
 *
 * An unknown status is `muted`, not a guess: a bundle older than the server
 * draws a neutral grey rather than a confident colour.
 */
export function agentStatusTone(status: AgentStatus | string): StatusTone {
  switch (status) {
    case 'running':
      return 'running';
    case 'completed':
      return 'ok';
    case 'failed':
      return 'danger';
    case 'cancelled':
    case 'interrupted':
      return 'warn';
    default:
      return 'muted';
  }
}

/** What one session list row says about its run: a word plus the tone that paints it. */
export interface SessionStatusView {
  /**
   * The word to show — and to hand a screen reader — or `null` when the row has
   * no state worth shouting about (a completed, cancelled or never-run session).
   */
  label: string | null;
  /**
   * The tone to paint, or `null` for no indicator at all. It is always non-null
   * exactly when `label` is: a word with no colour and a colour with no word are
   * both half an indicator.
   */
  tone: StatusTone | null;
}

/**
 * The one state a session list shouts about, from the two fields every list row
 * carries (`running`, `last_run_status`).
 *
 * Shared by the workbench's session rail and the control plane's session lists,
 * so a failed run cannot read `Failed` in one and `bad` in the other. Only three
 * states earn an indicator at all — a run in flight, a failure and an
 * interruption — because a list where every row wears a badge is a list where no
 * badge means anything. Everything else is `{ label: null, tone: null }`.
 *
 * A run in flight wins over the last settled run: `last_run_status` still names
 * the *previous* run until the server rewrites it, so a session that is running
 * now must not be announced by what its last run did.
 *
 * Both halves are derived, not copied: the word comes from {@link runStatusLabel}
 * (the same function the live announcer and the context drawer read) and the
 * colour from {@link agentStatusTone}, so this helper cannot drift from either.
 *
 * A view that always has a word — a facts panel, a badge, an `<option>` —
 * composes the two directly instead: `runStatusLabel(session.last_run_status ?? null)`
 * (which says `Idle` when the session has never run) plus
 * `agentStatusTone(session.last_run_status ?? '')`.
 *
 * The tone is only ever `running`, `danger` or `warn`, which are exactly the
 * three the shared stylesheet paints (`.status-dot.status-<tone>` in base.css,
 * `.session-row-quiet .session-state-<tone>` in workbench-navigation.css). A row
 * adopting this helper therefore needs no new CSS.
 */
export function sessionStatusView(
  session: Pick<SessionSummary, 'running' | 'last_run_status'>,
): SessionStatusView {
  // `running` is the server's word for "a run is in flight right now", so it
  // wins: `last_run_status` still names the *previous* run until the server
  // rewrites it, and a live session must not be announced by a settled one.
  if (session.running) {
    return { label: runStatusLabel('running'), tone: agentStatusTone('running') };
  }
  // `last_run_status` is optional on the wire (`skip_serializing_if`), so an
  // absent key and an explicit `null` are the same "never ran". A stored
  // `running` here is a stale row (a crash mid-run) rather than a live one — the
  // `running` flag above is the authority — so it stays quiet instead of
  // painting a live dot on a session that is not running.
  const status = session.last_run_status ?? null;
  if (status === 'failed' || status === 'interrupted') {
    return { label: runStatusLabel(status), tone: agentStatusTone(status) };
  }
  return { label: null, tone: null };
}

export function summaryFromDetail(detail: SessionDetail): SessionSummary {
  return {
    ...detail.session,
    message_count: detail.messages.length,
    last_run_status: detail.runs.length
      ? [...detail.runs].sort((a, b) => a.started_at - b.started_at)[detail.runs.length - 1]?.status ?? null
      : null,
    running: detail.running,
  };
}

export function lastRunState(state: ChatState): RunStatus | null {
  return state.runStatus;
}

export function totalUsage(state: ChatState): string {
  if (!state.usage) return '';
  const base = `${state.usage.input_tokens.toLocaleString()} in · ${state.usage.output_tokens.toLocaleString()} out`;
  // The prompt cache is the difference between a cheap long run and an
  // expensive one, so the hit rate is part of the HUD, not a hidden metric.
  const cached = state.usage.cached_tokens;
  if (cached === undefined || state.usage.input_tokens === 0) return base;
  const percent = Math.round((cached / state.usage.input_tokens) * 100);
  return `${base} · ${percent}% cached`;
}

// ---------------------------------------------------------------------------
// Context window usage
// ---------------------------------------------------------------------------

/** Filled-in usage for the composer's context pill. */
export interface ContextUsage {
  /** The window the percentage is against (the model's `context_window`). */
  window: number;
  /** Prompt tokens the provider actually billed for the newest call. */
  tokens: number;
  /** `tokens / window`, clamped to 0..1. */
  fraction: number;
}

/**
 * How full the context window is.
 *
 * `usage.input_tokens` is *per model call*, and every `usage` event in this state
 * is summed for the session HUD — so the total is a running cost, not a window
 * fill. The window fill is the **last** call's prompt size, which is why the
 * newest call's count is kept separately in `lastPromptTokens`.
 *
 * The number is only ever the provider's own: `input_tokens` from the newest
 * `usage` event. A local estimate from the transcript would be wrong in exactly
 * the case that matters — compaction keeps the *sent* window far below the
 * stored one, so counting the transcript inflates the fill and would show "100%"
 * on a session the server has already compacted back to 15%.
 *
 * That means the pill is empty until the session's first model call finishes, and
 * that is deliberate: better no number than a fabricated one.
 *
 * `window` comes from the active model entry (the session pin wins over the
 * install default); an unknown window yields `null` rather than 0%.
 */
export function contextUsage(state: ChatState, window: number | null): ContextUsage | null {
  if (window === null || !Number.isFinite(window) || window <= 0) return null;
  const tokens = state.lastPromptTokens ?? 0;
  if (tokens <= 0) return null;
  return {
    window,
    tokens,
    fraction: Math.min(1, Math.max(0, tokens / window)),
  };
}

/** `57.4%` — one decimal above 10%, whole numbers below it, where it is noise. */
export function formatContextPercent(fraction: number): string {
  if (!Number.isFinite(fraction) || fraction <= 0) return '0%';
  const percent = Math.min(100, fraction * 100);
  return percent >= 10 ? `${percent.toFixed(1)}%` : `${Math.round(percent)}%`;
}

/** Fill bands for the pill's colour: the point where compaction starts to bite. */
export type ContextTone = 'calm' | 'warn' | 'hot';

export function contextTone(fraction: number): ContextTone {
  if (fraction >= 0.9) return 'hot';
  if (fraction >= 0.7) return 'warn';
  return 'calm';
}