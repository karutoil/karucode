import { describe, expect, it } from 'vitest';
import {
  distanceFromBottom,
  isGrowthAfterFollow,
  isPinnedToBottom,
  shouldFollow,
  type FollowAnchor,
  type ScrollMetrics,
} from './stickyBottom';

/** A viewport of 400px showing 1200px of content, scrolled to `scrollTop`. */
function view(scrollTop: number, scrollHeight = 1200, clientHeight = 400): ScrollMetrics {
  return { scrollTop, scrollHeight, clientHeight };
}

/** What following a 1200px transcript left behind: at the bottom, gap 0. */
function anchoredAtBottom(scrollHeight = 1200, clientHeight = 400): FollowAnchor {
  return { scrollTop: scrollHeight - clientHeight, scrollHeight, clientHeight };
}

describe('distanceFromBottom', () => {
  it('is zero at the bottom and never negative past it', () => {
    expect(distanceFromBottom(view(800))).toBe(0);
    expect(distanceFromBottom(view(900))).toBe(0);
    expect(distanceFromBottom(view(300))).toBe(500);
  });
});

describe('isPinnedToBottom', () => {
  it('accepts the threshold and rejects beyond it', () => {
    expect(isPinnedToBottom(view(752))).toBe(true);
    expect(isPinnedToBottom(view(700))).toBe(false);
  });
});

describe('isGrowthAfterFollow', () => {
  it('refuses without an anchor to compare against', () => {
    expect(isGrowthAfterFollow(view(300), null)).toBe(false);
  });

  it('is false when nothing moved and nothing grew', () => {
    expect(isGrowthAfterFollow(view(800), anchoredAtBottom())).toBe(false);
  });

  // The bug: a tool card lands between the follow write and the frame that
  // judges it, so the measurement sees a fresh scrollHeight against the
  // scrollTop the write left behind. The gap is the card, not the reader.
  it('credits a block of content that landed after the follow', () => {
    const before = anchoredAtBottom(1200);
    // 900px of cards arrived; scrollTop is exactly where following left it.
    expect(isGrowthAfterFollow(view(800, 2100), before)).toBe(true);
  });

  // The reported symptom: several big tool results mount back to back, so more
  // than one batch of content lands before the view can measure itself. A
  // follow accounts for the whole gap, however many batches produced it.
  it('credits several batches of big tool cards arriving back to back', () => {
    const before = anchoredAtBottom(1200);
    // Two cards of ~600px each, then a third on top of them.
    expect(isGrowthAfterFollow(view(800, 2400), before)).toBe(true);
    expect(isGrowthAfterFollow(view(800, 3100), before)).toBe(true);
    // And a burst large enough to fill several screens at once.
    expect(isGrowthAfterFollow(view(800, 8000), before)).toBe(true);
  });

  it('refuses a gap wider than the growth it knows about', () => {
    const before = anchoredAtBottom(1200);
    // Only 120px arrived, but the view sits 600px higher: that is a gesture.
    expect(isGrowthAfterFollow(view(200, 1320), before)).toBe(false);
  });

  it('refuses any movement of the viewport', () => {
    const before = anchoredAtBottom(1200);
    // Downward movement is still the reader, even with growth behind it.
    // (Within 1px is rounding and is covered by the next test.)
    for (const top of [700, 795, 810, 850]) {
      expect(isGrowthAfterFollow(view(top, 2100), before)).toBe(false);
    }
  });

  it('tolerates sub-pixel rounding of its own write', () => {
    const before: FollowAnchor = { scrollTop: 800.4, scrollHeight: 1200, clientHeight: 400 };
    // Chrome reports fractional scroll offsets at non-integer zoom levels; a
    // follow that comes back 0.4px off is still a follow, not a gesture.
    expect(isGrowthAfterFollow(view(800, 1500), before)).toBe(true);
    expect(isGrowthAfterFollow(view(799.6, 1500), before)).toBe(true);
  });

  it('refuses a shorter box, which is a new layout and not growth', () => {
    const before = anchoredAtBottom(1200);
    expect(isGrowthAfterFollow(view(800, 900), before)).toBe(false);
  });

  it('refuses a stale anchor whose viewport height changed', () => {
    const before = anchoredAtBottom(1200, 400);
    expect(isGrowthAfterFollow(view(800, 2100, 300), before)).toBe(false);
  });
});

describe('shouldFollow', () => {
  it('follows while pinned, whatever the geometry says', () => {
    expect(shouldFollow(view(100), null, true, false)).toBe(true);
  });

  it('follows while a smooth scroll it started is still settling', () => {
    expect(shouldFollow(view(100), null, false, true)).toBe(true);
  });

  it('follows when the view is simply at the bottom', () => {
    expect(shouldFollow(view(800), null, false, false)).toBe(true);
  });

  it('follows when the only gap is content that arrived after the follow', () => {
    expect(shouldFollow(view(800, 2100), anchoredAtBottom(), false, false)).toBe(true);
  });

  it('stays put once the reader has scrolled into history', () => {
    // The reviewed failure this must never reintroduce: following a reader who
    // deliberately moved up, however much content lands meanwhile.
    const anchored = anchoredAtBottom();
    expect(shouldFollow(view(200, 4000), anchored, false, false)).toBe(false);
    expect(shouldFollow(view(200, 4000), null, false, false)).toBe(false);
  });
});
