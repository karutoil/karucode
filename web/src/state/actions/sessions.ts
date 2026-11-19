import { ApiError, toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type {
  KarucodeEvent,
  SessionDetail,
  SessionModelSelection,
  SessionSummary,
} from '../../api/types';
import {
  emptyChatState,
  mergeTranscriptRows,
  seedChatState,
  withStoredMessageIds,
  type ChatState,
  type MessageItem,
  type TranscriptRows,
} from '../chat';
import { applyEventToChat } from '../eventReducer';
import * as promptTokens from '../promptTokens';
import { summaryFromDetail } from '../selectors';
import * as store from '../store';
import { refreshAgents, seedAgentState } from './control';
import {
  clearSelection,
  enterAuthState,
  isUnauthorized,
  mergeSummary,
  projectSessionsOnly,
  resetPluginPanelData,
  rt,
} from './shared';

export interface OpenSessionOptions {
  silent?: boolean;
  /** Refetch even when this session is already selected (used by Retry). */
  force?: boolean;
  /**
   * Keep a transcript that is still streaming instead of reseeding it. Set by the
   * stream-recovery paths: `GET /api/sessions/:id` only returns persisted
   * messages, so replacing the view mid-answer would delete the text arriving now.
   */
  preserveLive?: boolean;
  /** Internal owner shared across an await in `openSessionInProject`. */
  navigationOwner?: number;
}

interface SessionModelMutation {
  sessionId: string;
  generation: number;
  sessionRead: number;
  projectRead: number;
}

/**
 * Close the open session: no session, no transcript, no remembered pin.
 *
 * Exported because the control plane deletes sessions it did not create through
 * `deleteSessionById` (that one opens the next session in the workspace rail,
 * which is not what a plane that shows its own list wants). A bare
 * `currentSessionId = null` would leave the transcript, the pin record and the
 * remembered last session behind.
 */
export function closeSession(): void {
  // An explicit close is a later navigation too: a pending create must not reopen
  // the session it was about to select after the reader deliberately closed it.
  rt.navigationGeneration += 1;
  rt.sessionGeneration += 1;
  clearSelection();
}

/**
 * Is an answer being streamed into the open session right now?
 *
 * Only a live message counts. A pending approval or a running tool has nothing to
 * lose from a reseed - and those are exactly the states where a refetch has to be
 * allowed through, because the lost events may be the approval card itself.
 */
function hasLiveAnswer(sessionId: string): boolean {
  const chat = store.chat.value;
  if (chat.sessionId !== sessionId) return false;
  return chat.items.some(
    (item) =>
      item.kind === 'message' && item.streaming && (item.content !== '' || item.reasoning !== ''),
  );
}

/**
 * Splice the answer that is still streaming back into a freshly seeded
 * transcript.
 *
 * `seedChatState` rebuilds everything the server knows - persisted messages,
 * tool cards, pending approvals, notices - but it cannot know the text arriving
 * right now, because an assistant turn is only persisted once it completes.
 * Reseeding alone would delete that text; skipping the reseed (what this used to
 * do) loses every card whose frame was one of the dropped ones. Merging keeps
 * both: the snapshot supplies the cards, the live state supplies the prose.
 */
function mergeLiveAnswer(candidate: ChatState, live: ChatState): ChatState {
  const streaming = live.items.filter(
    (item): item is MessageItem =>
      item.kind === 'message' && item.streaming && (item.content !== '' || item.reasoning !== ''),
  );
  if (streaming.length === 0) return candidate;

  // The seed adds an empty placeholder for the in-flight run; the real message
  // replaces it, so drop every streaming item before appending the live ones.
  const items = candidate.items.filter((item) => !(item.kind === 'message' && item.streaming));

  // A subagent that spawned before the hole may not be in the snapshot yet.
  const known = new Set(candidate.agents.map((agent) => agent.id));
  const agents = [...candidate.agents, ...live.agents.filter((agent) => !known.has(agent.id))];

  return {
    ...candidate,
    items: [...items, ...streaming],
    agents,
    running: true,
    runStatus: 'running',
  };
}

export function applyChatEvent(event: KarucodeEvent): void {
  const next = applyEventToChat(store.chat.value, event);
  // The context pill's number is the provider's own prompt count for the newest
  // call. Persisting it is what lets the pill survive a reload: `usage` is
  // live-only state and the session snapshot never carries it.
  if (
    event.type === 'usage' &&
    !event.agent_id &&
    next.lastPromptTokens !== store.chat.value.lastPromptTokens &&
    next.lastPromptTokens !== null
  ) {
    promptTokens.rememberPromptTokens(next.sessionId, next.lastPromptTokens);
  }
  // The Session tab's numbers are server-side totals, not something this state
  // can derive: a `usage` frame is one call's tokens and the tab shows the sum
  // of every call. Neither event arrives often enough to need a poll, and the
  // schedule below collapses a burst into one read.
  if (event.type === 'usage' || event.type === 'run_finished' || event.type === 'agent_finished') {
    scheduleSessionInfoRefresh();
  }
  store.chat.value = next;
}

function drainRefreshQueue(settled = false): void {
  const queued = rt.refreshQueue;
  rt.refreshQueue = [];
  rt.refreshingSessionId = null;
  // With no run in flight the snapshot contains every completed turn, so
  // replaying a text delta for one of those runs would append the same words to a
  // message that is already persisted. Deltas belonging to a run the snapshot does
  // not know (it started during the fetch) are still replayed.
  const persisted = new Set<string>();
  if (settled) {
    for (const item of store.chat.value.items) {
      if (item.kind === 'message' && item.role === 'assistant' && !item.streaming && item.runId) {
        persisted.add(item.runId);
      }
    }
  }
  for (const event of queued) {
    const isDelta = event.type === 'text_delta' || event.type === 'reasoning_delta';
    if (settled && isDelta && event.run_id && persisted.has(event.run_id)) continue;
    applyChatEvent(event);
  }
}

/**
 * Did the session list gain, lose or rename a row?
 *
 * Order and the counters inside a row - message count, running, last status -
 * change on every run boundary and are already carried by the event stream.
 * Only ids and titles decide whether the *agents* view, which is grouped and
 * labelled by session, has to be re-read from the server.
 */
function sessionsChanged(
  before: readonly SessionSummary[],
  after: readonly SessionSummary[],
): boolean {
  if (before.length !== after.length) return true;
  const titles = new Map(before.map((session) => [session.id, session.title]));
  return after.some((session) => titles.get(session.id) !== session.title);
}

async function doRefreshSessions(): Promise<void> {
  const projectId = store.currentProjectId.value;
  const projectRead = rt.projectGeneration;
  const read = ++rt.sessionListReadGeneration;
  const isCurrent = () =>
    projectId === store.currentProjectId.value &&
    projectRead === rt.projectGeneration &&
    read === rt.sessionListReadGeneration;
  try {
    const { sessions: list, annotations } = await api.listSessionsWithAnnotations(projectId);
    if (!isCurrent()) return;
    // Even an unscoped read (no open project) must not put a control chat or
    // another project's session into the rail: see `projectSessionsOnly`.
    const rows = projectSessionsOnly(list);
    // Display only, and read straight through: the server already merged and
    // validated these by id, and nothing here may filter the session list itself.
    store.sessionAnnotations.value = annotations;
    const membershipChanged = sessionsChanged(store.sessions.value, rows);
    store.sessions.value = rows;
    store.sessionsLoading.value = false;
    // Agents belong to sessions: a deleted or renamed session changes both the
    // list and the labels the Agents view groups by. Nothing else about a run
    // boundary does - status and notes arrive on the event stream - and this read
    // is the whole project's agent list, which on a busy project is a multi-
    // megabyte response. Re-reading it after every `run_started`/`run_finished`
    // was a large, repeated transfer for data nothing could have changed. A
    // project switch re-reads it explicitly, because that list is project-scoped.
    if (membershipChanged) void refreshAgents();
    const current = store.currentSessionId.value;
    if (current && !rows.some((session) => session.id === current)) {
      // A project-less session (the control chat) is never in a project's list, so
      // refreshing that list must not close it. Ask the global list before
      // treating the open session as gone.
      let projectless = false;
      try {
        const all = await api.listSessions();
        projectless = all.some((session) => session.id === current && !session.project_id);
      } catch {
        // Offline: keep the current session rather than closing it on a guess.
        projectless = true;
      }
      if (!isCurrent()) return;
      if (projectless) return;
      const next = rows[0] ?? null;
      if (next) void openSession(next.id);
      else clearSelection();
    }
  } catch (error) {
    if (!isCurrent()) return;
    store.sessionsLoading.value = false;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.connection.value = {
      ...store.connection.value,
      api: 'offline',
      lastError: toErrorMessage(error),
    };
  }
}

export function refreshSessions(): Promise<void> {
  return doRefreshSessions();
}

export function scheduleSessionsRefresh(): void {
  if (typeof window === 'undefined') return;
  if (rt.sessionsRefreshTimer !== null) window.clearTimeout(rt.sessionsRefreshTimer);
  rt.sessionsRefreshTimer = window.setTimeout(() => {
    rt.sessionsRefreshTimer = null;
    void doRefreshSessions();
  }, 450);
}

/**
 * Move the workspace onto the project an opened session belongs to.
 *
 * `openSession` is also reached without a project switch: a notification record
 * that has lost its project, or an action that only holds a session id. The rail,
 * the file tree and the terminals are project-scoped, so a session from another
 * project must bring its project with it - otherwise the workspace shows one
 * project's files beside another project's conversation, and the session has no
 * row in the rail. Unlike `switchProject` this never selects a session (one is
 * already open) and does not close anything: it moves the project, empties the
 * list and reloads it, and the caller merges the open session's own row into it.
 */
function adoptProjectForSession(projectId: string): void {
  if (store.currentProjectId.value === projectId) return;
  // A project move invalidates every in-flight read of the old project, exactly
  // as `switchProjectInternal` does. `sessionGeneration` is deliberately left
  // alone: this runs inside a session read that is still committing.
  rt.projectGeneration += 1;
  store.currentProjectId.value = projectId;
  store.writeLastProjectId(projectId);
  resetPluginPanelData();
  store.sessions.value = [];
  store.sessionsLoading.value = true;
  store.sessionInfo.value = null;
  store.sessionInfoFor.value = null;
  store.sessionInfoError.value = null;
  void doRefreshSessions();
}

export async function openSession(id: string, options: OpenSessionOptions = {}): Promise<boolean> {
  const silent = options.silent ?? false;
  if (id === store.currentSessionId.value && !silent && !options.force) return false;
  if (options.navigationOwner === undefined) rt.navigationGeneration += 1;
  else if (options.navigationOwner !== rt.navigationGeneration) return false;

  const generation = (rt.sessionGeneration += 1);
  const sessionChanged = store.currentSessionId.value !== id;
  // Opening a session is a navigation intent too: an unsent draft is abandoned
  // rather than kept in the background, so "New session" never haunts a session
  // the reader went back to.
  store.draftSession.value = null;
  // Pages the reader already paged back to belong to this same session; a later
  // silent refresh merges its newest tail into them instead of throwing them
  // away (see `mergeTranscriptRows`). A forced read is the exception: it is how
  // rewind and recovery ask for exactly what the server has now.
  const previousHistory = store.sessionHistory.value?.id === id ? store.sessionHistory.value : null;
  if (store.sessionHistory.value?.id !== id) store.sessionHistory.value = null;
  else if (store.sessionHistory.value.loading) {
    store.sessionHistory.value = { ...store.sessionHistory.value, loading: false };
  }
  store.currentSessionId.value = id;
  if (sessionChanged) resetPluginPanelData();
  // The pills must never show the previous session's pin while this one is
  // being read, but they must not vanish either: an unknown pin is not the
  // same as no pin, so `undefined` is recorded until the read answers.
  if (sessionChanged) store.rememberSessionModel(id, undefined);
  store.writeLastSessionId(id);
  // Opening a session is what "reads" its notifications: the badge clears here
  // rather than on a timer, so it reflects what you have actually looked at.
  store.clearUnread(id);
  store.chatError.value = null;
  if (!silent) {
    store.chatLoading.value = true;
    store.chat.value = emptyChatState(id);
    // The previous session's numbers must not be shown under this one's title
    // while the read is in flight.
    store.sessionInfo.value = null;
    store.sessionInfoFor.value = null;
    store.sessionInfoError.value = null;
  }

  // Only a *different* session starts a fresh queue. A second read of the same
  // session - the resync path does exactly this while the first read is still in
  // flight - must not throw away the frames the queue is holding: they are the
  // live answer, and the silent snapshot it is about to apply cannot contain a
  // turn the server has not persisted yet. Whichever read settles first drains
  // them, and the `generation` guard still discards a stale snapshot.
  if (rt.refreshingSessionId !== id) rt.refreshQueue = [];
  rt.refreshingSessionId = id;

  let applied = false;
  // Whether the snapshot closes the book on every turn it contains.
  let settled = false;

  try {
    const detail = await api.getSession(id);
    if (generation !== rt.sessionGeneration) return false;
    const rows: TranscriptRows =
      options.force || !previousHistory
        ? {
            messages: detail.messages,
            before: detail.messages_before ?? null,
            truncated: !!detail.messages_truncated,
          }
        : mergeTranscriptRows(previousHistory, detail);
    store.sessionHistory.value = {
      id,
      messages: rows.messages,
      before: rows.before,
      truncated: rows.truncated,
      loading: false,
      error: null,
    };
    // The snapshot's runs, approvals and agents are still the server's; only the
    // rows are widened to what this tab already had, so seeding from it keeps
    // every loaded page and still persists the new tail.
    const seededDetail: SessionDetail =
      rows.messages === detail.messages
        ? detail
        : {
            ...detail,
            messages: rows.messages,
            messages_truncated: rows.truncated,
            messages_before: rows.before,
          };
    if (options.preserveLive && detail.running && hasLiveAnswer(id)) {
      // Never delete an answer that is still streaming - but do not throw the
      // snapshot away either. It is the only thing that can restore a tool card,
      // an approval or a notice whose frame was one of the lost ones.
      store.chat.value = mergeLiveAnswer(
        seedChatState(seededDetail, store.chat.value),
        store.chat.value,
      );
    } else {
      store.chat.value = seedChatState(seededDetail, store.chat.value);
      settled = !detail.running;
    }
    // The snapshot cannot carry `usage` (it is not persisted), so the context
    // pill would be blank until the next call finished. Restore the last number
    // this browser saw for the session; `seedChatState` already kept a live
    // in-flight value, so only fill a hole.
    if (store.chat.value.lastPromptTokens === null) {
      const remembered = promptTokens.recallPromptTokens(id);
      if (remembered !== null) {
        store.chat.value = { ...store.chat.value, lastPromptTokens: remembered };
      }
    }
    seedAgentState(detail);
    const summary = summaryFromDetail(detail);
    // The workspace follows the conversation: a session opened from a
    // notification record (or any caller holding only an id) may live in another
    // project, and the rail, file tree and terminals are project-scoped. Move
    // first, then merge, so the row lands in the list it belongs to.
    if (summary.project_id) adoptProjectForSession(summary.project_id);
    // The session's own model pin is recorded by `mergeSummary` below, which
    // is the one place that sees the server's answer for a session.
    mergeSummary(summary);
    store.chatLoading.value = false;
    store.chatError.value = null;
    applied = true;
  } catch (error) {
    if (generation !== rt.sessionGeneration) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
      store.chatLoading.value = false;
      return false;
    }
    if (error instanceof ApiError && error.status === 404) {
      store.sessions.value = store.sessions.value.filter((session) => session.id !== id);
      store.chatLoading.value = false;
      store.writeLastSessionId(null);
      const next = store.sessions.value[0] ?? null;
      store.pushToast('That session no longer exists.', 'info');
      if (next) await openSession(next.id);
      else clearSelection();
      return false;
    }
    store.chatLoading.value = false;
    if (!silent) {
      store.chat.value = emptyChatState(id);
      store.chatError.value = toErrorMessage(error);
      store.pushToast(toErrorMessage(error));
    }
  } finally {
    if (generation === rt.sessionGeneration) {
      drainRefreshQueue(settled);
    }
  }
  return applied;
}

