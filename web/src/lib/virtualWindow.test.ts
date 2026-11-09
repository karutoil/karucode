import { describe, expect, it } from 'vitest';
import { clampEstimate, prefixHeights, visibleRange } from './virtualWindow';

describe('prefixHeights', () => {
  it('turns row heights into cumulative offsets and a total', () => {
    const { offsets, total } = prefixHeights([100, 200, 50]);
    expect(offsets).toEqual([0, 100, 300, 350]);
    expect(total).toBe(350);
  });

  it('is empty for no rows', () => {
    expect(prefixHeights([])).toEqual({ offsets: [0], total: 0 });
  });

  it('never lets a negative or non-finite height move the offsets backwards', () => {
    const { offsets, total } = prefixHeights([100, -20, Number.NaN, 50]);
    expect(offsets).toEqual([0, 100, 100, 100, 150]);
    expect(total).toBe(150);
  });
});

describe('visibleRange', () => {
  // Ten 100px rows: row i spans [i*100, (i+1)*100).
  const offsets = prefixHeights(Array.from({ length: 10 }, () => 100)).offsets;

  it('renders the rows around the top of the transcript', () => {
    expect(visibleRange(offsets, 0, 300, 0)).toEqual({ start: 0, end: 3 });
  });

  it('includes the row cut by the viewport top', () => {
    // 250 is halfway through row 2, so rows 2..5 are the viewport.
    expect(visibleRange(offsets, 250, 300, 0)).toEqual({ start: 2, end: 6 });
  });

  it('adds the overscan on both sides', () => {
    expect(visibleRange(offsets, 500, 200, 100)).toEqual({ start: 4, end: 8 });
  });

  it('never asks for rows below the end', () => {
    expect(visibleRange(offsets, 950, 400, 200)).toEqual({ start: 7, end: 10 });
  });

  it('returns an empty range for an empty transcript', () => {
    expect(visibleRange([0], 0, 500, 200)).toEqual({ start: 0, end: 0 });
  });

  it('clamps a scroll position past the end to the last row', () => {
    expect(visibleRange(offsets, 99_999, 300, 0)).toEqual({ start: 9, end: 10 });
  });

  it('keeps a very tall row whole instead of slicing it', () => {
    // One 5000px row, then normal ones: a viewport inside it mounts it entire.
    const tall = prefixHeights([5000, 100, 100]).offsets;
    expect(visibleRange(tall, 4000, 300, 0)).toEqual({ start: 0, end: 1 });
  });
});

describe('clampEstimate', () => {
  it('bounds a guess so one unmeasured row cannot fake a huge document', () => {
    expect(clampEstimate(-1)).toBe(0);
    expect(clampEstimate(Number.POSITIVE_INFINITY)).toBe(0);
    expect(clampEstimate(400)).toBe(400);
    expect(clampEstimate(1_000_000)).toBe(4_000);
  });
});
