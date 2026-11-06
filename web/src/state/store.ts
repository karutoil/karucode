import { computed, signal } from '@preact/signals';
import type {
  AgentAction,
  AgentStatus,
  AgentSummary,
  Config,
  ConnectionState,
  Health,
  NotifyKind,
  NotifyPayload,
  PluginPanelSummary,
  Project,
  ProviderProfile,
  QueuedAttachment,
  SessionInfo,
  SessionModelSelection,
  SessionSummary,
  Toast,
} from '../api/types';
import { isAgentLive } from '../api/types';
import { emptyChatState, type ChatState } from './chat';
import { controlChats as controlChatList } from './controlPlane';
import { agentActivity, pendingApprovals } from './selectors';
import type { AgentActivity } from './agentConsole';

/**
 * A built-in inspector tab. Plugin panels add more ids at runtime
 * (`plugin__<plugin>__<panel>`), so the active tab is typed as `string` and the
 * built-ins stay a closed union for the components that need one.
 */
export type BuiltinInspectorTab = 'workspace' | 'changes' | 'session' | 'agents';
export type InspectorTab = string;

/** One plugin panel advertised by `GET /api/config`, ready to render as a tab. */
export interface PluginPanelTab extends PluginPanelSummary {
  /** The plugin that owns it, kept alongside so the tab can name it. */
  plugin: string;
  /** The plugin's version, shown as a hint in the panel header. */
  version: string;
}

/** Fetch state for one panel payload. */
export interface PluginPanelState {
  loading: boolean;
  /** The payload, when the last read succeeded. */
  data: unknown;
  /** Title the plugin returned, which may differ from the manifest's. */
  title: string | null;
  /** Timestamp of the last successful read (`fetched_ms`). */
  fetchedAt: number | null;
  /** A readable failure, when the last read failed. */
  error: string | null;
}
export type MobileSheet = 'sessions' | 'inspector' | null;
export type BootState = 'loading' | 'ready' | 'auth' | 'error';

/** Agents are shown for the current session, or for the whole project. */
export type AgentScope = 'workspace' | 'session';
/** `all` keeps every status; the rest match `AgentStatus`. */
export type AgentFilter = 'all' | AgentStatus;

const RAIL_KEY = 'karucode.railCollapsed';
const INSPECTOR_KEY = 'karucode.inspectorCollapsed';
const LAST_SESSION_KEY = 'karucode.lastSession';
const LAST_PROJECT_KEY = 'karucode.lastProject';
/** Browser-side notification preferences. Server-side routing lives in config. */
const NOTIFY_SOUND_KEY = 'karucode.notify.sound';
const NOTIFY_VOLUME_KEY = 'karucode.notify.volume';
const NOTIFY_DESKTOP_KEY = 'karucode.notify.desktop';

function readBool(key: string, fallback: boolean): boolean {
  try {
    if (typeof localStorage === 'undefined') return fallback;
    const value = localStorage.getItem(key);
    if (value === null) return fallback;
    return value === 'true';
  } catch {
    return fallback;
  }
}

/** Read a number clamped to a range, falling back when the stored value is junk. */
function readNumber(key: string, fallback: number, min: number, max: number): number {
  try {
    if (typeof localStorage === 'undefined') return fallback;
    const value = localStorage.getItem(key);
    if (value === null) return fallback;
    const parsed = Number(value);
    if (!Number.isFinite(parsed)) return fallback;
    return Math.min(max, Math.max(min, parsed));
  } catch {
    return fallback;
  }
}

function writeString(key: string, value: string): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(key, value);
  } catch {
    // Storage may be disabled; the in-memory signal still works for this tab.
  }
}

export function persistBool(key: 'rail' | 'inspector', value: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(key === 'rail' ? RAIL_KEY : INSPECTOR_KEY, String(value));
    }
  } catch {
    // Storage may be disabled; the in-memory signal still works for this tab.
  }
}

export function readLastSessionId(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(LAST_SESSION_KEY);
  } catch {
    return null;
  }
}

