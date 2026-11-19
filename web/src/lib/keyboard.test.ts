import { describe, expect, it } from 'vitest';
import { isImeComposing } from './keyboard';

/** A keydown with a `keyCode` jsdom's constructor will not take from the init. */
function keydown(init: { key: string; isComposing?: boolean }, keyCode: number): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key: init.key, isComposing: init.isComposing ?? false });
  Object.defineProperty(event, 'keyCode', { configurable: true, get: () => keyCode });
  return event;
}

describe('isImeComposing', () => {
  it('reports a keydown the standard flag says is composing', () => {
    expect(isImeComposing(keydown({ key: 'Enter', isComposing: true }, 13))).toBe(true);
    expect(isImeComposing(keydown({ key: 'ArrowDown', isComposing: true }, 40))).toBe(true);
  });

  it('reports keyCode 229 with isComposing false, which is how Gecko spells a commit', () => {
    const commit = keydown({ key: 'Enter' }, 229);
    expect(commit.isComposing).toBe(false);
    expect(isImeComposing(commit)).toBe(true);
  });

  it('lets ordinary keys through on both platforms', () => {
    expect(isImeComposing(keydown({ key: 'Enter' }, 13))).toBe(false);
    expect(isImeComposing(keydown({ key: 'Escape' }, 27))).toBe(false);
    expect(isImeComposing(keydown({ key: 'ArrowUp' }, 38))).toBe(false);
  });

  it('answers without claiming the key: preventDefault stays the caller’s decision', () => {
    const event = keydown({ key: 'Enter', isComposing: true }, 229);
    expect(isImeComposing(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
  });
});