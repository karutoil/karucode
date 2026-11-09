/**
 * Geometry for a windowed transcript.
 *
 * A long conversation can be tens of thousands of rows; mounting them all costs
 * more than the browser's `content-visibility` saves, because Preact still builds
 * every row and every Markdown block. The transcript therefore renders only the
 * rows around the viewport and represents the rest with top/bottom padding. This
 * module is the arithmetic: offsets from measured or estimated row heights, and
 * the visible slice for one scroll position. No DOM, no state.
 */

/** Cumulative offsets: `offsets[i]` is the top of row `i`, `offsets[n]` the end. */
export interface WindowGeometry {
  offsets: number[];
  total: number;
}

/** Tallest a single row can plausibly be before the browser has measured it. */
const MAX_ESTIMATE_PX = 4_000;

/** How much of the transcript a height estimate may claim. */
export function clampEstimate(px: number): number {
  if (!Number.isFinite(px) || px < 0) return 0;
  return Math.min(px, MAX_ESTIMATE_PX);
}

export function prefixHeights(heights: readonly number[]): WindowGeometry {
  const offsets = new Array<number>(heights.length + 1);
  offsets[0] = 0;
  let total = 0;
  for (let index = 0; index < heights.length; index += 1) {
    const height = heights[index] ?? 0;
    total += Number.isFinite(height) ? Math.max(0, height) : 0;
    offsets[index + 1] = total;
  }
  return { offsets, total };
}

export interface Range {
  /** First rendered row index, inclusive. */
  start: number;
  /** One past the last rendered row index. */
  end: number;
}

/** The index of the last row whose top is at or above `y`, or -1. */
function lastAtOrAbove(offsets: readonly number[], y: number): number {
  let low = 0;
  let high = offsets.length - 1;
  // `offsets` has n+1 entries; search the top edges 0..n-1.
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if ((offsets[mid] ?? 0) <= y) low = mid;
    else high = mid - 1;
  }
  return low;
}

/** The index of the last top edge strictly above `y` (the end edge is exclusive). */
function lastBelow(offsets: readonly number[], y: number): number {
  let low = 0;
  let high = offsets.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if ((offsets[mid] ?? 0) < y) low = mid;
    else high = mid - 1;
  }
  return low;
}

/**
 * The rows to mount for one scroll position.
 *
 * `overscan` is the number of pixels kept mounted above and below the viewport:
 * enough that a wheel tick does not reveal an empty band while the next frame
 * renders. The range always contains the row straddling the viewport top and the
 * one straddling the bottom, so a very tall Markdown block is never sliced.
 */
export function visibleRange(
  offsets: readonly number[],
  scrollTop: number,
  viewportHeight: number,
  overscan: number,
): Range {
  const count = Math.max(0, offsets.length - 1);
  if (count === 0) return { start: 0, end: 0 };
  const top = Math.max(0, scrollTop - Math.max(0, overscan));
  const bottom = Math.max(top + 1, scrollTop + Math.max(0, viewportHeight) + Math.max(0, overscan));
  // `lastAtOrAbove(top)` is the row containing `top` (or the last row above it);
  // include it so the row cut by the viewport top is still mounted.
  const start = Math.max(0, Math.min(count - 1, lastAtOrAbove(offsets, top)));
  const end = Math.min(count, lastBelow(offsets, bottom) + 1);
  return { start, end: Math.max(start + 1, end) };
}