export function writeLastSessionId(id: string | null): void {
  try {
    if (typeof localStorage === 'undefined') return;
    if (id) localStorage.setItem(LAST_SESSION_KEY, id);
    else localStorage.removeItem(LAST_SESSION_KEY);
  } catch {
    // Ignore storage failures.
  }
}

export function readLastProjectId(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(LAST_PROJECT_KEY);
  } catch {
    return null;
  }
}

export function writeLastProjectId(id: string | null): void {
  try {
    if (typeof localStorage === 'undefined') return;
    if (id) localStorage.setItem(LAST_PROJECT_KEY, id);
    else localStorage.removeItem(LAST_PROJECT_KEY);
  } catch {
    // Ignore storage failures.
  }
}

export const boot = signal<BootState>('loading');
export const bootError = signal<string | null>(null);
export const health = signal<Health | null>(null);
export const config = signal<Config | null>(null);
export const authError = signal<string | null>(null);

export const sessions = signal<SessionSummary[]>([]);
export const sessionsLoading = signal(true);
export const currentSessionId = signal<string | null>(null);

/** Projects (workspaces) and the one the rail is currently showing. */
export const projects = signal<Project[]>([]);
export const projectsLoading = signal(true);
export const currentProjectId = signal<string | null>(readLastProjectId());
export const projectSheetOpen = signal(false);
/**
 * Object URLs for the icon each project ships (`favicon.ico`), keyed by project
 * id. A project with no icon is simply absent, and the rail draws its initials.
 *
 * The bytes come from `GET /api/projects/:id/favicon` through the API client, so
 * they carry the bearer token; an `<img src>` could not. The URLs are owned by
 * `state/projectIcons.ts`, which revokes them when they are replaced.
 */
export const projectIcons = signal<Record<string, string>>({});
export const chat = signal<ChatState>(emptyChatState());
export const sessionHistory = signal<{ id: string; messages: import('../api/types').StoredMessage[]; before: number | null; truncated: boolean; loading: boolean; error: string | null } | null>(null);
export const agentHistory = signal<{ id: string; messages: import('../api/types').StoredMessage[]; before: number | null; truncated: boolean; loading: boolean; error: string | null } | null>(null);
export const chatLoading = signal(false);
export const chatError = signal<string | null>(null);

export const connection = signal<{
  api: 'unknown' | 'online' | 'offline';
  stream: ConnectionState;
  lastError: string | null;
  checkedAt: number | null;
}>({ api: 'unknown', stream: 'idle', lastError: null, checkedAt: null });

export const narrowViewport = signal<boolean>(
  typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia('(max-width: 1023px)').matches
    : false,
);

export const railCollapsed = signal(readBool(RAIL_KEY, false));
export const inspectorCollapsed = signal(readBool(INSPECTOR_KEY, false));
export const mobileSheet = signal<MobileSheet>(null);
export const inspectorTab = signal<InspectorTab>('workspace');
/**
 * One entry per plugin panel the server advertises, flattened from
 * `GET /api/config`'s `plugin_panels`. Empty when no plugin contributes a panel,
 * which is the normal case.
 *
 * Derived rather than assigned: every place that refreshes `config` (boot,
 * settings save, a plugin enable/disable) then updates the tabs for free, and
 * there is no second copy of the truth to keep in step.
 */
export const pluginPanelTabs = computed<PluginPanelTab[]>(() => {
  const groups = config.value?.plugin_panels ?? [];
  const tabs: PluginPanelTab[] = [];
  for (const group of groups) {
    for (const panel of group.panels ?? []) {
      // The id lands in a DOM id and a URL path segment, so a malformed one is
      // dropped rather than rendered as a broken tab.
      if (!panel?.hosted_id || !panel.hosted_id.startsWith('plugin__')) continue;
      tabs.push({ ...panel, plugin: group.plugin, version: group.version });
    }
  }
  return tabs;
});
/**
 * Payloads for the currently mounted panel context, keyed by hosted id.
 * `actions.refreshPluginPanel` owns the backing cache, whose keys also include
 * project and session so another context's response cannot populate this view.
 */
