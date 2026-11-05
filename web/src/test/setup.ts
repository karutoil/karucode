import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/preact';
import { options } from 'preact';
// The shipped bundle always loads `preact/compat` (the transcript and the
// terminal tab import it), and compat re-maps Preact's synthetic events: an
// `onBlur` prop becomes a `focusout` listener, an `onFocus` prop a `focusin`
// one. Importing it here makes every test file see the same event semantics as
// the app, instead of only the files that happen to import a component which
// pulls compat in.
import 'preact/compat';

/**
 * Flush Preact's post-commit effects on a microtask instead of a frame.
 *
 * `useEffect` bodies do not run during `commit`; Preact queues the component and
 * flushes the queue from `afterNextFrame`, which is `setTimeout(…, 35)` racing a
 * `requestAnimationFrame`. Two things make that unusable here:
 *
 *   1. `afterNextFrame` latches `HAS_RAF = typeof requestAnimationFrame ===
 *      'function'` **at module evaluation** — jsdom has rAF, so the latch is
 *      permanently true — and its `done()` calls the *global*
 *      `cancelAnimationFrame` when the timer fires.
 *   2. Vitest tears the jsdom globals down when the file's last test ends. A
 *      render that happened outside `act()` (an assertion that throws midway, a
 *      dialog closed in `afterEach`, any render whose effects were still queued)
 *      leaves that 35 ms timer pending; it fires into a torn-down environment and
 *      throws `ReferenceError: cancelAnimationFrame is not defined` from a Node
 *      timer. Nothing in the test can catch it, so it is reported against
 *      whichever file was running: "an unhandled error after teardown".
 *
 * `options.requestAnimationFrame` is Preact's documented seam for exactly this:
 * when set, `afterPaint()` calls it instead of `afterNextFrame()`, so no frame
 * and no fallback timer are ever scheduled and nothing can outlive the test.
 * A microtask keeps the ordering contract (effects run after commit, before any
 * timer) and makes them observable with an `await`, which is also what
 * `act()` already does — it swaps this same option for its own queue and
 * restores ours afterwards.
 *
 * This is test infrastructure, not a behaviour change: the browser build never
 * loads this file, and `src/test/domScheduling.test.tsx` pins the raw-render
 * behaviour with the RAF globals deleted.
 */
options.requestAnimationFrame = (callback: () => void): number => {
  queueMicrotask(callback);
  return 0;
};

// A few suites run in the `node` environment (they read repository files rather
// than rendering components), so every DOM shim below is guarded.
const hasDom = typeof window !== 'undefined';

// jsdom lacks matchMedia; the app uses it for responsive behaviour and motion.
if (hasDom && typeof window.matchMedia !== 'function') {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }),
  });
}

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => undefined;
}

if (typeof window !== 'undefined' && typeof window.scrollTo !== 'function') {
  window.scrollTo = () => undefined;
}

if (typeof navigator !== 'undefined') {
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: async () => undefined,
    },
  });
}

afterEach(() => {
  if (hasDom) cleanup();
});