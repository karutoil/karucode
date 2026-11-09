/**
 * Keep the composer above the on-screen keyboard.
 *
 * The workspace is a fixed-height shell (`height: 100dvh`, `overflow: hidden`)
 * with the composer docked to the bottom of the conversation pane. Android
 * Chrome resizes the layout viewport for the keyboard when the page asks for it
 * (`interactive-widget=resizes-content` in `index.html`), and there `innerHeight`
 * shrinks with it — nothing to do. iOS Safari does not resize the layout
 * viewport at all: the keyboard paints *over* the bottom of the page, so the
 * dock, the Stop button and the approval badge end up underneath it with no way
 * to scroll them into view.
 *
 * `visualViewport` is the one place that reports the actually-visible rectangle
 * on both. Its height minus the layout height is the covered strip; publishing
 * that as `--kb-inset` lets `layout.css` shorten the shell by exactly that much,
 * which lifts the dock above the keyboard without touching the transcript's own
 * scrolling.
 *
 * Two guards keep this from misfiring on an ordinary phone:
 *
 *   - the strip must exceed `MIN_KEYBOARD_RATIO` of the layout height. Mobile
 *     Safari's collapsing toolbar moves `visualViewport.height` by ~40-70px, and
 *     treating that as a keyboard would shrink the app for no reason;
 *   - a pinch-zoomed page (`scale !== 1`) also reports a smaller visual viewport,
 *     and zoom is not a keyboard.
 */

/** Below this fraction of the layout height the strip is browser chrome, not a keyboard. */
const MIN_KEYBOARD_RATIO = 0.15;

/** Property name (without `--`) written on the document element. */
export const KEYBOARD_INSET_PROPERTY = '--kb-inset';

/**
 * The strip of the layout viewport currently covered by browser UI below the
 * visual viewport, in CSS pixels. Zero when there is none, when the page is
 * pinch-zoomed, or when the engine has no `visualViewport`.
 */
export function keyboardInset(view: VisualViewport | null | undefined, layoutHeight: number): number {
  if (!view || !Number.isFinite(layoutHeight) || layoutHeight <= 0) return 0;
  // A pinch-zoomed page reports a smaller visual viewport for reasons that have
  // nothing to do with a keyboard.
  if (Math.abs(view.scale - 1) > 0.01) return 0;
  const visible = view.height + view.offsetTop;
  const covered = layoutHeight - visible;
  if (!Number.isFinite(covered) || covered <= 0) return 0;
  return covered > layoutHeight * MIN_KEYBOARD_RATIO ? Math.round(covered) : 0;
}

/**
 * Start publishing `--kb-inset` and return the teardown. Safe to call where
 * `visualViewport` does not exist (jsdom, older browsers): it listens to
 * `window.resize` alone, which on those engines is already the keyboard signal.
 */
export function initViewportInset(): () => void {
  const root = document.documentElement;

  const publish = (): void => {
    const inset = keyboardInset(window.visualViewport, window.innerHeight);
    if (inset > 0) root.style.setProperty(KEYBOARD_INSET_PROPERTY, `${inset}px`);
    else root.style.removeProperty(KEYBOARD_INSET_PROPERTY);
  };

  const view = window.visualViewport;
  // `scroll` matters as well as `resize`: iOS scrolls the visual viewport down to
  // reveal a focused field without changing its height, and the covered strip is
  // what the layout has to make room for either way.
  view?.addEventListener('resize', publish);
  view?.addEventListener('scroll', publish);
  window.addEventListener('resize', publish);
  window.addEventListener('orientationchange', publish);
  publish();

  return () => {
    view?.removeEventListener('resize', publish);
    view?.removeEventListener('scroll', publish);
    window.removeEventListener('resize', publish);
    window.removeEventListener('orientationchange', publish);
    root.style.removeProperty(KEYBOARD_INSET_PROPERTY);
  };
}
