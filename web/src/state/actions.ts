import { ApiError, getAuthToken, onUnauthorized, setAuthToken, toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import { startEventStream, type EventStreamHandle } from '../api/events';
import type {
  AgentAction,
  AgentSummary,
  ApprovalDecision,
  Config,
  ConfigPatch,
  CatalogLookupResponse,
  CatalogResponse,
  KarucodeEvent,
  NotifyPayload,
  Project,
  ProviderTestRequest,
  ProviderTestResult,
  QueuedAttachment,
  QueuedKind,
  RawEvent,
  RewindMode,
  RewindPreview,
  Session,
  SessionDetail,
  SessionSummary,
} from '../api/types';
import { isNarrowViewport } from '../lib/dom';
import {
  emptyChatState,
  normalizeQueuedMessage,
  seedChatState,
  type ChatState,
  type MessageItem,
} from './chat';
import * as agentConsole from './agentConsole';
import { applyEventToChat, applyEventToSessions, markUserRun, sessionAgentFromRecord } from './eventReducer';
import { syncProjectIcons } from './projectIcons';
import { controlPlaneOpen } from './controlPlane';
import * as railPeek from './railPeek';
import * as notifyWiring from './notifyWiring';
import * as promptTokens from './promptTokens';
import * as workbench from './workbench';
import { summaryFromDetail } from './selectors';
import * as store from './store';

let stream: EventStreamHandle | null = null;
let bootstrapPromise: Promise<void> | null = null;
let sessionGeneration = 0;
let projectGeneration = 0;
// Independent read generations handle overlapping reads even within one project;
// projectGeneration handles the more dangerous A -> B -> A stale-response case.
let sessionListReadGeneration = 0;
let agentListReadGeneration = 0;
let agentDetailGeneration = 0;
let refreshingSessionId: string | null = null;
let refreshQueue: KarucodeEvent[] = [];
let sessionsRefreshTimer: number | null = null;
let gapResyncTimer: number | null = null;
let resyncInFlight = false;
let resyncAgain = false;
let resyncFailures = 0;
/** How often a failed refetch retries before it says so once and stops. */
const RESYNC_RETRIES = 3;
const RESYNC_BACKOFF_MS = 1_000;
let localMessageSequence = 0;
let sendInFlight = false;
/** Queued ids with a recall request in flight; module-scoped so a remount cannot double-fire. */
const recallingQueued = new Set<string>();
let handoffToken = 0;
let rewindGeneration = 0;
let resultPanelWidthWriteGeneration = 0;
const resultPanelWidthWrites = new Map<string, number>();

function nextHandoffToken(): number {
  handoffToken += 1;
  return handoffToken;
}

type ReplaceableOverlay =
  | 'settings'
  | 'palette'
  | 'themeStudio'
  | 'notificationCenter'
  | 'fileViewer'
  | 'rewind';

/**
 * Replace the application's modal/drawer surface in one place.
 *
 * Keeping this exhaustive prevents newer surfaces (the notification centre and
 * rewind dialog) from being accidentally stackable with Ctrl/Cmd+K, Settings or
 * Theme Studio. Closing rewind also invalidates its in-flight preview/apply read,
 * which must not clear a later target or navigate back to an old session.
 */
function replaceOverlay(next: ReplaceableOverlay | null): void {
  store.settingsOpen.value = next === 'settings';
  store.paletteOpen.value = next === 'palette';
  store.themeStudioOpen.value = next === 'themeStudio';
  store.notificationCenterOpen.value = next === 'notificationCenter';
  store.fileViewerPath.value = null;
  store.projectSheetOpen.value = false;
  if (next !== 'rewind' && store.rewindTarget.value !== null) rewindGeneration += 1;
  if (next !== 'rewind') store.rewindTarget.value = null;
  store.mobileSheet.value = null;
  workbench.closeContext();
  // The rail's fly-out is a hover affordance, not a modal surface.
  railPeek.closePeek();
}

function enterAuthState(): void {
  store.boot.value = 'auth';
  replaceOverlay(null);
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.isUnauthorized;
}

function mergeSummary(summary: SessionSummary): void {
  const list = store.sessions.value;
  const index = list.findIndex((session) => session.id === summary.id);
  // The merged row is the server's own answer for that session, so for the open
  // session it is also the answer to "what is this pinned to?" - recorded in the
  // open session's own signal, where it survives the row being dropped from
  // every list (a control chat, or a list refresh mid-flight).
  if (summary.id === store.currentSessionId.value) {
    store.rememberSessionModel(summary.id, summary.model ?? null);
  }
  // A control chat belongs to the control plane's list, never to `sessions.value`:
  // that list is "this project's sessions" and the workspace rail draws it
  // unfiltered, so inserting a project-less chat here made it appear as a project
  // row and then vanish at the next refresh (a second flicker of its own).
  if (!list.some((session) => session.id === summary.id) && (summary.control || summary.project_id === null)) {
    return;
  }
  if (index === -1) {
    store.sessions.value = [summary, ...list];
    return;
  }
  const next = [...list];
  next[index] = summary;
  store.sessions.value = next;
}

function summaryFromSession(session: Session): SessionSummary {
  return { ...session, message_count: 0, last_run_status: null, running: false };
}

function clearSelection(): void {
  if (store.currentSessionId.value !== null) resetPluginPanelData();
  store.currentSessionId.value = null;
  // Nothing is open, so there is no session whose pin the pills may show.
  store.rememberSessionModel(null, null);
  store.chat.value = emptyChatState(null);
  store.chatLoading.value = false;
  store.chatError.value = null;
  // Nothing is open, so no event can be waiting for a snapshot of it any more.
  refreshingSessionId = null;
  refreshQueue = [];
  store.sessionInfo.value = null;
  store.sessionInfoFor.value = null;
  store.sessionInfoError.value = null;
  store.sessionInfoLoading.value = false;
  store.writeLastSessionId(null);
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
  clearSelection();
}

function pickSession(): SessionSummary | null {
  const list = store.sessions.value;
  const last = store.readLastSessionId();
  return list.find((session) => session.id === last) ?? list[0] ?? null;
}

export function startStream(): void {
  stopStream();
  if (store.boot.value !== 'ready') return;
  let opens = 0;
  stream = startEventStream({
    token: getAuthToken(),
    onEvent: handleRawEvent,
    // Normal reconnects resume from Last-Event-ID. A remaining seq hole, an
    // explicit lag marker or a resync control still requires an authoritative read.
    onGap: () => scheduleGapResync(),
    onResync: () => scheduleGapResync(),
    onStatus: (state, detail) => {
      store.connection.value = {
        ...store.connection.value,
        stream: state,
        lastError: detail?.error ?? (state === 'open' ? null : store.connection.value.lastError),
      };
      if (state === 'unauthorized') {
        enterAuthState();
      } else if (state === 'open') {
        store.connection.value = { ...store.connection.value, api: 'online', lastError: null };
      }
    },
    onOpen: () => {
      opens += 1;
      // The first open follows bootstrap; only reconnect needs a resync.
      if (opens === 1) return;
      void refreshSessions();
      const id = store.currentSessionId.value;
      if (id) void openSession(id, { silent: true, preserveLive: true });
    },
  });
}

export function stopStream(): void {
  stream?.stop();
  stream = null;
}

/** Refetch what the stream can no longer tell us: the session list and transcript. */
async function resyncFromServer(): Promise<void> {
  // One refetch at a time; a burst of holes must not fan out into N of them.
  if (resyncInFlight) {
    resyncAgain = true;
    return;
  }
  resyncInFlight = true;
  let repaired = false;
  try {
    await refreshSessions();
    // `doRefreshSessions` swallows its own failure into the connection state.
    if (store.connection.value.api !== 'offline') {
      const id = store.currentSessionId.value;
      repaired = id === null || (await openSession(id, { silent: true, preserveLive: true }));
    }
  } finally {
    resyncInFlight = false;
    if (repaired) {
      resyncFailures = 0;
    } else {
      // A failed repair would leave a stale transcript on screen for good, which
      // is the bug this whole path exists to kill: retry a few times, then say so
      // once instead of silently giving up.
      resyncFailures += 1;
      if (resyncFailures <= RESYNC_RETRIES) {
        scheduleGapResync(RESYNC_BACKOFF_MS * 2 ** (resyncFailures - 1));
      } else if (resyncFailures === RESYNC_RETRIES + 1) {
        store.pushToast('Could not resynchronise this session; reload the page if it looks stale.');
      }
    }
    if (resyncAgain) {
      resyncAgain = false;
      scheduleGapResync();
    }
  }
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

/**
 * Repair the open session after the event stream lost frames (server lag, a
 * stalled proxy hop, a connection that had to be re-established, or a lag marker
 * from the pump). This is what the *reload the page* workaround used to do by
 * hand.
 *
 * The refetch always happens - it is how a stuck `running` flag is cleared when
 * the terminal event itself was the lost one. Whether it may *replace* the
 * transcript is decided later, in `openSession`, from the server's `running` flag
 * plus `hasLiveAnswer`, so a live answer is never deleted. Coalesced at 300 ms.
 */
function scheduleGapResync(delayMs = 300): void {
  if (typeof window === 'undefined') {
    void resyncFromServer();
    return;
  }
  if (gapResyncTimer !== null) return;
  gapResyncTimer = window.setTimeout(() => {
    gapResyncTimer = null;
    void resyncFromServer();
  }, delayMs);
}

export function restartStream(): void {
  if (store.boot.value === 'ready') startStream();
}

function handleRawEvent(raw: RawEvent): void {
  // The payload shape is validated by parseEventPayload; unknown `type` values
  // are ignored by the reducers.
  const event = raw as unknown as KarucodeEvent;

  // Notifications are handled before every filter below. The server reports every
  // terminal run, including ones in sessions this tab is not showing — that is the
  // case the feature exists for — so the notifier must see the frame even when the
  // chat reducer would drop it.
  if (event.type === 'notification') {
    void handleNotificationEvent(event.notification);
    return;
  }

  // Subagent frames are handled before the current-session filter below: the tab
  // shows agents running anywhere in the project, not only in the open session.
  handleAgentEvent(event);

  const wasKnown = store.sessions.value.some((session) => session.id === event.session_id);
  if (event.type === 'session_created' && !wasKnown) {
    // This event is global and carries no project_id. The scoped list refresh is
    // the only safe way to learn whether it belongs here; inventing a null-project
    // row both leaks foreign work into the rail and used to auto-open it on an
    // empty workspace. A session created by this tab already has a local summary.
    scheduleSessionsRefresh();
    return;
  }
  store.sessions.value = applyEventToSessions(store.sessions.value, event);

  if (event.type === 'session_created') {
    scheduleSessionsRefresh();
    return;
  }

  if (event.type === 'session_deleted') {
    if (store.currentSessionId.value === event.session_id) {
      const next = store.sessions.value[0] ?? null;
      store.writeLastSessionId(null);
      if (next) void openSession(next.id);
      else clearSelection();
    }
    return;
  }

  if (event.type === 'session_model_changed') {
    // The list row is patched by the reducer above, but the open session's own
    // pin lives outside every list: a control chat has no row, and a list
    // refresh drops the row of a project-less session. The pills read the pin
    // from the signal, so the event has to be recorded there too.
    if (event.session_id === store.currentSessionId.value) {
      store.rememberSessionModel(event.session_id, event.model ?? null);
    }
  }

  if (event.type === 'session_renamed' || event.type === 'session_model_changed') {
    return;
  }

  if (event.type === 'session_rewound') {
    // The hidden tail is already on screen. Refetch rather than splice: the
    // server owns the normalisation (an assistant turn resolves to the prompt
    // that produced it) and the file journal's effects are not in the transcript
    // at all.
    if (event.session_id === store.currentSessionId.value) {
      void openSession(event.session_id, { force: true, silent: true });
    }
    scheduleSessionsRefresh();
    return;
  }

  if (event.type === 'run_finished' || event.type === 'run_started' || event.type === 'run_cancelled') {
    scheduleSessionsRefresh();
  }

  if (event.session_id && event.session_id !== store.currentSessionId.value) return;

  if (refreshingSessionId !== null && event.session_id === refreshingSessionId) {
    refreshQueue.push(event);
    return;
  }

  applyChatEvent(event);
}

function applyChatEvent(event: KarucodeEvent): void {
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

/**
 * Accumulate one agent-scoped frame into the workspace-wide agent view.
 *
 * Lifecycle events update the registry; text/tool/approval events update that
 * agent's transcript only. The chat reducer deliberately drops subagent text and
 * tool frames (they belong to another conversation), so this is where they land.
 */
/**
 * Hand one notification to the page's notifier.
 *
 * Kept out of the chat reducer on purpose: a notification is not a transcript item,
 * and the reducer only ever sees the open session. The notifier decides which of
 * the four surfaces fires; a failure there must never break the stream, so it is
 * caught and logged rather than propagated.
 */
async function handleNotificationEvent(payload: NotifyPayload | undefined): Promise<void> {
  if (!payload || typeof payload.id !== 'string') return;
  try {
    await notifyWiring.handleNotification(payload);
  } catch (error) {
    console.error('karucode: a notification could not be shown', error);
  }
}

function handleAgentEvent(event: KarucodeEvent): void {
  const agentId = agentConsole.agentEventId(event);
  if (!agentId) return;
  const inScope = store.sessions.value.some((session) => session.id === event.session_id);
  if (agentConsole.isAgentLifecycleEvent(event)) {
    store.agentsAll.value = agentConsole.applyAgentRegistry(
      store.agentsAll.value,
      event,
      store.sessions.value,
    );
  }
  if (!inScope) return;
  const current = store.agentActivities.value[agentId] ?? agentConsole.emptyActivity();
  const next = agentConsole.applyAgentEvent(current, event);
  if (next === current) return;
  store.agentActivities.value = { ...store.agentActivities.value, [agentId]: next };
}

/**
 * Seed the workspace list and live transcripts from a session snapshot.
 *
 * The snapshot carries every message, subagent rows included, so opening a
 * session fills in the history of agents that ran before this page loaded
 * without a request per agent.
 */
function seedAgentState(detail: SessionDetail): void {
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
    const seeded = agentConsole.activityFromMessages(detail.messages, agent.id);
    if (seeded.items.length === 0) continue;
    const live = next[agent.id];
    next[agent.id] = live ? agentConsole.mergeActivities(seeded, live) : seeded;
    changed = true;
  }
  if (changed) store.agentActivities.value = next;
}

function drainRefreshQueue(settled = false): void {
  const queued = refreshQueue;
  refreshQueue = [];
  refreshingSessionId = null;
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

async function doRefreshSessions(): Promise<void> {
  const projectId = store.currentProjectId.value;
  const projectRead = projectGeneration;
  const read = ++sessionListReadGeneration;
  const isCurrent = () =>
    projectId === store.currentProjectId.value &&
    projectRead === projectGeneration &&
    read === sessionListReadGeneration;
  try {
    const list = await api.listSessions(projectId);
    if (!isCurrent()) return;
    store.sessions.value = list;
    store.sessionsLoading.value = false;
    // Agents belong to sessions: a deleted or renamed session changes both the
    // list and the labels the Agents view groups by.
    void refreshAgents();
    const current = store.currentSessionId.value;
    if (current && !list.some((session) => session.id === current)) {
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
      const next = list[0] ?? null;
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
  if (sessionsRefreshTimer !== null) window.clearTimeout(sessionsRefreshTimer);
  sessionsRefreshTimer = window.setTimeout(() => {
    sessionsRefreshTimer = null;
    void doRefreshSessions();
  }, 450);
}

export async function bootstrap(force = false): Promise<void> {
  if (bootstrapPromise && !force) return bootstrapPromise;
  bootstrapPromise = (async () => {
    store.boot.value = 'loading';
    store.bootError.value = null;
    store.sessionsLoading.value = true;
    replaceOverlay(null);
    try {
      const [health, config, projectList] = await Promise.all([
        api.getHealth(),
        api.getConfig(),
        api.listProjects(),
      ]);
      store.health.value = health;
      store.config.value = config;
      store.projects.value = projectList.projects;
      store.projectsLoading.value = false;
      // Per-project icons are fetched in the background: the rail draws initials
      // first and swaps in a favicon when one arrives, so this never delays boot.
      syncProjectIcons(projectList.projects);
      // Keep the remembered project when it still exists, otherwise open the
      // first one (the bootstrap-created default).
      const remembered = store.readLastProjectId();
      const project =
        projectList.projects.find((candidate) => candidate.id === remembered) ??
        projectList.projects[0] ??
        null;
      store.currentProjectId.value = project?.id ?? null;
      store.writeLastProjectId(project?.id ?? null);
      const list = await api.listSessions(project?.id ?? null);
      store.sessions.value = list;
      store.sessionsLoading.value = false;
      await refreshAgents();
      store.authError.value = null;
      store.connection.value = {
        api: 'online',
        stream: store.connection.value.stream,
        lastError: null,
        checkedAt: Date.now(),
      };
      const target = pickSession();
      store.boot.value = 'ready';
      if (target) await openSession(target.id);
      else clearSelection();
      startStream();
    } catch (error) {
      store.sessionsLoading.value = false;
      store.projectsLoading.value = false;
      if (isUnauthorized(error)) {
        enterAuthState();
        store.authError.value = null;
        return;
      }
      store.boot.value = 'error';
      store.bootError.value = toErrorMessage(error);
      store.connection.value = {
        ...store.connection.value,
        api: 'offline',
        lastError: toErrorMessage(error),
        checkedAt: Date.now(),
      };
    }
  })().finally(() => {
    bootstrapPromise = null;
  });
  return bootstrapPromise;
}

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
}

/**
 * Bring the workspace forward and open a session.
 *
 * The click handler of a desktop notification: a notification you can click that
 * does nothing is worse than one that cannot be clicked.
 */
export function focusSession(sessionId: string, projectId?: string | null): void {
  try {
    // `window.focus` is best-effort: a browser may refuse it when the click did not
    // come from a real gesture, and there is nothing to do about that.
    if (typeof window !== 'undefined' && typeof window.focus === 'function') {
      window.focus();
    }
  } catch {
    // Ignore: the navigation below is the part that matters.
  }
  if (sessionId) {
    // ToastRegion's existing Open button supplies only the session. Resolve that
    // against the persisted notification record; desktop notifications pass the
    // payload's project directly. Both paths go through the project-aware action.
    const resolvedProject = projectId === undefined
      ? store.notifications.value.find((entry) => entry.sessionId === sessionId)?.projectId ?? null
      : projectId;
    store.clearUnread(sessionId);
    void openSessionInProject(resolvedProject, sessionId);
  }
}

export async function openSession(id: string, options: OpenSessionOptions = {}): Promise<boolean> {
  const silent = options.silent ?? false;
  if (id === store.currentSessionId.value && !silent && !options.force) return false;

  const generation = (sessionGeneration += 1);
  const sessionChanged = store.currentSessionId.value !== id;
  if (store.sessionHistory.value?.id !== id) store.sessionHistory.value = null;
  else if (store.sessionHistory.value.loading) store.sessionHistory.value = { ...store.sessionHistory.value, loading: false };
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

  refreshingSessionId = id;
  refreshQueue = [];

  let applied = false;
  // Whether the snapshot closes the book on every turn it contains.
  let settled = false;

  try {
    const detail = await api.getSession(id);
    if (generation !== sessionGeneration) return false;
    store.sessionHistory.value = { id, messages: detail.messages, before: detail.messages_before ?? null, truncated: !!detail.messages_truncated, loading: false, error: null };
    if (options.preserveLive && detail.running && hasLiveAnswer(id)) {
      // Never delete an answer that is still streaming - but do not throw the
      // snapshot away either. It is the only thing that can restore a tool card,
      // an approval or a notice whose frame was one of the lost ones.
      store.chat.value = mergeLiveAnswer(seedChatState(detail, store.chat.value), store.chat.value);
    } else {
      store.chat.value = seedChatState(detail, store.chat.value);
      settled = !detail.running;
    }
    // The snapshot cannot carry `usage` (it is not persisted), so the context
    // pill would be blank until the next call finished. Restore the last number
    // this browser saw for the session; `seedChatState` already kept a live
    // in-flight value, so only fill a hole.
    if (store.chat.value.lastPromptTokens === null) {
      const remembered = promptTokens.recallPromptTokens(id);
      if (remembered !== null) store.chat.value = { ...store.chat.value, lastPromptTokens: remembered };
    }
    seedAgentState(detail);
    // The session's own model pin is recorded by `mergeSummary` below, which
    // is the one place that sees the server's answer for a session.
    mergeSummary(summaryFromDetail(detail));
    store.chatLoading.value = false;
    store.chatError.value = null;
    applied = true;
  } catch (error) {
    if (generation !== sessionGeneration) return false;
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
    if (generation === sessionGeneration) {
      drainRefreshQueue(settled);
    }
  }
  return applied;
}

export async function createSession(): Promise<Session | null> {
  try {
    const session = await api.createSession(undefined, store.currentProjectId.value);
    mergeSummary(summaryFromSession(session));
    store.mobileSheet.value = null;
    await openSession(session.id);
    store.composerFocusTick.value += 1;
    return session;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
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

export async function sendChatMessage(content: string): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const generation = sessionGeneration;
  const projectRead = projectGeneration;
  const trimmed = content.trim();
  if (!sessionId || trimmed === '') return false;
  if (store.chat.value.running || sendInFlight) return false;

  sendInFlight = true;
  localMessageSequence += 1;
  const localId = `local:${Date.now()}:${localMessageSequence}`;
  const optimistic: MessageItem = {
    kind: 'message',
    id: localId,
    at: Date.now(),
    role: 'user',
    content: trimmed,
    reasoning: '',
    streaming: false,
    runId: null,
    agentId: null,
  };
  store.chat.value = {
    ...store.chat.value,
    items: [...store.chat.value.items, optimistic],
    running: true,
    runStatus: 'running',
    runError: null,
    iteration: 0,
  };
  // The reader wants to see the answer, so the transcript re-pins to the bottom.
  store.sentMessageTick.value += 1;

  try {
    const result = await api.sendMessage(sessionId, trimmed);
    if (store.currentSessionId.value === sessionId) {
      const current = store.chat.value;
      store.chat.value = {
        ...current,
        runId: result.run_id,
        items: markUserRun(current.items, result.run_id),
      };
    }
    scheduleSessionsRefresh();
    return true;
  } catch (error) {
    if (store.currentSessionId.value === sessionId) {
      const current = store.chat.value;
      store.chat.value = {
        ...current,
        items: current.items.filter((item) => item.id !== localId),
        running: false,
        runStatus: null,
      };
    }
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 409) {
      store.pushToast('A run is already active in this session.', 'info');
    } else {
      store.pushToast(toErrorMessage(error));
    }
    if (
      generation === sessionGeneration &&
      projectRead === projectGeneration &&
      store.currentSessionId.value === sessionId
    ) {
      void openSession(sessionId, { silent: true });
    }
    return false;
  } finally {
    sendInFlight = false;
  }
}

export async function stopRun(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return;
  store.stopping.value = true;
  try {
    await api.cancelRun(sessionId);
    scheduleSessionsRefresh();
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
  } finally {
    store.stopping.value = false;
  }
}

/**
 * Send a message into the run that is already working.
 *
 * A *steer* folds into the turn in flight and a *follow-up* starts the run after
 * it; either way the message is queued rather than submitted, so this returns
 * once the server has accepted it and nothing is optimistically added to the
 * transcript. The message appears there only when the run applies it (a steer
 * gets a `steer_applied` frame, a follow-up its own run), which is what keeps the
 * transcript in the order the model actually saw things.
 */
export async function queueChatMessage(
  mode: QueuedKind,
  message: { content: string; draft?: string; attachments?: QueuedAttachment[] },
): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const generation = sessionGeneration;
  const projectRead = projectGeneration;
  const stillCurrent = () =>
    generation === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId;
  const sameSession = () =>
    projectRead === projectGeneration && store.currentSessionId.value === sessionId;
  if (!sessionId || message.content.trim() === '') return false;
  if (!store.chat.value.running) return false;
  try {
    const result = await api.sendQueuedMessage(sessionId, mode, message);
    // The acceptance carries the entry, so the pending panel is correct even
    // before the `queued_messages` frame arrives (and stays correct if it never
    // does, because a reconnect refetches the snapshot).
    if (sameSession() && result.queued_message) {
      const current = store.chat.value;
      // Normalised on the way in: the acceptance body is the raw struct too, so
      // `draft` may be absent (`normalizeQueuedMessage`).
      const entry = normalizeQueuedMessage(result.queued_message);
      store.chat.value = {
        ...current,
        queued:
          current.queued.some((queued) => queued.id === entry.id) ||
          current.retiredQueuedIds.includes(entry.id)
            ? current.queued
            : [...current.queued, entry],
      };
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 409) {
      // The run ended between the click and the request. Say so plainly: the
      // message is still in the composer, and sending it again is a plain send.
      store.pushToast(
        'That run finished before the message could be queued. Send it as a normal message instead.',
        'info',
      );
      // Resynchronise: the queue this browser shows is now wrong, and so is the
      // `running` flag it decided from.
      if (stillCurrent()) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  }
}

/**
 * Take a queued message back out of the queue and put it in the composer.
 *
 * The message never reached the model, so this is the undo for "sent too soon":
 * the draft goes back in the textarea and the attachment chips come back, which
 * is why the server hands the entry back rather than just a `deleted: true`.
 */
export async function recallQueuedMessage(queuedId: string): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const generation = sessionGeneration;
  const projectRead = projectGeneration;
  const stillCurrent = () =>
    generation === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId;
  const sameSession = () =>
    projectRead === projectGeneration && store.currentSessionId.value === sessionId;
  if (!sessionId || queuedId === '') return false;
  if (recallingQueued.has(queuedId)) return false;
  recallingQueued.add(queuedId);
  store.recallingQueued.value = { ...store.recallingQueued.value, [queuedId]: true };
  try {
    const result = await api.recallQueued(sessionId, queuedId);
    const entry = result.recalled;
    if (sameSession()) {
      const current = store.chat.value;
      store.chat.value = {
        ...current,
        queued: current.queued.filter((queued) => queued.id !== queuedId),
        retiredQueuedIds: [...new Set([...current.retiredQueuedIds, queuedId])].slice(-100),
      };
      // The composer consumes the hand-off and appends to whatever the user has
      // typed since, so a recall never silently replaces a fresh draft. The
      // recalled entry is normalised on the way in (`normalizeQueuedMessage`), so
      // `draft` is a string here even when the server omitted it.
      const recalled = normalizeQueuedMessage(entry);
      store.composerHandoff.value = {
        text: recalled.draft ?? recalled.content,
        attachments: recalled.attachments ?? [],
        token: nextHandoffToken(),
      };
      store.composerFocusTick.value += 1;
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 404) {
      // It landed while the click was in flight. The queue on screen is stale, so
      // refresh instead of resurrecting a message the model already saw.
      store.pushToast('That message was already applied to the run.', 'info');
      if (stillCurrent()) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  } finally {
    recallingQueued.delete(queuedId);
    const next = { ...store.recallingQueued.value };
    delete next[queuedId];
    store.recallingQueued.value = next;
  }
}

export async function resolveApproval(
  approvalId: string,
  decision: Exclude<ApprovalDecision, 'expired'>,
): Promise<boolean> {
  if (store.resolvingApprovals.value[approvalId]) return false;
  store.resolvingApprovals.value = { ...store.resolvingApprovals.value, [approvalId]: true };
  try {
    await api.resolveApproval(approvalId, decision);
    const current = store.chat.value;
    store.chat.value = {
      ...current,
      items: current.items.map((item) =>
        item.kind === 'approval' && item.approval.id === approvalId
          ? {
              ...item,
              approval: { ...item.approval, status: 'resolved', decision },
            }
          : item,
      ),
    };
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else {
      store.pushToast(toErrorMessage(error));
      if (error instanceof ApiError && error.status === 404) {
        const current = store.chat.value;
        store.chat.value = {
          ...current,
          items: current.items.map((item) =>
            item.kind === 'approval' && item.approval.id === approvalId
              ? { ...item, approval: { ...item.approval, status: 'expired' } }
              : item,
          ),
        };
      }
      // Any other failure leaves the card saying "waiting for this decision",
      // which is a claim about the *server's* state that this call just proved it
      // cannot read. Resolve it the way `sendChatMessage` resolves its own failed
      // send: refetch, silently and forced, so the server's pending-approvals
      // snapshot decides what the card says. Nothing is guessed locally - the
      // approval may still be pending (a network blip), already applied, or
      // expired by the runtime, and only the server knows which.
      const sessionId = store.currentSessionId.value;
      if (sessionId) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    }
    return false;
  } finally {
    const next = { ...store.resolvingApprovals.value };
    delete next[approvalId];
    store.resolvingApprovals.value = next;
  }
}

export async function checkHealth(): Promise<boolean> {
  try {
    const result = await api.getHealth();
    store.health.value = result;
    store.connection.value = {
      ...store.connection.value,
      api: 'online',
      lastError: null,
      checkedAt: Date.now(),
    };
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.connection.value = {
      ...store.connection.value,
      api: 'offline',
      lastError: toErrorMessage(error),
      checkedAt: Date.now(),
    };
    return false;
  }
}

export async function retryConnection(): Promise<void> {
  const ok = await checkHealth();
  if (!ok) return;
  if (store.boot.value === 'auth') return;
  if (store.boot.value === 'error' || store.sessions.value.length === 0) {
    await bootstrap(true);
    return;
  }
  await refreshSessions();
  const id = store.currentSessionId.value;
  if (id) await openSession(id, { silent: true, preserveLive: true });
  restartStream();
}

export async function submitToken(token: string): Promise<boolean> {
  store.authError.value = null;
  const trimmed = token.trim();
  if (trimmed === '') {
    store.authError.value = 'Enter the bearer token configured on the server.';
    return false;
  }
  try {
    const result = await api.getHealth({ token: trimmed });
    setAuthToken(trimmed);
    store.health.value = result;
    store.boot.value = 'loading';
    await bootstrap(true);
    return true;
  } catch (error) {
    store.authError.value = isUnauthorized(error)
      ? 'That token was rejected by the server.'
      : toErrorMessage(error);
    return false;
  }
}

export async function loadConfig(): Promise<Config> {
  const result = await api.getConfig();
  store.config.value = result;
  return result;
}

export async function saveConfig(patch: ConfigPatch): Promise<Config> {
  const result = await api.patchConfig(patch);
  store.config.value = result;
  store.pushToast('Settings saved.', 'success');
  void checkHealth();
  return result;
}

export async function clearApiKey(): Promise<Config> {
  const result = await api.patchConfig({ model: { api_key: null, clear_api_key: true } });
  store.config.value = result;
  store.pushToast('API key cleared.', 'info');
  void checkHealth();
  return result;
}

export function testProvider(body: ProviderTestRequest = {}): Promise<ProviderTestResult> {
  return api.testProvider(body);
}

/** Save provider profiles and/or select the active provider+model. */
export async function saveProviders(
  patch: Pick<ConfigPatch, 'providers' | 'active'>,
  options: { toast?: string | null } = {},
): Promise<Config | null> {
  try {
    const result = await api.patchConfig(patch);
    store.config.value = result;
    if (options.toast !== null) store.pushToast(options.toast ?? 'Providers saved.', 'success');
    void checkHealth();
    return result;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return null;
    }
    store.pushToast(toErrorMessage(error));
    return null;
  }
}

/** Switch the active model (optionally on another provider) for every project. */
export async function switchModel(providerId: string, modelId: string): Promise<boolean> {
  const current = store.config.value;
  if (current?.active.provider === providerId && current.active.model === modelId) return true;
  const result = await saveProviders(
    { active: { provider: providerId, model: modelId } },
    { toast: null },
  );
  if (!result) return false;
  const provider = result.providers.find((profile) => profile.id === providerId);
  const label = provider?.models.find((model) => model.id === modelId)?.name ?? modelId;
  store.pushToast(`Now using ${label}.`, 'success');
  return true;
}

/**
 * Pin the current session to a provider + model. `reasoningEffort` rides along
 * (`none` turns reasoning off; absent keeps the endpoint's default). Pass no
 * model at all (clear) to follow the install-wide default again.
 */
export async function setSessionModelSelection(
  providerId: string,
  modelId: string,
  reasoningEffort?: string | null,
): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return false;
  const selection = {
    provider: providerId,
    model: modelId,
    ...(reasoningEffort ? { reasoning_effort: reasoningEffort } : {}),
  };
  try {
    const session = await api.setSessionModel(sessionId, selection);
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
    if (isUnauthorized(error)) {
      enterAuthState();
      return false;
    }
    store.pushToast(toErrorMessage(error));
    return false;
  }
}

/** Drop the session's model pin so it follows the install-wide default. */
export async function clearSessionModelSelection(): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return false;
  try {
    const session = await api.setSessionModel(sessionId, null);
    store.sessions.value = store.sessions.value.map((entry) =>
      entry.id === sessionId ? { ...entry, model: session.model ?? null } : entry,
    );
    // The pin is gone: the session follows the install default again, and the
    // pill must say so without waiting for a list refresh.
    store.rememberSessionModel(sessionId, session.model ?? null);
    store.pushToast('This session follows the install default again.', 'success');
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

/** models.dev catalog: provider list or one provider's models. */
export async function loadCatalog(
  provider?: string,
  refresh = false,
): Promise<CatalogResponse | null> {
  try {
    return await api.getModelsCatalog({ provider, refresh });
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
}

/**
 * models.dev metadata for one model id, gateway prefixes stripped server-side.
 * Best-effort: any failure returns null and the caller adds the id as-is.
 */
export async function lookupModel(model: string): Promise<CatalogLookupResponse | null> {
  try {
    return await api.lookupModelMetadata(model);
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    return null;
  }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export async function refreshProjects(): Promise<void> {
  try {
    const result = await api.listProjects();
    store.projects.value = result.projects;
    store.projectsLoading.value = false;
    syncProjectIcons(result.projects);
    const current = store.currentProjectId.value;
    if (!result.projects.some((project) => project.id === current)) {
      const next = result.projects[0] ?? null;
      store.currentProjectId.value = next?.id ?? null;
      store.writeLastProjectId(next?.id ?? null);
    }
  } catch (error) {
    store.projectsLoading.value = false;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.pushToast(toErrorMessage(error));
  }
}

/**
 * Show another project. Sessions are fetched per project from the server, so
 * switching never mixes two projects' histories (or their chat state).
 */
export async function switchProject(id: string): Promise<void> {
  if (store.currentProjectId.value === id) {
    store.projectSheetOpen.value = false;
    return;
  }
  // Invalidate in-flight chat/session/agent reads before changing the project.
  const generation = ++projectGeneration;
  sessionGeneration += 1;
  store.currentProjectId.value = id;
  store.writeLastProjectId(id);
  store.projectSheetOpen.value = false;
  store.mobileSheet.value = null;
  store.inspectorTab.value = 'workspace';
  // Panel payloads are project-scoped: the previous project's answers must not
  // be shown against the new one.
  resetPluginPanelData();
  store.sessions.value = [];
  store.sessionsLoading.value = true;
  // The Agents view is project-scoped: drop the previous project's agents and
  // transcripts instead of showing them until the first refresh lands.
  store.agentsAll.value = [];
  store.agentActivities.value = {};
  closeAgentDetail();
  store.agentFilter.value = 'all';
  store.sessionInfo.value = null;
  store.sessionInfoFor.value = null;
  store.sessionInfoError.value = null;
  store.fileViewerPath.value = null;
  clearSelection();
  await refreshSessions();
  if (generation !== projectGeneration || store.currentProjectId.value !== id) return;
  const next = pickSession();
  if (next) await openSession(next.id);
}

export async function createProject(name: string, path: string): Promise<Project | null> {
  try {
    const project = await api.createProject(path, name.trim() || undefined);
    store.projects.value = [...store.projects.value, project];
    syncProjectIcons(store.projects.value);
    store.pushToast(`Added ${project.name}.`, 'success');
    await switchProject(project.id);
    return project;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
}

export async function renameProjectById(id: string, name: string): Promise<boolean> {
  const trimmed = name.trim();
  if (!trimmed) return false;
  try {
    const project = await api.renameProject(id, trimmed);
    store.projects.value = store.projects.value.map((candidate) =>
      candidate.id === id ? { ...candidate, ...project } : candidate,
    );
    store.pushToast('Project renamed.', 'success');
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return false;
  }
}

/**
 * Persist only the project preference; the separator owns its immediate local
 * preview. A failed/offline write therefore never snaps the open pane back or
 * discards session state. Late responses cannot overwrite a newer drag.
 */
export async function saveProjectResultPanelWidth(id: string, width: number): Promise<boolean> {
  if (!store.projects.value.some((project) => project.id === id)) return false;
  const generation = ++resultPanelWidthWriteGeneration;
  resultPanelWidthWrites.set(id, generation);
  try {
    const saved = await api.updateProjectResultPanelWidth(id, width);
    if (resultPanelWidthWrites.get(id) === generation) {
      store.projects.value = store.projects.value.map((candidate) =>
        candidate.id === id ? { ...candidate, ...saved } : candidate,
      );
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else if (resultPanelWidthWrites.get(id) === generation) {
      store.pushToast(
        `Result width is kept for this window, but could not be saved: ${toErrorMessage(error)}`,
        'error',
        0,
      );
    }
    return false;
  }
}

export async function deleteProjectById(id: string, force = false): Promise<boolean> {
  try {
    const result = await api.deleteProject(id, force);
    const remaining = store.projects.value.filter((project) => project.id !== id);
    store.projects.value = remaining;
    // Drop the deleted project's icon (and revoke its object URL) instead of
    // leaving it in memory for the rest of the page's life.
    syncProjectIcons(remaining);
    store.pushToast(
      result.deleted_sessions > 0
        ? `Project deleted with ${result.deleted_sessions} session(s).`
        : 'Project removed from the rail (its files are untouched).',
      'info',
    );
    if (store.currentProjectId.value === id) {
      const next = remaining[0] ?? null;
      store.currentProjectId.value = null;
      clearSelection();
      if (next) await switchProject(next.id);
      else store.sessions.value = [];
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return false;
  }
}

export async function refreshSessionAgents(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  const generation = sessionGeneration;
  const projectRead = projectGeneration;
  if (!sessionId) return;
  const current = () =>
    generation === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId;
  try {
    const detail = await api.getSession(sessionId);
    if (current()) {
      store.chat.value = { ...store.chat.value, agents: detail.agents.map(sessionAgentFromRecord) };
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
 */
export async function refreshAgents(): Promise<void> {
  const projectId = store.currentProjectId.value;
  const projectRead = projectGeneration;
  const read = ++agentListReadGeneration;
  const current = () =>
    projectId === store.currentProjectId.value &&
    projectRead === projectGeneration &&
    read === agentListReadGeneration;
  store.agentsLoading.value = true;
  try {
    const list = await api.listAgents({ project: projectId });
    if (!current()) return;
    // The server omits `note`, `progress` and `owned_paths` when empty; normalise
    // here so every row in the store is fully populated.
    store.agentsAll.value = list.map(sessionAgentFromRecord);
    store.agentsError.value = null;
  } catch (error) {
    if (!current()) return;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.agentsError.value = toErrorMessage(error);
  } finally {
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
  agentDetailGeneration += 1;
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
  const generation = ++agentDetailGeneration;
  store.selectedAgentId.value = id;
  store.agentHistory.value = null;
  store.agentDetailError.value = null;
  store.mobileSheet.value = null;
  store.agentDetailLoading.value = true;
  try {
    const detail = await api.getAgent(id);
    if (generation !== agentDetailGeneration || store.selectedAgentId.value !== id) return;
    store.agentHistory.value = { id, messages: detail.messages, before: detail.messages_before ?? null, truncated: detail.messages_truncated, loading: false, error: null };
    store.agentsAll.value = agentConsole.mergeAgentList(store.agentsAll.value, [detail.agent]);

  } catch (error) {
    if (generation !== agentDetailGeneration || store.selectedAgentId.value !== id) return;
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.agentDetailError.value = toErrorMessage(error);
  } finally {
    if (generation === agentDetailGeneration && store.selectedAgentId.value === id) store.agentDetailLoading.value = false;
  }
}


/** Fetch an older page without discarding live events or the already loaded tail. */
export async function loadOlderSession(): Promise<void> {
  const history = store.sessionHistory.value;
  if (!history || history.loading || history.before === null) return;
  const generation = sessionGeneration;
  store.sessionHistory.value = { ...history, loading: true, error: null };
  try {
    const detail = await api.getSession(history.id, {}, history.before);
    if (generation !== sessionGeneration || store.sessionHistory.value?.id !== history.id) return;
    const current = store.sessionHistory.value;
    const byId = new Map(current.messages.map((message) => [message.id, message]));
    for (const message of detail.messages) byId.set(message.id, message);
    detail.messages = [...byId.values()].sort((a, b) => a.id - b.id);
    store.sessionHistory.value = { id: history.id, messages: detail.messages, before: detail.messages_before ?? null, truncated: !!detail.messages_truncated, loading: false, error: null };
    const live = store.chat.value;
    const seeded = seedChatState(detail, live);
    seeded.items = seeded.items.map((item) => {
      const current = live.items.find((entry) => entry.id === item.id);
      return current ?? item;
    });
    const ids = new Set(seeded.items.map((item) => item.id));
    const extra = live.items.filter((item) => item.id !== 'notice:history-truncated' && !ids.has(item.id));
    store.chat.value = { ...live, items: [...seeded.items, ...extra].sort((a, b) => a.id === 'notice:history-truncated' ? -1 : b.id === 'notice:history-truncated' ? 1 : a.at - b.at) };
  } catch (error) {
    if (generation === sessionGeneration && store.sessionHistory.value?.id === history.id)
      store.sessionHistory.value = { ...store.sessionHistory.value, loading: false, error: toErrorMessage(error) };
  }
}

export async function loadOlderAgent(): Promise<void> {
  const history = store.agentHistory.value;
  if (!history || history.loading || history.before === null) return;
  const generation = agentDetailGeneration;
  store.agentHistory.value = { ...history, loading: true, error: null };
  try {
    const detail = await api.getAgent(history.id, { before: history.before });
    if (generation !== agentDetailGeneration || store.selectedAgentId.value !== history.id || store.agentHistory.value?.id !== history.id) return;
    const byId = new Map(store.agentHistory.value.messages.map((message) => [message.id, message]));
    for (const message of detail.messages) byId.set(message.id, message);
    store.agentHistory.value = { id: history.id, messages: [...byId.values()].sort((a, b) => a.id - b.id), before: detail.messages_before ?? null, truncated: detail.messages_truncated, loading: false, error: null };
  } catch (error) {
    if (generation === agentDetailGeneration && store.selectedAgentId.value === history.id && store.agentHistory.value?.id === history.id)
      store.agentHistory.value = { ...store.agentHistory.value, loading: false, error: toErrorMessage(error) };
  }
}

/** Jump from an agent to the session it belongs to (switching projects if needed). */
export async function openAgentSession(agent: AgentSummary): Promise<void> {
  if (agent.project_id && agent.project_id !== store.currentProjectId.value) {
    await switchProject(agent.project_id);
  }
  closeAgentDetail();
  await openSession(agent.session_id);
}

// ---------------------------------------------------------------------------
// Session information tab
// ---------------------------------------------------------------------------

/**
 * One read in flight **per session-generation/project-generation**.
 *
 * Keyed, not a single flag: session B must never join A's URL or in-flight promise,
 * and revisiting the same session id after switching projects must not join its
 * old generation either. Owners keep one read's `finally` from clearing another's.
 */
const sessionInfoInFlight = new Map<string, Promise<void>>();
const sessionInfoOwners = new Map<string, object>();
let sessionInfoCoalesce: number | null = null;

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
  const generation = sessionGeneration;
  const projectRead = projectGeneration;
  const readKey = `${sessionId}\0${generation}\0${projectRead}`;
  const current = () =>
    generation === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId;
  const existing = sessionInfoInFlight.get(readKey);
  if (existing) return existing;
  store.sessionInfoLoading.value = true;
  // `finally` compares against an owner token created *before* the async body can
  // run, not against the promise const: an async function starts executing
  // synchronously, so a throw before the first `await` unwinds through `finally`
  // while that const is still in its temporal dead zone.
  const owner = {};
  sessionInfoOwners.set(readKey, owner);
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
      if (sessionInfoOwners.get(readKey) === owner) {
        sessionInfoOwners.delete(readKey);
        sessionInfoInFlight.delete(readKey);
        if (current()) store.sessionInfoLoading.value = false;
      }
    }
  })();
  // Only publish the promise if it is still running: a read that already settled
  // synchronously must not be joinable.
  if (sessionInfoOwners.get(readKey) === owner) sessionInfoInFlight.set(readKey, read);
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
  if (sessionInfoCoalesce !== null) window.clearTimeout(sessionInfoCoalesce);
  sessionInfoCoalesce = window.setTimeout(() => {
    sessionInfoCoalesce = null;
    void refreshSessionInfo();
  }, delayMs);
}

export function dismissRunError(): void {
  store.chat.value = { ...store.chat.value, runError: null };
}

export function toggleRail(): void {
  if (isNarrowViewport()) {
    store.mobileSheet.value = store.mobileSheet.value === 'sessions' ? null : 'sessions';
    return;
  }
  const next = !store.railCollapsed.value;
  store.railCollapsed.value = next;
  store.persistBool('rail', next);
}

export function toggleInspector(): void {
  if (isNarrowViewport()) {
    store.mobileSheet.value = store.mobileSheet.value === 'inspector' ? null : 'inspector';
    return;
  }
  const next = !store.inspectorCollapsed.value;
  store.inspectorCollapsed.value = next;
  store.persistBool('inspector', next);
}

export function closeSheets(): void {
  workbench.closeContext();
  store.mobileSheet.value = null;
}

export function selectInspectorTab(tab: store.InspectorTab): void {
  store.inspectorTab.value = tab;
  // A plugin tab reads its payload when it is opened, not before: most sessions
  // never look at one, and a poll would cost a plugin call for nothing.
  const pluginTab = store.pluginPanelTabs.value.find((panel) => panel.hosted_id === tab);
  if (pluginTab) void refreshPluginPanel(pluginTab);
}

/**
 * Read one plugin panel's payload.
 *
 * The route is scoped by project and session like every other project route, so
 * a workspace-scoped plugin is asked about the project the user is looking at
 * rather than about whichever project last ran a prompt.
 *
 * Concurrent calls for the same panel in the same context are coalesced: a poll
 * interval and a manual refresh can overlap without asking the plugin twice.
 * A changed project/session is a different read and gets its own in-flight entry.
 */
interface PluginPanelContext {
  projectId: string | null;
  sessionId: string | null;
}

interface PluginPanelRequest {
  run: Promise<void>;
  owner: object;
  epoch: number;
}

// The signal retains its hosted-id view for the mounted component. This backing
// cache and the in-flight table use the complete read scope, so two contexts can
// coexist without a slow A response becoming B's payload, error or loading state.
const pluginPanelCache = new Map<string, store.PluginPanelState>();
const pluginPanelInFlight = new Map<string, PluginPanelRequest>();
let pluginPanelEpoch = 0;

function pluginPanelKey(panel: store.PluginPanelTab, context: PluginPanelContext): string {
  // JSON tuple encoding avoids delimiter collisions in arbitrary project/session ids.
  return JSON.stringify([panel.hosted_id, context.projectId, context.sessionId]);
}

function publishPluginPanel(
  panel: store.PluginPanelTab,
  context: PluginPanelContext,
  key: string,
  state: store.PluginPanelState,
): void {
  pluginPanelCache.set(key, state);
  if (
    context.projectId !== store.currentProjectId.value ||
    context.sessionId !== store.currentSessionId.value
  ) {
    return;
  }
  store.pluginPanelData.value = {
    ...store.pluginPanelData.value,
    [panel.hosted_id]: state,
  };
}

export function refreshPluginPanel(panel: store.PluginPanelTab): Promise<void> {
  const context = {
    projectId: store.currentProjectId.value,
    sessionId: store.currentSessionId.value,
  };
  const key = pluginPanelKey(panel, context);
  const epoch = pluginPanelEpoch;
  const existing = pluginPanelInFlight.get(key);
  if (existing && existing.epoch === epoch) return existing.run;

  const cached = pluginPanelCache.get(key);
  let running = true;
  const owner = {};
  const run = (async () => {
    publishPluginPanel(panel, context, key, {
      loading: true,
      // Keep this context's last payload visible while refreshing. Crucially, do
      // not read the hosted-id signal: it may describe another project/session.
      data: cached?.data ?? null,
      title: cached?.title ?? null,
      fetchedAt: cached?.fetchedAt ?? null,
      error: null,
    });
    try {
      const result = await api.getPluginPanel(panel.plugin, panel.id, {
        project: context.projectId,
        session: context.sessionId,
      });
      if (epoch !== pluginPanelEpoch) return;
      publishPluginPanel(panel, context, key, {
        loading: false,
        data: result.data,
        title: result.title || panel.title,
        fetchedAt: result.fetched_ms || Date.now(),
        error: null,
      });
    } catch (error) {
      if (epoch !== pluginPanelEpoch) return;
      publishPluginPanel(panel, context, key, {
        loading: false,
        data: cached?.data ?? null,
        title: cached?.title ?? null,
        fetchedAt: cached?.fetchedAt ?? null,
        error: toErrorMessage(error),
      });
    } finally {
      running = false;
      const active = pluginPanelInFlight.get(key);
      if (active?.owner === owner) pluginPanelInFlight.delete(key);
    }
  })();
  // A synchronously failed request has already cleaned up and must not leave a
  // resolved promise joinable by a future refresh.
  if (running && epoch === pluginPanelEpoch) {
    pluginPanelInFlight.set(key, { run, owner, epoch });
  }
  return run;
}

/**
 * Drop cached payloads and invalidate every read, including requests restarted
 * afterwards in the same context. Owner checks prevent their old `finally` blocks
 * from deleting those replacement requests.
 */
export function resetPluginPanelData(): void {
  pluginPanelEpoch += 1;
  pluginPanelCache.clear();
  pluginPanelInFlight.clear();
  store.pluginPanelData.value = {};
}

export function openSettings(): void {
  replaceOverlay('settings');
}

export function closeSettings(): void {
  store.settingsOpen.value = false;
}

export function togglePalette(): void {
  if (store.paletteOpen.value) {
    store.paletteOpen.value = false;
    return;
  }
  replaceOverlay('palette');
}

export function closePalette(): void {
  store.paletteOpen.value = false;
}

/** Open the Theme Studio drawer, closing any other overlay first. */
export function openThemeStudio(): void {
  replaceOverlay('themeStudio');
}

export function closeThemeStudio(): void {
  store.themeStudioOpen.value = false;
}

export function toggleThemeStudio(): void {
  if (store.themeStudioOpen.value) closeThemeStudio();
  else openThemeStudio();
}

/**
 * Open the notification centre.
 *
 * Mutually exclusive with the other overlays for the same reason they are with each
 * other: two dialogs stacked is one too many, and the notification centre is exactly
 * the thing you open to leave the thing you were doing.
 */
export function openNotificationCenter(): void {
  replaceOverlay('notificationCenter');
}

export function closeNotificationCenter(): void {
  store.notificationCenterOpen.value = false;
}

export function toggleNotificationCenter(): void {
  if (store.notificationCenterOpen.value) closeNotificationCenter();
  else openNotificationCenter();
}

/**
 * Open the session a notification is about, and mark it read.
 *
 * The session may belong to another project, which is the whole point of the
 * centre: a run finished somewhere you are not looking.
 */
export async function openNotification(
  entry: Pick<store.NotificationEntry, 'id' | 'sessionId' | 'projectId'>,
): Promise<void> {
  store.markNotificationRead(entry.id);
  closeNotificationCenter();
  if (entry.sessionId === '') return;
  await openSessionInProject(entry.projectId, entry.sessionId);
}

/** Put text in the composer and focus it (used by the empty state's prompts). */
export function fillComposer(text: string): void {
  // The composer subscribes to `composerDraft` itself, so this lands even while
  // it is already mounted. The tick is a cheap second nudge: any listener that
  // only knows to "focus the composer" still fires, and it is idempotent.
  store.composerDraft.value = text;
  store.composerFocusTick.value += 1;
}

export function openFileViewer(path: string): void {
  replaceOverlay('fileViewer');
  store.fileViewerPath.value = path;
}

export function closeFileViewer(): void {
  store.fileViewerPath.value = null;
}

/**
 * Tell the workspace explorer that a file changed on disk from the UI itself, so
 * its cached listing (sizes, and a file the editor just created) catches up.
 */
export function noteWorkspaceFileChanged(_path: string): void {
  store.workspaceRefreshTick.value += 1;
}

export function switchSession(id: string): void {
  store.mobileSheet.value = null;
  store.paletteOpen.value = false;
  void openSession(id);
}

/**
 * Open a session that may live in another project.
 *
 * The store holds one project's sessions at a time, so opening a session from
 * elsewhere (the notification centre, the collapsed rail's fly-out) has to move the
 * project first — otherwise the rail would keep listing the old project while the
 * chat shows a session from the new one. The project switch already opens that
 * project's newest session, so this follows it with the one that was asked for.
 */
export async function openSessionInProject(
  projectId: string | null,
  sessionId: string,
): Promise<boolean> {
  if (projectId !== null && projectId !== store.currentProjectId.value) {
    if (controlPlaneOpen.value) controlPlaneOpen.value = false;
    await switchProject(projectId);
    // A later navigation may have won while this switch was awaiting its list.
    if (store.currentProjectId.value !== projectId) return false;
  }
  store.mobileSheet.value = null;
  store.paletteOpen.value = false;
  return openSession(sessionId);
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/** Open the rewind dialog for one transcript message. */
export function openRewind(target: store.RewindTarget): void {
  if (store.chat.value.running) {
    // A live run holds the conversation in memory; the server refuses the rewind
    // too, so say why here instead of letting the dialog offer something that
    // will come back a 409.
    store.pushToast('Stop the run before rewinding: the agent is still writing.', 'info');
    return;
  }
  if (store.currentSessionId.value === null) return;
  replaceOverlay('rewind');
  rewindGeneration += 1;
  store.rewindTarget.value = target;
}

export function closeRewind(): void {
  if (store.rewindTarget.value !== null) rewindGeneration += 1;
  store.rewindTarget.value = null;
}

/** What a rewind of the open target would do: the file list and the counts. */
export async function loadRewindPreview(): Promise<RewindPreview | null> {
  const sessionId = store.currentSessionId.value;
  const target = store.rewindTarget.value;
  if (!sessionId || !target) return null;
  const generation = rewindGeneration;
  const sessionRead = sessionGeneration;
  const projectRead = projectGeneration;
  const current = () =>
    generation === rewindGeneration &&
    sessionRead === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId &&
    store.rewindTarget.value === target;
  try {
    const response = await api.previewRewind(sessionId, target.messageId);
    return current() ? response.preview : null;
  } catch (error) {
    if (!current()) return null;
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
}

/**
 * Perform the rewind the dialog described.
 *
 * Returns true when the server applied it. A `retry` or `note` also starts a
 * run; the transcript is refetched either way, because hiding rows is not
 * something the client can reproduce (the server normalises the rewind point and
 * the journal's effects are not in the transcript).
 */
export async function applyRewind(options: {
  mode: RewindMode;
  revert: boolean;
  note?: string;
}): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const target = store.rewindTarget.value;
  if (!sessionId || !target) return false;
  const generation = rewindGeneration;
  const sessionRead = sessionGeneration;
  const projectRead = projectGeneration;
  const current = () =>
    generation === rewindGeneration &&
    sessionRead === sessionGeneration &&
    projectRead === projectGeneration &&
    store.currentSessionId.value === sessionId &&
    store.rewindTarget.value === target;
  const note = options.note?.trim() ?? '';
  if (options.mode === 'note' && note === '') {
    store.pushToast('Write the note you want the agent to act on.', 'info');
    return false;
  }

  try {
    const response = await api.rewindSession(sessionId, {
      message_id: target.messageId,
      revert: options.revert,
      mode: options.mode,
      ...(note !== '' ? { note } : {}),
    });
    if (!current()) return true;
    rewindGeneration += 1;
    store.rewindTarget.value = null;
    // The reload is what makes the hidden tail disappear; the server owns both
    // the cut and the normalisation. It runs only while this completion still
    // owns its target; an old apply must not navigate away from a later session.
    await openSession(sessionId, { force: true, silent: true });
    scheduleSessionsRefresh();
    // The files the revert could not put back are the interesting part of a
    // successful call: saying nothing would imply a clean revert.
    if (response.files_skipped > 0) {
      store.pushToast(
        `${response.files_restored} file(s) restored, ${response.files_skipped} could not be.`,
        'info',
      );
    } else if (response.files_restored > 0) {
      store.pushToast(
        `${response.files_restored} file(s) restored.`,
        'info',
      );
    } else if (options.mode !== 'none') {
      store.pushToast(
        options.mode === 'retry' ? 'Retrying that request.' : 'Sending your note.',
        'info',
      );
    }
    return true;
  } catch (error) {
    if (!current()) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 409) {
      store.pushToast('A run is still active in this session; stop it first.', 'info');
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  }
}

/** Wires the global 401 handler; called once from the app root. */
export function registerAuthHandler(): () => void {
  onUnauthorized(() => {
    enterAuthState();
    store.authError.value = null;
  });
  return () => onUnauthorized(null);
}

/** Test seam: forget module-level stream/refresh state without touching storage. */
export function resetRuntimeForTests(): void {
  stopStream();
  bootstrapPromise = null;
  replaceOverlay(null);
  // Invalidate, rather than reset to zero, so a deferred response from before a
  // test teardown cannot become current again in the next test.
  sessionGeneration += 1;
  projectGeneration += 1;
  agentDetailGeneration += 1;
  rewindGeneration += 1;
  resultPanelWidthWriteGeneration += 1;
  resultPanelWidthWrites.clear();
  sessionListReadGeneration += 1;
  agentListReadGeneration += 1;
  sendInFlight = false;
  refreshingSessionId = null;
  refreshQueue = [];
  resyncInFlight = false;
  resyncAgain = false;
  resyncFailures = 0;
  resetPluginPanelData();
  sessionInfoInFlight.clear();
  sessionInfoOwners.clear();
  if (sessionInfoCoalesce !== null) {
    window.clearTimeout(sessionInfoCoalesce);
    sessionInfoCoalesce = null;
  }
  if (gapResyncTimer !== null) {
    window.clearTimeout(gapResyncTimer);
    gapResyncTimer = null;
  }
  if (sessionsRefreshTimer !== null) {
    window.clearTimeout(sessionsRefreshTimer);
    sessionsRefreshTimer = null;
  }
}