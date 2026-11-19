/**
 * `copyText`, on the path that actually runs over plain HTTP.
 *
 * `navigator.clipboard` is a secure-context API in every engine, so the
 * documented LAN deployment (`http://` on a private address) always takes the
 * legacy `execCommand` branch. That branch used to select without focusing and
 * then remove the selected element, which dropped focus to `<body>`: the copy
 * worked and the composer silently stopped taking keystrokes until the user
 * clicked it again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { copyText } from './clipboard';

/** The scratch element `copyText` appends for the legacy path. */
function scratchField(): HTMLTextAreaElement | null {
  return document.querySelector('textarea');
}

describe('copyText', () => {
  let restoreExec: () => void;

  beforeEach(() => {
    const descriptor = Object.getOwnPropertyDescriptor(document, 'execCommand');
    restoreExec = () => {
      if (descriptor) Object.defineProperty(document, 'execCommand', descriptor);
      else Reflect.deleteProperty(document as unknown as Record<string, unknown>, 'execCommand');
    };
  });

  afterEach(() => {
    restoreExec();
    document.body.innerHTML = '';
    // The shared setup installs a Clipboard API; leave it as it found it.
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async () => undefined },
    });
  });

  it('uses the Clipboard API when the page has one', async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });

    expect(await copyText('hello')).toBe(true);
    expect(writeText).toHaveBeenCalledWith('hello');
    // Nothing was appended for a copy that never needed the fallback.
    expect(scratchField()).toBeNull();
  });

  it('focuses the scratch field and gives the caret back afterwards', async () => {
    // The plain-HTTP case: there is no Clipboard API at all.
    Reflect.deleteProperty(navigator, 'clipboard');
    expect(navigator.clipboard).toBeUndefined();

    const composer = document.createElement('input');
    document.body.appendChild(composer);
    composer.focus();
    expect(document.activeElement).toBe(composer);

    let focusedDuringCopy: Element | null = null;
    let selectedRange = '';
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      writable: true,
      value: vi.fn(() => {
        focusedDuringCopy = document.activeElement;
        const field = scratchField();
        selectedRange = field ? `${field.selectionStart}:${field.selectionEnd}` : '';
        return true;
      }),
    });

    expect(await copyText('copied text')).toBe(true);

    // `execCommand('copy')` copies from the focused control, and `select()`
    // alone does not focus one.
    expect(focusedDuringCopy).toBeInstanceOf(HTMLTextAreaElement);
    // iOS ignores `select()` on a readonly field but honours the explicit range.
    expect(selectedRange).toBe(`0:${'copied text'.length}`);
    // The scratch field is gone and the caret is back where it was.
    expect(scratchField()).toBeNull();
    expect(document.activeElement).toBe(composer);
  });

  it('still restores focus when the browser refuses the copy', async () => {
    Reflect.deleteProperty(navigator, 'clipboard');
    const composer = document.createElement('input');
    document.body.appendChild(composer);
    composer.focus();
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      writable: true,
      value: vi.fn(() => false),
    });

    expect(await copyText('nope')).toBe(false);
    expect(scratchField()).toBeNull();
    expect(document.activeElement).toBe(composer);
  });

  it('does not fail when nothing was focused before the copy', async () => {
    Reflect.deleteProperty(navigator, 'clipboard');
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      writable: true,
      value: vi.fn(() => true),
    });

    // `document.activeElement` is `<body>` here: there is no caret to restore and
    // restoring one must not throw.
    await expect(copyText('hello')).resolves.toBe(true);
    expect(scratchField()).toBeNull();
  });
});
