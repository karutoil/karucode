import { describe, expect, it } from 'vitest';
import { act, fireEvent, render } from '@testing-library/preact';
import { createElement } from 'preact';
import { useStickyBottom, type StickyBottom } from './stickyBottom';
import {
  distanceFromBottom,
  followingNewest,
  forgetFollow,
  isGrowthAfterFollow,
  isPinnedToBottom,
  isResizeAfterFollow,
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

/**
 * jsdom lays nothing out, so the three numbers the follow rules read are stubbed
 * on the node itself.
 */
function setGeometry(node: HTMLDivElement, next: Partial<ScrollMetrics>): void {
  for (const [property, value] of Object.entries(next)) {
    Object.defineProperty(node, property, { configurable: true, writable: true, value });
  }
}

/** A transcript the follow hook owns: 1200px of rows in a 400px viewport. */
function mountTranscript(): { node: HTMLDivElement; unmount: () => void } {
  let element: HTMLDivElement | null = null;

  function Transcript() {
    const sticky = useStickyBottom({ sessionKey: 's_1', contentKey: 'rows:20', sentMessageTick: 0 });
    return createElement(
      'div',
      {
        ref: (node: HTMLDivElement | null) => {
          if (node) sticky.scrollRef.current = node;
          element = node;
        },
      },
      createElement('button', { id: 'approve' }, 'Approve'),
    );
  }

  const view = render(createElement(Transcript, {}));
  const node = element as unknown as HTMLDivElement;
  setGeometry(node, { scrollHeight: 1200, clientHeight: 400, scrollTop: 800 });
  return { node, unmount: () => view.unmount() };
}

/**
 * A follow that is only visible through the growth it has not caught up with:
 * rows arrived below the viewport after the follow and `scrollTop` is where the
 * follow left it. `followingNewest` answers from the recorded anchor, so it
 * changes the moment a gesture forgets it — which is the thing under test.
 */
function armFollow(node: HTMLDivElement): void {
  setGeometry(node, { scrollHeight: 1200, scrollTop: 800 });
  noteFollow(node);
  setGeometry(node, { scrollHeight: 2100 });
  expect(followingNewest(node)).toBe(true);
}

/**
 * The numbers a real scroller reports, as a mutable record: change
 * `clientHeight` and the element is a resized one, exactly as the browser would
 * report it (a shrink leaves `scrollTop` where it was, a grow clamps it).
 */
interface FakeBox {
  scrollHeight: number;
  clientHeight: number;
  scrollTop: number;
}

function fakeScrollBox(element: HTMLElement, box: FakeBox): void {
  Object.defineProperty(element, 'scrollHeight', {
    configurable: true,
    get: () => box.scrollHeight,
  });
  Object.defineProperty(element, 'clientHeight', {
    configurable: true,
    get: () => box.clientHeight,
  });
  Object.defineProperty(element, 'scrollTop', {
    configurable: true,
    get: () => box.scrollTop,
    set: (value: number) => {
      box.scrollTop = Math.max(0, Math.min(value, Math.max(0, box.scrollHeight - box.clientHeight)));
    },
  });
}

interface MountedBox {
  box: FakeBox;
  node: HTMLDivElement;
  sticky: () => StickyBottom;
  unmount: () => void;
}

/** A transcript the follow hook owns, laid out by `box`. */
function mountScrollBox(box: FakeBox): MountedBox {
  let element: HTMLDivElement | null = null;
  let latest: StickyBottom | null = null;

  function Transcript() {
    const sticky = useStickyBottom({ sessionKey: 's_1', contentKey: 'rows:20', sentMessageTick: 0 });
    latest = sticky;
    return createElement(
      'div',
      {
        ref: (node: HTMLDivElement | null) => {
          if (node) {
            sticky.scrollRef.current = node;
            fakeScrollBox(node, box);
          }
          element = node;
        },
      },
      createElement('button', { id: 'approve' }, 'Approve'),
    );
  }

  const view = render(createElement(Transcript, {}));
  const sticky = (): StickyBottom => {
    if (!latest) throw new Error('hook did not render');
    return latest;
  };
  return { box, node: element as unknown as HTMLDivElement, sticky, unmount: () => view.unmount() };
}

/** Let the hook look again, as its ResizeObserver would after a layout change. */
function recheck(sticky: () => StickyBottom): void {
  act(() => sticky().recheck());
}

/** The scroll listener is frame-throttled: give the frame a chance to run. */
async function settleFrame(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 32));
  });
}

