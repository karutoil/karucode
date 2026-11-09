/**
 * Where a streaming message splits into "settled" and "still arriving".
 *
 * # Why this exists
 *
 * `Markdown` parses its whole input with markdown-it, sanitises the result with
 * DOMPurify and swaps it in with `innerHTML`. That is the right trade for a
 * finished message: it runs once. A live message made the same call on *every*
 * token delta, so the per-token cost grew with the answer: measured in the
 * shipping bundle, a 120 KiB answer cost ~50 ms per render, and a 40+ delta/s
 * stream turned that into a permanently blocked main thread - a 14 fps tab,
 * where switching project or opening a session felt like "it takes forever".
 *
 * Nothing about a delta needs the whole message re-parsed. A block that is
 * finished can be rendered once and left alone; only the block still being
 * written changes. This module decides where that line is:
 *
 * * everything before `stable` is settled text, rendered as Markdown and
 *   re-rendered only when the boundary moves;
 * * everything after it is the live tail, re-rendered on every delta - which is
 *   only affordable because it is one block, not one message.
 *
 * The boundary is only moved in steps, so a long answer re-renders its prefix a
 * handful of times rather than once per block: `STREAM_STEP_MIN_CHARS` at
 * first, then a third of the prefix itself, so the prefix grows geometrically
 * and the total Markdown work over one message is a small multiple of its
 * length. The per-token work is the size of the tail - the block being written -
 * and is therefore independent of how long the answer has grown.
 *
 * # Fences
 *
 * A boundary is never taken inside a fenced code block: a fence is one block,
 * and splitting it in half would render the first half as prose. That also
 * means a fence stays in the live tail until it closes, which is why
 * `StreamingTail` draws an unterminated fence on the monospace surface it is
 * about to become.
 *
 * Everything here is a pure function of (previous state, text) so the same
 * code can be tested without a DOM, and the callers can keep the state in a
 * ref between renders.
 */

/** The settled prefix only re-renders after growing by at least this much. */
export const STREAM_STEP_MIN_CHARS = 1_024;
/** …or by a third of what is already rendered, whichever is larger. */
export const STREAM_STEP_RATIO = 3;
/** Blank-line boundaries remembered while scanning (bounded memory). */
const BOUNDARY_MEMORY = 64;
/** Characters of already-scanned text used to notice an in-place rewrite. */
const ANCHOR_CHARS = 64;

/** A fence opener: up to three spaces of indent, then ``` or ~~~. */
const FENCE_LINE = /^\s{0,3}(?:```|~~~)/;

export interface StreamingSplit {
  /** Characters rendered as Markdown; the rest is the live tail. */
  stable: number;
  /** How much of the text has been scanned for boundaries (always a line start). */
  scanned: number;
  /** Whether the scanned prefix ended inside an open fence. */
  inFence: boolean;
  /** Recent blank-line boundaries outside a fence, oldest first. */
  boundaries: number[];
  /** The last `ANCHOR_CHARS` of scanned text, to detect a rewrite. */
  anchor: string;
}

export function initialSplit(): StreamingSplit {
  return { stable: 0, scanned: 0, inFence: false, boundaries: [], anchor: '' };
}

function anchorAt(text: string, at: number): string {
  return text.slice(Math.max(0, at - ANCHOR_CHARS), at);
}

function remember(boundaries: number[], boundary: number): number[] {
  const next = boundaries.slice(-(BOUNDARY_MEMORY - 1));
  next.push(boundary);
  return next;
}

/**
 * Advance the split for `text`, which is the same message as `prev` saw plus
 * whatever arrived since. Returns `prev` itself when nothing changed, so a
 * caller can compare by identity to skip work.
 *
 * `text` is expected to grow by appending. When it does not - a shorter
 * snapshot, or a `text_replaced` that rewrote what was already shown - the
 * state resets and the whole text becomes the live tail until a boundary is
 * found again. That is the safe direction: a stale boundary would render
 * half-parsed Markdown, and a full plain-text render is merely plain.
 */
export function advanceSplit(prev: StreamingSplit, text: string): StreamingSplit {
  const continues =
    text.length >= prev.scanned &&
    (prev.scanned === 0 || anchorAt(text, prev.scanned) === prev.anchor);

  let scanned = continues ? prev.scanned : 0;
  let inFence = continues ? prev.inFence : false;
  let boundaries = continues ? prev.boundaries : [];

  for (;;) {
    const newline = text.indexOf('\n', scanned);
    if (newline === -1) break;
    const line = text.slice(scanned, newline);
    if (FENCE_LINE.test(line)) {
      inFence = !inFence;
    } else if (!inFence && line.trim() === '') {
      boundaries = remember(boundaries, newline + 1);
    }
    scanned = newline + 1;
  }

  const previousStable = continues ? prev.stable : 0;
  // The newest blank line outside a fence: everything after it is the block
  // still being written, which is exactly the part that cannot be rendered once
  // and left alone.
  const eligible = boundaries.length > 0 ? (boundaries[boundaries.length - 1] as number) : -1;
  const step = Math.max(STREAM_STEP_MIN_CHARS, Math.floor(previousStable / STREAM_STEP_RATIO));
  const stable = eligible >= 0 && eligible - previousStable >= step ? eligible : previousStable;

  const unchanged =
    continues && stable === prev.stable && scanned === prev.scanned && inFence === prev.inFence;
  if (unchanged) return prev;

  return {
    stable,
    scanned,
    inFence,
    boundaries,
    anchor: anchorAt(text, scanned),
  };
}

/** The two halves `Markdown` renders for a streaming message. */
export function splitStreamingText(
  text: string,
  split: StreamingSplit,
): { stable: string; live: string } {
  const stable = Math.min(Math.max(split.stable, 0), text.length);
  return { stable: text.slice(0, stable), live: text.slice(stable) };
}

/**
 * Whether `text` begins with a fenced code block (optionally after up to three
 * spaces of indent).
 *
 * The live tail is drawn on the monospace surface when the block being written
 * *is* a fence. The boundary never falls inside one, so an open fence always
 * begins the tail - but a short message has no boundary yet, which is why the
 * caller checks the tail's own first line rather than only the scanned state.
 */
export function startsFence(text: string): boolean {
  const newline = text.indexOf('\n');
  const first = newline === -1 ? text : text.slice(0, newline);
  return FENCE_LINE.test(first);
}
