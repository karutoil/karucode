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
  scroller: RefObject<HTMLElement>;
  /** The marker rendered above the oldest loaded row. */
  sentinel: RefObject<HTMLElement>;
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
 * position is moved by exactly the height the page added so the row the reader
 * was reading stays where it is.
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
      if (!sentinelReached(marker.getBoundingClientRect(), box)) return;
      const element = scrollElement(host);
      const version = current.version;
      busy.current = true;
      anchor.current = element
        ? { element, scrollTop: element.scrollTop, scrollHeight: element.scrollHeight }
        : null;
      void current.load().then(
        () => {
          busy.current = false;
          if (cancelled) return;
          requestAnimationFrame(() => {
            if (cancelled) return;
            const pending = anchor.current;
            anchor.current = null;
            if (pending) {
              const delta = pending.element.scrollHeight - pending.scrollHeight;
              // Prepending grows the content above; moving down by the same
              // height keeps the row that was at the top of the viewport there.
              if (delta > 0) pending.element.scrollTop = pending.scrollTop + delta;
            }
            // A page shorter than the viewport leaves the sentinel visible, so
            // keep filling until it is pushed out (or the start is reached). A
            // request that added nothing must stop here instead of spinning.
            if (latest.current.version !== version) run();
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
