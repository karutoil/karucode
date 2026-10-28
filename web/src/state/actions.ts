import { ApiError, getAuthToken, onUnauthorized, setAuthToken, toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import { startEventStream, type EventStreamHandle } from '../api/events';
import type {
  ApprovalDecision,
  Config,
  ConfigPatch,
  CatalogResponse,
  KarucodeEvent,
  Project,
  ProviderTestRequest,
  ProviderTestResult,
  RawEvent,
  Session,
  SessionSummary,
} from '../api/types';
import { isNarrowViewport } from '../lib/dom';
import { emptyChatState, seedChatState, type MessageItem } from './chat';
import { applyEventToChat, applyEventToSessions, markUserRun } from './eventReducer';
import { summaryFromDetail } from './selectors';
import * as store from './store';

let stream: EventStreamHandle | null = null;
let bootstrapPromise: Promise<void> | null = null;
let sessionGeneration = 0;
let refreshingSessionId: string | null = null;
let refreshQueue: KarucodeEvent[] = [];
let sessionsRefreshTimer: number | null = null;
let localMessageSequence = 0;
let sendInFlight = false;

function enterAuthState(): void {
  store.boot.value = 'auth';
  store.settingsOpen.value = false;
  store.paletteOpen.value = false;
  store.fileViewerPath.value = null;
  store.mobileSheet.value = null;
}

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.isUnauthorized;
}

function mergeSummary(summary: SessionSummary): void {
  const list = store.sessions.value;
  const index = list.findIndex((session) => session.id === summary.id);
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
  store.currentSessionId.value = null;
  store.chat.value = emptyChatState(null);
  store.chatLoading.value = false;
  store.chatError.value = null;
  store.writeLastSessionId(null);
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
      if (id) void openSession(id, { silent: true });
    },
  });
}

export function stopStream(): void {
  stream?.stop();
  stream = null;
}

export function restartStream(): void {
  if (store.boot.value === 'ready') startStream();
}

