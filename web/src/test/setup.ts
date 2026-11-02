import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/preact';
// The shipped bundle always loads `preact/compat` (the transcript and the
// terminal tab import it), and compat re-maps Preact's synthetic events: an
// `onBlur` prop becomes a `focusout` listener, an `onFocus` prop a `focusin`
// one. Importing it here makes every test file see the same event semantics as
// the app, instead of only the files that happen to import a component which
// pulls compat in.
import 'preact/compat';

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