export async function renameSessionById(id: string, title: string): Promise<boolean> {
  const trimmed = title.trim();
  if (trimmed === '') return false;
  const original = store.sessions.value.find((session) => session.id === id);
  store.sessions.value = store.sessions.value.map((session) =>
    session.id === id ? { ...session, title: trimmed, updated_at: Date.now() } : session,
  );
  try {
    const updated = await api.renameSession(id, trimmed);
    store.sessions.value = store.sessions.value.map((session) =>
      session.id === id ? { ...session, ...updated } : session,
    );
    return true;
  } catch (error) {
    if (original) {
      store.sessions.value = store.sessions.value.map((session) =>
        session.id === id ? original : session,
      );
    }
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return false;
  }
}

export async function deleteSessionById(id: string): Promise<boolean> {
  try {
    await api.deleteSession(id);
    promptTokens.forgetPromptTokens(id);
    store.forgetSessionNotifications(id);
    store.sessions.value = store.sessions.value.filter((session) => session.id !== id);
    if (store.currentSessionId.value === id) {
      const next = store.sessions.value[0] ?? null;
      if (next) await openSession(next.id);
      else clearSelection();
    }
    store.pushToast('Session deleted.', 'info');
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return false;
  }
}

function beginSessionModelMutation(sessionId: string): SessionModelMutation {
  const mutation = {
    sessionId,
    generation: ++rt.modelMutationGeneration,
    sessionRead: rt.sessionGeneration,
    projectRead: rt.projectGeneration,
  };
  rt.modelMutationGenerations.set(sessionId, mutation.generation);
  rt.modelMutationOwner = { sessionId, generation: mutation.generation };
  store.sessionModelMutationPending.value = sessionId;
  return mutation;
}