export const pluginPanelData = signal<Record<string, PluginPanelState>>({});
export const settingsOpen = signal(false);
export const paletteOpen = signal(false);
/**
 * The message a rewind dialog is open for.
 *
 * `messageId` is what the reader clicked, which may be an assistant turn: the
 * server normalises it to the prompt that produced it, and the dialog shows the
 * resolved prompt rather than the click target so what gets replaced is explicit.
 */
export interface RewindTarget {
  /** The message the reader asked to rewind to. */
  messageId: number;
  /** The visible content of that message, for the dialog's heading. */
  preview: string;
  /** True when the clicked message is the agent's answer, not a prompt. */
  isAssistant: boolean;
}

/**
 * The message a rewind dialog is open for, if any.
 *
 * The dialog is a separate surface from the transcript because the choice it
 * asks for (keep or revert the files, retry or write a note) has to be made with
 * the preview's file list in front of the reader.
 */
export const rewindTarget = signal<RewindTarget | null>(null);
/** The Theme Studio drawer (right side). Mutually exclusive with other overlays. */
export const themeStudioOpen = signal(false);
/** Text another surface asked the composer to take (empty-state quick prompts). */
export const composerDraft = signal<string | null>(null);

/**
 * A message the user recalled, handed back to the composer.
 *
 * Separate from `composerDraft` because a recall restores the *form*, not just
 * text: the attachment chips come back too, and the draft is appended to what the
 * user has typed since rather than replacing it. `token` makes each hand-off
 * unique, so recalling the same message twice in a row is still two events.
 */
export interface ComposerHandoff {
  text: string;
  attachments: QueuedAttachment[];
  token: number;
}
export const composerHandoff = signal<ComposerHandoff | null>(null);
/** Queued ids with a recall in flight, so a double click cannot fire twice. */
export const recallingQueued = signal<Record<string, boolean>>({});
export const fileViewerPath = signal<string | null>(null);
/** Bumped when the UI itself writes a workspace file, so the explorer refetches. */
export const workspaceRefreshTick = signal(0);
export const toasts = signal<Toast[]>([]);

/**
 * One notification the server sent, kept so it can be read later.
 *
 * The toast disappears, the desktop notification is gone once dismissed, and an
 * unread dot on a row says *that* something happened without saying what. This is
 * the record behind the notification centre: what arrived, for which session and
 * project, and whether it has been read.
 */
export interface NotificationEntry {
  /** The server's id (`run:<id>`, `approval:<id>`, `plugin:<name>:<title>`). */
  id: string;
  kind: NotifyKind;
  title: string;
  body: string;
  /** The session it is about; `''` for a notice that names none. */
  sessionId: string;
  /** The session's title at emit time, so a row reads on its own. */
  sessionTitle: string;
  /** The project that session belongs to, when the server said. */
  projectId: string | null;
  /** `runtime`, or the plugin that raised it. */
  source: string;
  /** Milliseconds since the epoch, in this browser's clock, when it arrived. */
  at: number;
  read: boolean;
}

/** How many notifications the centre keeps. */
export const NOTIFICATION_LIMIT = 100;
const NOTIFICATION_LOG_KEY = 'karucode.notifications';
/**
 * Entries older than this are dropped when the log is loaded.
 *
 * The list is a convenience, not an archive: a week is longer than anyone scrolls,
 * and an unbounded file in `localStorage` is a slow leak.
 */
const NOTIFICATION_MAX_AGE_MS = 7 * 86_400_000;

const NOTIFY_KINDS: readonly NotifyKind[] = ['run_finished', 'needs_input', 'error', 'info'];

function isNotificationKind(value: unknown): value is NotifyKind {
  return typeof value === 'string' && (NOTIFY_KINDS as readonly string[]).includes(value);
}

