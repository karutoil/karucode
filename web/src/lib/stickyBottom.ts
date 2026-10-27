import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
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
      if (isPinnedToBottom(readMetrics(element))) {
        pinnedRef.current = true;
        endSettle();
        setShowJump(false);
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
      if (!pinnedRef.current && !settlingRef.current) return;
      if (isPinnedToBottom(readMetrics(element))) return;
      scrollToBottom(element, 'auto');
    };

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

    // A real scroll gesture wins over a smooth scroll we started.
    const onUserScrollIntent = () => endSettle();
    element.addEventListener('wheel', onUserScrollIntent, { passive: true });
    element.addEventListener('touchstart', onUserScrollIntent, { passive: true });
    element.addEventListener('keydown', onUserScrollIntent);

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
  useEffect(() => {
    if (sessionRef.current === sessionKey) return;
    const firstRender = sessionRef.current === undefined;
    sessionRef.current = sessionKey;
    if (firstRender && sessionKey === null) return;
    pinToBottom('auto');
  }, [sessionKey, pinToBottom]);

  // Streamed deltas and appended items.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    if (!pinnedRef.current && !settlingRef.current) return;
    if (isPinnedToBottom(readMetrics(element))) return;
    scrollToBottom(element, 'auto');
  }, [contentKey]);

  // Sending a message means the reader wants to see the answer.
  useEffect(() => {
    if (sentRef.current === sentMessageTick) return;
    sentRef.current = sentMessageTick;
    pinToBottom('smooth');
  }, [sentMessageTick, pinToBottom]);

  return { scrollRef, showJump, jumpToLatest };
}