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
  return `${state.usage.input_tokens.toLocaleString()} in · ${state.usage.output_tokens.toLocaleString()} out`;
}