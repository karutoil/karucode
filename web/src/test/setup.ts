import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/preact';

// jsdom lacks matchMedia; the app uses it for responsive behaviour and motion.
if (typeof window.matchMedia !== 'function') {
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

Object.defineProperty(navigator, 'clipboard', {
  configurable: true,
  value: {
    writeText: async () => undefined,
  },
});

afterEach(() => {
  cleanup();
});