describe('isResizeAfterFollow', () => {
  it('credits a scroller that shrank under a view that was at the bottom', () => {
    // The on-screen keyboard: 2000px of transcript, 600px of viewport, flush
    // with the newest row - then 300px of viewport go away under it.
    const before = anchoredAtBottom(2000, 600);
    expect(isResizeAfterFollow(view(1400, 2000, 300), before)).toBe(true);
    // A grow is the same event: the URL bar collapsed, the keyboard closed.
    expect(isResizeAfterFollow(view(1400, 2000, 900), before)).toBe(true);
  });

  it('credits a stream that landed while the height changed', () => {
    const before = anchoredAtBottom(2000, 600);
    // 500px of rows arrived and the keyboard took 300px: one gesture to the
    // reader, two events to the hook.
    expect(isResizeAfterFollow(view(1400, 2500, 300), before)).toBe(true);
  });

  it('refuses a resize of a view that was not at the bottom', () => {
    // The follow anchor was taken 600px up the history: that is a reader.
    const before: FollowAnchor = { scrollTop: 800, scrollHeight: 2000, clientHeight: 600 };
    expect(isResizeAfterFollow(view(800, 2000, 300), before)).toBe(false);
  });

  it('refuses when the view moved down, which is what a reader does', () => {
    const before = anchoredAtBottom(2000, 600);
    expect(isResizeAfterFollow(view(1600, 2000, 300), before)).toBe(false);
  });

  it('refuses a gap the height change cannot explain', () => {
    const before = anchoredAtBottom(2000, 600);
    // 600px gone from the viewport, but the view sits 1000px higher: a gesture.
    expect(isResizeAfterFollow(view(400, 2000, 600), before)).toBe(false);
  });

  it('refuses a zero height on either side, and no anchor at all', () => {
    const before = anchoredAtBottom(2000, 600);
    // Zero is the absence of a viewport (a hidden panel), not a shorter one.
    expect(isResizeAfterFollow(view(0, 0, 0), before)).toBe(false);
    expect(isResizeAfterFollow(view(1400, 2000, 300), anchoredAtBottom(2000, 0))).toBe(false);
    expect(isResizeAfterFollow(view(1400, 2000, 300), null)).toBe(false);
    // And an unchanged height is not a resize.
    expect(isResizeAfterFollow(view(1400, 2000, 600), before)).toBe(false);
  });
});