/**
 * Read the persisted log, keeping only entries that still make sense.
 *
 * Everything here is untrusted: it comes back from `localStorage`, which another
 * version of the app (or a person with the dev tools open) may have written. A row
 * that cannot be read is dropped rather than rendered as `undefined`.
 */
export function readNotificationLog(): NotificationEntry[] {
  try {
    if (typeof localStorage === 'undefined') return [];
    const raw = localStorage.getItem(NOTIFICATION_LOG_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const cutoff = Date.now() - NOTIFICATION_MAX_AGE_MS;
    const entries: NotificationEntry[] = [];
    for (const candidate of parsed) {
      if (!candidate || typeof candidate !== 'object') continue;
      const row = candidate as Record<string, unknown>;
      if (typeof row.id !== 'string' || typeof row.title !== 'string') continue;
      if (!isNotificationKind(row.kind)) continue;
      const at = typeof row.at === 'number' && Number.isFinite(row.at) ? row.at : 0;
      if (at < cutoff) continue;
      entries.push({
        id: row.id,
        kind: row.kind,
        title: row.title,
        body: typeof row.body === 'string' ? row.body : '',
        sessionId: typeof row.sessionId === 'string' ? row.sessionId : '',
        sessionTitle: typeof row.sessionTitle === 'string' ? row.sessionTitle : '',
        projectId: typeof row.projectId === 'string' && row.projectId !== '' ? row.projectId : null,
        source: typeof row.source === 'string' ? row.source : '',
        at,
        read: row.read === true,
      });
      if (entries.length >= NOTIFICATION_LIMIT) break;
    }
    return entries;
  } catch {
    // Malformed JSON, a denied storage read: an empty list is the honest answer.
    return [];
  }
}

/**
 * Every notification this browser has seen, newest first.
 *
 * In memory and in `localStorage`: the server does not keep them (a notification
 * describes live state), so this is the only copy, and losing it on every reload
 * would make the centre useless exactly when a run finished while the tab was
 * closed.
 */
export const notifications = signal<NotificationEntry[]>(readNotificationLog());
/** The notification centre (a dialog) is open. */
export const notificationCenterOpen = signal(false);

function persistNotifications(list: NotificationEntry[]): void {
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(NOTIFICATION_LOG_KEY, JSON.stringify(list));
  } catch {
    // Storage disabled or full: the list still works for this tab.
  }
}

function setNotifications(next: NotificationEntry[]): void {
  notifications.value = next.slice(0, NOTIFICATION_LIMIT);
  persistNotifications(notifications.value);
}

/**
 * Record one notification, newest first, deduped by the server's id.
 *
 * `read` is true when the notification arrives about something already in front of
 * the reader: the toast was seen, so it must not also raise a badge. The entry is
 * still kept — the centre is a history, not a queue of things to dismiss.
 */
export function recordNotification(payload: NotifyPayload, read: boolean): void {
  if (typeof payload.id !== 'string' || payload.id === '') return;
  if (notifications.value.some((entry) => entry.id === payload.id)) return;
  const entry: NotificationEntry = {
    id: payload.id,
    kind: isNotificationKind(payload.kind) ? payload.kind : 'info',
    title: typeof payload.title === 'string' ? payload.title : '',
    body: typeof payload.body === 'string' ? payload.body : '',
    sessionId: typeof payload.session_id === 'string' ? payload.session_id : '',
    sessionTitle: typeof payload.session_title === 'string' ? payload.session_title : '',
    projectId: typeof payload.project_id === 'string' && payload.project_id !== '' ? payload.project_id : null,
    source: typeof payload.source === 'string' ? payload.source : '',
    at: Date.now(),
    read,
  };
  setNotifications([entry, ...notifications.value]);
}

/** Mark one notification read. */
export function markNotificationRead(id: string): void {
  if (!notifications.value.some((entry) => entry.id === id && !entry.read)) return;
  setNotifications(
    notifications.value.map((entry) => (entry.id === id ? { ...entry, read: true } : entry)),
  );
}

