import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { RefObject } from 'preact';
import { prefersReducedMotion } from './dom';
import { clearOwnScroll, ownScrollNote } from './ownScroll';

/**
 * Where the last follow left the view, per scroller.
 *
 * `anchorRef` inside the hook is private to it, but the question it answers -
 * "is this gap ours, or did the reader leave?" - is asked by the other component
 * that writes the scroll (`ChatTranscript`'s measurement correction). Two
 * components answering it separately is how a transcript ends up writing two
 * scroll positions per frame: the follow sees growth it has not caught up with
 * and scrolls down, while the correction sees a viewport that is not at the
 * bottom and holds an anchor, and scrolls back up. Same geometry, opposite
 * decisions, every frame.
 */
const lastFollow = new WeakMap<HTMLElement, FollowAnchor>();

/** Record where a follow left the view; `null` forgets it. */
function rememberFollow(element: HTMLElement, anchor: FollowAnchor | null): void {
  if (anchor === null) lastFollow.delete(element);
  else lastFollow.set(element, anchor);
}

/**
 * Record that the app just followed `element` to its newest content.
 *
 * Every writer that moves this view for the reader calls it, so the next
 * "should I follow?" question is answered against the position the app left
 * behind rather than against a reader's.
 */
export function noteFollow(element: HTMLElement): FollowAnchor {
  const anchor = readFollowAnchor(element);
  rememberFollow(element, anchor);
  return anchor;
}

/** Forget the last follow: the reader has taken the view somewhere themselves. */
export function forgetFollow(element: HTMLElement | null): void {
  if (element) rememberFollow(element, null);
}

/**
 * Elements with a smooth scroll the app started still in flight.
 *
 * While one is running the view is on its way to the newest content, so nothing
 * else may move it: a correction that "held an anchor" mid-animation drags the
 * reader back the way they came, and one that jumped to the bottom would fight
 * the animation it is watching.
 */
const settling = new WeakSet<HTMLElement>();

/** Mark a smooth scroll the app started as running (or finished). */
export function noteSettling(element: HTMLElement | null, active: boolean): void {
  if (!element) return;
  if (active) settling.add(element);
  else settling.delete(element);
}

/**
 * True while this scroller is following its newest content.
 *
 * The follow decision itself, not an approximation of it: everything that
 * decides whether the view "should follow" consults this, so the correction and
 * the follow cannot disagree about the reader.
 */
export function followingNewest(element: HTMLElement): boolean {
  if (settling.has(element)) return true;
  const metrics = readMetrics(element);
  // `isPinnedToBottom` is the follow's own tolerance (`PIN_THRESHOLD_PX`), so a
  // reader parked a few pixels above the newest row counts as following here too.
  // That is deliberate: the two writers must agree on the answer, and the
  // threshold is the app's existing definition of "at the bottom". The cost is
  // that the correction stops holding an anchor for that reader - they are close
  // enough to the newest content that a row-height shift is unlikely to matter,
  // and the follow will close the last few pixels on the next growth.
  if (isPinnedToBottom(metrics)) return true;
  return shouldFollow(metrics, lastFollow.get(element) ?? null, false);
}

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
 * True when the current gap is what a *shrink* above the view left behind.
 *
 * The mirror of [`isGrowthAfterFollow`], and it is the case that opens a session
 * short of its own newest row: a follow writes `scrollTop` to the bottom, then
 * the window measures the rows it estimated (they are usually shorter than the
 * estimate), the document gets shorter under the viewport, and the browser
 * clamps `scrollTop` down with it. Nobody scrolled — following is exactly what
 * the reader asked for — but a rule that only credits *growth* reads the clamp
 * as a decision to stop, shows "Jump to latest" at the bottom of the transcript,
 * and leaves the newest row permanently out of reach.
 *
 * The anchor is what makes this safe to act on: a user gesture drops it
 * (`onUserScrollIntent`), so an anchor still in hand means no reader has moved
 * the view since the last follow, and the only thing that can have shortened the
 * document is the app's own layout.
 */
export function isShrinkAfterFollow(
  metrics: ScrollMetrics,
  anchor: FollowAnchor | null,
  threshold: number = PIN_THRESHOLD_PX,
): boolean {
  if (!anchor) return false;
  if (metrics.clientHeight !== anchor.clientHeight) return false;
  const shrink = anchor.scrollHeight - metrics.scrollHeight;
  if (shrink <= 0) return false;
  // The clamp can only have moved the view up by at most the height that went
  // away; anything further is a scroll of its own.
  const moved = anchor.scrollTop - metrics.scrollTop;
  if (moved < -1) return false;
  return moved <= shrink + threshold;
}

