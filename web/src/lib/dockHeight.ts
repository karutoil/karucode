import { useEffect } from 'preact/hooks';
import type { RefObject } from 'preact';

/**
 * Publish an element's rendered height as a CSS custom property on a target.
 *
 * The composer is a *floating* dock: it paints over the bottom of the transcript
 * instead of taking a flex row of its own, so the scroller has to know how tall
 * it currently is to reserve room for it (`padding-bottom: var(--composer-h)`).
 * That height is not a constant — the textarea grows with the draft, an
 * attachment chip row appears, the bottom row wraps on a narrow viewport, and the
 * footnote hides while the reader scrolls — so it is measured rather than
 * guessed.
 *
 * Written as a plain property assignment rather than through a signal so a
 * resize repaints CSS without re-rendering the component tree.
 */
export function useHeightVariable(options: {
  /** The element to measure. */
  ref: RefObject<HTMLElement>;
  /** The element to publish `--<name>` on. */
  target: RefObject<HTMLElement>;
  /** Property name without the leading `--`. */
  name: string;
  /**
   * Ignore heights of zero. An element in a `[hidden]` panel or a tab that is
   * not mounted measures 0, and writing that would collapse the transcript's
   * reserved band.
   */
  skipZero?: boolean;
}): void {
  const { ref, target, name, skipZero = true } = options;

  useEffect(() => {
    const element = ref.current;
    const host = target.current;
    if (!element || !host) return;

    const publish = (height: number): void => {
      if (skipZero && height <= 0) return;
      host.style.setProperty(`--${name}`, `${Math.round(height)}px`);
    };

    // `offsetHeight` is an integer, so a fractional layout height cannot put the
    // property into a feedback loop with the observer below.
    publish(element.offsetHeight);

    if (typeof ResizeObserver !== 'function') return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      // `borderBoxSize` is the outer height, which is what the dock occupies.
      const box = entry.borderBoxSize?.[0];
      publish(box ? box.blockSize : entry.contentRect.height);
    });
    observer.observe(element);

    return () => {
      observer.disconnect();
      host.style.removeProperty(`--${name}`);
    };
  }, [ref, target, name, skipZero]);
}
