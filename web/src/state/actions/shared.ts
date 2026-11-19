import { ApiError } from '../../api/client';
import * as api from '../../api/endpoints';
import type { EventStreamHandle } from '../../api/events';
import type {
  AuthStatus,
  KarucodeEvent,
  RawEvent,
  Session,
  SessionSummary,
} from '../../api/types';
import { emptyChatState } from '../chat';
import * as railPeek from '../railPeek';
import * as store from '../store';
import * as workbench from '../workbench';

export interface BootstrapEventBuffer {
  events: RawEvent[];
  needsResync: boolean;
}

export type ReplaceableOverlay =
  | 'settings'
  | 'palette'
  | 'themeStudio'
  | 'notificationCenter'
  | 'fileViewer'
  | 'rewind';

export interface PluginPanelContext {
  projectId: string | null;
  sessionId: string | null;
}

export interface PluginPanelRequest {
  run: Promise<void>;
  owner: object;
  epoch: number;
}

/** How often a failed refetch retries before it says so once and stops. */
export const RESYNC_RETRIES = 3;
export const RESYNC_BACKOFF_MS = 1_000;

/**
 * Shared mutable runtime state for the actions modules.
 *
 * Kept in one object so domain modules stay a clean DAG without ESM live-binding
 * reassignment limits, and `resetRuntimeForTests` can reset every counter, queue,
 * map and timer in one place.
 */
export const rt = {
  stream: null as EventStreamHandle | null,
  bootstrapPromise: null as Promise<void> | null,
  bootstrapEventBuffer: null as BootstrapEventBuffer | null,
  /**
   * Which forced boot is allowed to decide the outcome.
   *
   * `Retry`, a sign-in and the startup call can overlap: the boot screen's Retry
   * starts a fresh run while the first one is still in flight. Only the newest run
   * may write `ready`/`error`/`onboarding`, otherwise the run that finishes last
   * wins even when it started first - a stale failure would paint the error screen
   * over a workspace that has already loaded, and a stale success would hide a
   * real one.
   */
  bootstrapRun: 0,
  authStatusInFlight: null as Promise<AuthStatus | null> | null,
  sessionGeneration: 0,
  projectGeneration: 0,
  /** User navigation intents, including time spent waiting for a project switch. */
  navigationGeneration: 0,
  healthGeneration: 0,
  // Independent read generations handle overlapping reads even within one project;
  // projectGeneration handles the more dangerous A -> B -> A stale-response case.
  sessionListReadGeneration: 0,
  agentListReadGeneration: 0,
  /** The project whose agent list is being read, so two reads cannot overlap. */
  agentsReadInFlight: null as string | null,
  agentDetailGeneration: 0,
  refreshingSessionId: null as string | null,
  refreshQueue: [] as KarucodeEvent[],
  sessionsRefreshTimer: null as number | null,
  gapResyncTimer: null as number | null,
  resyncInFlight: false,
  resyncAgain: false,
  resyncFailures: 0,
  localMessageSequence: 0,
  sendInFlight: new Set<string>(),
  modelMutationGeneration: 0,
  modelMutationGenerations: new Map<string, number>(),
  modelMutationOwner: null as { sessionId: string; generation: number } | null,
  /** Queued ids with a recall request in flight; module-scoped so a remount cannot double-fire. */
  recallingQueued: new Set<string>(),
  handoffToken: 0,
  rewindGeneration: 0,
  resultPanelWidthWriteGeneration: 0,
  resultPanelWidthWrites: new Map<string, number>(),
  /** Client-side identity for a draft; it never reaches the server. */
  draftSequence: 0,
  /** The create currently on the wire for a draft; see `createDraftSession`. */
  draftCreation: null as { draftId: number; promise: Promise<string | null> } | null,
  /**
   * One read in flight **per session-generation/project-generation**.
   *
   * Keyed, not a single flag: session B must never join A's URL or in-flight promise,
   * and revisiting the same session id after switching projects must not join its
   * old generation either. Owners keep one read's `finally` from clearing another's.
   */
  sessionInfoInFlight: new Map<string, Promise<void>>(),
  sessionInfoOwners: new Map<string, object>(),
  sessionInfoCoalesce: null as number | null,
  storedIdCoalesce: null as number | null,
  // The signal retains its hosted-id view for the mounted component. This backing
  // cache and the in-flight table use the complete read scope, so two contexts can
  // coexist without a slow A response becoming B's payload, error or loading state.
  pluginPanelCache: new Map<string, store.PluginPanelState>(),
  pluginPanelInFlight: new Map<string, PluginPanelRequest>(),
  pluginPanelEpoch: 0,
  /** Kept so two rapid clicks on one contributed button run one request. */
  pluginUiInFlight: new Set<string>(),
};

export function nextHandoffToken(): number {
  rt.handoffToken += 1;
  return rt.handoffToken;
}

/**
 * Replace the application's modal/drawer surface in one place.
 *
 * Keeping this exhaustive prevents newer surfaces (the notification centre and
 * rewind dialog) from being accidentally stackable with Ctrl/Cmd+K, Settings or
 * Theme Studio. Closing rewind also invalidates its in-flight preview/apply read,
 * which must not clear a later target or navigate back to an old session.
 */
