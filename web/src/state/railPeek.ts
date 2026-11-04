/**
 * Hovering a project while the session list is collapsed peeks at its sessions.
 *
 * `Ctrl/Cmd+B` hides the sessions rail by giving it zero width, which is the right
 * default for a wide conversation but a dead end when all you want is to open a
 * session or rename one: the only way back was to restore the rail, do the thing,
 * and collapse it again. So hovering a project's tile while the rail is collapsed
 * shows that project's sessions as a fly-out — the same list, over the chat instead
 * of beside it, gone as soon as the pointer leaves.
 *
 * Three rules keep it from becoming a nuisance:
 *
 * 1. **Only when the rail is collapsed, on a wide viewport, with no overlay open.**
 *    A peek that fights a dialog for the same space is worse than none, and on a
 *    phone the rail is already a sheet.
 * 2. **Hovering a project shows *that* project.** Hovering another project costs one
 *    bounded read of its session list (cached for a few seconds, coalesced), because
 *    the client only holds the open project's sessions.
 * 3. **Leaving closes it after a short delay.** The fly-out sits directly against the
 *    project rail, so there is no gap to cross, and the delay is only there so a
 *    pointer that clips the edge on its way past does not flash the list.
 *
 * Nothing here is durable state: it is a view, and closing it costs nothing.
 */

import { signal } from '@preact/signals';
import type { SessionSummary } from '../api/types';
import * as api from '../api/endpoints';
import { controlPlaneOpen } from './controlPlane';
import * as store from './store';

/** The project whose sessions are showing, or `null` when nothing is peeked. */
export const peekProjectId = signal<string | null>(null);
/** Session lists read for other projects, keyed by project id. */
export const peekSessions = signal<Record<string, SessionSummary[]>>({});
export const peekLoading = signal<Record<string, boolean>>({});
export const peekError = signal<Record<string, string | null>>({});

/** How long the pointer may be off the rail before the fly-out closes. */
export const PEEK_CLOSE_MS = 260;
/** How long a read of another project's sessions is trusted. */
const PEEK_TTL_MS = 15_000;

let closeTimer: number | null = null;
const fetchedAt: Record<string, number> = {};
const inFlight = new Map<string, Promise<void>>();

function now(): number {
  return Date.now();
}

/** Whether a hover should peek at all, in this window, right now. */
export function peekEnabled(): boolean {
  if (store.narrowViewport.value) return false;
  if (!store.railCollapsed.value) return false;
  if (controlPlaneOpen.value) return false;
  return !(
    store.settingsOpen.value ||
    store.paletteOpen.value ||
    store.themeStudioOpen.value ||
    store.notificationCenterOpen.value ||
    store.rewindTarget.value !== null ||
    store.fileViewerPath.value !== null
  );
}

function cancelClose(): void {
  if (closeTimer !== null) {
    window.clearTimeout(closeTimer);
    closeTimer = null;
  }
}

/** Show one project's sessions. A no-op when a peek does not apply right now. */
export function openPeek(projectId: string): void {
  if (projectId === '') return;
  cancelClose();
  if (!peekEnabled()) return;
  peekProjectId.value = projectId;
  // The open project's list is already in the store and stays fresh; every other
  // project is read once, then cached briefly.
  if (projectId !== store.currentProjectId.value) void loadPeekSessions(projectId);
}

/** Keep it open: the pointer is inside the fly-out. */
export function holdPeek(): void {
  cancelClose();
}

/** Close after the grace period, unless the pointer comes back first. */
export function schedulePeekClose(delayMs = PEEK_CLOSE_MS): void {
  cancelClose();
  if (typeof window === 'undefined') {
    peekProjectId.value = null;
    return;
  }
  closeTimer = window.setTimeout(() => {
    closeTimer = null;
    peekProjectId.value = null;
  }, delayMs);
}

/** Close immediately (a session was opened, Escape was pressed, the rail came back). */
export function closePeek(): void {
  cancelClose();
  peekProjectId.value = null;
}

/**
 * Read one project's sessions for the fly-out, at most once per TTL.
 *
 * Failures are kept rather than swallowed: an empty list would read as "this
 * project has no sessions", which is a different and wrong answer.
 */
export function loadPeekSessions(projectId: string, force = false): Promise<void> {
  const existing = inFlight.get(projectId);
  if (existing && !force) return existing;
  const last = fetchedAt[projectId] ?? 0;
  if (!force && last !== 0 && now() - last < PEEK_TTL_MS) return Promise.resolve();

  peekLoading.value = { ...peekLoading.value, [projectId]: true };
  peekError.value = { ...peekError.value, [projectId]: null };
  const run = (async () => {
    try {
      const list = await api.listSessions(projectId);
      peekSessions.value = { ...peekSessions.value, [projectId]: list };
      fetchedAt[projectId] = now();
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Could not read those sessions.';
      peekError.value = { ...peekError.value, [projectId]: message };
    } finally {
      peekLoading.value = { ...peekLoading.value, [projectId]: false };
      inFlight.delete(projectId);
    }
  })();
  inFlight.set(projectId, run);
  return run;
}

/**
 * Forget one project's cached list (or every list).
 *
 * Called when the UI itself changes sessions — a rename, a delete, a new session —
 * so the fly-out never shows a row the user just removed.
 */
export function invalidatePeekSessions(projectId?: string): void {
  if (projectId === undefined) {
    peekSessions.value = {};
    for (const key of Object.keys(fetchedAt)) delete fetchedAt[key];
    return;
  }
  if (!(projectId in fetchedAt) && !(projectId in peekSessions.value)) return;
  delete fetchedAt[projectId];
  const next = { ...peekSessions.value };
  delete next[projectId];
  peekSessions.value = next;
}

/** Test seam: drop every timer and cached read. */
export function resetRailPeek(): void {
  cancelClose();
  peekProjectId.value = null;
  peekSessions.value = {};
  peekLoading.value = {};
  peekError.value = {};
  inFlight.clear();
  for (const key of Object.keys(fetchedAt)) delete fetchedAt[key];
}