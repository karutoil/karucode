import { useMemo, useRef } from 'preact/hooks';
import type { ChatItem } from '../../state/chat';
import { buildEntries } from './entries';
import type { Entry } from './types';

/** How long a row that just joined the conversation keeps its entry animation. */
const ENTRY_ANIMATION_MS = 300;

export function useTranscriptEntries(
  items: ChatItem[],
  sessionKey: string | null | undefined,
): {
  entries: Entry[];
  collectFresh: (visible: readonly Entry[]) => Set<string>;
} {
  // Rebuilt only when the item list changes, and reusing the previous entry
  // objects for every row that did not: the memo boundaries below compare by
  // identity, so a fresh `parts` array for an unchanged turn would defeat them.
  const previousEntries = useRef<Entry[]>([]);
  /**
   * When each row's entry animation expires, by entry key.
   *
   * The animation belongs to a row that has just joined the conversation, not to
   * the row element: a windowed transcript unmounts and remounts rows as the
   * reader scrolls, and when the animation lived on `.msg` every remount replayed
   * it - rows the reader was looking at faded in from transparent under the wheel.
   * A deadline rather than "the render that saw it arrive" keeps the class on the
   * row for the whole animation (a streaming token re-renders the row ~60 times a
   * second, and dropping the class early cancelled the fade mid-flight).
   */
  const freshUntil = useRef(new Map<string, number>());
  /** The conversation whose rows have already been drawn once. */
  const drawnFor = useRef<string | null | undefined>(undefined);

  const entries = useMemo(() => {
    const previous = previousEntries.current;
    const next = buildEntries(items, previous);
    const known = new Set(previous.map((entry) => entry.key));
    previousEntries.current = next;
    // Only rows that join a conversation already on screen fade in. The first
    // fill - a session being opened, a history being restored - is drawn as it
    // is: announcing every row of a conversation that was already written is not
    // an arrival, and on a session switch it animated the whole visible window.
    if (drawnFor.current !== sessionKey) {
      drawnFor.current = sessionKey;
      return next;
    }
    const now = Date.now();
    for (const [key, until] of freshUntil.current) {
      if (until <= now) freshUntil.current.delete(key);
    }
    for (const entry of next) {
      if (!known.has(entry.key) && !freshUntil.current.has(entry.key)) {
        freshUntil.current.set(entry.key, now + ENTRY_ANIMATION_MS);
      }
    }
    return next;
  }, [items, sessionKey]);

  const collectFresh = (visible: readonly Entry[]): Set<string> => {
    // A row fades in only while its arrival is inside the animation window.
    // Remounting it later (the window scrolling) is not an arrival, so it does not
    // animate: that replay is the flicker this replaced.
    const freshVisible = new Set<string>();
    const now = Date.now();
    for (const entry of visible) {
      const until = freshUntil.current.get(entry.key);
      if (until === undefined) continue;
      if (until <= now) {
        freshUntil.current.delete(entry.key);
        continue;
      }
      freshVisible.add(entry.key);
    }
    return freshVisible;
  };

  return { entries, collectFresh };
}