/**
 * The decision every follow path shares: keep up with the newest content while
 * the view is at the bottom, while the only reason it is not is content that
 * landed after the last follow, or while a smooth scroll we started is still
 * arriving.
 *
 * Deliberately *not* "…or while we were pinned a moment ago". A recorded pin is
 * one frame behind the reader: a wheel gesture clears the follow anchor, but the
 * frame that would clear the pin (a `requestAnimationFrame` behind the scroll
 * event) has not run yet, and a stream produces growth in every one of those
 * frames. Trusting the pin there re-pins to the tail straight through the
 * gesture — measured in a real browser as a 757px jump away from the reader's
 * finger and then a 58px-per-frame sawtooth as each new delta yanked again,
 * which is the "scrolling jumps me up and I cannot get away from the bottom"
 * report. Live geometry cannot disagree with the reader: the view is at the
 * bottom now, or the gap is explained by our own uncaught-up growth, or nothing
 * moves.
 */
export function shouldFollow(
  metrics: ScrollMetrics,
  anchor: FollowAnchor | null,
  settling: boolean,
): boolean {
  if (settling) return true;
  if (isPinnedToBottom(metrics)) return true;
  if (isGrowthAfterFollow(metrics, anchor)) return true;
  return isShrinkAfterFollow(metrics, anchor);
}

/** How many viewports a jump may still travel with the smooth animation. */
export const JUMP_ANIMATE_VIEWPORTS = 3;

/**
 * How "Jump to latest" should move the view.
 *
 * Smooth scrolling reads as motion over a screen or two and as a strobe over a
 * whole history: asked for 150 000px, Chromium covers it in one or two enormous
 * steps, and every step mounts a screenful of rows it has never drawn. A jump
 * that far is a jump - snap it, and let the window and the scroll position change
 * in the same commit.
 */
