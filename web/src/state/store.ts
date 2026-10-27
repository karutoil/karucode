import { computed, signal } from '@preact/signals';
import type { Config, ConnectionState, Health, SessionSummary, Toast } from '../api/types';
import { emptyChatState, type ChatState } from './chat';
import { agentActivity, pendingApprovals } from './selectors';

export type InspectorTab = 'workspace' | 'changes' | 'commands' | 'agents';
export type MobileSheet = 'sessions' | 'inspector' | null;
export type BootState = 'loading' | 'ready' | 'auth' | 'error';

const RAIL_KEY = 'karucode.railCollapsed';
const INSPECTOR_KEY = 'karucode.inspectorCollapsed';
const LAST_SESSION_KEY = 'karucode.lastSession';

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

export const boot = signal<BootState>('loading');
export const bootError = signal<string | null>(null);
export const health = signal<Health | null>(null);
export const config = signal<Config | null>(null);
export const authError = signal<string | null>(null);

export const sessions = signal<SessionSummary[]>([]);
export const sessionsLoading = signal(true);
export const currentSessionId = signal<string | null>(null);
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
export const settingsOpen = signal(false);
export const paletteOpen = signal(false);
export const fileViewerPath = signal<string | null>(null);
export const toasts = signal<Toast[]>([]);
export const resolvingApprovals = signal<Record<string, boolean>>({});
export const stopping = signal(false);
export const clock = signal(Date.now());
export const composerFocusTick = signal(0);
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

export const activeSession = computed<SessionSummary | null>(
  () => sessions.value.find((session) => session.id === currentSessionId.value) ?? null,
);

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
  if (state.stream === 'reconnecting') return 'Reconnecting';
  if (state.stream === 'connecting') return 'Connecting';
  if (state.stream === 'open') return 'Live';
  if (state.api === 'online') return 'Connected';
  return 'Unknown';
});