import { getAuthToken } from '../../api/client';
import { startSharedEventStream } from '../../api/sharedStream';
import type { KarucodeEvent, NotifyPayload, RawEvent } from '../../api/types';
import * as agentConsole from '../agentConsole';
import { applyEventToSessions } from '../eventReducer';
import * as notifyWiring from '../notifyWiring';
import * as store from '../store';
import {
  applyChatEvent,
  openSession,
  refreshSessions,
  scheduleSessionsRefresh,
  scheduleStoredMessageIds,
} from './sessions';
import {
  clearSelection,
  enterAuthState,
  RESYNC_BACKOFF_MS,
  RESYNC_RETRIES,
  rt,
  type BootstrapEventBuffer,
} from './shared';

function receiveRawEvent(event: RawEvent): void {
  if (rt.bootstrapEventBuffer) {
    rt.bootstrapEventBuffer.events.push(event);
    return;
  }
  handleRawEvent(event);
}

function requestStreamResync(): void {
  if (rt.bootstrapEventBuffer) {
    rt.bootstrapEventBuffer.needsResync = true;
    return;
  }
  scheduleGapResync();
}

export function openStream(duringBootstrap: boolean): void {
  stopStream();
  if (!duringBootstrap && store.boot.value !== 'ready') return;
  let opens = 0;
  rt.stream = startSharedEventStream({
    token: getAuthToken(),
    onEvent: receiveRawEvent,
    // Normal reconnects resume from Last-Event-ID. A remaining seq hole, an
    // explicit lag marker or a resync control still requires an authoritative read.
    onGap: requestStreamResync,
    onResync: requestStreamResync,
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
      // Bootstrap owns the first authoritative reads. Reconnect opens during it
      // must not launch a second, unordered snapshot either.
      if (opens === 1 || rt.bootstrapEventBuffer) return;
      void refreshSessions();
      const id = store.currentSessionId.value;
      if (id) void openSession(id, { silent: true, preserveLive: true });
    },
  });
}

export function startStream(): void {
  openStream(false);
}

export function stopStream(): void {
  rt.stream?.stop();
  rt.stream = null;
}

/** Refetch what the stream can no longer tell us: the session list and transcript. */
async function resyncFromServer(): Promise<void> {
  // One refetch at a time; a burst of holes must not fan out into N of them.
  if (rt.resyncInFlight) {
    rt.resyncAgain = true;
    return;
  }
  rt.resyncInFlight = true;
  let repaired = false;
  try {
    await refreshSessions();
    // `doRefreshSessions` swallows its own failure into the connection state.
    if (store.connection.value.api !== 'offline') {
      const id = store.currentSessionId.value;
      repaired = id === null || (await openSession(id, { silent: true, preserveLive: true }));
    }
  } finally {
    rt.resyncInFlight = false;
    if (repaired) {
      rt.resyncFailures = 0;
    } else {
      // A failed repair would leave a stale transcript on screen for good, which
      // is the bug this whole path exists to kill: retry a few times, then say so
      // once instead of silently giving up.
      rt.resyncFailures += 1;
      if (rt.resyncFailures <= RESYNC_RETRIES) {
        scheduleGapResync(RESYNC_BACKOFF_MS * 2 ** (rt.resyncFailures - 1));
      } else if (rt.resyncFailures === RESYNC_RETRIES + 1) {
        store.pushToast('Could not resynchronise this session; reload the page if it looks stale.');
      }
    }
    if (rt.resyncAgain) {
      rt.resyncAgain = false;
      scheduleGapResync();
    }
  }
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
  if (rt.gapResyncTimer !== null) return;
  rt.gapResyncTimer = window.setTimeout(() => {
    rt.gapResyncTimer = null;
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
    store.forgetSessionNotifications(event.session_id);
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

  // The rows of the run that just ended were built from events and carry no
  // stored ids, so their rewind controls cannot resolve a message. Reading them
  // back is what a page refresh used to be needed for.
  if (
    (event.type === 'run_finished' || event.type === 'run_cancelled') &&
    !event.agent_id &&
    event.session_id === store.currentSessionId.value
  ) {
    scheduleStoredMessageIds();
  }

  if (event.session_id && event.session_id !== store.currentSessionId.value) return;

  if (rt.refreshingSessionId !== null && event.session_id === rt.refreshingSessionId) {
    rt.refreshQueue.push(event);
    return;
  }

  applyChatEvent(event);
}

export function releaseBootstrapEvents(buffer: BootstrapEventBuffer): void {
  if (rt.bootstrapEventBuffer !== buffer) {
    // A superseded run reaching its `finally`: the newer run owns the gate now.
    // Its own frames are still worth keeping - they were real events - so hand
    // them to the owner, and ask for a resync when there are any, because the
    // newer run's snapshot was taken before these arrived. Dropping them silently
    // was the older behaviour, and nothing scheduled the resync that would have
    // repaired the hole.
    const owner = rt.bootstrapEventBuffer;
    if (owner === null) return;
    if (buffer.events.length > 0) {
      owner.events.push(...buffer.events);
      owner.needsResync = true;
    }
    return;
  }
  // Open the live gate before replaying: a handler is then indistinguishable from
  // an event that arrives immediately after this synchronous release pass.
  rt.bootstrapEventBuffer = null;
  let dropped = false;
  for (const event of buffer.events) {
    try {
      handleRawEvent(event);
    } catch (error) {
      // This runs in `bootstrap`'s `finally`, so a single frame shape the reducers
      // do not defend against (an `approval_requested` with no approval, a field
      // renamed by a newer server) used to throw out of the release pass and
      // reject the whole boot: the app stayed on the skeleton with no message and
      // no Retry. Skip that one frame, keep the rest, and let the resync below
      // repair what it dropped.
      dropped = true;
      console.error('karucode: a buffered event could not be applied during boot', error);
    }
  }
  if (buffer.needsResync || dropped) scheduleGapResync(0);
}

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

/**
 * Accumulate one agent-scoped frame into the workspace-wide agent view.
 *
 * Lifecycle events update the registry; text/tool/approval events update that
 * agent's transcript only. The chat reducer deliberately drops subagent text and
 * tool frames (they belong to another conversation), so this is where they land.
 */
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
  // `recordAgentActivity` (not a spread) is what bounds the map: a tab that runs
  // subagent after subagent for a day used to hold every finished feed forever.
  store.agentActivities.value = agentConsole.recordAgentActivity(
    store.agentActivities.value,
    agentId,
    next,
  );
}