function ownsVisibleSessionModelMutation(mutation: SessionModelMutation): boolean {
  return (
    rt.modelMutationGenerations.get(mutation.sessionId) === mutation.generation &&
    mutation.sessionRead === rt.sessionGeneration &&
    mutation.projectRead === rt.projectGeneration &&
    store.currentSessionId.value === mutation.sessionId
  );
}

function finishSessionModelMutation(mutation: SessionModelMutation): void {
  if (rt.modelMutationGenerations.get(mutation.sessionId) === mutation.generation) {
    rt.modelMutationGenerations.delete(mutation.sessionId);
  }
  if (
    rt.modelMutationOwner?.sessionId === mutation.sessionId &&
    rt.modelMutationOwner.generation === mutation.generation
  ) {
    rt.modelMutationOwner = null;
    store.sessionModelMutationPending.value = null;
  }
}

/**
 * Pin the current session to a provider + model. `reasoningEffort` rides along
 * (`none` turns reasoning off; absent keeps the endpoint's default). Pass no
 * model at all (clear) to follow the install-wide default again.
 *
 * On an unsent draft there is no session to PATCH: the choice is held in the
 * draft and sent as part of `POST /api/sessions`, so the first message already
 * runs on it.
 */
export async function setSessionModelSelection(
  providerId: string,
  modelId: string,
  reasoningEffort?: string | null,
): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const selection: SessionModelSelection = {
    provider: providerId,
    model: modelId,
    ...(reasoningEffort ? { reasoning_effort: reasoningEffort } : {}),
  };
  const draft = store.draftSession.value;
  if (!sessionId) {
    if (!draft) return false;
    store.draftSession.value = { ...draft, model: selection };
    const provider = store.config.value?.providers.find((profile) => profile.id === providerId);
    const label = provider?.models.find((model) => model.id === modelId)?.name ?? modelId;
    store.pushToast(`New conversations will use ${label}.`, 'success');
    return true;
  }
  const mutation = beginSessionModelMutation(sessionId);
  try {
    const session = await api.setSessionModel(sessionId, selection);
    if (!ownsVisibleSessionModelMutation(mutation)) return false;
    store.sessions.value = store.sessions.value.map((entry) =>
      entry.id === sessionId ? { ...entry, model: session.model ?? null } : entry,
    );
    // The server's answer is authoritative for the open session, and the pills
    // read it from their own signal rather than from the list row.
    store.rememberSessionModel(sessionId, session.model ?? null);
    const provider = store.config.value?.providers.find((profile) => profile.id === providerId);
    const label = provider?.models.find((model) => model.id === modelId)?.name ?? modelId;
    store.pushToast(`This session now uses ${label}.`, 'success');
    return true;
  } catch (error) {
    if (!ownsVisibleSessionModelMutation(mutation)) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.pushToast(toErrorMessage(error));
    return false;
  } finally {
    finishSessionModelMutation(mutation);
  }
}

