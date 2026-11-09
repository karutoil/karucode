// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { initViewportInset, keyboardInset, KEYBOARD_INSET_PROPERTY } from './viewportInset';

/**
 * iOS Safari paints the keyboard over the page instead of resizing the layout
 * viewport, so the docked composer and its Stop button end up underneath it. The
 * shell shortens by `--kb-inset` instead; these tests pin which measurements
 * count as a keyboard, because an over-eager answer shrinks the whole workspace
 * after the address bar collapses.
 */

class FakeViewport extends EventTarget {
  height: number;
  offsetTop = 0;
  scale = 1;
  constructor(height: number) {
    super();
    this.height = height;
  }
}

function setViewport(view: FakeViewport | undefined): void {
  Object.defineProperty(window, 'visualViewport', { configurable: true, value: view });
}

function setInnerHeight(height: number): void {
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: height });
}

describe('keyboardInset', () => {
  const view = (over: Partial<{ height: number; offsetTop: number; scale: number }>): VisualViewport => ({
    height: 844, offsetTop: 0, scale: 1, ...over,
  }) as VisualViewport;

  it('reports the strip the keyboard covers', () => {
    // An iPhone keyboard is ~300-350px; 844 - 524 = 320.
    expect(keyboardInset(view({ height: 524 }), 844)).toBe(320);
    // The visual viewport can also be scrolled down to reveal a focused field.
    expect(keyboardInset(view({ height: 524, offsetTop: 120 }), 844)).toBe(200);
  });

  it('ignores a collapsing browser toolbar', () => {
    // Mobile Safari's address bar coming and going moves the visual viewport by
    // 40-70px. Treating that as a keyboard would shrink the shell for nothing.
    expect(keyboardInset(view({ height: 784 }), 844)).toBe(0);
  });

  it('ignores a pinch-zoomed page', () => {
    expect(keyboardInset(view({ height: 400, scale: 2 }), 844)).toBe(0);
  });

  it('is zero when nothing is covered or there is nothing to measure', () => {
    expect(keyboardInset(view({ height: 844 }), 844)).toBe(0);
    expect(keyboardInset(view({ height: 900 }), 844)).toBe(0);
    expect(keyboardInset(null, 844)).toBe(0);
    expect(keyboardInset(undefined, 844)).toBe(0);
    expect(keyboardInset(view({ height: 500 }), 0)).toBe(0);
  });
});

describe('initViewportInset', () => {
  let stop: (() => void) | undefined;

  beforeEach(() => {
    setInnerHeight(844);
    document.documentElement.style.removeProperty(KEYBOARD_INSET_PROPERTY);
  });

  afterEach(() => {
    stop?.();
    stop = undefined;
    setViewport(undefined);
    document.documentElement.style.removeProperty(KEYBOARD_INSET_PROPERTY);
  });

  const property = (): string => document.documentElement.style.getPropertyValue(KEYBOARD_INSET_PROPERTY);

  it('publishes the keyboard height and clears it when the keyboard closes', () => {
    const view = new FakeViewport(844);
    setViewport(view);
    stop = initViewportInset();
    expect(property()).toBe('');

    view.height = 524;
    view.dispatchEvent(new Event('resize'));
    expect(property()).toBe('320px');

    view.height = 844;
    view.dispatchEvent(new Event('resize'));
    expect(property()).toBe('');
  });

  it('reacts to the visual viewport scrolling, not only resizing', () => {
    // iOS scrolls the visual viewport to reveal a focused field without changing
    // its height; the covered strip is what the shell has to make room for.
    const view = new FakeViewport(524);
    view.offsetTop = 60;
    setViewport(view);
    stop = initViewportInset();
    expect(property()).toBe('260px');
  });

  it('falls back to the window resize signal with no visualViewport at all', () => {
    // jsdom and older engines have no `visualViewport`; `resize` is then the only
    // signal, and it must not leave a stale inset behind.
    setViewport(undefined);
    stop = initViewportInset();
    setInnerHeight(600);
    window.dispatchEvent(new Event('resize'));
    // innerHeight is also the layout height, so nothing reads as covered.
    expect(property()).toBe('');
  });

  it('stops publishing once torn down', () => {
    const view = new FakeViewport(844);
    setViewport(view);
    stop = initViewportInset();
    stop();
    stop = undefined;

    view.height = 524;
    view.dispatchEvent(new Event('resize'));
    expect(property()).toBe('');
  });
});
