import { apiRequest } from '../client';
import type { ControlChatSummary, ControlOverview } from '../types/control';
import type { CallOptions } from './common';

// ---------------------------------------------------------------------------
// Control plane
// ---------------------------------------------------------------------------

/**
 * One snapshot of every project: counters, control chats, running and failed
 * sessions, and the live agents behind them.
 *
 * Deliberately one request: the control plane polls this every few seconds
 * (`state/controlPlane.ts`), and a fan-out over projects would make a poll
 * cost grow with the workspace.
 */
export function getControlOverview(options: CallOptions = {}): Promise<ControlOverview> {
  return apiRequest<ControlOverview>('/api/control/overview', options);
}

/** The control chats alone, for a caller that does not need the whole overview. */
export function listControlChats(options: CallOptions = {}): Promise<ControlChatSummary[]> {
  return apiRequest<{ sessions: ControlChatSummary[] }>('/api/control/sessions', options).then(
    (r) => r.sessions,
  );
}
