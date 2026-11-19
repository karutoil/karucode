import { onUnauthorized, setAuthToken, toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { AccessSettingsSummary, OnboardingState } from '../../api/types';
import { getPasskeyAssertion, PasskeyError } from '../../lib/passkey';
import { syncProjectIcons } from '../projectIcons';
import * as store from '../store';
import * as workspaceTabs from '../workspaceTabs';
import { refreshAgents } from './control';
import { openStream, releaseBootstrapEvents, restartStream, stopStream } from './events';
import { openSession, refreshSessions } from './sessions';
import { checkHealth, refreshUpdateStatus } from './settings';
import {
  bootFailed,
  clearSelection,
  enterAuthState,
  isUnauthorized,
  pickSession,
  projectSessionsOnly,
  refreshAuthStatus,
  replaceOverlay,
  resetPluginPanelData,
  rt,
  type BootstrapEventBuffer,
} from './shared';

export { refreshAuthStatus };

/** Sign in with the password. Returns whether the session was established. */
export async function signInWithPassword(password: string): Promise<boolean> {
  if (store.authPending.value) return false;
  store.authError.value = null;
  const trimmed = password;
  if (trimmed === '') {
    store.authError.value = 'Enter the password for this server.';
    return false;
  }
  store.authPending.value = true;
  try {
    await api.login({ password: trimmed });
    // A cookie was set - and it is the credential from here on. Any stale bearer
    // token in local storage is dropped: it is not what just authenticated us, and
    // leaving it would send it on every request for no reason.
    setAuthToken(null);
    store.boot.value = 'loading';
    await bootstrap(true);
    // `bootstrap` no longer rejects (it lands on the error screen instead), so the
    // answer has to come from the state it left behind: reporting success here
    // would tell the caller the workspace is up when the boot screen is showing.
    return !bootFailed();
  } catch (error) {
    store.authError.value = toErrorMessage(error);
    return false;
  } finally {
    store.authPending.value = false;
  }
}

/** Sign in with a passkey. Returns whether the session was established. */
export async function signInWithPasskey(): Promise<boolean> {
  if (store.authPending.value) return false;
  store.authError.value = null;
  store.authPending.value = true;
  try {
    const options = await api.passkeyLoginOptions();
    const assertion = await getPasskeyAssertion(options);
    await api.passkeyLogin({ ...assertion, challenge_id: options.challenge_id });
    setAuthToken(null);
    store.boot.value = 'loading';
    await bootstrap(true);
    return !bootFailed();
  } catch (error) {
    store.authError.value =
      error instanceof PasskeyError ? error.message : toErrorMessage(error);
    return false;
  } finally {
    store.authPending.value = false;
  }
}

/** Sign this browser out and show the sign-in screen again. */
export async function signOut(): Promise<void> {
  try {
    await api.logout();
  } catch (error) {
    if (!isUnauthorized(error)) {
      store.pushToast(`Could not sign out: ${toErrorMessage(error)}`, 'error');
    }
  }
  setAuthToken(null);
  store.authStatus.value = null;
  store.config.value = null;
  store.sessions.value = [];
  store.currentSessionId.value = null;
  stopStream();
  enterAuthState();
}

/** The Access tab's payload (password state, passkeys, sessions, policy). */
export async function refreshAccessSettings(): Promise<AccessSettingsSummary | null> {
  try {
    const state = await api.getOnboarding();
    store.accessSettings.value = state.access as AccessSettingsSummary;
    return store.accessSettings.value;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return null;
    }
    return null;
  }
}

/**
 * Read the onboarding state, and commit it only if this run still owns the boot.
 *
 * `isCurrent` is passed in because the read is awaited: a superseded run must not
 * write `store.onboarding`, and must certainly not `enterAuthState()` for a 401
 * that belongs to a boot which has already been replaced.
 */
export async function refreshOnboarding(
  isCurrent: () => boolean = () => true,
): Promise<OnboardingState | null> {
  try {
    const state = await api.getOnboarding();
    if (!isCurrent()) return null;
    store.onboarding.value = state;
    return state;
  } catch (error) {
    if (!isCurrent()) return null;
    if (isUnauthorized(error)) {
      enterAuthState();
      return null;
    }
    return null;
  }
}

/**
 * Finish (or skip) the wizard.
 *
 * The marker is written on the server, so this is what makes it stay finished:
 * a reload, another browser and a restart all see `completed`.
 */
export async function finishOnboarding(skipped = false): Promise<void> {
  try {
    await api.completeOnboarding({ skipped });
    if (store.onboarding.value) {
      store.onboarding.value = { ...store.onboarding.value, completed: true, needed: false };
    }
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return;
    }
    store.pushToast(`Could not record setup: ${toErrorMessage(error)}`, 'error');
    return;
  }
  store.onboardingStep.value = 0;
  if (store.boot.value === 'onboarding') {
    store.boot.value = 'loading';
    await bootstrap(true);
  }
}