function handleRawEvent(raw: RawEvent): void {
  // The payload shape is validated by parseEventPayload; unknown `type` values
  // are ignored by the reducers.
  const event = raw as unknown as KarucodeEvent;

  const wasKnown = store.sessions.value.some((session) => session.id === event.session_id);
  store.sessions.value = applyEventToSessions(store.sessions.value, event);

  if (event.type === 'session_created') {
    if (!wasKnown && !store.currentSessionId.value) {
      void openSession(event.session_id);
    }
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

  if (event.type === 'session_renamed') {
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
  store.chat.value = applyEventToChat(store.chat.value, event);
}

function drainRefreshQueue(): void {
  const queued = refreshQueue;
  refreshQueue = [];
  refreshingSessionId = null;
  for (const event of queued) applyChatEvent(event);
}

async function doRefreshSessions(): Promise<void> {
  try {
    const list = await api.listSessions(store.currentProjectId.value);
    store.sessions.value = list;
    store.sessionsLoading.value = false;
    const current = store.currentSessionId.value;
    if (current && !list.some((session) => session.id === current)) {
      const next = list[0] ?? null;
      if (next) void openSession(next.id);
      else clearSelection();
    }
  } catch (error) {
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
    store.settingsOpen.value = false;
    store.paletteOpen.value = false;
    store.fileViewerPath.value = null;
    store.mobileSheet.value = null;
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
}

export async function openSession(id: string, options: OpenSessionOptions = {}): Promise<void> {
  const silent = options.silent ?? false;
  if (id === store.currentSessionId.value && !silent && !options.force) return;

  const generation = (sessionGeneration += 1);
  store.currentSessionId.value = id;
  store.writeLastSessionId(id);
  store.chatError.value = null;
  if (!silent) {
    store.chatLoading.value = true;
    store.chat.value = emptyChatState(id);
  }

  refreshingSessionId = id;
  refreshQueue = [];

  try {
    const detail = await api.getSession(id);
    if (generation !== sessionGeneration) return;
    store.chat.value = seedChatState(detail);
    mergeSummary(summaryFromDetail(detail));
    store.chatLoading.value = false;
    store.chatError.value = null;
  } catch (error) {
    if (generation !== sessionGeneration) return;
    if (isUnauthorized(error)) {
      enterAuthState();
      store.chatLoading.value = false;
      return;
    }
    if (error instanceof ApiError && error.status === 404) {
      store.sessions.value = store.sessions.value.filter((session) => session.id !== id);
      store.chatLoading.value = false;
      store.writeLastSessionId(null);
      const next = store.sessions.value[0] ?? null;
      store.pushToast('That session no longer exists.', 'info');
      if (next) await openSession(next.id);
      else clearSelection();
      return;
    }
    store.chatLoading.value = false;
    if (!silent) {
      store.chat.value = emptyChatState(id);
      store.chatError.value = toErrorMessage(error);
      store.pushToast(toErrorMessage(error));
    }
  } finally {
    if (generation === sessionGeneration) {
      drainRefreshQueue();
    }
  }
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
    void openSession(sessionId, { silent: true });
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
  if (id) await openSession(id, { silent: true });
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

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export async function refreshProjects(): Promise<void> {
  try {
    const result = await api.listProjects();
    store.projects.value = result.projects;
    store.projectsLoading.value = false;
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
  // Invalidate any in-flight chat/session read from the previous project.
  sessionGeneration += 1;
  store.currentProjectId.value = id;
  store.writeLastProjectId(id);
  store.projectSheetOpen.value = false;
  store.mobileSheet.value = null;
  store.inspectorTab.value = 'workspace';
  store.sessions.value = [];
  store.sessionsLoading.value = true;
  store.fileViewerPath.value = null;
  clearSelection();
  await refreshSessions();
  const next = pickSession();
  if (next) await openSession(next.id);
}

export async function createProject(name: string, path: string): Promise<Project | null> {
  try {
    const project = await api.createProject(path, name.trim() || undefined);
    store.projects.value = [...store.projects.value, project];
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

export async function deleteProjectById(id: string, force = false): Promise<boolean> {
  try {
    const result = await api.deleteProject(id, force);
    const remaining = store.projects.value.filter((project) => project.id !== id);
    store.projects.value = remaining;
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
  if (!sessionId) return;
  try {
    const detail = await api.getSession(sessionId);
    if (store.currentSessionId.value === sessionId) {
      store.chat.value = { ...store.chat.value, agents: detail.agents };
    }
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
  }
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
  store.mobileSheet.value = null;
}

export function selectInspectorTab(tab: store.InspectorTab): void {
  store.inspectorTab.value = tab;
}

export function openSettings(): void {
  store.fileViewerPath.value = null;
  store.paletteOpen.value = false;
  store.mobileSheet.value = null;
  store.settingsOpen.value = true;
}

export function closeSettings(): void {
  store.settingsOpen.value = false;
}

export function togglePalette(): void {
  if (store.paletteOpen.value) {
    store.paletteOpen.value = false;
    return;
  }
  store.settingsOpen.value = false;
  store.fileViewerPath.value = null;
  store.mobileSheet.value = null;
  store.paletteOpen.value = true;
}

export function closePalette(): void {
  store.paletteOpen.value = false;
}

export function openFileViewer(path: string): void {
  store.mobileSheet.value = null;
  store.fileViewerPath.value = path;
}

export function closeFileViewer(): void {
  store.fileViewerPath.value = null;
}

export function switchSession(id: string): void {
  store.mobileSheet.value = null;
  store.paletteOpen.value = false;
  void openSession(id);
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
  sessionGeneration = 0;
  sendInFlight = false;
  refreshingSessionId = null;
  refreshQueue = [];
  if (sessionsRefreshTimer !== null) {
    window.clearTimeout(sessionsRefreshTimer);
    sessionsRefreshTimer = null;
  }
}