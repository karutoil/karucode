/**
 * The data behind the control plane: every project in one read.
 *
 * The control plane is a *view*, never a second chat state — the session it
 * opens is the ordinary session, driven by the ordinary store. What it does own
 * is the cross-project snapshot: the counters, the control chats, which sessions
 * are running or need attention, and the live agents behind those chats.
 *
 * Three rules keep it honest:
 *
 * 1. **One read, one snapshot.** `GET /api/control/overview` returns everything
 *    the browser pane and the hero counters draw, so the screen can never
 *    disagree with itself (a control chat saying "2 agents" while the agent list
 *    says "1" is a bug nobody can reproduce).
 * 2. **A stale response never wins.** Reads are numbered; the older one is
 *    dropped on arrival instead of overwriting a newer answer, because a 5 s poll
 *    overlaps whenever the server is slow.
 * 3. **A failure is not an empty workspace.** The previous overview is kept and
 *    the message is recorded, so a blip shows "stale" rather than "nothing is
 *    running anywhere" — which is a different and wrong answer.
 *
 * Nothing here is durable state and nothing here throws: the poller is a timer,
 * and an unhandled rejection there would be a console error every 5 s.
 */

import { signal } from '@preact/signals';
import { toErrorMessage } from '../api/client';
import type { AgentSummary, ControlChatSummary, ControlOverview } from '../api/types';
import * as api from '../api/endpoints';

/** Whether the control plane replaced the workspace on screen. */
export const controlPlaneOpen = signal(false);

/** The newest snapshot, or `null` before the first successful read. */
export const controlOverview = signal<ControlOverview | null>(null);
/** Why the last read failed. Kept alongside the previous overview, never instead of it. */
export const controlOverviewError = signal<string | null>(null);
export const controlOverviewLoading = signal(false);
/** The chat-list read is tracked apart from the overview: it is a different request. */
export const controlChatsLoading = signal(false);
export const controlChatsError = signal<string | null>(null);

/** Default poll interval. Fast enough to feel live, slow enough to be free. */
export const CONTROL_REFRESH_MS = 5000;

/**
 * Bumped by every read. A response carrying an older number is discarded, so a
 * slow first poll can never land on top of a fast second one.
 */
let overviewRead = 0;
let chatsRead = 0;

/**
 * Read the whole-control-plane snapshot.
 *
 * Never throws: an unauthorized or unreachable server records the message in
 * `controlOverviewError` and leaves the previous snapshot in place for the UI to
 * mark as stale.
 */
export async function loadControlOverview(): Promise<void> {
  const read = ++overviewRead;
  controlOverviewLoading.value = true;
  try {
    const overview = await api.getControlOverview();
    if (read !== overviewRead) return;
    controlOverview.value = overview;
    controlOverviewError.value = null;
  } catch (error) {
    if (read !== overviewRead) return;
    controlOverviewError.value = toErrorMessage(error);
  } finally {
    if (read === overviewRead) controlOverviewLoading.value = false;
  }
}

/**
 * Read the control chats alone, with the same contract as the overview.
 *
 * `controlOverview` is still the single source of truth for the list — this read
 * replaces only its `control` array — so a refresh button on the rail updates the
 * chats without a whole snapshot, and the two never drift into two lists.
 */
export async function loadControlChats(): Promise<void> {
  const read = ++chatsRead;
  controlChatsLoading.value = true;
  try {
    const chats = await api.listControlChats();
    if (read !== chatsRead) return;
    const current = controlOverview.value;
    // With no snapshot yet there is nowhere to put the list; the next overview
    // carries it. A fabricated overview here would make the counters lie.
    if (current) controlOverview.value = { ...current, control: chats };
    controlChatsError.value = null;
  } catch (error) {
    if (read !== chatsRead) return;
    controlChatsError.value = toErrorMessage(error);
  } finally {
    if (read === chatsRead) controlChatsLoading.value = false;
  }
}

/** The control chats, newest activity first as the server sends them. */
export function controlChats(): ControlChatSummary[] {
  return controlOverview.value?.control ?? [];
}

/**
 * The live agents belonging to one control chat.
 *
 * `live_agents` is the flat list the snapshot carries; filtering by
 * `session_id` is what turns it into a per-chat badge, and doing it from the one
 * snapshot keeps the badge and the agent console reading the same rows.
 */
export function agentsForControlChat(id: string): AgentSummary[] {
  if (id === '') return [];
  return (controlOverview.value?.live_agents ?? []).filter((agent) => agent.session_id === id);
}

/**
 * Poll the overview until the returned disposer is called.
 *
 * The interval belongs to the caller, not to the module: two mounted control
 * planes would otherwise share one timer, and whichever unmounted first would
 * silently stop the other's refresh. Each call returns its own disposer.
 */
export function startControlPlaneRefresh(intervalMs = CONTROL_REFRESH_MS): () => void {
  if (typeof window === 'undefined') return () => {};
  const timer = window.setInterval(() => void loadControlOverview(), intervalMs);
  return () => window.clearInterval(timer);
}

/** Test seam: forget the snapshot, the errors and every in-flight read. */
export function resetControlPlane(): void {
  // Bumping both counters orphans whatever is still in flight, so a response
  // that lands after a reset cannot repopulate the signals.
  overviewRead++;
  chatsRead++;
  controlOverview.value = null;
  controlOverviewError.value = null;
  controlOverviewLoading.value = false;
  controlChatsLoading.value = false;
  controlChatsError.value = null;
}