/** Mark everything read: the centre's "Mark all read" button. */
export function markAllNotificationsRead(): void {
  if (!notifications.value.some((entry) => !entry.read)) return;
  setNotifications(notifications.value.map((entry) => ({ ...entry, read: true })));
}

/** Forget the history. The unread counts go with it, since they are derived. */
export function clearNotifications(): void {
  if (notifications.value.length === 0) return;
  setNotifications([]);
}

/**
 * Drop one session's notifications (the session was deleted).
 *
 * Distinct from marking them read: a deleted session is not going to be opened, so
 * keeping the row would leave a permanent unread badge with nothing behind it.
 */
export function forgetSessionNotifications(sessionId: string): void {
  if (sessionId === '') return;
  if (!notifications.value.some((entry) => entry.sessionId === sessionId)) return;
  setNotifications(notifications.value.filter((entry) => entry.sessionId !== sessionId));
}

/**
 * Unread counts per session, derived from the log.
 *
 * A session rail row shows a dot for a nonzero count. This is the surface that
 * works when the desktop notification does not: it needs no permission, it survives
 * a flash of the tab, and it is the only thing visible from another tab (through the
 * title counter below). Derived rather than counted separately so the badge, the
 * project counter and the notification centre can never disagree.
 */
export const unread = computed<Record<string, number>>(() => {
  const counts: Record<string, number> = {};
  for (const entry of notifications.value) {
    if (entry.read || entry.sessionId === '') continue;
    counts[entry.sessionId] = (counts[entry.sessionId] ?? 0) + 1;
  }
  return counts;
});

/**
 * Unread notifications per project, for the counts on the project rail's tiles.
 *
 * Keyed by project id and independent of which project is open: the server reports
 * every terminal run, and a run in a project you are not looking at is exactly the
 * case a badge is for. A notification the server could not attribute to a project
 * (a plugin notice about no session) counts in `unreadTotal` only.
 */
export const unreadByProject = computed<Record<string, number>>(() => {
  const counts: Record<string, number> = {};
  for (const entry of notifications.value) {
    if (entry.read || entry.projectId === null) continue;
    counts[entry.projectId] = (counts[entry.projectId] ?? 0) + 1;
  }
  return counts;
});

/** Total unread across every session, for the document title and the bell. */
export const unreadTotal = computed<number>(
  () => notifications.value.reduce((sum, entry) => sum + (entry.read ? 0 : 1), 0),
);

/**
 * Whether the user wants sound. A browser preference, not a server setting: it
 * describes *this* device, and the server has no business knowing.
 */
export const soundEnabled = signal(readBool(NOTIFY_SOUND_KEY, true));
/** Cue volume, 0..1. */
export const soundVolume = signal(readNumber(NOTIFY_VOLUME_KEY, 0.8, 0, 1));
/**
 * Whether desktop notifications are wanted. Separate from the browser permission:
 * permission says it is *possible*, this says it is *wanted*. Both must be true.
 */
export const desktopEnabled = signal(readBool(NOTIFY_DESKTOP_KEY, false));

export function persistSound(enabled: boolean): void {
  soundEnabled.value = enabled;
  writeString(NOTIFY_SOUND_KEY, String(enabled));
}

/** Re-read the persisted preferences, for a change made in another tab. */
export function readSoundEnabled(): boolean {
  return readBool(NOTIFY_SOUND_KEY, true);
}

export function readSoundVolume(): number {
  return readNumber(NOTIFY_VOLUME_KEY, 0.8, 0, 1);
}

export function readDesktopEnabled(): boolean {
  return readBool(NOTIFY_DESKTOP_KEY, false);
}

export function persistVolume(volume: number): void {
  const clamped = Math.min(1, Math.max(0, volume));
  soundVolume.value = clamped;
  writeString(NOTIFY_VOLUME_KEY, String(clamped));
}

export function persistDesktopEnabled(enabled: boolean): void {
  desktopEnabled.value = enabled;
  writeString(NOTIFY_DESKTOP_KEY, String(enabled));
}