/** Drop the session's model pin so it follows the install-wide default again. */
export async function clearSessionModelSelection(): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) {
    // A draft that chose "follow the default" simply forgets the pin; there is no
    // server row to clear.
    const draft = store.draftSession.value;
    if (!draft) return false;
    store.draftSession.value = { ...draft, model: null };
    store.pushToast('New conversations follow the install default again.', 'success');
    return true;
  }
  const mutation = beginSessionModelMutation(sessionId);
  try {
    const session = await api.setSessionModel(sessionId, null);
    if (!ownsVisibleSessionModelMutation(mutation)) return false;
    store.sessions.value = store.sessions.value.map((entry) =>
      entry.id === sessionId ? { ...entry, model: session.model ?? null } : entry,
    );
    // The pin is gone: the session follows the install default again, and the
    // pill must say so without waiting for a list refresh.
    store.rememberSessionModel(sessionId, session.model ?? null);
    store.pushToast('This session follows the install default again.', 'success');
    return true;
  } catch (error) {
    if (!ownsVisibleSessionModelMutation(mutation)) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.pushToast(toErrorMessage(error));
    return false;
  } finally {
    finishSessionModelMutation(mutation);
  }
}

/**
 * Set this session's subagent-worktree override, or clear it to follow the
 * install-wide `subagents.worktrees` default again.
 *
 * This is the UI half of the same three scopes the agent has: the settings screen
 * owns the install default, this owns one conversation, and a single spawn can
 * still override it with its `worktree` argument. The server's answer is
 * authoritative and is written into the open session's `SessionInfo` so the
 * control reflects it without waiting for the next poll.
 */
