/**
 * The collapsed rail's fly-out state.
 *
 * What matters here is *when* a hover may peek and *what* it shows: the current
 * project needs no read at all, another project costs exactly one, and none of it
 * may fight a dialog, the control plane, or a phone.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { listSessions } from '../api/endpoints';
import { secondSummaryFixture, summaryFixture } from '../test/fixtures';
import { controlPlaneOpen } from './controlPlane';
import * as railPeek from './railPeek';
import * as store from './store';

vi.mock('../api/endpoints', () => ({ listSessions: vi.fn() }));

const PEEK_CLOSE_MS = railPeek.PEEK_CLOSE_MS;

function setWindow(overrides: { collapsed?: boolean; narrow?: boolean } = {}): void {
  store.railCollapsed.value = overrides.collapsed ?? true;
  store.narrowViewport.value = overrides.narrow ?? false;
}

beforeEach(() => {
  railPeek.resetRailPeek();
  setWindow();
  controlPlaneOpen.value = false;
  store.settingsOpen.value = false;
  store.paletteOpen.value = false;
  store.themeStudioOpen.value = false;
  store.notificationCenterOpen.value = false;
  store.rewindTarget.value = null;
  store.fileViewerPath.value = null;
  store.currentProjectId.value = 'p_karucode';
  store.sessions.value = [summaryFixture, secondSummaryFixture];
  vi.mocked(listSessions).mockReset();
  vi.mocked(listSessions).mockResolvedValue([]);
});

describe('when a hover may peek', () => {
  it('is allowed only with the rail collapsed, on a wide viewport, with no overlay', () => {
    expect(railPeek.peekEnabled()).toBe(true);

    setWindow({ collapsed: false });
    expect(railPeek.peekEnabled()).toBe(false);

    setWindow({ narrow: true });
    expect(railPeek.peekEnabled()).toBe(false);

    setWindow();
    controlPlaneOpen.value = true;
    expect(railPeek.peekEnabled()).toBe(false);
    controlPlaneOpen.value = false;

    for (const flag of [
      store.settingsOpen,
      store.paletteOpen,
      store.themeStudioOpen,
      store.notificationCenterOpen,
    ]) {
      flag.value = true;
      expect(railPeek.peekEnabled()).toBe(false);
      flag.value = false;
    }

    store.rewindTarget.value = { messageId: 1, preview: 'x', isAssistant: false };
    expect(railPeek.peekEnabled()).toBe(false);
    store.rewindTarget.value = null;

    store.fileViewerPath.value = 'README.md';
    expect(railPeek.peekEnabled()).toBe(false);
    store.fileViewerPath.value = null;

    expect(railPeek.peekEnabled()).toBe(true);
  });

  it('does nothing at all when the rail is open', () => {
    setWindow({ collapsed: false });
    railPeek.openPeek('p_docs');
    expect(railPeek.peekProjectId.value).toBeNull();
    expect(listSessions).not.toHaveBeenCalled();
  });
});

describe('what a hover shows', () => {
  it('shows the open project from the store, without reading anything', () => {
    railPeek.openPeek('p_karucode');
    expect(railPeek.peekProjectId.value).toBe('p_karucode');
    expect(listSessions).not.toHaveBeenCalled();
  });

  it('reads another project exactly once, then serves it from the cache', async () => {
    vi.mocked(listSessions).mockResolvedValue([summaryFixture]);
    railPeek.openPeek('p_docs');
    await vi.waitFor(() => expect(railPeek.peekSessions.value.p_docs).toHaveLength(1));
    expect(listSessions).toHaveBeenCalledWith('p_docs');
    expect(railPeek.peekLoading.value.p_docs).toBe(false);

    railPeek.closePeek();
    railPeek.openPeek('p_docs');
    expect(listSessions).toHaveBeenCalledTimes(1);
  });

  it('re-reads on demand, so a rename or a delete is reflected', async () => {
    vi.mocked(listSessions).mockResolvedValue([summaryFixture]);
    railPeek.openPeek('p_docs');
    await vi.waitFor(() => expect(railPeek.peekSessions.value.p_docs).toHaveLength(1));

    vi.mocked(listSessions).mockResolvedValue([secondSummaryFixture]);
    railPeek.invalidatePeekSessions('p_docs');
    await railPeek.loadPeekSessions('p_docs', true);
    expect(railPeek.peekSessions.value.p_docs?.map((session) => session.id)).toEqual(['s_cd34']);
  });

  it('keeps a failed read as an error instead of an empty list', async () => {
    // An empty list would read as "this project has no sessions", a different and
    // wrong answer.
    vi.mocked(listSessions).mockRejectedValue(new Error('the server said no'));
    railPeek.openPeek('p_docs');
    await vi.waitFor(() => expect(railPeek.peekError.value.p_docs).toBe('the server said no'));
    expect(railPeek.peekSessions.value.p_docs).toBeUndefined();
    expect(railPeek.peekLoading.value.p_docs).toBe(false);
  });
});

describe('closing', () => {
  it('closes after the grace period, so clipping the edge does not flash it', () => {
    vi.useFakeTimers();
    try {
      railPeek.openPeek('p_karucode');
      railPeek.schedulePeekClose();
      vi.advanceTimersByTime(PEEK_CLOSE_MS - 1);
      expect(railPeek.peekProjectId.value).toBe('p_karucode');
      vi.advanceTimersByTime(2);
      expect(railPeek.peekProjectId.value).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stays open while the pointer is inside it', () => {
    vi.useFakeTimers();
    try {
      railPeek.openPeek('p_karucode');
      railPeek.schedulePeekClose();
      railPeek.holdPeek();
      vi.advanceTimersByTime(PEEK_CLOSE_MS * 4);
      expect(railPeek.peekProjectId.value).toBe('p_karucode');
    } finally {
      vi.useRealTimers();
    }
  });

  it('switches project without a close in between', () => {
    vi.useFakeTimers();
    try {
      railPeek.openPeek('p_karucode');
      railPeek.schedulePeekClose();
      railPeek.openPeek('p_docs');
      vi.advanceTimersByTime(PEEK_CLOSE_MS * 2);
      expect(railPeek.peekProjectId.value).toBe('p_docs');
    } finally {
      vi.useRealTimers();
    }
  });
});