/**
 * Mark one session's notifications read, because it was opened.
 *
 * This is what "reading" means: the badge clears when the session is in front of
 * the reader, not after a timer. The entry stays in the centre, marked read.
 */
export function clearUnread(sessionId: string | null): void {
  if (!sessionId) return;
  if (!notifications.value.some((entry) => entry.sessionId === sessionId && !entry.read)) return;
  setNotifications(
    notifications.value.map((entry) =>
      entry.sessionId === sessionId && !entry.read ? { ...entry, read: true } : entry,
    ),
  );
}

/** Mark one project's notifications read (the tile's menu does this). */
export function markProjectNotificationsRead(projectId: string): void {
  if (!projectId) return;
  if (
    !notifications.value.some((entry) => entry.projectId === projectId && !entry.read)
  ) {
    return;
  }
  setNotifications(
    notifications.value.map((entry) =>
      entry.projectId === projectId && !entry.read ? { ...entry, read: true } : entry,
    ),
  );
}

export const resolvingApprovals = signal<Record<string, boolean>>({});
export const stopping = signal(false);
export const clock = signal(Date.now());
export const composerFocusTick = signal(0);
/** Bumped when the user sends a message, so the transcript re-pins to the newest content. */
export const sentMessageTick = signal(0);
export const streamEpoch = signal(0);

let toastSequence = 0;
let toastTimers = new Map<number, number>();

export function pushToast(
  message: string,
  tone: Toast['tone'] = 'error',
  ttlMs = 7000,
  sessionId?: string,
): number {
  const id = (toastSequence += 1);
  toasts.value = [...toasts.value, { id, tone, message, ...(sessionId ? { sessionId } : {}) }];
  if (typeof window !== 'undefined' && ttlMs > 0) {
    const timer = window.setTimeout(() => removeToast(id), ttlMs);
    toastTimers.set(id, timer);
  }
  return id;
}

export function removeToast(id: number): void {
  toasts.value = toasts.value.filter((toast) => toast.id !== id);
  const timer = toastTimers.get(id);
  if (timer !== undefined) {
    window.clearTimeout(timer);
    toastTimers.delete(id);
  }
}

export function resetToastTimers(): void {
  for (const timer of toastTimers.values()) window.clearTimeout(timer);
  toastTimers = new Map();
  toasts.value = [];
}

/** Every agent in the current project, newest first (workspace-wide view). */
export const agentsAll = signal<AgentSummary[]>([]);
export const agentsLoading = signal(false);
export const agentsError = signal<string | null>(null);
export const agentScope = signal<AgentScope>('session');
export const agentFilter = signal<AgentFilter>('all');
/**
 * Which control call is in flight per agent id.
 *
 * A pause request is not instant (it lands at the agent's next iteration), so the
 * button has to show that something is happening and refuse a second click. Keyed
 * by agent so pausing one does not disable the rest.
 */
export const agentControlPending = signal<Record<string, AgentAction>>({});
/** The agent whose transcript is open in the Agents tab, if any. */
export const selectedAgentId = signal<string | null>(null);
/**
 * Live per-agent transcripts, keyed by agent id. Subagent text and tool events
 * are dropped from the chat reducer (they belong to another conversation) and
 * accumulated here instead.
 */
export const agentActivities = signal<Record<string, AgentActivity>>({});
export const agentDetailLoading = signal(false);
export const agentDetailError = signal<string | null>(null);

/**
 * Subagents alive anywhere in the current project (the inspector tab's badge).
 *
 * "Alive" is `running` **or** `paused`: a paused agent still holds a subagent
 * slot and still exists as a loop, so the server counts it in its own `running`
 * tally (`routes/agents.rs:80-89`, "a paused agent still holds a subagent slot")
 * and `isAgentLive` is the shared definition. Counting only `running` here made
 * the badge disagree with the server and with the rows underneath it, which
 * already sort and label a paused agent as live.
 */
