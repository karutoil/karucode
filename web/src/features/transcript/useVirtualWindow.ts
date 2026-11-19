import type { RefObject } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { isOwnScroll, noteOwnScroll } from '../../lib/ownScroll';
import { elementAtBottom, followingNewest } from '../../lib/stickyBottom';
import { prefixHeights, visibleRange } from '../../lib/virtualWindow';
import type { ChatItem } from '../../state/chat';
import { estimateEntryHeight } from './entries';
import type { Entry } from './types';
import { useTranscriptEntries } from './useTranscriptEntries';

/**
 * Below this many rows the transcript renders every row instead of windowing.
 *
 * A window is only worth its padding above this size: the padding changes on
 * every scroll, and a padding change moves the content under the reader no
 * matter how the scroll is corrected afterwards. A short conversation has
 * nothing to gain from it, and `content-visibility` is off for every row now
 * (see `styles/chat.css`), so an unwindowed short transcript lays out its rows
 * for real either way. Reported, and first fixed, in PR #16.
 */
const VIRTUAL_MIN_ENTRIES = 150;
/** Kept mounted above and below the viewport, so a wheel tick never reveals a hole. */
const OVERSCAN_MIN_PX = 480;
/** Viewport assumed for the first paint, before the real element is measured. */
const FALLBACK_VIEWPORT_PX = 900;

export interface VirtualWindowState {
  transcriptRef: RefObject<HTMLDivElement | null>;
  virtual: boolean;
  pending: boolean;
  topPadding: number;
  bottomPadding: number;
  visible: Entry[];
  freshVisible: Set<string>;
}

