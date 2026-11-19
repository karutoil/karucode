import type { SessionSummary } from '../../api/types';
import * as store from '../../state/store';

const DAY = 86_400_000;

/** The rail's one-tap filters, in the mock's order. */
export type SessionFilter = 'all' | 'needs' | 'running' | 'changes';

export const SESSION_FILTERS: ReadonlyArray<{
  id: SessionFilter;
  label: string;
  title: string;
}> = [
  { id: 'all', label: 'All', title: 'Every session in this project' },
  { id: 'needs', label: 'Needs you', title: 'Sessions with unread news or a failed run' },
  { id: 'running', label: 'Running', title: 'Sessions with a run in flight' },
  { id: 'changes', label: 'Changes', title: 'Sessions that touched files' },
];

/** Hover intent for the desktop project dropdown: crossing the trigger on the way
 *  to the composer must not flash a list, but the pause before opening should be
 *  short enough to read as "it wants to open". */
export const PROJECTS_HOVER_OPEN_MS = 120;
/** The pointer crossing the gap between the trigger and the panel takes a beat;
 *  the panel waits this long before it treats the exit as leaving. */
export const PROJECTS_HOVER_CLOSE_MS = 200;

/** Does one session pass the rail's filter? */
export function passesFilter(
  session: SessionSummary,
  filter: SessionFilter,
  unread: number,
): boolean {
  if (filter === 'all') return true;
  if (filter === 'needs') {
    return unread > 0 || session.last_run_status === 'failed' || session.last_run_status === 'interrupted';
  }
  if (filter === 'running') return session.running;
  // "Changes" is about what the session *did*, and only a session the server has
  // described can answer it. Unknown is not "no": it stays visible under All.
  return false;
}

/** Bucket a session's last activity into the rail's date groups. */
export function sessionGroup(updatedAt: number, now: number): string {
  const startOfDay = (value: number): number => {
    const date = new Date(value);
    date.setHours(0, 0, 0, 0);
    return date.getTime();
  };
  const today = startOfDay(now);
  const day = startOfDay(updatedAt);
  if (day >= today) return 'Today';
  if (day >= today - DAY) return 'Yesterday';
  if (day >= today - 7 * DAY) return 'Previous 7 days';
  return 'Older';
}

// The tones a chip may use, exactly as `uiContributions.UI_TONES` names them. An
// unknown tone is drawn neutral rather than as the empty class `session-chip-`,
// which would silently drop the chip's only visual signal.
export function toneClass(tone: string): string {
  return ['neutral', 'accent', 'success', 'warn', 'danger', 'muted'].includes(tone) ? tone : 'neutral';
}

export function connectionTone(): string {
  const value = store.connection.value;
  if (value.api === 'offline') return 'danger';
  if (value.stream === 'unauthorized') return 'danger';
  if (value.stream === 'reconnecting' || value.stream === 'connecting') return 'warn';
  if (value.stream === 'open') return 'ok';
  if (value.api === 'online') return 'ok';
  return 'muted';
}
