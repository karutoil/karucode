import { computed, signal } from '@preact/signals';
import type {
  AgentStatus,
  AgentSummary,
  Config,
  ConnectionState,
  Health,
  PluginPanelSummary,
  Project,
  ProviderProfile,
  SessionSummary,
  Toast,
} from '../api/types';
import { emptyChatState, type ChatState } from './chat';
import { agentActivity, pendingApprovals } from './selectors';
import type { AgentActivity } from './agentConsole';

/**
 * A built-in inspector tab. Plugin panels add more ids at runtime
 * (`plugin__<plugin>__<panel>`), so the active tab is typed as `string` and the
 * built-ins stay a closed union for the components that need one.
 */
export type BuiltinInspectorTab = 'workspace' | 'changes' | 'commands' | 'agents';
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
/** The payload of each panel that has been read, keyed by hosted id. */
export const pluginPanelData = signal<Record<string, PluginPanelState>>({});
export const settingsOpen = signal(false);
export const paletteOpen = signal(false);
/** The Theme Studio drawer (right side). Mutually exclusive with other overlays. */
export const themeStudioOpen = signal(false);
/** Text another surface asked the composer to take (empty-state quick prompts). */
export const composerDraft = signal<string | null>(null);
export const fileViewerPath = signal<string | null>(null);
/** Bumped when the UI itself writes a workspace file, so the explorer refetches. */
export const workspaceRefreshTick = signal(0);
export const toasts = signal<Toast[]>([]);
export const resolvingApprovals = signal<Record<string, boolean>>({});
export const stopping = signal(false);
export const clock = signal(Date.now());
export const composerFocusTick = signal(0);
/** Bumped when the user sends a message, so the transcript re-pins to the newest content. */
export const sentMessageTick = signal(0);
export const streamEpoch = signal(0);

let toastSequence = 0;
let toastTimers = new Map<number, number>();

export function pushToast(message: string, tone: Toast['tone'] = 'error', ttlMs = 7000): number {
  const id = (toastSequence += 1);
  toasts.value = [...toasts.value, { id, tone, message }];
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
export const agentScope = signal<AgentScope>('workspace');
export const agentFilter = signal<AgentFilter>('all');
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

/** Subagents running anywhere in the current project (the tab's badge). */
export const runningAgents = computed(
  () => agentsAll.value.filter((agent) => agent.status === 'running').length,
);

export const activeSession = computed<SessionSummary | null>(
  () => sessions.value.find((session) => session.id === currentSessionId.value) ?? null,
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
 */
export const modelSelection = computed<ModelSelection>(() => {
  const current = config.value;
  const pin = activeSession.value?.model ?? null;
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