import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { caretInEditable, isDialogOpen } from './helpers';

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  resetAppState();
});

describe('isDialogOpen', () => {
  it('counts both phone sheets as dialogs', () => {
    // The sessions and Results sheets render `role="dialog"` + `aria-modal="true"`
    // at 92vw, so a shortcut that fired there would act on the chat underneath.
    expect(isDialogOpen()).toBe(false);

    store.mobileSheet.value = 'sessions';
    expect(isDialogOpen()).toBe(true);

    store.mobileSheet.value = 'inspector';
    expect(isDialogOpen()).toBe(true);

    store.mobileSheet.value = null;
    expect(isDialogOpen()).toBe(false);
  });

  it('treats the Context drawer as a dialog only below the breakpoint', () => {
    workbench.openContext('now');
    store.narrowViewport.value = false;
    expect(workbench.contextOpen.value).toBe(true);
    // Docked, Context is a column: the workspace keeps its shortcuts.
    expect(isDialogOpen()).toBe(false);

    store.narrowViewport.value = true;
    expect(isDialogOpen()).toBe(true);
  });

  it('still covers the dismissable overlays it always did', () => {
    for (const open of [
      () => {
        store.settingsOpen.value = true;
      },
      () => {
        store.paletteOpen.value = true;
      },
      () => {
        store.themeStudioOpen.value = true;
      },
      () => {
        store.notificationCenterOpen.value = true;
      },
      () => {
        store.rewindTarget.value = { messageId: 1, preview: 'p', isAssistant: false };
      },
      () => {
        store.fileViewerPath.value = '/notes.md';
      },
    ]) {
      resetAppState();
      expect(isDialogOpen()).toBe(false);
      open();
      expect(isDialogOpen()).toBe(true);
    }
  });
});

describe('caretInEditable', () => {
  it('answers for a field and for the document itself', () => {
    const field = document.createElement('input');
    document.body.append(field);
    expect(caretInEditable({ target: field } as unknown as Event)).toBe(true);
    expect(caretInEditable({ target: document.body } as unknown as Event)).toBe(false);
    field.remove();
  });
});