export function replaceOverlay(next: ReplaceableOverlay | null): void {
  store.settingsOpen.value = next === 'settings';
  store.paletteOpen.value = next === 'palette';
  store.themeStudioOpen.value = next === 'themeStudio';
  store.notificationCenterOpen.value = next === 'notificationCenter';
  store.fileViewerPath.value = null;
  store.projectSheetOpen.value = false;
  if (next !== 'rewind' && store.rewindTarget.value !== null) rt.rewindGeneration += 1;
  if (next !== 'rewind') store.rewindTarget.value = null;
  store.mobileSheet.value = null;
  workbench.closeContext();
  // The rail's fly-out is a hover affordance, not a modal surface.
  railPeek.closePeek();
}

export function enterAuthState(): void {
  store.boot.value = 'auth';
  replaceOverlay(null);
  // The sign-in screen needs to know *how* to sign in (password? passkey? token?).
  // Asking on entry keeps a token-only deployment showing the token field and a
  // password deployment showing its form, without guessing.
  void refreshAuthStatus();
}

/**
 * Fetch `GET /api/auth/status` into the store.
 *
 * Public on the server, so this is safe to call while signed out - and it is the
 * only way the screen learns which methods exist. Concurrent callers share one
 * request: the sign-in state is asked for on boot, on a 401 and by the screen
 * itself, and three identical round trips would only make the screen flicker.
 */
export function refreshAuthStatus(): Promise<AuthStatus | null> {
  if (rt.authStatusInFlight) return rt.authStatusInFlight;
  rt.authStatusInFlight = (async () => {
    try {
      const status = await api.getAuthStatus();
      store.authStatus.value = status;
      return status;
    } catch {
      // A server old enough not to have the route, or a transient failure: the
      // screen falls back to the token form, which every version supports.
      store.authStatus.value = null;
      return null;
    } finally {
      rt.authStatusInFlight = null;
    }
  })();
  return rt.authStatusInFlight;
}

/**
 * Whether the workspace gave up.
 *
 * A function rather than a direct read because TypeScript narrows
 * `store.boot.value` to `'loading'` after the assignment above it and keeps that
 * narrowing across the `await`, which would make the post-boot check below a
 * comparison the compiler thinks cannot happen.
 */
export function bootFailed(): boolean {
  return store.boot.value === 'error';
}

export function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.isUnauthorized;
}

/**
 * Drop cached payloads and invalidate every read, including requests restarted
 * afterwards in the same context. Owner checks prevent their old `finally` blocks
 * from deleting those replacement requests.
 */
export function resetPluginPanelData(): void {
  rt.pluginPanelEpoch += 1;
  rt.pluginPanelCache.clear();
  rt.pluginPanelInFlight.clear();
  store.pluginPanelData.value = {};
}

/**
 * The rows the workbench rail may hold: the open project's sessions, and nothing
 * else.
 *
 * `sessions.value` is read unfiltered by the rail, the palette and the collapsed
 * rail's fly-out, so its meaning has to be enforced where it is written rather
 * than at each reader. Two kinds of row do not belong in it: a **control chat**
 * (the plane's own list, project-less by design) and a session from **another
 * project** (the client holds one project at a time; a foreign row makes the rail
 * disagree with the workspace the file tools and terminals are pointed at). An
 * unscoped `/api/sessions` read - what happens while no project is open - returns
 * both, which is exactly how a plane conversation used to surface in the
 * workbench list until the next project switch replaced it.
 *
 * A project-less *ordinary* session stays listed: it is not a control chat, and
 * the server attaches those rows to a project on its next start. The one open is
 * also kept by `doRefreshSessions`, which asks the global list before closing it.
 */
export function projectSessionsOnly(list: SessionSummary[]): SessionSummary[] {
  const project = store.currentProjectId.value;
  return list.filter(
    (session) =>
      session.control !== true &&
      (project === null || session.project_id === null || session.project_id === project),
  );
}

export function mergeSummary(summary: SessionSummary): void {
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
  if (index === -1 && (summary.control || summary.project_id === null)) return;
  if (index === -1) {
    // Only this project's rows may be inserted. `openSession` moves the project
    // first (see `adoptProjectForSession`); a summary that still names another
    // project is stale relative to the store and is not this list's work.
    if (summary.project_id !== store.currentProjectId.value) return;
    store.sessions.value = [summary, ...list];
    return;
  }
  const next = [...list];
  next[index] = summary;
  store.sessions.value = next;
}

export function summaryFromSession(session: Session): SessionSummary {
  return { ...session, message_count: 0, last_run_status: null, running: false };
}

export function clearSelection(): void {
  if (store.currentSessionId.value !== null) resetPluginPanelData();
  store.currentSessionId.value = null;
  // Nothing is open, so there is no session whose pin the pills may show - and no
  // draft either: every caller of this is saying "the thing that was here is
  // gone", which includes the unsent conversation.
  store.draftSession.value = null;
  store.draftToSession.value = null;
  store.rememberSessionModel(null, null);
  store.chat.value = emptyChatState(null);
  store.chatLoading.value = false;
  store.chatError.value = null;
  // Nothing is open, so no event can be waiting for a snapshot of it any more.
  rt.refreshingSessionId = null;
  rt.refreshQueue = [];
  store.sessionInfo.value = null;
  store.sessionInfoFor.value = null;
  store.sessionInfoError.value = null;
  store.sessionInfoLoading.value = false;
  store.writeLastSessionId(null);
}

export function pickSession(): SessionSummary | null {
  const list = store.sessions.value;
  const last = store.readLastSessionId();
  return list.find((session) => session.id === last) ?? list[0] ?? null;
}