export async function setSessionWorktrees(worktrees: boolean | null): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return false;
  try {
    const session = await api.setSessionWorktrees(sessionId, worktrees);
    const info = store.sessionInfo.value;
    if (info && info.session.id === sessionId && store.currentSessionId.value === sessionId) {
      store.sessionInfo.value = {
        ...info,
        session: { ...info.session, worktrees: session.worktrees ?? null },
      };
    }
    store.pushToast(
      worktrees === true
        ? 'Subagents in this session now get their own worktree.'
        : worktrees === false
          ? 'Subagents in this session now share this working tree.'
          : 'Subagents in this session follow the install default again.',
      'success',
    );
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.pushToast(toErrorMessage(error));
    return false;
  }
}

/** Fetch an older page without discarding live events or the already loaded tail. */
export async function loadOlderSession(): Promise<void> {
  const history = store.sessionHistory.value;
  if (!history || history.loading || history.before === null) return;
  const generation = rt.sessionGeneration;
  store.sessionHistory.value = { ...history, loading: true, error: null };
  try {
    const detail = await api.getSession(history.id, {}, history.before);
    if (generation !== rt.sessionGeneration || store.sessionHistory.value?.id !== history.id) return;
    const current = store.sessionHistory.value;
    const byId = new Map(current.messages.map((message) => [message.id, message]));
    for (const message of detail.messages) byId.set(message.id, message);
    // The cursor describes the union: a page can overlap rows this tab already
    // holds (a retried page, a refresh that widened the window), and the next
    // request must still ask for rows older than everything on screen.
    const before = detail.messages_before ?? null;
    const messages = [...byId.values()].sort((a, b) => a.id - b.id);
    store.sessionHistory.value = {
      id: history.id,
      messages,
      before,
      truncated: before !== null,
      loading: false,
      error: null,
    };
    const live = store.chat.value;
    const seeded = seedChatState(
      { ...detail, messages, messages_before: before, messages_truncated: before !== null },
      live,
    );
    seeded.items = seeded.items.map((item) => {
      const existing = live.items.find((entry) => entry.id === item.id);
      return existing ?? item;
    });
    const ids = new Set(seeded.items.map((item) => item.id));
    const extra = live.items.filter(
      (item) => item.id !== 'notice:history-truncated' && !ids.has(item.id),
    );
    store.chat.value = {
      ...live,
      items: [...seeded.items, ...extra].sort((a, b) =>
        a.id === 'notice:history-truncated'
          ? -1
          : b.id === 'notice:history-truncated'
            ? 1
            : a.at - b.at,
      ),
    };
  } catch (error) {
    if (generation === rt.sessionGeneration && store.sessionHistory.value?.id === history.id) {
      store.sessionHistory.value = {
        ...store.sessionHistory.value,
        loading: false,
        error: toErrorMessage(error),
      };
    }
  }
}

