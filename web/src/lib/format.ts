/**
 * "How long ago" for anything with an `updated_at`: the session rail, the agent
 * roster, the command palette, the notification centre and the session-info tab
 * all read the same recency out of this one function, so a row cannot say
 * `2m ago` in one list and `2 minutes` in another. It is the canonical recency
 * formatter — any new list (the control plane's included) reads this rather than
 * writing its own. `now` is injectable for tests and for a rail that re-renders
 * on a shared clock tick rather than on `Date.now()`.
 *
 * A negative delta (a server clock ahead of the browser's) clamps to `just now`
 * instead of reporting a time in the future.
 */
export function relativeTime(timestamp: number, now: number = Date.now()): string {
  const delta = Math.max(0, now - timestamp);
  const seconds = Math.round(delta / 1000);
  if (seconds < 45) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  const date = new Date(timestamp);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
}

/**
 * Local wall-clock label, e.g. `12:14 PM` — the time of day on its own.
 *
 * The transcript renders this inside each message's `<time>`, and it is the
 * canonical "when, on the clock" formatter: any other view that wants an absolute
 * moment beside `relativeTime`'s "how long ago" reads this instead of writing its
 * own `toLocaleTimeString`, so the two cannot disagree about a session's
 * timestamp.
 *
 * Empty — never `Invalid Date` — when the caller has no usable timestamp: a row
 * with no time renders no `<time>` at all. `dayLabel` (the transcript's
 * "Today · October 28, 2026" date marker) stays private to `ChatTranscript.tsx`;
 * it is that view's grouping vocabulary, not a shared formatter.
 */
export function clockTime(at: number): string {
  if (!Number.isFinite(at) || at <= 0) return '';
  return new Date(at).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

export function formatDuration(ms: number | null): string {
  if (ms === null || !Number.isFinite(ms)) return '';
  // Sub-millisecond work (a small file read) reads as "0ms", which looks like a
  // broken timer; say "<1ms" instead.
  if (Math.round(ms) < 1) return '<1ms';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return `${minutes}m ${rest}s`;
}

export function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return `${text.slice(0, Math.max(0, max - 1))}…`;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatCount(count: number): string {
  return count === 1 ? '1 message' : `${count} messages`;
}

export function jsonSummary(value: unknown, max = 90): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return truncate(value.replace(/\s+/g, ' '), max);
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) return truncate(JSON.stringify(value), max);
  if (typeof value === 'object') {
    const record = value as Record<string, unknown>;
    for (const key of ['command', 'path', 'pattern', 'query', 'url', 'name', 'task']) {
      const candidate = record[key];
      if (typeof candidate === 'string' && candidate !== '') {
        return truncate(candidate.replace(/\s+/g, ' '), max);
      }
    }
    const entries = Object.entries(record);
    if (entries.length === 0) return '{}';
    return truncate(
      entries
        .slice(0, 3)
        .map(([key, val]) => `${key}: ${typeof val === 'string' ? val : JSON.stringify(val)}`)
        .join(', '),
      max,
    );
  }
  return '';
}

export function pluralise(count: number, singular: string, plural = `${singular}s`): string {
  return count === 1 ? `1 ${singular}` : `${count} ${plural}`;
}