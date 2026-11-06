import type { AgentSummary, RunStatus, SessionSummary } from '../../api/types';
import { agentStatusLabel } from '../../state/selectors';
import type { StatusTone } from '../../state/selectors';
import { projectInitials } from '../ProjectRail';

export { relativeTime } from '../../lib/format';

/**
 * The plane's own vocabulary: the shapes its regions pass around, and the few
 * facts that belong to it alone.
 *
 * This module used to carry a second copy of the app's status words, tones,
 * recency formatter and project initials, because the canonical versions were
 * file-private when the plane was written. They are shared now, and so is this:
 * a session's state is spelled and coloured the same in the rail, the workbench
 * and the plane, because all three ask `state/selectors.ts`. What is left here is
 * genuinely the plane's — its filter set, the launch box's model choices, the
 * starter text it hands a new chat, and the tooltip for a chat's agent badge.
 *
 * Pure functions with no Preact in them, so the rules the plane applies can be
 * read — and tested — without rendering anything.
 */

export type Filter = 'all' | 'running' | 'finished';

/** One flat list of `provider:model` choices, so the select value is never parsed. */
export interface ModelChoice {
  key: string;
  providerId: string;
  modelId: string;
  label: string;
  canReason: boolean;
}

/**
 * The status a row shows, as the app's shared word-and-tone pair.
 *
 * Re-exported rather than re-implemented: the tone is `StatusTone`, which is the
 * name the shared stylesheet already paints (`status-dot status-<tone>`,
 * `session-state-<tone>`), so a row that adopts those classes needs no
 * translation table of its own.
 *
 * Unlike the workbench rail, the plane shows a word for *every* state including
 * settled ones — a list of sessions across every project is read by scanning for
 * "which of these needs me", and `No runs` is an answer there. `label` is
 * therefore always non-null; callers that want the quiet behaviour can drop it
 * when the tone is `muted`.
 */
export interface SessionStatus {
  label: string;
  tone: StatusTone;
}

/** The plane's own session-status view: the shared tones, plus a word for the quiet states. */
export function sessionStatus(
  session: Pick<SessionSummary, 'running' | 'last_run_status'>,
): SessionStatus {
  if (session.running) return { label: 'Running', tone: 'running' };
  switch (session.last_run_status as RunStatus | null | undefined) {
    case 'completed':
      return { label: 'Finished', tone: 'ok' };
    case 'failed':
      return { label: 'Failed', tone: 'danger' };
    case 'interrupted':
      return { label: 'Interrupted', tone: 'warn' };
    case 'limit_reached':
      return { label: 'Limit reached', tone: 'warn' };
    case 'cancelled':
      return { label: 'Cancelled', tone: 'muted' };
    default:
      return { label: 'No runs', tone: 'muted' };
  }
}

/** Two initials for a project glyph — the project rail's own function, so the two cannot disagree. */
export const initials = projectInitials;

/**
 * A stable 0–3 tone per project id.
 *
 * Genuinely the plane's own: it is the one screen that lists every project at
 * once, so it colours the glyph to make a list scannable by project. The
 * workbench shows one project at a time and has no equivalent. The hue carries
 * no meaning beyond identity — it is never a status.
 */
export function tone(id: string | null | undefined): number {
  let hash = 0;
  for (const char of id ?? '') hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash % 4;
}

/** What a chat's agent badge means, spelled out for the tooltip. */
export function agentBadgeTitle(agents: { live: number; total: number }): string {
  const running = agents.live === 1 ? 'subagent' : 'subagents';
  return `${agents.live} ${running} running · ${agents.total} total`;
}

/** "Untitled chat" rather than an empty row: the server creates chats unnamed on purpose. */
export function chatTitle(chat: Pick<SessionSummary, 'title'>): string {
  return chat.title.trim() === '' ? 'Untitled chat' : chat.title;
}

/** A ready-to-send instruction that names the target project by id, so it is never guessed. */
export function controlStarter(project: { id: string; name: string }): string {
  return `Start work in project "${project.name}" (project_id: ${project.id}).\n\n`;
}

/** The status as a word, never a colour — the shared agent vocabulary, so "done" is "done" everywhere. */
export function agentStatusWord(status: AgentSummary['status']): string {
  return agentStatusLabel(status);
}
