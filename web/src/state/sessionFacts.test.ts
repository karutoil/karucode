import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionInfo } from '../api/types';

const { getSessionInfo } = vi.hoisted(() => ({
  getSessionInfo: vi.fn<(id: string) => Promise<SessionInfo>>(),
}));
vi.mock('../api/endpoints', () => ({ getSessionInfo }));

import {
  CONCURRENCY,
  MAX_TRACKED,
  TTL_MS,
  invalidateFacts,
  loadFacts,
  loadFactsFor,
  sessionFacts,
} from './sessionFacts';

function info(_id: string, calls = 2): SessionInfo {
  return {
    session: {} as SessionInfo['session'], running: false,
    stats: {
      messages_total: 4, messages_main: 3, messages_subagent: 1,
      messages_by_role: {}, messages_main_by_role: { user: 2 },
      tool_calls_total: calls, tool_calls: {}, runs_by_status: {}, runs_total: 1, working_ms: 1,
    },
    usage: { main: {} as SessionInfo['usage']['main'], by_agent: {}, total: {} as SessionInfo['usage']['total'] },
    context: { model: 'test', window: null, used: 0 }, agents: [],
    messages: { total: 4, main: 3, subagent: 1, by_role: {}, main_by_role: {} },
    tools: { total: calls, by_name: [{ name: 'run_command', count: calls }], truncated: false },
    runs: { total: 1, by_status: {}, working_ms: 1, recent: [] },
  };
}

beforeEach(() => {
  vi.useRealTimers();
  getSessionInfo.mockReset();
  invalidateFacts();
});

describe('sessionFacts loader', () => {
  it('coalesces concurrent reads and publishes the aggregate', async () => {
    let resolve!: (value: SessionInfo) => void;
    getSessionInfo.mockReturnValueOnce(new Promise((r) => { resolve = r; }));
    const first = loadFacts('s1');
    const second = loadFacts('s1');
    expect(getSessionInfo).toHaveBeenCalledTimes(1);
    resolve(info('s1'));
    await expect(first).resolves.toMatchObject({ turns: 2, calls: 2, tools: ['run_command'] });
    await expect(second).resolves.toEqual(await first);
    expect(sessionFacts.value.s1?.turns).toBe(2);
  });

  it('refreshes expired entries in a batch but skips fresh ones', async () => {
    vi.useFakeTimers();
    getSessionInfo.mockResolvedValue(info('s1'));
    await loadFactsFor(['s1']);
    expect(getSessionInfo).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(TTL_MS + 1);
    await loadFactsFor(['s1']);
    expect(getSessionInfo).toHaveBeenCalledTimes(2);
  });

  it('limits a long batch to the newest tracked sessions and concurrency', async () => {
    let active = 0;
    let peak = 0;
    getSessionInfo.mockImplementation(async (id) => {
      active += 1; peak = Math.max(peak, active);
      await Promise.resolve();
      active -= 1;
      return info(id);
    });
    await loadFactsFor(Array.from({ length: MAX_TRACKED + 10 }, (_, i) => `s${i}`));
    expect(getSessionInfo).toHaveBeenCalledTimes(MAX_TRACKED);
    expect(peak).toBeLessThanOrEqual(CONCURRENCY);
  });

  it('does not let an invalidated response repopulate the cache', async () => {
    let resolve!: (value: SessionInfo) => void;
    getSessionInfo.mockReturnValueOnce(new Promise((r) => { resolve = r; }));
    const pending = loadFacts('stale');
    invalidateFacts(['stale']);
    resolve(info('stale'));
    await pending;
    expect(sessionFacts.value.stale).toBeUndefined();
  });
});
