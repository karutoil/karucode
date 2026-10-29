import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/preact';

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