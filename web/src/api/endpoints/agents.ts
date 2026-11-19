import { apiRequest, withQuery } from '../client';
import type { AgentAction, AgentDetail, AgentStatus, AgentSummary } from '../types/agents';
import type { ApprovalDecision, ApprovalRecord } from '../types/approvals';
import type { CallOptions } from './common';

// ---------------------------------------------------------------------------
// Subagents
// ---------------------------------------------------------------------------

/**
 * Every agent the server knows about, newest first.
 *
 * `project` scopes the list to one workspace; `session` and `statuses` narrow
 * it further. This is what lets the Agents view show a subagent running in a
 * session the user is not currently looking at.
 */
export function listAgents(
  params: { project?: string | null; session?: string | null; statuses?: AgentStatus[] } = {},
  options: CallOptions = {},
): Promise<AgentSummary[]> {
  return apiRequest<{ agents: AgentSummary[] }>(
    withQuery('/api/agents', {
      project: params.project ?? undefined,
      session: params.session ?? undefined,
      status: params.statuses && params.statuses.length > 0 ? params.statuses.join(',') : undefined,
    }),
    options,
  ).then((r) => r.agents);
}

/** One agent plus the transcript it wrote (its own messages only). */
export function getAgent(
  id: string,
  params: { messages?: number; before?: number } = {},
  options: CallOptions = {},
): Promise<AgentDetail> {
  return apiRequest<AgentDetail>(
    withQuery(`/api/agents/${encodeURIComponent(id)}`, { messages: params.messages, before: params.before }),
    options,
  );
}

/**
 * Pause, resume or cancel one live subagent.
 *
 * Only meaningful while the agent's loop exists: an id that already finished
 * answers 404, which the caller shows as "it finished" rather than an error the
 * user has to act on.
 */
export function controlAgent(
  id: string,
  action: AgentAction,
  options: CallOptions = {},
): Promise<{ agent_id: string; action: AgentAction; status: AgentStatus }> {
  return apiRequest(
    `/api/agents/${encodeURIComponent(id)}/${action}`,
    { method: 'POST', ...options },
  );
}

export function listApprovals(sessionId: string, options: CallOptions = {}): Promise<ApprovalRecord[]> {
  return apiRequest<{ approvals: ApprovalRecord[] }>(
    withQuery('/api/approvals', { session: sessionId }),
    options,
  ).then((r) => r.approvals);
}

export function resolveApproval(
  approvalId: string,
  decision: Exclude<ApprovalDecision, 'expired'>,
  options: CallOptions = {},
): Promise<{ resolved: boolean; decision: ApprovalDecision }> {
  return apiRequest<{ resolved: boolean; decision: ApprovalDecision }>(
    `/api/approvals/${encodeURIComponent(approvalId)}`,
    { ...options, method: 'POST', body: { decision } },
  );
}
