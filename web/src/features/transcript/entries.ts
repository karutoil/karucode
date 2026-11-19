import { clampEstimate } from '../../lib/virtualWindow';
import type { ChatItem } from '../../state/chat';
import type { Entry } from './types';

const DAY_MS = 86_400_000;

/**
 * The stored message id behind a transcript item, or `null`.
 *
 * Items carry ids like `msg:42` (a persisted row), `tool:call_1` (a live tool
 * call) and `local:…` (an optimistic send). Only a persisted row can be a rewind
 * point, so everything else is refused rather than guessed at.
 */
export function storedMessageId(id: string): number | null {
  if (!id.startsWith('msg:')) return null;
  const parsed = Number(id.slice(4));
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

export function validAt(at: number): boolean {
  return Number.isFinite(at) && at > 0;
}

/**
 * Day grouping for the date markers.
 *
 * The clock label that sits beside these (`12:14 PM`) moved to `lib/format.ts`
 * as `clockTime`, where any other view — the control plane's timestamps are the
 * next reader — can share it instead of writing a second `toLocaleTimeString`.
 * `validAt`, `dayStart` and `dayLabel` stay here: they are the transcript's own
 * grouping vocabulary, not shared formatters.
 */
export function dayStart(at: number): number {
  const date = new Date(at);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** "Today · October 28, 2026", "Yesterday · …", or the plain date. */
export function dayLabel(at: number, now: number = Date.now()): string {
  const date = new Date(at);
  const pretty = date.toLocaleDateString(undefined, {
    month: 'long',
    day: 'numeric',
    year: 'numeric',
  });
  const day = dayStart(at);
  if (day === dayStart(now)) return `Today · ${pretty}`;
  if (day === dayStart(now - DAY_MS)) return `Yesterday · ${pretty}`;
  return pretty;
}

/** True when two part lists hold the same items, in the same order, by identity. */
function sameParts(a: readonly ChatItem[], b: readonly ChatItem[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let index = 0; index < a.length; index += 1) if (a[index] !== b[index]) return false;
  return true;
}

/**
 * Group items into the rows the transcript draws.
 *
 * `previous` is the entry list from the last render. A row whose items are
 * unchanged is **reused by identity** rather than rebuilt, because every memo
 * boundary downstream compares `entry`, `parts` and `item` by `Object.is`: a
 * freshly built `parts` array for a turn that did not change would make that
 * turn's boundary fail on every keystroke of another turn's answer, which is
 * exactly the cost this is here to avoid. The reducer already keeps settled
 * `ChatItem`s by identity, so identity equality here means "nothing changed".
 */
export function buildEntries(items: readonly ChatItem[], previous: readonly Entry[] = []): Entry[] {
  const entries: Entry[] = [];
  let lastDay: number | null = null;
  let turn: { key: string; at: number; parts: ChatItem[] } | null = null;

  const flush = (): void => {
    if (!turn) return;
    const index = entries.length;
    const before = previous[index];
    if (before?.kind === 'turn' && before.key === turn.key && sameParts(before.parts, turn.parts)) {
      entries.push(before);
    } else {
      entries.push({ kind: 'turn', key: turn.key, at: turn.at, parts: turn.parts });
    }
    turn = null;
  };

  for (const item of items) {
    if (validAt(item.at)) {
      const day = dayStart(item.at);
      if (day !== lastDay) {
        flush();
        lastDay = day;
        const index = entries.length;
        const key = `day-${day}`;
        const before = previous[index];
        entries.push(
          before?.kind === 'day' && before.key === key && before.at === item.at
            ? before
            : { kind: 'day', key, at: item.at },
        );
      }
    }

    if (item.kind === 'message' && item.role === 'user') {
      flush();
      const index = entries.length;
      const before = previous[index];
      entries.push(
        before?.kind === 'user' && before.key === item.id && before.item === item
          ? before
          : { kind: 'user', key: item.id, item },
      );
      continue;
    }

    // Everything else belongs to the assistant turn; open one if the run started
    // with a tool call instead of prose.
    if (!turn) turn = { key: `turn:${item.id}`, at: item.at, parts: [] };
    turn.parts.push(item);
  }
  flush();
  return entries;
}

/**
 * A height guess for one entry, used only until the browser measures the row.
 *
 * It is deliberately generous: an overestimate leaves a little padding above
 * the window that collapses on the first measurement, while an underestimate
 * makes the scrollbar jump as rows are discovered. The estimate never wins over
 * a measured height.
 */
export function estimateEntryHeight(entry: Entry): number {
  if (entry.kind === 'day') return 48;
  if (entry.kind === 'user') return 96 + Math.ceil(entry.item.content.length / 96) * 18;
  let height = 104;
  for (const part of entry.parts) {
    if (part.kind === 'message') {
      height += 32 + Math.ceil((part.content.length + part.reasoning.length) / 96) * 18;
    } else if (part.kind === 'tool' || part.kind === 'command') {
      height += 46;
    } else if (part.kind === 'approval') {
      height += 148;
    } else {
      height += 40;
    }
  }
  return clampEstimate(height);
}
