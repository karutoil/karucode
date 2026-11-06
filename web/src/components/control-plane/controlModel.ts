import type { AgentSummary, SessionSummary } from '../../api/types';

/**
 * The plane's vocabulary: the word and tone a row shows for a session's state,
 * the compact timestamps beside it, and the shapes the rail and the launch box
 * pass around.
 *
 * Pure functions with no Preact in them, so the rules the plane applies can be
 * read - and tested - without rendering anything.
 */

export type Filter = 'all' | 'running' | 'finished';

export interface Status {
  label: string;
  tone: 'live' | 'good' | 'bad' | 'warn' | 'idle';
}

/** One flat list of `provider:model` choices, so the select value is never parsed. */
export interface ModelChoice {
  key: string;
  providerId: string;
  modelId: string;
  label: string;
  canReason: boolean;
}

/** "just now", "9m", "3h", "2d" — compact enough for a dense row. */
export function shortAgo(ms: number, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 45) return 'just now';
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 86_400) return `${Math.round(seconds / 3600)}h ago`;
  return `${Math.round(seconds / 86_400)}d ago`;
}

/** Two initials for a project glyph; a project with no name falls back to one dot. */
export function initials(name: string): string {
  const words = name.split(/[^A-Za-z0-9]+/).filter(Boolean);
  if (words.length === 0) return '?';
  return words.slice(0, 2).map((word) => word[0]).join('').toUpperCase();
}

export function sessionStatus(session: Pick<SessionSummary, 'running' | 'last_run_status'>): Status {
  if (session.running) return { label: 'Running', tone: 'live' };
  switch (session.last_run_status) {
    case 'completed': return { label: 'Finished', tone: 'good' };
    case 'failed': return { label: 'Failed', tone: 'bad' };
    case 'interrupted': return { label: 'Interrupted', tone: 'warn' };
    case 'limit_reached': return { label: 'Limit reached', tone: 'warn' };
    case 'cancelled': return { label: 'Cancelled', tone: 'idle' };
    default: return { label: 'No runs', tone: 'idle' };
  }
}

/** Stable 0–3 tone per project, so the list is scannable by project without colour meaning anything else. */
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

export function shortMoment(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** A ready-to-send instruction that names the target project by id, so it is never guessed. */
export function controlStarter(project: { id: string; name: string }): string {
  return `Start work in project "${project.name}" (project_id: ${project.id}).\n\n`;
}

/** The status as a word, never a colour: `completed` reads better as "done". */
export function agentStatusWord(status: AgentSummary['status']): string {
  switch (status) {
    case 'completed': return 'done';
    case 'running': return 'running';
    case 'paused': return 'paused';
    case 'failed': return 'failed';
    case 'cancelled': return 'cancelled';
    case 'interrupted': return 'interrupted';
    default: return status;
  }
}
