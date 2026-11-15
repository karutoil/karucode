/**
 * Scrolls the app makes on the transcript's behalf, so the sticky-bottom can
 * tell them apart from the reader's.
 *
 * The transcript moves its own scroll in two places: the window's measurement
 * correction (`components/ChatTranscript.tsx` re-lays the rows it swapped for
 * padding and keeps the row at the top of the viewport still) and the history
 * loader's reveal (`lib/historyLoader.ts`). Both write `scrollTop` while the
 * reader is doing nothing at all, and to anything watching scroll events they are
 * indistinguishable from a gesture:
 *
 * * the write fires a `scroll` event,
 * * `scrollTop` has moved since the last follow, so the "the gap is growth we
 *   have not caught up with" rule refuses it,
 * * and the view is therefore declared the reader's decision.
 *
 * That is how opening a long session ended 97px short of its newest row with
 * "Jump to latest" showing on a transcript nobody had touched: the correction
 * after the pin was read as a gesture.
 *
 * So the correction says so. A note is a position and a moment: it expires on
 * its own, and it only covers the position it was written for, so a later
 * gesture is never mistaken for the app's own bookkeeping. `null` when the
 * element has not been marked.
 */
const own = new WeakMap<HTMLElement, { top: number; at: number; atBottom: boolean }>();

/**
 * How long a note stands for. One frame is enough for the scroll handler that
 * runs from a `requestAnimationFrame`, and anything longer risks swallowing a
 * real gesture that lands immediately after the correction.
 */
export const OWN_SCROLL_MS = 200;

/** How far the view may be from the noted position and still be our own write. */
const OWN_SCROLL_SLOP_PX = 2;

/**
 * Record that the scroll on `element` is the app's own.
 *
 * `atBottom` is read by the caller *before* its write and says whether the view
 * was showing the newest content at that moment. The sticky-bottom needs it: a
 * correction that lands while the reader is at the bottom must not leave them
 * short of it, and a correction that lands while they are reading history must
 * not pull them back. A recorded pin cannot answer that — it is one frame behind,
 * and a programmatic scroll never clears it at all.
 */
/** How long one burst of app-made corrections is treated as a single write. */
const OWN_SCROLL_BURST_MS = 40;

/** How far a continuing correction may move the view and still be a continuation. */
const OWN_SCROLL_BURST_PX = 128;

export function noteOwnScroll(element: HTMLElement, atBottom: boolean): void {
  // A correction can arrive in several passes in the same frame (the window
  // re-measures, re-renders, re-measures again). The second pass measures "not
  // at the bottom" only because the first already moved the view, so within one
  // burst the flag is inherited — judged by where the reader was when the burst
  // started. The burst is deliberately short and position-bound: a gesture (or
  // anything else that takes the view somewhere else) starts a new one.
  const existing = own.get(element);
  const now = Date.now();
  const contiguous =
    existing !== undefined &&
    now - existing.at <= OWN_SCROLL_BURST_MS &&
    Math.abs(element.scrollTop - existing.top) <= OWN_SCROLL_BURST_PX;
  own.set(element, {
    top: element.scrollTop,
    at: now,
    atBottom: contiguous ? existing.atBottom || atBottom : atBottom,
  });
}

/** The note standing for `element`, or `null` when there is none (or it expired). */
export function ownScrollNote(
  element: HTMLElement,
  now: number = Date.now(),
): { atBottom: boolean } | null {
  const entry = own.get(element);
  if (!entry) return null;
  if (now - entry.at > OWN_SCROLL_MS) {
    own.delete(element);
    return null;
  }
  return Math.abs(element.scrollTop - entry.top) <= OWN_SCROLL_SLOP_PX ? { atBottom: entry.atBottom } : null;
}

/** True when the element's current scroll position is one the app just made. */
export function isOwnScroll(element: HTMLElement, now: number = Date.now()): boolean {
  return ownScrollNote(element, now) !== null;
}

/** Forget a note, for a caller that knows the correction was not the last one. */
export function clearOwnScroll(element: HTMLElement): void {
  own.delete(element);
}