export const runningAgents = computed(
  () => agentsAll.value.filter((agent) => isAgentLive(agent.status)).length,
);

/**
 * The open session's counts and token totals, for the inspector's Session tab.
 *
 * Fetched from `GET /api/sessions/:id/info` rather than derived from `chat`:
 * the transcript is a bounded window, and the token rows are persisted server
 * side precisely because a `usage` event is live-only. `null` means "not read
 * yet" - a different state from "a session with nothing in it", which is why the
 * tab draws a loading line instead of a page of zeros.
 */
export const sessionInfo = signal<SessionInfo | null>(null);
/** The session `sessionInfo` describes, so a stale read for another one is dropped. */
export const sessionInfoFor = signal<string | null>(null);
export const sessionInfoLoading = signal(false);
export const sessionInfoError = signal<string | null>(null);
/** Bumped by a run/usage event, so an open tab refreshes without polling. */
export const sessionInfoTick = signal(0);

/**
 * The open session when it is a control chat, otherwise `null`.
 *
 * A control chat is not inside a project: `sessions.value` only ever holds the
 * active project's rows, so this reads the control plane's own chat list
 * (`state/controlPlane.ts`). Keeping the lookup here means every consumer - the
 * composer's model pills, the chat header, the transcript - resolves "which
 * session am I in" the same way, whether it lives in a project or beside them.
 */
export const activeControlChat = computed<SessionSummary | null>(() => {
  const id = currentSessionId.value;
  if (id === null) return null;
  return controlChatList().find((session) => session.id === id) ?? null;
});

export const activeSession = computed<SessionSummary | null>(
  () => sessions.value.find((session) => session.id === currentSessionId.value)
    ?? activeControlChat.value,
);


export const currentProject = computed<Project | null>(() => {
  const list = projects.value;
  const id = currentProjectId.value;
  return list.find((project) => project.id === id) ?? list[0] ?? null;
});

/** The active provider profile (falls back to the first one). */
export const activeProvider = computed<ProviderProfile | null>(() => {
  const current = config.value;
  if (!current) return null;
  const id = current.active?.provider;
  return current.providers.find((profile) => profile.id === id) ?? current.providers[0] ?? null;
});

/** `provider / model` for the chat header, or null when nothing is selected. */
export const activeModelLabel = computed<string | null>(() => {
  const current = config.value;
  const provider = activeProvider.value;
  if (!current || !provider) return null;
  const modelId = current.active?.model ?? provider.models[0]?.id ?? current.model.model;
  const entry = provider.models.find((model) => model.id === modelId);
  return entry?.name?.trim() ? entry.name : modelId;
});

/**
 * The open session's own model pin, remembered outside every session list.
 *
 * The composer's model and reasoning pills must never appear and disappear:
 * they are the control a user reaches for mid-sentence, and a control that
 * blinks out because a list snapshot has not caught up yet is worse than no
 * control at all. A control chat is not in `sessions.value` and a project-less
 * session loses its row on every session-list refresh, so the pin cannot live
 * in a list row.
 *
 * Three states, not two, because "we have not read it yet" and "the server says
 * this session has no pin" are different answers and only one of them may be
 * shown as a fact:
 *
 * * `null` - no session is open;
 * * `{sessionId, model: undefined}` - that session is open and its pin is being
 *   read, so the pill falls back to the install default without claiming a pin;
 * * `{sessionId, model: Some | null}` - what the server said (a `null` model is
 *   "this session follows the default").
 *
 * Overloading "not read" with "no pin" made the pill change value on every
 * control-chat open: it showed the default, then flipped to the real pin when
 * the read landed.
 */
export const openSessionModel = signal<{
  sessionId: string;
  model: SessionModelSelection | null | undefined;
} | null>(null);

/**
 * Record the open session's model pin.
 *
 * `sessionId === null` closes the record (nothing is open). `model === undefined`
 * records the session whose pin is still being read; `null` is the server's
 * answer "no pin, follow the install default".
 */
