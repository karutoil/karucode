import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

/** How close to the top of the viewport the sentinel starts loading (px). */
export const HISTORY_PREFETCH_PX = 400;

export interface RectLike {
  top: number;
  bottom: number;
}

/**
 * True when the marker above the oldest loaded row is at (or near) the top of
 * the visible area.
 *
 * A marker far above the viewport means the reader is at the newest content of a
 * long transcript: that is not a request for older history. A marker inside the
 * viewport, or in the top band of it, means the reader has reached the start of
 * what is loaded, which is exactly when the next page should arrive.
 */
export function sentinelReached(
  marker: RectLike,
  viewport: RectLike,
  prefetchPx: number = HISTORY_PREFETCH_PX,
): boolean {
  return marker.bottom >= viewport.top && marker.top <= viewport.top + prefetchPx;
}

export interface HistoryLoaderOptions {
  /** The transcript's scroll container; `null` follows the page viewport. */
  scroller: RefObject<HTMLElement | null>;
  /** The marker rendered above the oldest loaded row. */
  sentinel: RefObject<HTMLElement | null>;
  /**
   * The element whose top edge is the start of the loaded rows — the transcript,
   * an agent's step list. Optional, because a windowed transcript is not the only
   * shape of paged content.
   *
   * Pages are prepended *inside* this element, so its top edge does not move when
   * one arrives. That is what makes it the anchor a reader who has scrolled past
   * every loaded row is put back on: they are shown the start of the page that
   * just arrived instead of being carried down by its height.
   */
  content?: RefObject<HTMLElement | null>;
  /** True while the server may still have older rows. */
  canLoad: boolean;
  /** True while a page is already in flight. */
  loading: boolean;
  /**
   * Fetch one older page. Its promise settles once the store holds the rows, so
   * the hook can restore the scroll position on the next frame.
   */
  load: () => Promise<void>;
  /**
   * Changes when rows are prepended. Used both to re-check after a manual load
   * and to notice that a request added nothing (an error or a duplicate page),
   * which must not leave the loop retrying forever.
   */
  version: number;
}

interface Anchor {
  element: HTMLElement;
  scrollTop: number;
  scrollHeight: number;
  /** The start of the loaded rows, when the caller named one. */
  content: HTMLElement | null;
  /**
   * True when no loaded row was on screen: the reader was looking at the marker
   * (or above it), so nothing they could see moved when the page was prepended.
   */
  atStart: boolean;
}

function scrollElement(host: HTMLElement | null): HTMLElement | null {
  if (host) return host;
  if (typeof document === 'undefined') return null;
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}

function metrics(host: HTMLElement | null): RectLike & { scrollTop: number; scrollHeight: number } {
  if (host) {
    const rect = host.getBoundingClientRect();
    return {
      top: rect.top,
      bottom: rect.bottom,
      scrollTop: host.scrollTop,
      scrollHeight: host.scrollHeight,
    };
  }
  const element = scrollElement(null);
  return {
    top: 0,
    bottom: typeof window === 'undefined' ? 0 : window.innerHeight,
    scrollTop: element?.scrollTop ?? 0,
    scrollHeight: element?.scrollHeight ?? 0,
  };
}

/**
 * Load older transcript pages as the reader reaches the top.
 *
 * The manual button this replaces made the reader ask page by page, and every
 * silent refresh used to throw the loaded pages away. Infinite loading is the
 * other half: a sentinel above the oldest row is observed against the transcript
 * scroller, a page is requested while it is in the top band, and the scroll
 * position is restored so the reader's line of sight does not move under them.
 *
 * Where that restore lands depends on what they were looking at. Inside the rows,
 * the page was inserted above them, so the scroll moves by exactly the height the
 * page added and the row being read stays put. Above every row — at the marker,
 * which is what "scroll up to the top of a huge session" means — nothing they
 * could see moved: the page landed *below* the marker, in front of them. Adding
 * the page's height there carries them past the very rows that arrived, which on
 * a page taller than the screen (a long transcript with 1 MiB pages) is tens of
 * thousands of pixels into history. So in that case the view is moved only far
 * enough to bring the start of the loaded rows to the top of the viewport: the
 * page that arrived is what they read, and the band above the marker is the room
 * they scroll into to ask for the next one.
 *
 * After every request it looks again, but only when the request actually added
 * rows: a page shorter than the viewport would otherwise need another gesture,
 * while an error or a duplicate page would spin.
 */