/**
 * Load the open session's counts and token totals.
 *
 * Deliberately a separate request from `openSession`: the transcript is a
 * bounded window and carries no token rows, while this endpoint carries no
 * message bodies. Reading it on demand is what lets the Session tab stay
 * current through a long run without refetching 50 000 messages.
 *
 * A read for a session the user has since left is dropped, so switching sessions
 * mid-flight cannot leave one session's numbers under another's heading.
 */
export async function refreshSessionInfo(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) {
    store.sessionInfo.value = null;
    store.sessionInfoFor.value = null;
    store.sessionInfoError.value = null;
    store.sessionInfoLoading.value = false;
    return;
  }
  const generation = rt.sessionGeneration;
  const projectRead = rt.projectGeneration;
  const readKey = `${sessionId}\0${generation}\0${projectRead}`;
  const current = () =>
    generation === rt.sessionGeneration &&
    projectRead === rt.projectGeneration &&
    store.currentSessionId.value === sessionId;
  const existing = rt.sessionInfoInFlight.get(readKey);
  if (existing) return existing;
  store.sessionInfoLoading.value = true;
  // `finally` compares against an owner token created *before* the async body can
  // run, not against the promise const: an async function starts executing
  // synchronously, so a throw before the first `await` unwinds through `finally`
  // while that const is still in its temporal dead zone.
  const owner = {};
  rt.sessionInfoOwners.set(readKey, owner);
  const read = (async () => {
    try {
      const info = await api.getSessionInfo(sessionId);
      if (!current()) return;
      store.sessionInfo.value = info;
      store.sessionInfoFor.value = sessionId;
      store.sessionInfoError.value = null;
    } catch (error) {
      if (!current()) return;
      if (isUnauthorized(error)) {
        enterAuthState();
        return;
      }
      store.sessionInfoError.value = toErrorMessage(error);
    } finally {
      // Only clear the state this call still owns. A second caller for the same
      // session joined this promise and relies on the same flag, and a read that
      // has already been superseded must not report "not loading" for one that
      // has not.
      if (rt.sessionInfoOwners.get(readKey) === owner) {
        rt.sessionInfoOwners.delete(readKey);
        rt.sessionInfoInFlight.delete(readKey);
        if (current()) store.sessionInfoLoading.value = false;
      }
    }
  })();
  // Only publish the promise if it is still running: a read that already settled
  // synchronously must not be joinable.
  if (rt.sessionInfoOwners.get(readKey) === owner) rt.sessionInfoInFlight.set(readKey, read);
  return read;
}