export async function bootstrap(force = false): Promise<void> {
  if (rt.bootstrapPromise && !force) return rt.bootstrapPromise;
  const buffer: BootstrapEventBuffer = { events: [], needsResync: false };
  rt.bootstrapEventBuffer = buffer;
  const run = (rt.bootstrapRun += 1);
  const isCurrent = (): boolean => run === rt.bootstrapRun;
  rt.bootstrapPromise = (async () => {
    store.boot.value = 'loading';
    store.bootError.value = null;
    store.sessionsLoading.value = true;
    replaceOverlay(null);
    try {
      // Subscribe before the first snapshot and hold its frames until every
      // snapshot has committed. Otherwise creation/notification frames between
      // the reads and startStream have no subscriber and are lost forever.
      openStream(true);
      const [health, config, projectList] = await Promise.all([
        api.getHealth(),
        api.getConfig(),
        api.listProjects(),
      ]);
      // Ownership is checked before *every* write, not once at the end: a boot
      // superseded while its reads were in flight must not leave its project
      // list and session list beside the newer run's chat.
      if (!isCurrent()) return;
      store.health.value = health;
      store.config.value = config;
      store.projects.value = projectList.projects;
      // Not awaited: this reaches the release host, and a slow or unreachable one
      // must not delay the first paint. The rail and Settings → Updates fill in
      // when it lands, and an older server answers 404 and is simply ignored.
      void refreshUpdateStatus();
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
      const { sessions: list, annotations } = await api.listSessionsWithAnnotations(
        project?.id ?? null,
      );
      if (!isCurrent()) return;
      // The same invariant as every other write: a boot with no project reads the
      // unscoped list, which contains control chats and every project's rows.
      store.sessions.value = projectSessionsOnly(list);
      store.sessionAnnotations.value = annotations;
      store.sessionsLoading.value = false;
      await refreshAgents();
      // A superseded boot must not overwrite the snapshot of the newer one.
      if (!isCurrent()) return;
      store.authError.value = null;
      store.connection.value = {
        api: 'online',
        stream: store.connection.value.stream,
        lastError: null,
        checkedAt: Date.now(),
      };
      const target = pickSession();
      // The wizard runs before the workspace does, and only once: the server
      // persists the completion marker, so an established install is never
      // interrupted by it.
      const onboardingState = await refreshOnboarding(isCurrent);
      if (!isCurrent()) return;
      if (onboardingState?.needed) {
        store.boot.value = 'onboarding';
        store.onboardingStep.value = 0;
        return;
      }
      store.boot.value = 'ready';
      if (target) await openSession(target.id);
      else clearSelection();
    } catch (error) {
      // A superseded boot stops at nothing: the newer run owns the stream and the
      // boot state, and stopping its stream here would strand it.
      if (!isCurrent()) return;
      stopStream();
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
    } finally {
      releaseBootstrapEvents(buffer);
    }
  })()
    .catch((error) => {
      // Whatever escapes the body above - a throw in the prologue, or anything the
      // release pass still could not contain - used to reject a promise nobody
      // awaits: `void bootstrap()` at startup, `await bootstrap(true)` inside the
      // sign-in paths (whose `catch` writes `authError`, which the skeleton never
      // renders), and the boot screen's Retry. The tab then sat on the loading
      // skeleton forever. Land on the error screen instead: it has the message and
      // the Retry button that re-runs this function.
      if (!isCurrent()) return;
      store.sessionsLoading.value = false;
      store.projectsLoading.value = false;
      store.boot.value = 'error';
      store.bootError.value = toErrorMessage(error);
      store.connection.value = {
        ...store.connection.value,
        api: 'offline',
        lastError: toErrorMessage(error),
        checkedAt: Date.now(),
      };
    })
    .finally(() => {
      // Only the newest run may clear the shared slot: an older boot settling later
      // must not make an in-flight one look like it is not running, or the next
      // `bootstrap()` would start a third boot instead of joining it.
      if (isCurrent()) rt.bootstrapPromise = null;
    });
  return rt.bootstrapPromise;
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
    // The token was accepted, but the boot after it can still fail: the boot
    // screen owns that failure, so do not report a workspace that is not there.
    return !bootFailed();
  } catch (error) {
    store.authError.value = isUnauthorized(error)
      ? 'That token was rejected by the server.'
      : toErrorMessage(error);
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
  rt.bootstrapPromise = null;
  rt.bootstrapEventBuffer = null;
  replaceOverlay(null);
  // Invalidate, rather than reset to zero, so a deferred response from before a
  // test teardown cannot become current again in the next test.
  rt.sessionGeneration += 1;
  rt.projectGeneration += 1;
  rt.navigationGeneration += 1;
  rt.healthGeneration += 1;
  rt.modelMutationGeneration += 1;
  rt.draftCreation = null;
  rt.modelMutationGenerations.clear();
  rt.modelMutationOwner = null;
  store.sessionModelMutationPending.value = null;
  rt.agentDetailGeneration += 1;
  rt.rewindGeneration += 1;
  rt.resultPanelWidthWriteGeneration += 1;
  rt.resultPanelWidthWrites.clear();
  rt.sessionListReadGeneration += 1;
  rt.agentListReadGeneration += 1;
  // A read whose response never arrives would otherwise block the next one for
  // the same project - which, in a shared module, means the next test.
  rt.agentsReadInFlight = null;
  rt.sendInFlight.clear();
  rt.refreshingSessionId = null;
  rt.refreshQueue = [];
  rt.resyncInFlight = false;
  rt.resyncAgain = false;
  rt.resyncFailures = 0;
  resetPluginPanelData();
  rt.sessionInfoInFlight.clear();
  rt.sessionInfoOwners.clear();
  if (rt.sessionInfoCoalesce !== null) {
    window.clearTimeout(rt.sessionInfoCoalesce);
    rt.sessionInfoCoalesce = null;
  }
  if (rt.storedIdCoalesce !== null) {
    window.clearTimeout(rt.storedIdCoalesce);
    rt.storedIdCoalesce = null;
  }
  if (rt.gapResyncTimer !== null) {
    window.clearTimeout(rt.gapResyncTimer);
    rt.gapResyncTimer = null;
  }
  if (rt.sessionsRefreshTimer !== null) {
    window.clearTimeout(rt.sessionsRefreshTimer);
    rt.sessionsRefreshTimer = null;
  }
  // Open file tabs and close guards are module state too: a test that opened one
  // must not leave it (or the selected view) for the next.
  workspaceTabs.resetWorkspaceTabs();
}