export function jumpBehavior(element: HTMLElement): 'auto' | 'smooth' {
  if (!canSmoothScroll(element)) return 'auto';
  return distanceFromBottom(readMetrics(element)) > element.clientHeight * JUMP_ANIMATE_VIEWPORTS
    ? 'auto'
    : 'smooth';
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

/**
 * True when `element` is showing its newest content right now.
 *
 * Exported for the components that move the transcript's scroll themselves
 * (`ChatTranscript`'s measurement correction, `historyLoader`'s reveal): they
 * must record this *before* they write, because afterwards the geometry they
 * left behind is indistinguishable from a reader who scrolled.
 */
export function elementAtBottom(element: HTMLElement): boolean {
  return isPinnedToBottom(readMetrics(element));
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
  scrollRef: RefObject<HTMLDivElement | null>;
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
  /**
   * Ask whether the view should follow again, after something changed the
   * transcript's geometry that no resize would report.
   *
   * The window measures the rows it mounts (`components/ChatTranscript`), and a
   * row that turns out not to be the height its estimate claimed shifts
   * everything below it — while the document's *total* height can stay exactly
   * the same, because the bottom padding absorbed the difference. Nothing then
   * fires, so a reader following the newest content is left standing 27px short
   * of it with no event to correct them. This is the missing event.
   */
  recheck: () => void;
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
  const settlingRef = useRef(false);
  const settleTimerRef = useRef(0);
  const frameRef = useRef(0);
  const sessionRef = useRef<string | null | undefined>(undefined);
  const sentRef = useRef(sentMessageTick);
  /**
   * Where the view stood the last time it was followed to the bottom. A later
   * measurement subtracts this height so that content arriving *after* a write
   * is credited as growth we have not caught up with yet, instead of being
   * mistaken for the reader scrolling away (see `syncPinnedState`).
   */
  const anchorRef = useRef<FollowAnchor | null>(null);
  /** The element a smooth scroll is running on, so its settle flag can be cleared. */
  const settleElRef = useRef<HTMLElement | null>(null);
  /** The follow, for the settle timer that runs after it is defined. */
  const reconcileRef = useRef<(() => void) | null>(null);
  const [showJump, setShowJump] = useState(false);

  const endSettle = useCallback(() => {
    settlingRef.current = false;
    noteSettling(settleElRef.current, false);
    settleElRef.current = null;
    if (settleTimerRef.current !== 0) {
      window.clearTimeout(settleTimerRef.current);
      settleTimerRef.current = 0;
    }
  }, []);

  /**
   * Record where the view stands as "the last follow" - inside the hook, and for
   * the other components that ask the same question (`followingNewest`).
   */
  const markFollow = useCallback((element: HTMLElement) => {
    anchorRef.current = noteFollow(element);
  }, []);

  /** The reader has left: forget the follow, inside the hook and outside it. */
  const dropFollow = useCallback((element: HTMLElement | null) => {
    anchorRef.current = null;
    forgetFollow(element);
  }, []);

  const pinToBottom = useCallback(
    (behavior: 'auto' | 'smooth') => {
      const element = scrollRef.current;
      if (!element) return;
      setShowJump(false);
      if (behavior === 'smooth' && canSmoothScroll(element)) {
        settlingRef.current = true;
        settleElRef.current = element;
        noteSettling(element, true);
        if (settleTimerRef.current !== 0) window.clearTimeout(settleTimerRef.current);
        settleTimerRef.current = window.setTimeout(() => {
          settlingRef.current = false;
          const element = settleElRef.current;
          noteSettling(element, false);
          settleElRef.current = null;
          settleTimerRef.current = 0;
          // A native smooth scroll can land a few pixels short, and content that
          // arrived while it ran was not followed (nothing may move the view
          // mid-animation). Close whatever is left *here*, once: this is a jump
          // the app started, so no gesture can be mid-flight - the same write on
          // every frame would drag a reader who is scrolling.
          if (element && isPinnedToBottom(readMetrics(element))) {
            scrollToBottom(element, 'auto');
            markFollow(element);
          }
          reconcileRef.current?.();
        }, SETTLE_MS);
      } else {
        endSettle();
      }
      scrollToBottom(element, behavior);
      // A smooth scroll has not arrived yet; the measurement that ends it (or a
      // later follow) records the anchor instead.
      //
      // A pin on an element that has not been laid out yet (opening a session
      // runs before the panel has its height) records *no* anchor on purpose:
      // both follow rules refuse an anchor whose viewport height changed, so an
      // anchor taken at `clientHeight: 0` poisons them for the whole session and
      // the transcript never catches up with its own first measurement. No anchor
      // means "nothing to compare against", which is the truth.
      if (!settlingRef.current && element.clientHeight > 0) {
        markFollow(element);
      }
    },
    [endSettle, markFollow],
  );

  const jumpToLatest = useCallback(() => {
    const element = scrollRef.current;
    pinToBottom(element ? jumpBehavior(element) : 'smooth');
    // The button unmounts on click; keep keyboard users inside the transcript
    // instead of dropping focus back to the document.
    if (element && typeof element.focus === 'function') element.focus({ preventScroll: true });
  }, [pinToBottom]);

  /**
   * Handle a scroll the app wrote itself, and say whether it was one.
   *
   * The windowed transcript moves `scrollTop` while it re-measures the rows it
   * swapped for padding, and the history loader moves it to reveal a page
   * (`lib/ownScroll.ts` carries the note). Both write while the reader is doing
   * nothing at all, and to the follow rules they look exactly like a gesture: the
   * position is not where the last follow left it, and the gap is not "growth we
   * have not caught up with" either. Reading one as a gesture is what parked an
   * opened session 97px short of its newest row, with "Jump to latest" showing on
   * a transcript nobody had touched.
   *
   * The note answers the only question that matters, and it answered it *before*
   * the write: was the newest content on screen at that moment? If it was, this
   * correction may not leave the reader short of it, so the gap it opened is
   * closed. If it was not, the reader is reading history and nothing may move
   * them — the anchor is dropped so the next correction cannot be mistaken for
   * growth, and the affordance is published so they can still get back.
   */
  const handleOwnScroll = useCallback((element: HTMLElement): boolean => {
    const note = ownScrollNote(element);
    if (!note) return false;
    clearOwnScroll(element);
    if (note.atBottom) {
      // The correction landed while the reader was showing the newest content,
      // so it must not leave them short of it: re-pin. The note recorded that
      // position before the write, which no flag on the hook can supply.
      scrollToBottom(element, 'auto');
      markFollow(element);
      setShowJump(false);
    } else {
      // Reading history: a correction may not pull them back, but it must not
      // silence the affordance either. The early return is the whole trap — a
      // gap that is the reader's is exactly the gap "Jump to latest" exists for,
      // and a correction that lands on the same frame as their scroll used to
      // leave the button unmounted with the view at the top of a long history.
      dropFollow(element);
      if (!isPinnedToBottom(readMetrics(element))) setShowJump(true);
    }
    return true;
  }, []);

  /**
   * The one follow decision, and the one place "the reader is away" is published.
   *
   * `showJump` used to be written only on a scroll event, which is a lie the
   * geometry can tell without one: a stream adds height below the viewport
   * without moving `scrollTop` at all, so the newest content slid out of reach
   * with no scroll to notice it — 4 000px short of the bottom, no affordance to
   * get back, and no way for the reader to know why ("Jump to latest doesn't
   * take me there" is the same bug once the button is stale). Every path that
   * learns the current geometry now re-publishes the affordance.
   */
  const reconcile = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return;
    // The observer can fire on the same frame as the window's measurement
    // correction. Settling that write first is the difference between closing the
    // gap it opened and treating it as a reader who scrolled away.
    if (handleOwnScroll(element)) return;
    const metrics = readMetrics(element);
    if (!shouldFollow(metrics, anchorRef.current, settlingRef.current)) {
      if (!settlingRef.current && !isPinnedToBottom(metrics)) setShowJump(true);
      return;
    }
    if (isPinnedToBottom(metrics)) {
      markFollow(element);
      return;
    }
    scrollToBottom(element, 'auto');
    markFollow(element);
  }, [handleOwnScroll, markFollow]);
  reconcileRef.current = reconcile;

  /**
   * Ask the follow to look again after a measurement moved the rows, without
   * interrupting a jump the app started.
   *
   * A measurement is not new content: while a smooth scroll is still arriving,
   * following from here would write an instant `scrollTop` and cut the animation
   * short - the very thing `noteSettling` exists to prevent. The settle timer
   * closes whatever is left once the animation lands.
   */
  const recheck = useCallback(() => {
    if (settlingRef.current) return;
    reconcile();
  }, [reconcile]);

  // Scroll tracking plus layout observation for reflow that produces no text delta.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const content = element.firstElementChild;

    const syncPinnedState = () => {
      frameRef.current = 0;
      const metrics = readMetrics(element);
      if (isPinnedToBottom(metrics)) {
        markFollow(element);
        endSettle();
        setShowJump(false);
        return;
      }
      // A scroll the app made itself — the window measuring the rows it swapped
      // for padding, the history loader revealing a page — is not the reader
      // moving. Settling it here is what stops the measurement correction from
      // being read as a gesture.
      if (handleOwnScroll(element)) return;
      // Not at the bottom. The transcript's own relayout moves `scrollTop` by a
      // few pixels while it re-lays the rows it swapped for padding, so "the
      // view moved" is not automatically "the reader moved"; the gap only
      // becomes their decision once it is bigger than anything the layout did.
      // Deciding that is `shouldFollow`'s job, and it is the same decision the
      // observer and stream paths take.
      if (shouldFollow(metrics, anchorRef.current, settlingRef.current)) {
        scrollToBottom(element, 'auto');
        markFollow(element);
        setShowJump(false);
        return;
      }
      setShowJump(!settlingRef.current);
    };

    const onScroll = () => {
      if (frameRef.current !== 0) return;
      frameRef.current = requestFrame(syncPinnedState);
    };

    // Growth without a text delta: Markdown finishing, a card expanding, an
    // approval arriving at the end of the transcript.
    const followGrowth = reconcile;

    // A real scroll gesture wins over a smooth scroll we started, and it also
    // ends following: from here on the view goes wherever the wheel takes it,
    // however much content lands meanwhile. Clearing the *pin* is the load-
    // bearing half — a stream grows in every frame, so a pin that outlives the
    // gesture is a licence to snap back to the tail under the reader's hand.
    const onUserScrollIntent = () => {
      endSettle();
      dropFollow(element);
      // A gesture also ends the app's own write: a correction the reader has
      // already scrolled away from is history, and it may not re-pin them.
      clearOwnScroll(element);
    };
    element.addEventListener('wheel', onUserScrollIntent, { passive: true });
    element.addEventListener('touchstart', onUserScrollIntent, { passive: true });
    element.addEventListener('keydown', onUserScrollIntent);
    // A drag of the scrollbar is a reader decision too, and it produces no wheel
    // event to say so.
    element.addEventListener('pointerdown', onUserScrollIntent, { passive: true });

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
      element.removeEventListener('pointerdown', onUserScrollIntent);
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      if (frameRef.current !== 0) cancelFrame(frameRef.current);
      frameRef.current = 0;
      endSettle();
    };
  }, [endSettle, reconcile, dropFollow]);

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
  //
  // A *layout* effect, not an effect: this write has to land in the frame that
  // added the content, before the browser paints it. As a passive effect it ran
  // after the paint, and the `ResizeObserver` below - which sees the same growth
  // a moment later - wrote again for it. A profile of a real Firefox session
  // caught exactly that: two `scrollTop` writes ~16ms apart per frame with
  // nobody at the wheel, a `scrollend` between them, and 170 forced reflows a
  // second as each scroll event re-measured the window. One follow per frame,
  // before the paint, is the whole budget.
  useLayoutEffect(() => {
    reconcile();
  }, [contentKey, reconcile]);

  // Sending a message means the reader wants to see the answer.
  useEffect(() => {
    if (sentRef.current === sentMessageTick) return;
    sentRef.current = sentMessageTick;
    pinToBottom('smooth');
  }, [sentMessageTick, pinToBottom]);

  return { scrollRef, showJump, scrolledAway: showJump, recheck, jumpToLatest };
}