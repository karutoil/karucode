import { describe, expect, it } from 'vitest';
import {
  distanceFromBottom,
  followingNewest,
  forgetFollow,
  isGrowthAfterFollow,
  isPinnedToBottom,
  jumpBehavior,
  noteFollow,
  noteSettling,
  shouldFollow,
  type FollowAnchor,
  type ScrollMetrics,
} from './stickyBottom';

/** A viewport of 400px showing 1200px of content, scrolled to `scrollTop`. */
function view(scrollTop: number, scrollHeight = 1200, clientHeight = 400): ScrollMetrics {
  return { scrollTop, scrollHeight, clientHeight };
}

/** A scroller element with the two fields `jumpBehavior` reads. */
function scroller(scrollTop: number, clientHeight = 400, scrollHeight = 1_000_000): HTMLElement {
  return {
    scrollTop,
    clientHeight,
    scrollHeight,
    scrollTo: () => undefined,
  } as unknown as HTMLElement;
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
  // The reported bug, as a decision: a recorded pin is not permission. A wheel
  // gesture clears the follow anchor, and the frame that would clear the pin has
  // not run yet — while a stream produces growth in every one of those frames.
  // Trusting the pin there snapped the view to the tail under the reader's hand.
  it('is decided by live geometry, not by a pin recorded a frame ago', () => {
    // Widget: the view sits 600px above the newest content, the anchor was
    // dropped by a gesture, and the recorded pin still says "following".
    expect(shouldFollow(view(200, 4000), null, false)).toBe(false);
  });

  it('follows while a smooth scroll it started is still settling', () => {
    expect(shouldFollow(view(100), null, true)).toBe(true);
  });

  it('follows when the view is simply at the bottom', () => {
    expect(shouldFollow(view(800), null, false)).toBe(true);
  });

  it('follows when the only gap is content that arrived after the follow', () => {
    expect(shouldFollow(view(800, 2100), anchoredAtBottom(), false)).toBe(true);
  });

  it('stays put once the reader has scrolled into history', () => {
    // The reviewed failure this must never reintroduce: following a reader who
    // deliberately moved up, however much content lands meanwhile.
    const anchored = anchoredAtBottom();
    expect(shouldFollow(view(200, 4000), anchored, false)).toBe(false);
    expect(shouldFollow(view(200, 4000), null, false)).toBe(false);
  });
});

describe('jumpBehavior', () => {
  it('animates a jump across a screen or two', () => {
    // 1_000_000px of content, 400px viewport: 600px from the bottom is a
    // screen and a half, which reads as motion.
    expect(jumpBehavior(scroller(999_600 - 600))).toBe('smooth');
  });

  it('snaps over a long jump instead of strobing through the history', () => {
    // Chromium covers a 150 000px "smooth" scroll in one or two enormous steps,
    // each of which mounts a screenful of rows it has never drawn: a strobe, not
    // an animation.
    expect(jumpBehavior(scroller(0))).toBe('auto');
    expect(jumpBehavior(scroller(500_000))).toBe('auto');
  });

  it('snaps when the browser cannot animate or motion is reduced', () => {
    const element = scroller(999_600 - 600);
    // No `scrollTo` is exactly the old browser / jsdom shape.
    expect(jumpBehavior({ ...element, scrollTo: undefined } as unknown as HTMLElement)).toBe('auto');
  });
});

/**
 * The scroller shape `followingNewest` reads: it only ever asks for these three
 * numbers, so a mutable record of them is the whole viewport.
 */
interface Panel {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

function panel(scrollTop: number, scrollHeight = 1200, clientHeight = 400): Panel {
  return { scrollTop, scrollHeight, clientHeight };
}

/** The DOM view of a panel: the decision helpers take an element, not a record. */
function asElement(record: Panel): HTMLElement {
  return record as unknown as HTMLElement;
}

describe('followingNewest', () => {
  // Two components write this scroll - the follow and the window's measurement
  // correction - and they used to decide "is the reader following?" separately.
  // During a stream that produced opposite answers on the same frame: the follow
  // saw growth it had not caught up with and scrolled down, the correction saw a
  // viewport that was not at the bottom and scrolled back up. A real Firefox
  // profile caught it as two scroll writes and five forced reflows per frame,
  // with nobody at the wheel. One answer, asked of one piece of state, is the fix.
  it('is true while the view is flush with the bottom', () => {
    expect(followingNewest(asElement(panel(800)))).toBe(true);
    // Sub-pixel rounding of our own write still counts as the bottom.
    expect(followingNewest(asElement(panel(799.6)))).toBe(true);
  });

  it('is false for a reader who scrolled away', () => {
    // 700px above the newest content, and nothing recorded as the app's own
    // movement: that gap is a decision, and neither writer may close it.
    expect(followingNewest(asElement(panel(100)))).toBe(false);
  });

  it('is true while the gap is only content that arrived after the follow', () => {
    const element = panel(800);
    noteFollow(asElement(element));
    // Nine hundred pixels of cards landed below the viewport, and `scrollTop`
    // is exactly where the follow left it.
    element.scrollHeight = 2100;
    expect(followingNewest(asElement(element))).toBe(true);
  });

  it('is false again the moment the reader takes the view over', () => {
    const element = panel(800);
    noteFollow(asElement(element));
    element.scrollHeight = 2100;
    expect(followingNewest(asElement(element))).toBe(true);
    // A wheel (or any gesture) drops the follow before the render that would
    // still see growth.
    forgetFollow(asElement(element));
    expect(followingNewest(asElement(element))).toBe(false);
  });

  it('is true while a smooth scroll it started is still arriving', () => {
    const element = panel(0, 1200);
    noteSettling(asElement(element), true);
    // Mid-animation the view is nowhere near the bottom, and both writers must
    // leave the animation alone.
    expect(followingNewest(asElement(element))).toBe(true);
    noteSettling(asElement(element), false);
    expect(followingNewest(asElement(element))).toBe(false);
  });

  it('records the position it followed to', () => {
    const element = panel(800);
    expect(noteFollow(asElement(element))).toEqual({ scrollTop: 800, scrollHeight: 1200, clientHeight: 400 });
  });
});
