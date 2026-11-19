import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { AgentSummary, Project } from '../../api/types';
import { controlPlaneOpen } from '../controlPlane';
import { syncProjectIcons } from '../projectIcons';
import * as store from '../store';
import * as workspaceTabs from '../workspaceTabs';
import { closeAgentDetail, refreshAgents } from './control';
import { openSession, refreshSessions, scheduleSessionsRefresh } from './sessions';
import {
  clearSelection,
  enterAuthState,
  isUnauthorized,
  mergeSummary,
  pickSession,
  resetPluginPanelData,
  rt,
  summaryFromSession,
} from './shared';

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
export interface ProjectSwitchOptions {
  navigationOwner?: number;
  /** A targeted open waits and selects its own session after the project read. */
  openLatest?: boolean;
}

export async function switchProject(id: string, options: ProjectSwitchOptions = {}): Promise<void> {
  await switchProjectInternal(id, options);
}

async function switchProjectInternal(id: string, options: ProjectSwitchOptions = {}): Promise<void> {
  if (options.navigationOwner !== undefined && options.navigationOwner !== rt.navigationGeneration) {
    return;
  }
  if (store.currentProjectId.value === id) {
    store.projectSheetOpen.value = false;
    return;
  }
  if (options.navigationOwner === undefined) rt.navigationGeneration += 1;
  const navigationOwner = options.navigationOwner ?? rt.navigationGeneration;
  // Invalidate in-flight chat/session/agent reads before changing the project.
  const generation = ++rt.projectGeneration;
  rt.sessionGeneration += 1;
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
  // File tabs belong to the project whose workspace they read: the same relative
  // path in another project is a different file, so the strip starts empty.
  workspaceTabs.resetWorkspaceTabs();
  clearSelection();
  // The agent list is project-scoped, and the rows above were just dropped, so it
  // is read again here rather than waiting for a session to change. A refresh
  // that the session read below also asks for is dropped as a duplicate.
  void refreshAgents();
  await refreshSessions();
  if (generation !== rt.projectGeneration || store.currentProjectId.value !== id) return;
  // A later create, close or manual selection owns navigation now. The switch's
  // old automatic selection must not overwrite it after this await.
  if (navigationOwner !== rt.navigationGeneration) return;
  if (options.openLatest === false) return;
  const next = pickSession();
  if (next) await openSession(next.id, { navigationOwner });
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
  const generation = ++rt.resultPanelWidthWriteGeneration;
  rt.resultPanelWidthWrites.set(id, generation);
  try {
    const saved = await api.updateProjectResultPanelWidth(id, width);
    if (rt.resultPanelWidthWrites.get(id) === generation) {
      store.projects.value = store.projects.value.map((candidate) =>
        candidate.id === id ? { ...candidate, ...saved } : candidate,
      );
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else if (rt.resultPanelWidthWrites.get(id) === generation) {
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
  const navigationOwner = ++rt.navigationGeneration;
  if (projectId !== null && projectId !== store.currentProjectId.value) {
    if (controlPlaneOpen.value) controlPlaneOpen.value = false;
    await switchProjectInternal(projectId, { navigationOwner, openLatest: false });
    // A later navigation may have won while this switch was awaiting its list,
    // including another session selected in this same destination project.
    if (
      navigationOwner !== rt.navigationGeneration ||
      store.currentProjectId.value !== projectId
    ) {
      return false;
    }
  }
  if (navigationOwner !== rt.navigationGeneration) return false;
  store.mobileSheet.value = null;
  store.paletteOpen.value = false;
  return openSession(sessionId, { navigationOwner });
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
    const resolvedProject =
      projectId === undefined
        ? store.notifications.value.find((entry) => entry.sessionId === sessionId)?.projectId ?? null
        : projectId;
    store.clearUnread(sessionId);
    void openSessionInProject(resolvedProject, sessionId);
  }
}

/** Jump from an agent to the session it belongs to (switching projects if needed). */
export async function openAgentSession(agent: AgentSummary): Promise<void> {
  await openSessionInProject(agent.project_id ?? null, agent.session_id);
  // The shared helper owns the whole project/session navigation. Do not close a
  // newer agent detail either if this old request lost that navigation race.
  if (store.currentSessionId.value === agent.session_id) closeAgentDetail();
}

/**
 * Create a new karucode session in `projectId`, immediately send `prompt`, and
 * optionally focus it in the Conversation view. Used by the Source Control tab
 * to launch sessions for one or all GitHub issues / pull requests.
 */
export async function launchSessionWithPrompt(options: {
  title: string;
  prompt: string;
  projectId?: string | null;
  focus?: boolean;
}): Promise<{ sessionId: string; title: string } | null> {
  const targetProject =
    options.projectId === undefined ? store.currentProjectId.value : options.projectId;
  const trimmedPrompt = options.prompt.trim();
  if (trimmedPrompt === '') return null;
  try {
    const session = await api.createSession(options.title, targetProject ?? undefined);
    mergeSummary(summaryFromSession(session));
    await api.sendMessage(session.id, trimmedPrompt);
    scheduleSessionsRefresh();
    if (options.focus !== false) {
      store.mobileSheet.value = null;
      workspaceTabs.selectWorkspaceView('chat');
      await openSessionInProject(targetProject ?? null, session.id);
    }
    return { sessionId: session.id, title: session.title };
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error), 'error');
    return null;
  }
}