export function useHistoryLoader(options: HistoryLoaderOptions): void {
  const latest = useRef(options);
  latest.current = options;
  const busy = useRef(false);
  const frame = useRef(0);
  const anchor = useRef<Anchor | null>(null);
  /** The installed effect's scheduled check, for the version-change effect. */
  const check = useRef<(() => void) | null>(null);

  const scroller = options.scroller;
  const sentinel = options.sentinel;

  useEffect(() => {
    let cancelled = false;

    const run = (): void => {
      frame.current = 0;
      const current = latest.current;
      if (cancelled || busy.current || current.loading || !current.canLoad) return;
      const marker = current.sentinel.current;
      const host = current.scroller.current;
      if (!marker) return;
      const box = metrics(host);
      const markerRect = marker.getBoundingClientRect();
      if (!sentinelReached(markerRect, box)) return;
      const element = scrollElement(host);
      const version = current.version;
      const content = current.content?.current ?? null;
      const contentRect = content ? content.getBoundingClientRect() : null;
      busy.current = true;
      anchor.current = element
        ? {
            element,
            scrollTop: element.scrollTop,
            scrollHeight: element.scrollHeight,
            content,
            // The rows start at the content's top edge, so a viewport top above
            // it means every loaded row is off screen.
            atStart: contentRect !== null && contentRect.top >= box.top,
          }
        : null;
      void current.load().then(
        () => {
          if (cancelled) return;
          requestAnimationFrame(() => {
            // The load is only finished once the view has been put back where it
            // was: a check that ran in between would start the next page against
            // geometry the reader has not been moved into yet.
            busy.current = false;
            if (cancelled) return;
            const pending = anchor.current;
            anchor.current = null;
            let revealed = false;
            if (pending) {
              if (pending.atStart && pending.content) {
                // Reveal the page that arrived: put the start of the loaded rows
                // at the top of the viewport, so the page is what the reader sees
                // and the band above the marker is the room left to scroll into.
                // Read from live geometry, so a reader who scrolled meanwhile is
                // left alone.
                const viewport = metrics(latest.current.scroller.current);
                const shift = pending.content.getBoundingClientRect().top - viewport.top;
                if (shift > 0) pending.element.scrollTop = pending.element.scrollTop + shift;
                revealed = true;
              } else {
                const delta = pending.element.scrollHeight - pending.scrollHeight;
                // Prepending grows the content above; moving down by the same
                // height keeps the row that was at the top of the viewport there.
                if (delta > 0) pending.element.scrollTop = pending.scrollTop + delta;
              }
            }
            // A page shorter than the viewport leaves the sentinel visible, so
            // keep filling until it is pushed out (or the start is reached). A
            // request that added nothing must stop here instead of spinning.
            //
            // A revealed page is the exception: it was placed in front of the
            // reader, and another page in the same frame would replace it before
            // it was read. The marker is still above the rows, so the next scroll
            // asks for the next page - which is the whole gesture.
            if (!revealed && latest.current.version !== version) run();
          });
        },
        () => {
          busy.current = false;
          anchor.current = null;
        },
      );
    };

    const schedule = (): void => {
      if (cancelled || frame.current !== 0) return;
      frame.current = requestAnimationFrame(run);
    };
    check.current = schedule;

    const host: HTMLElement | null = scroller.current;
    if (host) host.addEventListener('scroll', schedule, { passive: true });
    else window.addEventListener('scroll', schedule, { passive: true });
    window.addEventListener('resize', schedule);
    schedule();

    return () => {
      cancelled = true;
      check.current = null;
      if (host) host.removeEventListener('scroll', schedule);
      else window.removeEventListener('scroll', schedule);
      window.removeEventListener('resize', schedule);
      if (frame.current !== 0) cancelAnimationFrame(frame.current);
      frame.current = 0;
      busy.current = false;
      anchor.current = null;
    };
  }, [scroller, sentinel]);

  // A page loaded by some other path (a retry, another tab) gets the same "is
  // the marker visible now?" check. The post-load loop covers the pages this
  // hook fetched itself.
  useEffect(() => {
    if (!options.canLoad || options.loading) return;
    check.current?.();
  }, [options.version, options.canLoad, options.loading]);
}