/**
 * Ask for a refresh, coalesced.
 *
 * `usage` arrives once per model call and `run_finished` once per run, so a long
 * run would otherwise issue a request per provider call. 400 ms is short enough
 * that the tab looks live and long enough that a burst collapses into one read.
 */
export function scheduleSessionInfoRefresh(delayMs = 400): void {
  if (typeof window === 'undefined') {
    void refreshSessionInfo();
    return;
  }
  store.sessionInfoTick.value += 1;
  if (rt.sessionInfoCoalesce !== null) window.clearTimeout(rt.sessionInfoCoalesce);
  rt.sessionInfoCoalesce = window.setTimeout(() => {
    rt.sessionInfoCoalesce = null;
    void refreshSessionInfo();
  }, delayMs);
}

/**
 * Attach the stored message ids to the rows this run built from events.
 *
 * A live row is identified by `user:<run>` or `tool:<call_id>`; the rewind
 * endpoint speaks stored ids. Until the two are joined, every rewind control on
 * a freshly finished turn has to stay hidden - which is exactly the "Rewind only
 * shows up after I refresh" report: a reload re-reads the session and the rows
 * arrive with their ids.
 *
 * One snapshot read per settled run is enough and is what a reload would do
 * anyway, but cheaper: it only touches the ids (`withStoredMessageIds`) instead
 * of re-seeding the transcript, so no row re-mounts, nothing fades, and the
 * reader's scroll position is untouched. Coalesced, because a turn that ends and
 * immediately starts a queued follow-up emits both events within milliseconds.
 */
