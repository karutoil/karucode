import { ApiError, toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { AgentAction, AgentSummary, SessionDetail } from '../../api/types';
import { isAgentLive } from '../../api/types';
import { preserveIterations } from '../../lib/agentIterations';
import * as agentConsole from '../agentConsole';
import { sessionAgentFromRecord } from '../eventReducer';
import * as store from '../store';
import { enterAuthState, isUnauthorized, rt } from './shared';

/**
 * Seed the workspace list and live transcripts from a session snapshot.
 *
 * The snapshot carries every message, subagent rows included, so opening a
 * session fills in the history of agents that ran before this page loaded
 * without a request per agent.
 */
export function seedAgentState(detail: SessionDetail): void {
  const incoming = detail.agents.map((agent) => ({
    ...sessionAgentFromRecord(agent),
    session_title: detail.session.title,
    workspace: detail.session.workspace,
    project_id: detail.session.project_id ?? null,
  }));
  store.agentsAll.value = agentConsole.mergeAgentList(store.agentsAll.value, incoming);

  if (incoming.length === 0) return;
  const next = { ...store.agentActivities.value };
  let changed = false;
  for (const agent of detail.agents) {
    // The snapshot says when this agent last moved and whether it is still
    // running; without both, a running subagent's seeded feed ranks last and is
    // evicted (then deleted) while it is still working.
    const seeded = agentConsole.activityFromMessages(detail.messages, agent.id, false, {
      updatedAt: agent.updated_at,
      live: isAgentLive(agent.status),
    });
    if (seeded.items.length === 0) continue;
    const live = next[agent.id];
    next[agent.id] = live ? agentConsole.mergeActivities(seeded, live) : seeded;
    changed = true;
  }
  if (changed) store.agentActivities.value = agentConsole.boundAgentActivities(next);
}

export async function refreshSessionAgents(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  const generation = rt.sessionGeneration;
  const projectRead = rt.projectGeneration;
  if (!sessionId) return;
  const current = () =>
    generation === rt.sessionGeneration &&
    projectRead === rt.projectGeneration &&
    store.currentSessionId.value === sessionId;
  try {
    const detail = await api.getSession(sessionId);
    if (current()) {
      // Keep counts this page already watched advance: the snapshot is written
      // between iterations, so it can be older than the live progress frames.
      const agents = preserveIterations(
        store.chat.value.agents,
        detail.agents.map(sessionAgentFromRecord),
      );
      store.chat.value = { ...store.chat.value, agents };
    }
  } catch (error) {
    if (!current()) return;
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
  }
}

/**
 * Reload the workspace-wide agent list.
 *
 * Only the list comes from the server; each agent's live feed keeps streaming in
 * over SSE, so a refresh corrects membership and status without touching the
 * transcript already on screen.
 *
 * A second read of the *same* project while one is in flight is dropped: a
 * project switch and the session refresh that follows it both ask for it, and
 * that response is every agent the project ever spawned - on a project with
 * history, several megabytes. Two identical downloads change nothing.
 */
export async function refreshAgents(): Promise<void> {
  const projectId = store.currentProjectId.value;
  if (rt.agentsReadInFlight === projectId) return;
  const projectRead = rt.projectGeneration;
  const read = ++rt.agentListReadGeneration;
  const current = () =>
    projectId === store.currentProjectId.value &&
    projectRead === rt.projectGeneration &&
    read === rt.agentListReadGeneration;
  rt.agentsReadInFlight = projectId;
  store.agentsLoading.value = true;
  try {
    const list = await api.listAgents({ project: projectId });
    if (!current()) return;
    // The server omits `note`, `progress` and `owned_paths` when empty; normalise
    // here so every row in the store is fully populated. A running agent's row
    // can lag its live progress frame, so a refresh must not lower the count.
    store.agentsAll.value = preserveIterations(
      store.agentsAll.value,
      list.map(sessionAgentFromRecord),
    );
    store.agentsError.value = null;
  } catch (error) {
    if (!current()) return;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.agentsError.value = toErrorMessage(error);
  } finally {
    if (rt.agentsReadInFlight === projectId) rt.agentsReadInFlight = null;
    if (current()) store.agentsLoading.value = false;
  }
}

export function setAgentScope(scope: store.AgentScope): void {
  if (store.agentScope.value === scope) return;
  store.agentScope.value = scope;
  // A status filter chosen in one scope can be empty in the other; reset it so
  // switching scope shows agents instead of "no matches".
  store.agentFilter.value = 'all';
  if (scope === 'workspace') void refreshAgents();
}

export function setAgentFilter(filter: store.AgentFilter): void {
  store.agentFilter.value = filter;
}

/**
 * Pause, resume or cancel one live subagent.
 *
 * The response is the authority on what the agent is now, so the row is patched
 * from it rather than from an optimistic guess: a pause lands at the agent's next
 * iteration, and a cancel unwinds over a moment. A 404 means it finished while
 * the user was reaching for the button, which is not an error to shout about.
 */
export async function controlAgent(id: string, action: AgentAction): Promise<void> {
  if (store.agentControlPending.value[id]) return;
  store.agentControlPending.value = { ...store.agentControlPending.value, [id]: action };
  try {
    const result = await api.controlAgent(id, action);
    const patch = (agent: AgentSummary): AgentSummary =>
      agent.id === id ? { ...agent, status: result.status, updated_at: Date.now() } : agent;
    store.agentsAll.value = store.agentsAll.value.map(patch);
    store.chat.value = {
      ...store.chat.value,
      agents: store.chat.value.agents.map((agent) =>
        agent.id === id ? { ...agent, status: result.status } : agent,
      ),
    };
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    const status = error instanceof ApiError ? error.status : 0;
    if (status === 404) {
      // The agent settled between the click and the request. Refresh so the row
      // shows the real final status instead of a pause that will never land.
      store.pushToast('That agent already finished.');
      if (store.agentScope.value === 'session') void refreshSessionAgents();
      else void refreshAgents();
      return;
    }
    store.pushToast(toErrorMessage(error));
  } finally {
    const { [id]: _done, ...rest } = store.agentControlPending.value;
    store.agentControlPending.value = rest;
  }
}

export function closeAgentDetail(): void {
  rt.agentDetailGeneration += 1;
  store.agentDetailLoading.value = false;
  store.selectedAgentId.value = null;
  store.agentDetailError.value = null;
}

/**
 * Open one agent's transcript.
 *
 * The stored transcript fills in everything from before this page loaded; live
 * frames keep appending on top (merged by stable item id, never duplicated).
 */
export async function openAgentDetail(id: string): Promise<void> {
  const generation = ++rt.agentDetailGeneration;
  store.selectedAgentId.value = id;
  store.agentHistory.value = null;
  store.agentDetailError.value = null;
  store.mobileSheet.value = null;
  store.agentDetailLoading.value = true;
  try {
    const detail = await api.getAgent(id);
    if (generation !== rt.agentDetailGeneration || store.selectedAgentId.value !== id) return;
    store.agentHistory.value = {
      id,
      messages: detail.messages,
      before: detail.messages_before ?? null,
      truncated: detail.messages_truncated,
      loading: false,
      error: null,
    };
    store.agentsAll.value = agentConsole.mergeAgentList(store.agentsAll.value, [detail.agent]);
  } catch (error) {
    if (generation !== rt.agentDetailGeneration || store.selectedAgentId.value !== id) return;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.agentDetailError.value = toErrorMessage(error);
  } finally {
    if (generation === rt.agentDetailGeneration && store.selectedAgentId.value === id) {
      store.agentDetailLoading.value = false;
    }
  }
}

export async function loadOlderAgent(): Promise<void> {
  const history = store.agentHistory.value;
  if (!history || history.loading || history.before === null) return;
  const generation = rt.agentDetailGeneration;
  store.agentHistory.value = { ...history, loading: true, error: null };
  try {
    const detail = await api.getAgent(history.id, { before: history.before });
    if (
      generation !== rt.agentDetailGeneration ||
      store.selectedAgentId.value !== history.id ||
      store.agentHistory.value?.id !== history.id
    ) {
      return;
    }
    const byId = new Map(store.agentHistory.value.messages.map((message) => [message.id, message]));
    for (const message of detail.messages) byId.set(message.id, message);
    store.agentHistory.value = {
      id: history.id,
      messages: [...byId.values()].sort((a, b) => a.id - b.id),
      before: detail.messages_before ?? null,
      truncated: detail.messages_truncated,
      loading: false,
      error: null,
    };
  } catch (error) {
    if (
      generation === rt.agentDetailGeneration &&
      store.selectedAgentId.value === history.id &&
      store.agentHistory.value?.id === history.id
    ) {
      store.agentHistory.value = {
        ...store.agentHistory.value,
        loading: false,
        error: toErrorMessage(error),
      };
    }
  }
}