describe('useStickyBottom: a scroller resize is a follow event', () => {
  it('keeps the newest row in view when the keyboard takes 300px of the scroller', () => {
    // 2000px of transcript in a 600px viewport, opened at the newest row.
    const { box, sticky, unmount } = mountScrollBox({ scrollHeight: 2000, clientHeight: 600, scrollTop: 0 });
    try {
      expect(box.scrollTop).toBe(1400);
      expect(sticky().showJump).toBe(false);

      // The on-screen keyboard opens. The scroller is 300px shorter, `scrollTop`
      // is exactly where the follow left it, and the newest row is now 300px
      // below the fold - the state both growth rules refuse to touch.
      box.clientHeight = 300;
      recheck(sticky);

      // Still following: pinned to the new bottom, no pill over it.
      expect(box.scrollTop).toBe(1700);
      expect(sticky().showJump).toBe(false);
    } finally {
      unmount();
    }
  });

  it('closes the gap a collapse and a stream left in the same frame', () => {
    const { box, sticky, unmount } = mountScrollBox({ scrollHeight: 2000, clientHeight: 600, scrollTop: 0 });
    try {
      box.clientHeight = 300;
      box.scrollHeight = 2500;
      recheck(sticky);
      expect(box.scrollTop).toBe(2200);
      expect(sticky().showJump).toBe(false);
    } finally {
      unmount();
    }
  });

  it('leaves a reader who scrolled away before the resize where they were', async () => {
    const { box, node, sticky, unmount } = mountScrollBox({ scrollHeight: 2000, clientHeight: 600, scrollTop: 0 });
    try {
      // The reader takes the view (a touch is a gesture, and it drops the follow).
      box.scrollTop = 400;
      fireEvent.touchStart(node);
      fireEvent.scroll(node);
      await settleFrame();
      expect(sticky().showJump).toBe(true);

      // Only then does the keyboard open: a resize may not take the view back.
      box.clientHeight = 300;
      recheck(sticky);
      expect(box.scrollTop).toBe(400);
      expect(sticky().showJump).toBe(true);
    } finally {
      unmount();
    }
  });

  it('pins to the bottom when a hidden panel is shown again', () => {
    // The Conversation tab is not selected: `display: none` measures nothing at
    // all, for height, content and scroll position alike.
    const { box, sticky, unmount } = mountScrollBox({ scrollHeight: 0, clientHeight: 0, scrollTop: 0 });
    try {
      // A look while hidden publishes nothing - a follow recorded at `{0,0,0}`
      // is what used to leave a stale pill over a transcript at the bottom.
      expect(sticky().showJump).toBe(false);
      recheck(sticky);
      expect(sticky().showJump).toBe(false);

      // The tab is selected and the conversation is laid out.
      box.scrollHeight = 5000;
      box.clientHeight = 600;
      recheck(sticky);

      expect(box.scrollTop).toBe(4400);
      expect(sticky().showJump).toBe(false);
    } finally {
      unmount();
    }
  });

  it('does not re-pin a reader who was in history when the panel was hidden', async () => {
    const { box, node, sticky, unmount } = mountScrollBox({ scrollHeight: 5000, clientHeight: 600, scrollTop: 0 });
    try {
      box.scrollTop = 1200;
      fireEvent.touchStart(node);
      fireEvent.scroll(node);
      await settleFrame();
      expect(sticky().showJump).toBe(true);

      // Hidden (every number a hidden element reports) and shown again at the
      // same size: no gesture has changed, and no resize may take them to the tail.
      box.clientHeight = 0;
      box.scrollHeight = 0;
      recheck(sticky);
      box.clientHeight = 600;
      box.scrollHeight = 5000;
      recheck(sticky);

      expect(box.scrollTop).toBe(1200);
      expect(sticky().showJump).toBe(true);
    } finally {
      unmount();
    }
  });

  it('does not steal a scroll it merely observed for the first time', () => {
    // Nothing was ever laid out (an embedded renderer, jsdom), and the first look
    // with a real box arrives on a scroll event. That is a view that moved, not a
    // resize that has to be undone.
    const { box, sticky, unmount } = mountScrollBox({ scrollHeight: 0, clientHeight: 0, scrollTop: 0 });
    try {
      box.scrollHeight = 1600;
      box.clientHeight = 400;
      box.scrollTop = 200;
      recheck(sticky);
      expect(box.scrollTop).toBe(200);
    } finally {
      unmount();
    }
  });
});

describe('useStickyBottom: a keystroke is not a scroll', () => {
  it('keeps following on Tab, and on a key the transcript control owns', () => {
    const { node, unmount } = mountTranscript();
    try {
      armFollow(node);

      // Tab moves to the next card; it does not move the view.
      fireEvent.keyDown(node, { key: 'Tab' });
      expect(followingNewest(node)).toBe(true);

      // Space and Enter belong to the focused approval button, not to the
      // scrolling of the transcript it happens to live in.
      const approve = node.querySelector('#approve') as HTMLElement;
      fireEvent.keyDown(approve, { key: ' ' });
      expect(followingNewest(node)).toBe(true);
      fireEvent.keyDown(approve, { key: 'Enter' });
      expect(followingNewest(node)).toBe(true);
      // Nor is a character key a reason to stop following the run.
      fireEvent.keyDown(approve, { key: 'a' });
      expect(followingNewest(node)).toBe(true);
    } finally {
      unmount();
    }
  });

  it('ends following when the reader presses a key that scrolls the transcript', () => {
    const { node, unmount } = mountTranscript();
    try {
      armFollow(node);
      fireEvent.keyDown(node, { key: 'ArrowUp' });
      expect(followingNewest(node)).toBe(false);

      armFollow(node);
      fireEvent.keyDown(node, { key: 'PageUp' });
      expect(followingNewest(node)).toBe(false);
    } finally {
      unmount();
    }
  });
});