export async function learnStoredMessageIds(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return;
  const read = rt.sessionGeneration;
  try {
    const detail = await api.getSession(sessionId);
    // A snapshot for a session this tab has left (or re-read since) must not be
    // grafted onto the transcript that replaced it.
    if (read !== rt.sessionGeneration || store.currentSessionId.value !== sessionId) return;
    const chat = store.chat.value;
    if (chat.sessionId !== sessionId) return;
    const items = withStoredMessageIds(chat.items, detail);
    if (items !== chat.items) store.chat.value = { ...chat, items };
  } catch {
    // Best effort: the rows simply stay unresolvable, and the next run (or a
    // reload) tries again. A toast for this would interrupt the reader with a
    // failure they cannot act on.
  }
}

/** Coalesce `learnStoredMessageIds` across a run's terminal events. */
export function scheduleStoredMessageIds(delayMs = 150): void {
  if (typeof window === 'undefined') {
    void learnStoredMessageIds();
    return;
  }
  if (rt.storedIdCoalesce !== null) window.clearTimeout(rt.storedIdCoalesce);
  rt.storedIdCoalesce = window.setTimeout(() => {
    rt.storedIdCoalesce = null;
    void learnStoredMessageIds();
  }, delayMs);
}

export function switchSession(id: string): void {
  store.mobileSheet.value = null;
  store.paletteOpen.value = false;
  void openSession(id);
}
