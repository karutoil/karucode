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