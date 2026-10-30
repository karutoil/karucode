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

export function agentStatusTone(status: AgentStatus | string): 'ok' | 'warn' | 'danger' | 'muted' | 'running' {
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