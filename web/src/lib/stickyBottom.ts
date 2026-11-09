import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { RefObject } from 'preact';
import { prefersReducedMotion } from './dom';

/** Distance from the bottom (px) that still counts as "following the newest content". */
export const PIN_THRESHOLD_PX = 48;

/**
 * How long a programmatic smooth scroll may run before scroll events are
 * trusted again. Until then the view is treated as pinned so late reflow keeps
 * up with it, and the "jump to latest" affordance stays hidden.
 */
const SETTLE_MS = 700;

export interface ScrollMetrics {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

/**
 * Where following last left the view: `scrollTop`, `scrollHeight` and
 * `clientHeight` read back immediately after a write to the bottom.
 *
 * It exists to tell two situations apart that look identical in raw geometry. A
 * gap under the viewport can mean either "the reader scrolled away" or "content
 * landed after we followed and we have not caught up yet". Streamed output
 * produces gaps of the second kind constantly — with fast tools a single frame
 * can add a thousand pixels — and treating those as a reader decision is what
 * stopped auto-following for good.
 */
export interface FollowAnchor {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** Pixels of content hidden below the viewport. */
export function distanceFromBottom(metrics: ScrollMetrics): number {
  return Math.max(0, metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight);
}

/** True when the view is close enough to the newest content to keep following it. */
export function isPinnedToBottom(
  metrics: ScrollMetrics,
  threshold: number = PIN_THRESHOLD_PX,
): boolean {
  return distanceFromBottom(metrics) <= threshold;
}

function readMetrics(element: HTMLElement): ScrollMetrics {
  return {
    scrollHeight: element.scrollHeight,
    scrollTop: element.scrollTop,
    clientHeight: element.clientHeight,
  };
}

/** Records where a follow to the bottom actually left the view. */
export function readFollowAnchor(element: HTMLElement): FollowAnchor {
  return {
    scrollTop: element.scrollTop,
    scrollHeight: element.scrollHeight,
    clientHeight: element.clientHeight,
  };
}

/** True when every pixel of the current gap can be explained by content that
 * arrived after [`anchor`] was taken.
 *
 * A follow writes `scrollTop` to the bottom and nothing else: content appended
 * below the viewport grows `scrollHeight` while leaving `scrollTop` where it
 * was. So "the scroll position has not moved but the content got taller" is our
 * own gap, and catching up with it is not fighting anybody. Anything else — the
 * reader scrolling up to read history, or even scrolling down without reaching
 * the end — is a decision, and none of these follow paths override a decision.
 */
export function isGrowthAfterFollow(
  metrics: ScrollMetrics,
  anchor: FollowAnchor | null,
  threshold: number = PIN_THRESHOLD_PX,
): boolean {
  if (!anchor) return false;
  // Stale anchor: two heights that no longer describe one geometry (an
  // accordion opened, the window was resized, a phone was rotated).
  if (metrics.clientHeight !== anchor.clientHeight) return false;
  // Any movement of the viewport is the reader, not us.
  if (Math.abs(metrics.scrollTop - anchor.scrollTop) > 1) return false;
  // Content only grows; a shorter box means a new layout, not growth.
  const growth = metrics.scrollHeight - anchor.scrollHeight;
  if (growth <= 0) return false;
  // And the gap must be no wider than that growth (plus ordinary rounding).
  return distanceFromBottom(metrics) <= growth + threshold;
}

/**
 * The decision every follow path shares: keep up with the newest content while
 * the view is at the bottom, or while the only reason it is not is content that
 * landed after the last follow. `pinned` and `settling` are the recorded state,
 * so a follow already in progress is never interrupted by the growth that
 * caused it.
 */
export function shouldFollow(
  metrics: ScrollMetrics,
  anchor: FollowAnchor | null,
  pinned: boolean,
  settling: boolean,
): boolean {
  if (pinned || settling) return true;
  if (isPinnedToBottom(metrics)) return true;
  return isGrowthAfterFollow(metrics, anchor);
}

/** Native smooth scrolling is unavailable in jsdom and old browsers, and unwanted when motion is reduced. */
export function canSmoothScroll(element: HTMLElement | null): boolean {
  return (
    element !== null && typeof element.scrollTo === 'function' && !prefersReducedMotion()
  );
}

function scrollToBottom(element: HTMLElement, behavior: 'auto' | 'smooth'): void {
  if (behavior === 'smooth' && canSmoothScroll(element)) {
    element.scrollTo({ top: element.scrollHeight, behavior: 'smooth' });
    return;
  }
  // Instant path: assigning scrollTop is clamped by the browser to the maximum.
  element.scrollTop = element.scrollHeight;
}

const hasAnimationFrame =
  typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function';

/** rAF when available, a timer otherwise (jsdom), so a scroll burst costs at most one measurement per frame. */
function requestFrame(callback: () => void): number {
  if (hasAnimationFrame) return window.requestAnimationFrame(callback);
  return window.setTimeout(callback, 16);
}

function cancelFrame(handle: number): void {
  if (hasAnimationFrame) window.cancelAnimationFrame(handle);
  else window.clearTimeout(handle);
}

export interface StickyBottomOptions {
  /** Conversation identity. A change jumps to the newest content without animation. */
  sessionKey: string | null;
  /** Changes whenever the transcript grows: item count plus streamed tail length. */
  contentKey: string;
  /** Bumped when the user sends a message; the view re-pins and follows the answer. */
  sentMessageTick: number;
}

export interface StickyBottom {
  scrollRef: RefObject<HTMLDivElement>;
  /** True while the newest content is out of view and worth jumping to. */
  showJump: boolean;
  /**
   * True while the reader is away from the newest content — the dock's cue to
   * step back so the transcript behind it shows through.
   *
   * It is the same condition as `showJump` and kept as a separate name because
   * the two read differently at the call site: one decides whether to show a
   * button, the other whether to fade chrome. Both want "the reader is reading
   * history rather than following the run", and both must ignore a smooth scroll
   * we started ourselves — measuring raw geometry would flicker the dock
   * translucent halfway through an automatic scroll to the bottom.
   */
  scrolledAway: boolean;
  /** Scrolls to the newest content, re-pins and moves focus into the transcript. */
  jumpToLatest: () => void;
}

/**
 * Sticky-bottom scrolling for the chat transcript.
 *
 * The view follows the newest content while pinned (within `PIN_THRESHOLD_PX`
 * of the bottom) across *all* growth: streamed text, Markdown that renders
 * late, expanded tool cards and approval cards. As soon as the reader scrolls
 * up, nothing calls scroll until they ask for it again.
 */
export function useStickyBottom(options: StickyBottomOptions): StickyBottom {
  const { sessionKey, contentKey, sentMessageTick } = options;
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const settlingRef = useRef(false);
  const settleTimerRef = useRef(0);
  const frameRef = useRef(0);
  const sessionRef = useRef<string | null | undefined>(undefined);
  const sentRef = useRef(sentMessageTick);
  // Where the view stood the last time it was followed to the bottom. A later
  // measurement subtracts this height so that content arriving *after* a write
  // is credited as growth we have not caught up with yet, instead of being
  // mistaken for the reader scrolling away (see `syncPinnedState`).
  const anchorRef = useRef<FollowAnchor | null>(null);
  const [showJump, setShowJump] = useState(false);

  const endSettle = useCallback(() => {
    settlingRef.current = false;
    if (settleTimerRef.current !== 0) {
      window.clearTimeout(settleTimerRef.current);
      settleTimerRef.current = 0;
    }
  }, []);

  const pinToBottom = useCallback(
    (behavior: 'auto' | 'smooth') => {
      const element = scrollRef.current;
      if (!element) return;
      pinnedRef.current = true;
      setShowJump(false);
      if (behavior === 'smooth' && canSmoothScroll(element)) {
        settlingRef.current = true;
        if (settleTimerRef.current !== 0) window.clearTimeout(settleTimerRef.current);
        settleTimerRef.current = window.setTimeout(() => {
          settlingRef.current = false;
          settleTimerRef.current = 0;
        }, SETTLE_MS);
      } else {
        endSettle();
      }
      scrollToBottom(element, behavior);
      // A smooth scroll has not arrived yet; the measurement that ends it (or a
      // later follow) records the anchor instead.
      if (!settlingRef.current) anchorRef.current = readFollowAnchor(element);
    },
    [endSettle],
  );

  const jumpToLatest = useCallback(() => {
    const element = scrollRef.current;
    pinToBottom('smooth');
    // The button unmounts on click; keep keyboard users inside the transcript
    // instead of dropping focus back to the document.
    if (element && typeof element.focus === 'function') element.focus({ preventScroll: true });
  }, [pinToBottom]);

  // Scroll tracking plus layout observation for reflow that produces no text delta.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const content = element.firstElementChild;

    const syncPinnedState = () => {
      frameRef.current = 0;
      const metrics = readMetrics(element);
      if (isPinnedToBottom(metrics)) {
        pinnedRef.current = true;
        anchorRef.current = readFollowAnchor(element);
        endSettle();
        setShowJump(false);
        return;
      }
      // Not at the bottom. That is only the reader's decision if the gap is
      // bigger than the content that arrived since the last follow: our own
      // writes are rounded, and a big tool card can land between the write and
      // this measurement.
      if (isGrowthAfterFollow(metrics, anchorRef.current)) {
        pinnedRef.current = true;
        setShowJump(false);
        scrollToBottom(element, 'auto');
        anchorRef.current = readFollowAnchor(element);
        return;
      }
      pinnedRef.current = false;
      setShowJump(!settlingRef.current);
    };

    const onScroll = () => {
      if (frameRef.current !== 0) return;
      frameRef.current = requestFrame(syncPinnedState);
    };

    // Growth without a text delta: Markdown finishing, a card expanding, an
    // approval arriving at the end of the transcript.
    const followGrowth = () => {
      const metrics = readMetrics(element);
      if (!shouldFollow(metrics, anchorRef.current, pinnedRef.current, settlingRef.current)) return;
      if (isPinnedToBottom(metrics)) {
        anchorRef.current = readFollowAnchor(element);
        return;
      }
      scrollToBottom(element, 'auto');
      anchorRef.current = readFollowAnchor(element);
    };

    // A real scroll gesture wins over a smooth scroll we started, and it also
    // drops the follow anchor: from here on the view goes wherever the wheel
    // takes it, however much content lands meanwhile.
    const onUserScrollIntent = () => {
      endSettle();
      anchorRef.current = null;
    };
    element.addEventListener('wheel', onUserScrollIntent, { passive: true });
    element.addEventListener('touchstart', onUserScrollIntent, { passive: true });
    element.addEventListener('keydown', onUserScrollIntent);

    element.addEventListener('scroll', onScroll, { passive: true });

    let resizeObserver: ResizeObserver | null = null;
    let mutationObserver: MutationObserver | null = null;
    if (typeof ResizeObserver === 'function') {
      resizeObserver = new ResizeObserver(followGrowth);
      resizeObserver.observe(element);
      if (content) resizeObserver.observe(content);
    } else if (typeof MutationObserver === 'function' && content) {
      mutationObserver = new MutationObserver(followGrowth);
      mutationObserver.observe(content, { childList: true, subtree: true, characterData: true });
    }

    return () => {
      element.removeEventListener('scroll', onScroll);
      element.removeEventListener('wheel', onUserScrollIntent);
      element.removeEventListener('touchstart', onUserScrollIntent);
      element.removeEventListener('keydown', onUserScrollIntent);
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      if (frameRef.current !== 0) cancelFrame(frameRef.current);
      frameRef.current = 0;
      endSettle();
    };
  }, [endSettle]);

