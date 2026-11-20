import { fireEvent, renderHook } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { resetAppState } from '../test/fixtures';
import { useGlobalKeybindings } from './useGlobalKeybindings';

/** A keydown carrying fields jsdom's constructor will not take from the init. */
function composingKeydown(key: string, keyCode: number, isComposing = false): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    keyCode: { configurable: true, get: () => keyCode },
    isComposing: { configurable: true, get: () => isComposing },
  });
  return event;
}

beforeEach(() => {
  resetAppState();
  store.currentProjectId.value = 'p_one';
  store.boot.value = 'ready';
});

afterEach(() => {
  resetAppState();
  document.body.innerHTML = '';
});

describe('the shell shortcuts behind a phone sheet', () => {
  it('does nothing while the sessions sheet is open', () => {
    // A sheet is `role="dialog"` + `aria-modal="true"`; `Mod+N` used to start a
    // draft on the chat underneath it (and dismiss the sheet as a side effect).
    store.mobileSheet.value = 'sessions';
    renderHook(() => useGlobalKeybindings());

    fireEvent.keyDown(window, { key: 'n', metaKey: true });
    expect(store.draftSession.value).toBeNull();
    expect(store.mobileSheet.value).toBe('sessions');
  });

  it('still fires once the sheet is gone', () => {
    renderHook(() => useGlobalKeybindings());

    fireEvent.keyDown(window, { key: 'n', metaKey: true });
    expect(store.draftSession.value).not.toBeNull();
  });

  it('closes the sheet on Escape, but not from a field inside it', () => {
    store.mobileSheet.value = 'inspector';
    renderHook(() => useGlobalKeybindings());

    // The sheet's search/rename fields own Escape: their own handler runs first,
    // and the sheet must not close under them.
    const field = document.createElement('input');
    document.body.append(field);
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(store.mobileSheet.value).toBe('inspector');

    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(store.mobileSheet.value).toBeNull();
  });
});

describe('Escape and the Context drawer', () => {
  beforeEach(() => {
    store.narrowViewport.value = true;
    workbench.openContext('now');
  });

  it('leaves a key the field under the caret handles to that field', () => {
    renderHook(() => useGlobalKeybindings());
    const field = document.createElement('textarea');
    document.body.append(field);

    fireEvent.keyDown(field, { key: 'Escape' });
    expect(workbench.contextOpen.value).toBe(true);
  });

  it('ignores an Escape that cancels an IME candidate', () => {
    renderHook(() => useGlobalKeybindings());

    fireEvent.keyDown(document.body, { key: 'Escape', isComposing: true });
    expect(workbench.contextOpen.value).toBe(true);

    // The same answer when the flag is all that says so, without jsdom's init.
    document.body.dispatchEvent(composingKeydown('Escape', 27, true));
    expect(workbench.contextOpen.value).toBe(true);

    // Gecko/Blink spell the key an IME consumed as `keyCode === 229`, with
    // `isComposing` false.
    document.body.dispatchEvent(composingKeydown('Escape', 229));
    expect(workbench.contextOpen.value).toBe(true);
  });

  it('still closes the drawer on an Escape the workspace owns', () => {
    renderHook(() => useGlobalKeybindings());

    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(workbench.contextOpen.value).toBe(false);
  });
});