export function useVirtualWindow({
  items,
  viewportRef,
  contentRef,
  sessionKey = null,
  onMeasured,
}: {
  items: ChatItem[];
  viewportRef?: RefObject<HTMLElement | null>;
  contentRef?: RefObject<HTMLElement | null>;
  sessionKey?: string | null;
  onMeasured?: () => void;
}): VirtualWindowState {
  const { entries, collectFresh } = useTranscriptEntries(items, sessionKey);

  const transcriptRef = useRef<HTMLDivElement>(null);
  // The transcript element is the start of the loaded rows: the history loader
  // anchors on its top edge when it has to reveal a page that arrived while the
  // reader was above every row. Published rather than passed as a callback ref so
  // the internal ref above stays the one that measures rows.
  useLayoutEffect(() => {
    if (!contentRef) return;
    contentRef.current = transcriptRef.current;
    return () => {
      contentRef.current = null;
    };
  }, [contentRef]);
  /** Last measured height per entry key; estimates fill the gaps. */
  const measured = useRef(new Map<string, number>());
  const [measuredTick, setMeasuredTick] = useState(0);
  /**
   * The first row at or below the top of the viewport, and where it stood when
   * it was read: the row the reader's eye is on.
   *
   * Measuring a row rewrites the window's arithmetic, and correcting the scroll
   * from the *estimate* it is replacing assumes the row had been laid out at that
   * estimate. It had not: Chromium renders rows near the viewport, so a row that
   * was 820px on screen could be modelled at 200px, and the "correction" threw the
   * reader 600px for a layout that never moved. This anchor is measured instead of
   * calculated - the row's on-screen movement minus the scroll's - so the
   * correction is exactly the layout shift, whatever the estimate said.
   */
  const rowAnchor = useRef<{ key: string; top: number; scrollTop: number } | null>(null);
  /**
   * Where the top of the viewport falls *in the transcript's own coordinates*,
   * or `null` until the first real reading. A null top draws the tail, which is
   * where every session opens even though the element is only scrolled there
   * after the sticky-bottom effect runs.
   *
   * Measured from the two rectangles rather than read as `scrollTop`: the
   * transcript does not start at the top of the scroller (the history marker and
   * the banners sit above it), so `scrollTop` names a point ~100px lower than the
   * viewport really is. That put the window one row behind the reader - the top
   * sliver of the viewport could be unmounted, and a row was first measured only
   * as it became visible, which is exactly when its measurement is a jump.
   *
   * Tagged with the conversation: a row list replaced under a new session must
   * not reuse the previous session's scroll position while the pin lands.
   */
  const [windowReading, setWindowReading] = useState<{ key: string | null; top: number } | null>(null);
  const windowTop = windowReading && windowReading.key === sessionKey ? windowReading.top : null;
  /** The effect below is installed once; it must tag readings with the current session. */
  const sessionKeyRef = useRef(sessionKey);
  sessionKeyRef.current = sessionKey;
  const [viewportHeight, setViewportHeight] = useState<number | null>(null);
  /**
   * The last non-zero viewport height. A hidden chat panel (the Terminal tab is
   * open) measures zero, and that is not a reason to mount a whole long history:
   * the window keeps the height it had. Only a viewport that has never measured
   * anything - jsdom, an embedded renderer - falls back to rendering every row.
   */
  const seenHeight = useRef(0);

  useEffect(() => {
    const element = viewportRef?.current;
    if (!element) return;
    /**
     * Record the row at the top of the viewport *before* the next render moves
     * anything, so the measuring effect can put it back where it was.
     */
    const anchor = (): void => {
      const root = transcriptRef.current;
      if (!root) return;
      const view = element.getBoundingClientRect();
      rowAnchor.current = null;
      for (const row of root.children) {
        const box = row.getBoundingClientRect();
        if (box.bottom <= view.top) continue;
        rowAnchor.current = {
          key: row.getAttribute('data-entry-key') ?? '',
          top: box.top,
          scrollTop: element.scrollTop,
        };
        return;
      }
    };
    /** The viewport top in the transcript's own coordinates, plus the anchor. */
    const readWindow = (): void => {
      const root = transcriptRef.current;
      if (!root) return;
      const view = element.getBoundingClientRect();
      const box = root.getBoundingClientRect();
      // No layout at all (jsdom, an embedded renderer that has not been laid
      // out): the rectangles say nothing, so the scroll offset is the only
      // measurement there is. A real transcript always has a height.
      const top = box.height > 0 ? view.top - box.top : element.scrollTop;
      setWindowReading((previous) =>
        // A reading that has not moved is not news. Committing it would run the
        // measuring effect - the one that forces a layout - for a viewport that
        // is exactly where it already was.
        previous !== null &&
        previous.key === sessionKeyRef.current &&
        Math.abs(previous.top - top) < 0.5
          ? previous
          : { key: sessionKeyRef.current, top },
      );
    };
    /**
     * Re-read the window on the scroll itself, not on the next animation frame.
     *
     * A frame of delay is invisible while the wheel moves the view a little, and
     * obvious when something moves it a long way at once: "Jump to latest" sets
     * the scroll to the tail and, one frame later, the viewport is still covered
     * by the padding of the window that was there before - an empty screen
     * between two frames. Reading synchronously means the row set and the scroll
     * position change in the same commit, before the frame is painted.
     *
     * The anchor is read first and in the same handler: a measurement commit can
     * land between the scroll and the next frame, and it must see an anchor that
     * already includes this scroll (otherwise the correction adds the scroll
     * delta on top of the layout shift).
     */
    const onScroll = (): void => {
      // A scroll this component wrote itself - the measurement correction below
      // - leaves the reader exactly where they were: the row at the top of the
      // viewport is still at the top by construction. Re-reading it would rewrite
      // the window for a viewport that never moved: new padding, a new layout
      // shift, another correction, another scroll event. That is the loop a real
      // profile showed running at the frame rate with nobody at the wheel.
      if (isOwnScroll(element)) return;
      anchor();
      readWindow();
    };
    const measure = (): void => {
      const height = element.clientHeight;
      if (height > 0) {
        seenHeight.current = height;
        setViewportHeight(height);
      } else if (seenHeight.current === 0) {
        setViewportHeight(0);
      }
      // A session that fits the viewport is both top and bottom; a long one
      // starts at the tail and is only pinned to the bottom after this effect.
      if (element.scrollTop > 0 || element.scrollHeight <= element.clientHeight + 1) {
        anchor();
        readWindow();
      }
    };
    element.addEventListener('scroll', onScroll, { passive: true });
    measure();
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver === 'function') {
      observer = new ResizeObserver(measure);
      observer.observe(element);
    }
    return () => {
      element.removeEventListener('scroll', onScroll);
      observer?.disconnect();
    };
  }, [viewportRef]);

  const estimates = useMemo(() => entries.map(estimateEntryHeight), [entries]);
  const heights = useMemo(
    () => entries.map((entry, index) => measured.current.get(entry.key) ?? estimates[index] ?? 0),
    [entries, estimates, measuredTick],
  );
  const geometry = useMemo(() => prefixHeights(heights), [heights]);

  // `viewportHeight === null` is the first paint (the scroll effect has not run
  // yet); it still windows the tail so a long session never mounts every row.
  // A measured zero (jsdom, or a hidden panel) renders everything, because a
  // window without a viewport would hide most of the transcript for good.
  //
  // A short conversation is not windowed at all: windowing it buys nothing and
  // its padding is rewritten on every scroll, which is the one thing that can
  // move the content under the reader (see `VIRTUAL_MIN_ENTRIES`).
  const windowed = entries.length > VIRTUAL_MIN_ENTRIES;
  const virtual = windowed && viewportRef !== undefined && viewportHeight !== null && viewportHeight > 0;
  const pending = windowed && viewportRef !== undefined && viewportHeight === null;
  const bandHeight = viewportHeight !== null && viewportHeight > 0 ? viewportHeight : FALLBACK_VIEWPORT_PX;
  const bandTop = windowTop ?? geometry.total;
  const overscan = Math.max(bandHeight, OVERSCAN_MIN_PX);
  const range = virtual || pending
    ? visibleRange(geometry.offsets, bandTop, bandHeight, overscan)
    : { start: 0, end: entries.length };

  /**
   * The bottom margin the mounted rows carry, cached for the layout it was read
   * in: the density token on `<html>` plus the viewport's box. See the measuring
   * effect for why one reading can serve every row.
   */
  const rowMargin = useRef<{ key: string; margin: number; lastMargin: number | null } | null>(null);

  // Measure the mounted rows after every commit that can have changed one.
  //
  // Every mounted row is measured, not just the ones in view. No message row is
  // skipped by the browser any more (`content-visibility: visible` on `.msg`),
  // so each mounted row has a real box to read; measuring them all means the
  // window's arithmetic matches the DOM for the whole window,
  // including the overscan the reader is about to reach. Measuring only the
  // visible rows left the rest at their estimate, and the difference is what the
  // reader saw as a jump every time the window moved over an unmeasured row.
  //
  // Measuring changes the layout, and the reader must not see it: the anchor
  // below is the row at the top of the viewport, read before this commit. Its
  // screen movement minus the scroller's movement is exactly how far the layout
  // shifted under the reader, so subtracting it leaves the page still. Deriving
  // that from the estimate being replaced is what made the transcript jump by
  // hundreds of pixels per frame while scrolling (the estimate and the row's
  // actual box disagree by that much), so the geometry is only asked for
  // heights, never for corrections.
  useLayoutEffect(() => {
    if (!virtual) return;
    const root = transcriptRef.current;
    const element = viewportRef?.current;
    if (!root || !element) return;
    const rows = root.children;
    // The gap under a row is a property of the layout, not of the row: every
    // child of a *windowed* transcript carries the same bottom margin (`.msg` is
    // `margin: 0 0 var(--space-10)`, the date marker
    // `margin: var(--space-2) auto var(--space-10)`), and the one rule that
    // removes it (`.transcript:not(.is-windowed) > :last-child`) cannot match
    // while this effect runs. `getComputedStyle` forces the style system to
    // resolve the element it is asked about, so asking all ~200 mounted rows on
    // every commit spent the scroll in style resolution. Read once from a sample
    // row instead, and drop the reading when the thing that can change it - the
    // density token on `<html>` or the viewport's box - has changed. If a row
    // type ever needs a different gap, key this cache by the row's class too:
    // nothing below may assume there is only one value.
    const density = document.documentElement.dataset.density ?? '';
    const layoutKey = `${density}|${element.clientWidth}x${element.clientHeight}`;
    let margin = rowMargin.current;
    if (margin === null || margin.key !== layoutKey) {
      // A non-last row is the sample: `:last-child` is the only place a bottom
      // margin is overridden, and that one is answered separately below.
      const sample = (rows.length > 1 ? rows[1] : rows[0]) as HTMLElement | undefined;
      margin = {
        key: layoutKey,
        margin: sample ? Number.parseFloat(getComputedStyle(sample).marginBottom) || 0 : 0,
        lastMargin: null,
      };
      rowMargin.current = margin;
    }
    const lastRow = rows.length > 0 ? (rows[rows.length - 1] as HTMLElement) : null;
    if (lastRow && margin.lastMargin === null) {
      // Only the last child can carry the `:last-child` override, and only while
      // it is mounted. One read per layout, not one per commit.
      margin.lastMargin =
        rows.length > 1 ? Number.parseFloat(getComputedStyle(lastRow).marginBottom) || 0 : margin.margin;
    }
    const lastMargin = margin.lastMargin ?? margin.margin;

    // One read pass over the mounted rows, before anything writes to the scroll:
    // a `getBoundingClientRect` *after* a `scrollTop` write flushes layout again
    // for every row that follows it. The anchor correction below reuses these
    // boxes, so it cannot read a rect from a different layout than the heights.
    const scrollTop = element.scrollTop;
    const boxes: DOMRect[] = [];
    const keys: (string | null)[] = [];
    for (let index = 0; index < rows.length; index += 1) {
      const row = rows[index] as HTMLElement;
      boxes.push(row.getBoundingClientRect());
      keys.push(row.getAttribute('data-entry-key'));
    }
    let changed = false;
    for (let index = 0; index < boxes.length; index += 1) {
      const key = keys[index];
      if (!key) continue;
      // The row's *extent*, not its border box: the gap below it is a bottom
      // margin (`--space-10` on `.msg`), which `getBoundingClientRect` leaves
      // out. Replacing a row with padding of its height would then move the
      // layout by that margin every time the window's start crossed over it; the
      // anchor would correct that, the correction could cross back, and the two
      // would flip each other indefinitely at 32px a frame. The computed margin
      // is the used value, so a last child that does lose it is honoured too, and
      // every row's model height matches what it occupies in the document.
      const height =
        (boxes[index] as DOMRect).height + (index === boxes.length - 1 ? lastMargin : margin.margin);
      const previous = measured.current.get(key);
      if (previous !== undefined && Math.abs(previous - height) < 1) continue;
      measured.current.set(key, height);
      changed = true;
    }
    const before = rowAnchor.current;
    // Whether the reader is following decides *who* corrects this commit, and it
    // is the same question the follow in `useStickyBottom` asks - of the same
    // state, through `followingNewest`. Asking it differently here (this used to
    // be "is the viewport flush with the bottom") is what made the two write
    // against each other: a stream leaves the view a gap short of the bottom on
    // every frame, so the follow saw "growth I have not caught up with" and
    // scrolled down while this correction saw "not at the bottom" and held an
    // anchor, scrolling back up. Two positions, same frame, forever.
    if (before && before.key && !followingNewest(element)) {
      const index = keys.indexOf(before.key);
      if (index !== -1) {
        const box = boxes[index] as DOMRect;
        // A row's position *in the document* is its screen top plus the scroll.
        // Scrolling alone leaves that constant (moving up by 90px moves the row
        // down the screen by 90px), so any change in it is a layout shift, and
        // moving the scroll by exactly that much keeps the row still. The scroll
        // is the one read at the top of this pass: no write has happened yet.
        const shift = box.top - before.top + (scrollTop - before.scrollTop);
        if (Number.isFinite(shift) && Math.abs(shift) >= 0.5) {
          // The note carries whether the newest content was on screen *before*
          // this write, which no flag on the hook can supply: the sticky-bottom
          // reads it to tell a gap of its own from one a reader made. Read it
          // here, because after the write it is too late to tell.
          const atBottom = elementAtBottom(element);
          element.scrollTop += shift;
          noteOwnScroll(element, atBottom);
        }
      }
    }
    // Re-read the anchor against the layout this commit left behind, so the next
    // measurement is corrected from here rather than from a stale position.
    const view = element.getBoundingClientRect();
    rowAnchor.current = null;
    for (const row of rows) {
      const box = row.getBoundingClientRect();
      if (box.bottom <= view.top) continue;
      rowAnchor.current = {
        key: row.getAttribute('data-entry-key') ?? '',
        top: box.top,
        scrollTop: element.scrollTop,
      };
      break;
    }
    if (changed) {
      setMeasuredTick((tick) => tick + 1);
      // A row measured to a different height than the model claimed moves
      // everything below it, while the window's padding absorbs the difference -
      // so the document's total height can stay identical and no resize is
      // observed. Without this, a reader following the newest content is left a
      // few pixels short of it and nothing will ever say so.
      onMeasured?.();
    }
  }, [virtual, range.start, range.end, estimates, measuredTick, viewportRef, windowTop, onMeasured]);

  const topPadding = range.start > 0 ? geometry.offsets[range.start] ?? 0 : 0;
  const bottomPadding =
    range.end < entries.length ? Math.max(0, geometry.total - (geometry.offsets[range.end] ?? geometry.total)) : 0;
  const visible = entries.slice(range.start, range.end);
  const freshVisible = collectFresh(visible);

  return {
    transcriptRef,
    virtual,
    pending,
    topPadding,
    bottomPadding,
    visible,
    freshVisible,
  };
}
