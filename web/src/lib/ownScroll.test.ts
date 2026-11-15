import { beforeEach, describe, expect, it, vi } from 'vitest';
import { clearOwnScroll, isOwnScroll, noteOwnScroll, OWN_SCROLL_MS } from './ownScroll';

/** A scroller whose position the test can move without a real layout engine. */
function scroller(scrollTop = 0): HTMLElement {
  let top = scrollTop;
  const element = {
    get scrollTop() {
      return top;
    },
    set scrollTop(value: number) {
      top = value;
    },
  };
  return element as unknown as HTMLElement;
}

describe('own scroll notes', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('reports nothing for an element that was never marked', () => {
    expect(isOwnScroll(scroller(100))).toBe(false);
  });

  it('covers the position the app wrote', () => {
    const element = scroller(0);
    element.scrollTop = 420;
    noteOwnScroll(element, true);
    expect(isOwnScroll(element)).toBe(true);
  });

  // The note is tied to a position, not to the element: the reader's next scroll
  // moves off it, and that scroll is theirs even though the note has not expired.
  it('does not cover a later different position', () => {
    const element = scroller(0);
    element.scrollTop = 420;
    noteOwnScroll(element, true);
    element.scrollTop = 200;
    expect(isOwnScroll(element)).toBe(false);
  });

  it('tolerates sub-pixel rounding of its own write', () => {
    const element = scroller(0);
    element.scrollTop = 420.4;
    noteOwnScroll(element, true);
    element.scrollTop = 419.6;
    expect(isOwnScroll(element)).toBe(true);
  });

  it('expires', () => {
    vi.useFakeTimers();
    const element = scroller(0);
    element.scrollTop = 300;
    noteOwnScroll(element, true);
    vi.advanceTimersByTime(OWN_SCROLL_MS + 1);
    expect(isOwnScroll(element)).toBe(false);
    vi.useRealTimers();
  });

  it('can be forgotten early', () => {
    const element = scroller(0);
    element.scrollTop = 300;
    noteOwnScroll(element, true);
    clearOwnScroll(element);
    expect(isOwnScroll(element)).toBe(false);
  });
});