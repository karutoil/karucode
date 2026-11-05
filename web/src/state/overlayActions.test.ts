import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { jsonResponse, resetAppState } from '../test/fixtures';
import * as actions from './actions';
import { emptyChatState } from './chat';
import * as store from './store';

const realFetch = globalThis.fetch;
const firstTarget = { messageId: 1, preview: 'First prompt', isAssistant: false };
const secondTarget = { messageId: 2, preview: 'Second prompt', isAssistant: false };

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function preview(messageId: number) {
  return {
    session_id: 's_one',
    message_id: messageId,
    messages_removed: 1,
    files: [],
    incomplete: false,
    opaque_tools: [],
  };
}

beforeEach(() => {
  resetAppState();
  store.currentSessionId.value = 's_one';
  store.currentProjectId.value = 'p_one';
  store.chat.value = emptyChatState('s_one');
});

afterEach(() => {
  globalThis.fetch = realFetch;
  resetAppState();
  vi.restoreAllMocks();
});

describe('single-overlay replacement', () => {
  const cases = [
    ['settings', () => actions.openSettings()],
    ['palette', () => actions.togglePalette()],
    ['theme', () => actions.openThemeStudio()],
    ['notifications', () => actions.openNotificationCenter()],
    ['file', () => actions.openFileViewer('/new.txt')],
    ['rewind', () => actions.openRewind(firstTarget)],
  ] as const;

  it.each(cases)('opening %s closes every other overlay', (_name, open) => {
    // Set every signal to a non-target state, including the signal we are about
    // to open (palette would otherwise toggle closed rather than replace).
    store.settingsOpen.value = true;
    store.paletteOpen.value = _name === 'palette' ? false : true;
    store.themeStudioOpen.value = true;
    store.notificationCenterOpen.value = true;
    store.fileViewerPath.value = '/old.txt';
    store.rewindTarget.value = secondTarget;
    store.mobileSheet.value = 'sessions';
    open();

    expect(store.settingsOpen.value).toBe(_name === 'settings');
    expect(store.paletteOpen.value).toBe(_name === 'palette');
    expect(store.themeStudioOpen.value).toBe(_name === 'theme');
    expect(store.notificationCenterOpen.value).toBe(_name === 'notifications');
    expect(store.fileViewerPath.value).toBe(_name === 'file' ? '/new.txt' : null);
    if (_name === 'rewind') expect(store.rewindTarget.value).toEqual(firstTarget);
    else expect(store.rewindTarget.value).toBeNull();
    expect(store.mobileSheet.value).toBeNull();
  });
});

describe('rewind requests after overlay replacement', () => {
  it('drops a preview returned for a target that is no longer open', async () => {
    const pending = deferred<Response>();
    globalThis.fetch = vi.fn(async () => pending.promise) as unknown as typeof fetch;
    actions.openRewind(firstTarget);
    const loading = actions.loadRewindPreview();
    actions.togglePalette();
    expect(store.paletteOpen.value).toBe(true);
    expect(store.rewindTarget.value).toBeNull();

    pending.resolve(jsonResponse({ preview: preview(1) }));
    expect(await loading).toBeNull();
  });

  it('does not clear or refetch a newer rewind target when an old apply resolves', async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn(async (_input: RequestInfo | URL) => pending.promise);
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    actions.openRewind(firstTarget);
    const applying = actions.applyRewind({ mode: 'none', revert: false });
    actions.openRewind(secondTarget);

    pending.resolve(jsonResponse({
      session_id: 's_one',
      point: {
        session_id: 's_one', user_message_id: 1, prompt: 'First prompt',
        keep_through: 0, before_prompt: 1, messages_removed: 1,
      },
      preview: preview(1),
      outcomes: [], run_id: null, mode: 'none', files_restored: 0, files_skipped: 0,
    }));
    expect(await applying).toBe(true);
    await Promise.resolve();

    expect(store.rewindTarget.value).toEqual(secondTarget);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