  // A different conversation always starts at the newest message, instantly.
  //
  // A layout effect, because the windowed transcript renders the tail window
  // before it knows where the viewport is: pinning before paint means the reader
  // never sees the blank band above it for a frame.
  useLayoutEffect(() => {
    if (sessionRef.current === sessionKey) return;
    const firstRender = sessionRef.current === undefined;
    sessionRef.current = sessionKey;
    if (firstRender && sessionKey === null) return;
    pinToBottom('auto');
  }, [sessionKey, pinToBottom]);

  // Streamed deltas and appended items. Same decision as the observer path: a
  // gap caused by content that arrived after the last follow is caught up with,
  // a gap caused by the reader is left alone.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const metrics = readMetrics(element);
    if (!shouldFollow(metrics, anchorRef.current, pinnedRef.current, settlingRef.current)) return;
    if (isPinnedToBottom(metrics)) {
      anchorRef.current = readFollowAnchor(element);
      return;
    }
    scrollToBottom(element, 'auto');
    anchorRef.current = readFollowAnchor(element);
  }, [contentKey]);

  // Sending a message means the reader wants to see the answer.
  useEffect(() => {
    if (sentRef.current === sentMessageTick) return;
    sentRef.current = sentMessageTick;
    pinToBottom('smooth');
  }, [sentMessageTick, pinToBottom]);

  return { scrollRef, showJump, scrolledAway: showJump, jumpToLatest };
}