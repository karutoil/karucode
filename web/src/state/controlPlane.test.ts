/**
 * The control plane's snapshot read.
 *
 * What matters here is the contract the UI depends on: one read fills one
 * snapshot, a failure records itself *without* erasing what is on screen, an
 * out-of-order response is dropped, and the poller stops when its disposer runs.
 * The routes themselves are mocked: this module must not care how the server
 * builds them.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getControlOverview, listControlChats } from '../api/endpoints';
import type { AgentSummary, ControlChatSummary, ControlOverview } from '../api/types';
import { NOW, summaryFixture, wireAgentSummary } from '../test/fixtures';
import * as controlPlane from './controlPlane';

vi.mock('../api/endpoints', () => ({
  getControlOverview: vi.fn(),
  listControlChats: vi.fn(),
}));

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function chat(id: string, overrides: Partial<ControlChatSummary> = {}): ControlChatSummary {
  return {
    id,
    title: `Control chat ${id}`,
    workspace: '',
    project_id: null,
    created_at: NOW - 60_000,
    updated_at: NOW - 30_000,
    message_count: 4,
    last_run_status: 'completed',
    running: false,
    control: true,
    agents: { live: 0, total: 0 },
    model_label: null,
    ...overrides,
  };
}

function overview(overrides: Partial<ControlOverview> = {}): ControlOverview {
  return {
    generated_at: NOW,
    sessions: { total: 3, running: 1, failed: 0, control: 1 },
    agents: { live: 1, paused: 0, failed: 0, total: 2 },
    approvals: { pending: 0 },
    projects: [
      { id: 'p_karucode', name: 'karucode', path: '/home/me/karucode', sessions: 3, running: 1 },
    ],
    control: [chat('s_control')],
    running: [{ ...summaryFixture, running: true, last_run_status: 'running' }],
    attention: [],
    waiting: [],
    recent: [summaryFixture],
    live_agents: [],
    ...overrides,
  };
}

function agent(id: string, sessionId: string, status: AgentSummary['status'] = 'running'): AgentSummary {
  return wireAgentSummary(id, { session_id: sessionId, status });
}

beforeEach(() => {
  controlPlane.resetControlPlane();
  controlPlane.controlPlaneOpen.value = false;
  vi.mocked(getControlOverview).mockReset();
  vi.mocked(listControlChats).mockReset();
  vi.mocked(getControlOverview).mockResolvedValue(overview());
  vi.mocked(listControlChats).mockResolvedValue([chat('s_control')]);
});

describe('loading the overview', () => {
  it('fills the snapshot from one read and clears any previous error', async () => {
    controlPlane.controlOverviewError.value = 'an older failure';
    const snapshot = overview();
    vi.mocked(getControlOverview).mockResolvedValue(snapshot);

    await controlPlane.loadControlOverview();

    expect(getControlOverview).toHaveBeenCalledTimes(1);
    expect(controlPlane.controlOverview.value).toEqual(snapshot);
    expect(controlPlane.controlOverviewError.value).toBeNull();
    expect(controlPlane.controlOverviewLoading.value).toBe(false);
  });

  it('records a failure and keeps the previous snapshot on screen', async () => {
    const snapshot = overview();
    vi.mocked(getControlOverview).mockResolvedValueOnce(snapshot);
    await controlPlane.loadControlOverview();

    vi.mocked(getControlOverview).mockRejectedValueOnce(new Error('the server said no'));
    // Must not reject: the poller calls this on a timer.
    await expect(controlPlane.loadControlOverview()).resolves.toBeUndefined();

    expect(controlPlane.controlOverviewError.value).toBe('the server said no');
    expect(controlPlane.controlOverview.value).toEqual(snapshot);
    expect(controlPlane.controlOverviewLoading.value).toBe(false);
  });

  it('drops a stale response that arrives after a newer one', async () => {
    const slow = deferred<ControlOverview>();
    const fast = overview({ generated_at: NOW + 5_000, sessions: { total: 9, running: 0, failed: 0, control: 0 } });
    vi.mocked(getControlOverview)
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce(fast);

    const first = controlPlane.loadControlOverview();
    await controlPlane.loadControlOverview();
    expect(controlPlane.controlOverview.value).toEqual(fast);

    // The older read now lands; it must not win.
    slow.resolve(overview({ sessions: { total: 1, running: 0, failed: 0, control: 0 } }));
    await first;

    expect(controlPlane.controlOverview.value).toEqual(fast);
    expect(controlPlane.controlOverviewError.value).toBeNull();
  });

  it('does not let a stale response overwrite a newer snapshot', async () => {
    const slow = deferred<ControlOverview>();
    const fast = overview({ generated_at: NOW + 1_000 });
    vi.mocked(getControlOverview)
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce(fast);

    const first = controlPlane.loadControlOverview();
    await controlPlane.loadControlOverview();
    slow.resolve(overview({ generated_at: NOW - 10_000 }));

    await first;
    expect(controlPlane.controlOverviewError.value).toBeNull();
    expect(controlPlane.controlOverview.value?.generated_at).toBe(NOW + 1_000);
  });
});

describe('loading the chat list', () => {
  it('replaces only the control array of the snapshot', async () => {
    const snapshot = overview({ control: [chat('s_one')] });
    vi.mocked(getControlOverview).mockResolvedValue(snapshot);
    await controlPlane.loadControlOverview();

    const chats = [chat('s_two', { running: true, last_run_status: 'running' })];
    vi.mocked(listControlChats).mockResolvedValue(chats);
    await controlPlane.loadControlChats();

    expect(controlPlane.controlChats().map((item) => item.id)).toEqual(['s_two']);
    // Everything else in the snapshot is untouched.
    expect(controlPlane.controlOverview.value?.sessions).toEqual(snapshot.sessions);
    expect(controlPlane.controlOverview.value?.recent).toEqual(snapshot.recent);
    expect(controlPlane.controlChatsError.value).toBeNull();
    expect(controlPlane.controlChatsLoading.value).toBe(false);
  });

  it('keeps the previous list and records the failure', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(overview({ control: [chat('s_one')] }));
    await controlPlane.loadControlOverview();

    vi.mocked(listControlChats).mockRejectedValue(new Error('timed out'));
    await expect(controlPlane.loadControlChats()).resolves.toBeUndefined();

    expect(controlPlane.controlChatsError.value).toBe('timed out');
    expect(controlPlane.controlChats().map((item) => item.id)).toEqual(['s_one']);
  });

  it('drops an older chat response', async () => {
    await controlPlane.loadControlOverview();
    const slow = deferred<ControlChatSummary[]>();
    vi.mocked(listControlChats)
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce([chat('s_new')]);

    const first = controlPlane.loadControlChats();
    await controlPlane.loadControlChats();
    slow.resolve([chat('s_old')]);
    await first;

    expect(controlPlane.controlChats().map((item) => item.id)).toEqual(['s_new']);
  });
});

describe('reading what the snapshot holds', () => {
  it('falls back to an empty list before the first read', () => {
    expect(controlPlane.controlOverview.value).toBeNull();
    expect(controlPlane.controlChats()).toEqual([]);
    expect(controlPlane.agentsForControlChat('s_control')).toEqual([]);
  });

  it('filters live agents by session id', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(
      overview({
        live_agents: [
          agent('a_1', 's_control'),
          agent('a_2', 's_other'),
          agent('a_3', 's_control', 'paused'),
        ],
      }),
    );
    await controlPlane.loadControlOverview();

    expect(controlPlane.agentsForControlChat('s_control').map((item) => item.id)).toEqual(['a_1', 'a_3']);
    expect(controlPlane.agentsForControlChat('s_other').map((item) => item.id)).toEqual(['a_2']);
    expect(controlPlane.agentsForControlChat('s_missing')).toEqual([]);
    expect(controlPlane.agentsForControlChat('')).toEqual([]);
  });
});

describe('polling', () => {
  it('reads on a timer until its disposer runs, and each caller owns its own timer', async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(getControlOverview).mockResolvedValue(overview());

      const stop = controlPlane.startControlPlaneRefresh(1000);
      await vi.advanceTimersByTimeAsync(1000);
      expect(getControlOverview).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(2000);
      expect(getControlOverview).toHaveBeenCalledTimes(3);

      stop();
      await vi.advanceTimersByTimeAsync(5000);
      expect(getControlOverview).toHaveBeenCalledTimes(3);

      // A second call is a second interval, not a shared one.
      const stopSecond = controlPlane.startControlPlaneRefresh(1000);
      await vi.advanceTimersByTimeAsync(1000);
      expect(getControlOverview).toHaveBeenCalledTimes(4);
      stopSecond();
      await vi.advanceTimersByTimeAsync(5000);
      expect(getControlOverview).toHaveBeenCalledTimes(4);
    } finally {
      vi.useRealTimers();
    }
  });

  it('defaults to five seconds', async () => {
    vi.useFakeTimers();
    try {
      expect(controlPlane.CONTROL_REFRESH_MS).toBe(5000);
      const stop = controlPlane.startControlPlaneRefresh();
      await vi.advanceTimersByTimeAsync(4_999);
      expect(getControlOverview).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(getControlOverview).toHaveBeenCalledTimes(1);
      stop();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('resetting', () => {
  it('clears every signal, and a read still in flight cannot repopulate them', async () => {
    const pending = deferred<ControlOverview>();
    vi.mocked(getControlOverview).mockReturnValueOnce(pending.promise);
    const read = controlPlane.loadControlOverview();
    expect(controlPlane.controlOverviewLoading.value).toBe(true);

    controlPlane.resetControlPlane();
    expect(controlPlane.controlOverview.value).toBeNull();
    expect(controlPlane.controlOverviewError.value).toBeNull();
    expect(controlPlane.controlOverviewLoading.value).toBe(false);
    expect(controlPlane.controlChatsError.value).toBeNull();

    pending.resolve(overview());
    await read;
    expect(controlPlane.controlOverview.value).toBeNull();
  });
});