export function rememberSessionModel(
  sessionId: string | null,
  model: SessionModelSelection | null | undefined,
): void {
  if (sessionId === null) {
    openSessionModel.value = null;
    return;
  }
  openSessionModel.value = { sessionId, model };
}

/** The model a run of the open session would use right now. */
export interface ModelSelection {
  providerId: string | null;
  modelId: string | null;
  /** True when the session carries its own pin instead of following the default. */
  pinned: boolean;
  /** The pinned reasoning effort (`null` = endpoint default). */
  effort: string | null;
}

/**
 * The session's model pin, falling back to the install-wide default.
 *
 * One implementation, shared by the composer's model switcher (which displays
 * and edits it) and the context pill (which needs the same model's window).
 * Two copies of this resolution is how the pill ends up reporting against a
 * different model from the one named in the switcher beside it.
 *
 * The open session's own pin (`openSessionModel`) wins over a list row: the
 * pills must not depend on the session happening to be in `sessions.value`
 * right now, which is not true for a control chat and is briefly false for
 * every session on every list refresh. `pinned` is only true when a pin is
 * actually known - the pill must never claim "this session" on a guess.
 */
export const modelSelection = computed<ModelSelection>(() => {
  const current = config.value;
  const open = openSessionModel.value;
  // The open session's own record comes first, because it is the only source
  // that survives the session having no row in any list (a control chat) or a
  // list refresh that dropped the row. `undefined` is "not read yet" and falls
  // through to the row; `null` is the server's "no pin" and is a fact, so it
  // must not be replaced by a row that may be stale.
  const recorded = open && open.sessionId === currentSessionId.value ? open.model : undefined;
  const pin = recorded !== undefined ? recorded : activeSession.value?.model ?? null;
  if (pin) {
    return {
      providerId: pin.provider,
      modelId: pin.model,
      pinned: true,
      effort: pin.reasoning_effort ?? null,
    };
  }
  return {
    providerId: current?.active.provider ?? current?.providers[0]?.id ?? null,
    modelId: current?.active.model ?? current?.providers[0]?.models[0]?.id ?? null,
    pinned: false,
    effort: null,
  };
});

/** The selected model's curated entry, when the providers declare one. */
export const selectedModelEntry = computed(() => {
  const current = config.value;
  const { providerId, modelId } = modelSelection.value;
  const provider = current?.providers.find((profile) => profile.id === providerId) ?? null;
  return provider?.models.find((model) => model.id === modelId) ?? null;
});

/**
 * The context window the open session's model declares, in tokens.
 *
 * `null` when the entry does not say — a hand-added model with no
 * `context_window` — which is what makes the composer's context pill hide
 * instead of drawing a percentage against a made-up denominator.
 */
export const activeContextWindow = computed<number | null>(() => {
  const window = selectedModelEntry.value?.context_window;
  return typeof window === 'number' && Number.isFinite(window) && window > 0 ? window : null;
});

export const providerConfigured = computed<boolean>(() => {
  const current = health.value;
  if (current) return current.provider_configured;
  const model = config.value?.model;
  return Boolean(model && model.base_url.trim() !== '' && model.model.trim() !== '');
});

export const activity = computed(() => agentActivity(chat.value));
export const pendingApprovalList = computed(() => pendingApprovals(chat.value));

export const composerDisabledReason = computed<string | null>(() => {
  if (!currentSessionId.value) return 'Create or select a session to start.';
  if (!providerConfigured.value) return 'No model provider configured. Add one in Settings.';
  return null;
});

export const connectionLabel = computed<string>(() => {
  const state = connection.value;
  if (state.api === 'offline') return 'Disconnected';
  if (state.stream === 'unauthorized') return 'Auth required';
  if (state.stream === 'blocked') return 'Blocked';
  if (state.stream === 'reconnecting') return 'Reconnecting';
  if (state.stream === 'connecting') return 'Connecting';
  if (state.stream === 'open') return 'Live';
  if (state.api === 'online') return 'Connected';
  return 'Unknown';
});