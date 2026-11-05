import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { KarucodeEvent, SessionDetail, SessionSummary } from '../api/types';
import {
  detailFixture,
  errorResponse,
  jsonResponse,
  queuedFixture,
  resetAppState,
  summaryFixture,
  wireAgentSummary,
} from '../test/fixtures';
import * as actions from './actions';
import { applyEventToChat } from './eventReducer';
import { emptyChatState } from './chat';
import * as store from './store';

const realFetch = globalThis.fetch;

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function stubFetch(
  handler: (url: string, init?: RequestInit) => Response | Promise<Response>,
): ReturnType<typeof vi.fn> {
  const impl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    return handler(url, init);
  });
  globalThis.fetch = impl as unknown as typeof fetch;
  return impl;
}

function summary(id: string, projectId: string): SessionSummary {
  return { ...summaryFixture, id, title: id, project_id: projectId };
}

function detail(id: string, projectId: string): SessionDetail {
  return {
    ...detailFixture,
    session: { ...detailFixture.session, id, project_id: projectId },
  };
}

beforeEach(() => {
  resetAppState();
  store.currentProjectId.value = 'p_one';
});

afterEach(() => {
  globalThis.fetch = realFetch;
  resetAppState();
  vi.restoreAllMocks();
});

describe('project-scoped reads', () => {
  it('drops a stale session-list failure without clearing the new project loading state', async () => {
    const oldRead = deferred<Response>();
    const newRead = deferred<Response>();
    stubFetch((url) => {
      if (url === '/api/sessions?project=p_one') return oldRead.promise;
      if (url === '/api/sessions?project=p_two') return newRead.promise;
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      throw new Error(`unexpected ${url}`);
    });

    const oldCall = actions.refreshSessions();
    const switching = actions.switchProject('p_two');
    await vi.waitFor(() => expect(store.sessionsLoading.value).toBe(true));
    oldRead.resolve(errorResponse('offline', 'old project failed', 500));
    await oldCall;

    expect(store.currentProjectId.value).toBe('p_two');
    expect(store.sessionsLoading.value).toBe(true);
    expect(store.connection.value.api).not.toBe('offline');
    expect(store.sessions.value).toEqual([]);

    newRead.resolve(jsonResponse({ sessions: [summary('s_two', 'p_two')] }));
    await switching;
    await vi.waitFor(() => expect(store.currentSessionId.value).toBe('s_two'));
  });

  it('drops a stale session-list success instead of mixing foreign rows into the rail', async () => {
    const oldRead = deferred<Response>();
    stubFetch((url) => {
      if (url === '/api/sessions?project=p_one') return oldRead.promise;
      if (url === '/api/sessions?project=p_two') return jsonResponse({ sessions: [] });
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      return jsonResponse(detail('s_two', 'p_two'));
    });

    const oldCall = actions.refreshSessions();
    await actions.switchProject('p_two');
    oldRead.resolve(jsonResponse({ sessions: [summary('s_one', 'p_one')] }));
    await oldCall;

    expect(store.sessions.value).toEqual([]);
    expect(store.currentProjectId.value).toBe('p_two');
  });

  it('separates session-info reads when the same id belongs to a newer generation', async () => {
    const oldInfo = deferred<Response>();
    const newInfo = deferred<Response>();
    let infoReads = 0;
    stubFetch((url) => {
      if (url === '/api/sessions/s_shared/info') {
        return ++infoReads === 1 ? oldInfo.promise : newInfo.promise;
      }
      if (url === '/api/sessions/s_new') return jsonResponse(detail('s_new', 'p_one'));
      if (url === '/api/sessions/s_shared') return jsonResponse(detail('s_shared', 'p_one'));
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_shared';
    const oldRead = actions.refreshSessionInfo();
    await actions.openSession('s_new');
    await actions.openSession('s_shared');
    const newRead = actions.refreshSessionInfo();
    expect(infoReads).toBe(2);
    expect(store.sessionInfoLoading.value).toBe(true);

    oldInfo.resolve(errorResponse('offline', 'old generation failed', 500));
    await oldRead;
    expect(store.sessionInfoLoading.value).toBe(true);
    expect(store.sessionInfoError.value).toBeNull();

    newInfo.resolve(jsonResponse({ session: { id: 's_shared' } }));
    await newRead;
    expect(store.sessionInfoLoading.value).toBe(false);
    expect(store.sessionInfo.value?.session.id).toBe('s_shared');
    expect(store.sessionInfoError.value).toBeNull();
  });

  it('does not let an old agent-list error or finally block replace the current project', async () => {
    const oldRead = deferred<Response>();
    const newRead = deferred<Response>();
    const oldAgent = wireAgentSummary('a_old');
    store.agentsAll.value = [oldAgent];
    stubFetch((url) => {
      if (url === '/api/agents?project=p_one') return oldRead.promise;
      if (url === '/api/agents?project=p_two') return newRead.promise;
      if (url === '/api/sessions?project=p_two') return jsonResponse({ sessions: [] });
      throw new Error(`unexpected ${url}`);
    });

    const oldCall = actions.refreshAgents();
    const switching = actions.switchProject('p_two');
    oldRead.resolve(errorResponse('offline', 'old agents failed', 500));
    await oldCall;

    expect(store.agentsLoading.value).toBe(true);
    expect(store.agentsError.value).toBeNull();
    expect(store.agentsAll.value).toEqual([]);

    const newAgent = wireAgentSummary('a_new');
    newRead.resolve(jsonResponse({ agents: [newAgent] }));
    await switching;
    await vi.waitFor(() => expect(store.agentsLoading.value).toBe(false));
    expect(store.agentsAll.value.map((agent) => agent.id)).toEqual(['a_new']);
    expect(store.agentsError.value).toBeNull();
  });
});

describe('recovery after the reader moved on', () => {
  async function openOtherSession(fetchMock: ReturnType<typeof vi.fn>): Promise<void> {
    void fetchMock;
    await actions.openSession('s_new', { force: true });
    expect(store.currentSessionId.value).toBe('s_new');
  }

  it('does not navigate back when an old plain send fails and recovers', async () => {
    const send = deferred<Response>();
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions/s_old/messages' && init?.method === 'POST') return send.promise;
      if (url === '/api/sessions/s_new') return jsonResponse(detail('s_new', 'p_one'));
      if (url === '/api/sessions/s_old') return jsonResponse(detail('s_old', 'p_one'));
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_old';
    store.chat.value = emptyChatState('s_old');

    const sending = actions.sendChatMessage('hello old');
    await openOtherSession(fetchMock);
    send.resolve(errorResponse('offline', 'old send failed', 500));
    expect(await sending).toBe(false);
    await Promise.resolve();

    expect(store.currentSessionId.value).toBe('s_new');
    const oldReads = fetchMock.mock.calls.filter(
      ([url, init]) => String(url) === '/api/sessions/s_old' && (!init?.method || init.method === 'GET'),
    );
    expect(oldReads).toHaveLength(0);
  });

  it('does not navigate back when an old queue 409 requests recovery', async () => {
    const queued = deferred<Response>();
    const fetchMock = stubFetch((url) => {
      if (url.includes('/s_old/messages?mode=steer')) return queued.promise;
      if (url === '/api/sessions/s_new') return jsonResponse(detail('s_new', 'p_one'));
      if (url === '/api/sessions/s_old') return jsonResponse(detail('s_old', 'p_one'));
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_old';
    store.chat.value = { ...emptyChatState('s_old'), running: true, runStatus: 'running' };

    const accepting = actions.queueChatMessage('steer', { content: 'old steer' });
    await openOtherSession(fetchMock);
    queued.resolve(errorResponse('no_run', 'old run settled', 409));
    expect(await accepting).toBe(false);
    await Promise.resolve();

    expect(store.currentSessionId.value).toBe('s_new');
    expect(fetchMock.mock.calls.some(([url]) => String(url) === '/api/sessions/s_old')).toBe(false);
  });

  it('does not navigate back when an old recall 404 requests recovery', async () => {
    const recall = deferred<Response>();
    const fetchMock = stubFetch((url) => {
      if (url === '/api/sessions/s_old/queues/q_old') return recall.promise;
      if (url === '/api/sessions/s_new') return jsonResponse(detail('s_new', 'p_one'));
      if (url === '/api/sessions/s_old') return jsonResponse(detail('s_old', 'p_one'));
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_old';
    store.chat.value = { ...emptyChatState('s_old'), queued: [queuedFixture({ id: 'q_old' })] };

    const recalling = actions.recallQueuedMessage('q_old');
    await openOtherSession(fetchMock);
    recall.resolve(errorResponse('not_found', 'already applied', 404));
    expect(await recalling).toBe(false);
    await Promise.resolve();

    expect(store.currentSessionId.value).toBe('s_new');
    expect(store.composerHandoff.value).toBeNull();
    expect(store.recallingQueued.value).toEqual({});
    expect(fetchMock.mock.calls.some(([url]) => String(url) === '/api/sessions/s_old')).toBe(false);
  });
});

describe('queue acceptance versus live consumption', () => {
  it('cannot resurrect an entry that a reducer frame already consumed', async () => {
    const accepted = deferred<Response>();
    stubFetch(() => accepted.promise);
    store.currentSessionId.value = 's_one';
    store.chat.value = { ...emptyChatState('s_one'), running: true, runStatus: 'running' };
    const entry = queuedFixture({ id: 'q_late', kind: 'steer' });

    const accepting = actions.queueChatMessage('steer', { content: 'already landed' });
    const arrived = {
      seq: 1,
      ts_ms: 1,
      session_id: 's_one',
      type: 'queued_messages',
      steers: [entry],
      follow_ups: [],
    } as unknown as KarucodeEvent;
    store.chat.value = applyEventToChat(store.chat.value, arrived);
    expect(store.chat.value.queued.map((queued) => queued.id)).toEqual(['q_late']);
    const consumed = {
      seq: 2,
      ts_ms: 2,
      session_id: 's_one',
      type: 'steer_applied',
      steer_id: 'q_late',
      content: entry.content,
    } as KarucodeEvent;
    store.chat.value = applyEventToChat(store.chat.value, consumed);
    expect(store.chat.value.queued).toEqual([]);

    accepted.resolve(jsonResponse({
      mode: 'steer', session_id: 's_one', steer_id: 'q_late', queued: true,
      queued_message: entry,
    }, 202));
    expect(await accepting).toBe(true);
    expect(store.chat.value.queued).toEqual([]);
  });
});
