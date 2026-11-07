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

  it('leaves a newer same-session active snapshot untouched when the old send fails', async () => {
    const send = deferred<Response>();
    const run = detailFixture.runs[0]!;
    const runningDetail: SessionDetail = {
      ...detailFixture,
      session: { ...detailFixture.session, id: 's_old', project_id: 'p_one' },
      messages: [],
      runs: [{ ...run, id: 'r_from_snapshot', status: 'running', finished_at: null }],
      agents: [],
      approvals: [],
      claims: [],
      running: true,
    };
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions/s_old/messages' && init?.method === 'POST') return send.promise;
      if (url === '/api/sessions/s_old') return jsonResponse(runningDetail);
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_old';
    store.chat.value = emptyChatState('s_old');
    const sending = actions.sendChatMessage('request whose old response will fail');

    expect(await actions.openSession('s_old', { silent: true, force: true, preserveLive: true })).toBe(true);
    expect(store.chat.value.running).toBe(true);
    expect(store.chat.value.runId).toBe('r_from_snapshot');
    const authoritative = store.chat.value;

    send.resolve(errorResponse('offline', 'old response failed', 500));
    expect(await sending).toBe(false);
    await Promise.resolve();
    expect(store.chat.value).toBe(authoritative);
    expect(store.chat.value.runStatus).toBe('running');
    expect(store.chat.value.runId).toBe('r_from_snapshot');
    expect(store.toasts.value.some((toast) => toast.message === 'old response failed')).toBe(false);
    expect(fetchMock.mock.calls.filter(([url, init]) =>
      String(url) === '/api/sessions/s_old' && (!init?.method || init.method === 'GET'),
    )).toHaveLength(1);
  });

  it('still rolls back and reports a failure owned by the current send', async () => {
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions/s_old/messages' && init?.method === 'POST') {
        return errorResponse('offline', 'useful current send failure', 500);
      }
      if (url === '/api/sessions/s_old') {
        return jsonResponse({
          ...detail('s_old', 'p_one'),
          messages: [], runs: [], agents: [], approvals: [], claims: [], running: false,
        });
      }
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_old';
    store.chat.value = emptyChatState('s_old');

    expect(await actions.sendChatMessage('request fails now')).toBe(false);
    expect(store.chat.value.running).toBe(false);
    expect(store.chat.value.runStatus).toBeNull();
    expect(store.chat.value.items.some((item) =>
      item.kind === 'message' && item.content === 'request fails now',
    )).toBe(false);
    expect(store.toasts.value.some((toast) => toast.message === 'useful current send failure')).toBe(true);
    expect(fetchMock.mock.calls.filter(([url]) => String(url) === '/api/sessions/s_old')).toHaveLength(1);
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

describe('current navigation and request ownership', () => {
  it('lets a later create win over its destination switch automatic selection', async () => {
    const projectRead = deferred<Response>();
    const created = deferred<Response>();
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions?project=p_two') return projectRead.promise;
      if (url === '/api/sessions' && init?.method === 'POST') {
        expect(JSON.parse(String(init.body)).project).toBe('p_two');
        return created.promise;
      }
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      if (url === '/api/sessions/s_existing') return jsonResponse(detail('s_existing', 'p_two'));
      if (url === '/api/sessions/s_new') return jsonResponse(detail('s_new', 'p_two'));
      throw new Error(`unexpected ${url}`);
    });
    store.sessions.value = [summary('s_one', 'p_one')];
    const switching = actions.switchProject('p_two');
    await vi.waitFor(() => expect(store.currentProjectId.value).toBe('p_two'));

    // The create is later than the switch and is aimed at that same destination.
    const creating = actions.createSession();
    projectRead.resolve(jsonResponse({ sessions: [summary('s_existing', 'p_two')] }));
    await switching;
    // Defer/cancel the old automatic open instead of letting it invalidate create.
    expect(store.currentSessionId.value).toBeNull();
    expect(fetchMock.mock.calls.some(([url]) => String(url) === '/api/sessions/s_existing')).toBe(false);

    created.resolve(jsonResponse({
      ...detailFixture.session,
      id: 's_new',
      project_id: 'p_two',
      title: '',
    }));
    expect(await creating).toMatchObject({ id: 's_new', project_id: 'p_two' });
    expect(store.currentProjectId.value).toBe('p_two');
    expect(store.currentSessionId.value).toBe('s_new');
    expect(store.sessions.value.map((session) => session.id)).toContain('s_new');
  });

  it('reports a failed later create without reviving the superseded automatic open', async () => {
    const projectRead = deferred<Response>();
    const created = deferred<Response>();
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions?project=p_two') return projectRead.promise;
      if (url === '/api/sessions' && init?.method === 'POST') return created.promise;
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      if (url === '/api/sessions/s_existing') return jsonResponse(detail('s_existing', 'p_two'));
      throw new Error(`unexpected ${url}`);
    });
    const switching = actions.switchProject('p_two');
    await vi.waitFor(() => expect(store.currentProjectId.value).toBe('p_two'));
    const creating = actions.createSession();
    projectRead.resolve(jsonResponse({ sessions: [summary('s_existing', 'p_two')] }));
    await switching;

    created.resolve(errorResponse('offline', 'new session could not be created', 500));
    expect(await creating).toBeNull();
    expect(store.toasts.value.some((toast) => toast.message === 'new session could not be created')).toBe(true);
    expect(store.currentProjectId.value).toBe('p_two');
    expect(store.currentSessionId.value).toBeNull();
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s_existing']);
    expect(fetchMock.mock.calls.some(([url]) => String(url) === '/api/sessions/s_existing')).toBe(false);
  });

  it('does not merge or open a late create response after moving to another project', async () => {
    const created = deferred<Response>();
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') return created.promise;
      if (url === '/api/sessions?project=p_two') {
        return jsonResponse({ sessions: [summary('s_two', 'p_two')] });
      }
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      if (url === '/api/sessions/s_two') return jsonResponse(detail('s_two', 'p_two'));
      throw new Error(`unexpected ${url}`);
    });
    store.sessions.value = [summary('s_old_create', 'p_one')];

    const creating = actions.createSession();
    await actions.switchProject('p_two');
    expect(store.currentSessionId.value).toBe('s_two');

    created.resolve(jsonResponse({
      ...detailFixture.session,
      id: 's_late_create',
      project_id: 'p_one',
    }));
    expect(await creating).toBeNull();

    expect(store.currentProjectId.value).toBe('p_two');
    expect(store.currentSessionId.value).toBe('s_two');
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s_two']);
    expect(fetchMock.mock.calls.some(([url]) => String(url) === '/api/sessions/s_late_create')).toBe(false);
  });

  it('lets a later same-project selection win over a late create response', async () => {
    const created = deferred<Response>();
    stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') return created.promise;
      if (url === '/api/sessions/s_chosen') return jsonResponse(detail('s_chosen', 'p_one'));
      throw new Error(`unexpected ${url}`);
    });

    const creating = actions.createSession();
    await actions.openSession('s_chosen');
    created.resolve(jsonResponse({
      ...detailFixture.session,
      id: 's_late_same_project',
      project_id: 'p_one',
    }));
    expect(await creating).toBeNull();
    expect(store.currentSessionId.value).toBe('s_chosen');
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s_chosen']);
  });

  it('does not let old agent navigation select A after B was chosen during its project switch', async () => {
    const projectRead = deferred<Response>();
    const fetchMock = stubFetch((url) => {
      if (url === '/api/sessions?project=p_two') return projectRead.promise;
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      if (url === '/api/sessions/s_two') return jsonResponse(detail('s_two', 'p_two'));
      if (url === '/api/sessions/s_agent_old') return jsonResponse(detail('s_agent_old', 'p_two'));
      throw new Error(`unexpected ${url}`);
    });
    const agent = wireAgentSummary('a_old_navigation', {
      session_id: 's_agent_old',
      project_id: 'p_two',
    });

    const openingAgent = actions.openAgentSession(agent);
    await vi.waitFor(() => expect(store.currentProjectId.value).toBe('p_two'));
    await actions.openSession('s_two');
    projectRead.resolve(jsonResponse({ sessions: [
      summary('s_agent_old', 'p_two'),
      summary('s_two', 'p_two'),
    ] }));
    await openingAgent;

    expect(store.currentProjectId.value).toBe('p_two');
    expect(store.currentSessionId.value).toBe('s_two');
    expect(fetchMock.mock.calls.filter(([url]) => String(url) === '/api/sessions/s_agent_old')).toHaveLength(0);
  });

  it('keeps the newer online health result when an older failure finishes late', async () => {
    const oldRead = deferred<Response>();
    const healthy = {
      status: 'ok', version: 'new', uptime_secs: 2, workspace: null,
      projects: [], auth_required: false, provider_configured: true, db: 'ok',
    };
    stubFetch((url) => {
      if (url !== '/api/health') throw new Error(`unexpected ${url}`);
      return oldRead.promise;
    });

    const oldCheck = actions.checkHealth();
    globalThis.fetch = vi.fn(async () => jsonResponse(healthy)) as unknown as typeof fetch;
    expect(await actions.checkHealth()).toBe(true);
    expect(store.connection.value.api).toBe('online');
    expect(store.health.value?.version).toBe('new');

    oldRead.resolve(errorResponse('offline', 'old health failure', 500));
    expect(await oldCheck).toBe(false);
    expect(store.connection.value.api).toBe('online');
    expect(store.connection.value.lastError).toBeNull();
    expect(store.health.value?.version).toBe('new');
  });

  it('allows B to send while A is awaiting acceptance and still guards same-session duplicates', async () => {
    const a = deferred<Response>();
    const b = deferred<Response>();
    const fetchMock = stubFetch((url) => {
      if (url === '/api/sessions/s_a/messages') return a.promise;
      if (url === '/api/sessions/s_b/messages') return b.promise;
      throw new Error(`unexpected ${url}`);
    });
    store.currentSessionId.value = 's_a';
    store.chat.value = emptyChatState('s_a');
    const sendingA = actions.sendChatMessage('from A');

    store.currentSessionId.value = 's_b';
    store.chat.value = emptyChatState('s_b');
    const sendingB = actions.sendChatMessage('from B');
    expect(await actions.sendChatMessage('duplicate B')).toBe(false);
    expect(fetchMock.mock.calls.filter(([url]) => String(url) === '/api/sessions/s_b/messages')).toHaveLength(1);

    b.resolve(jsonResponse({ run_id: 'r_b', session_id: 's_b' }));
    expect(await sendingB).toBe(true);
    expect(store.chat.value.runId).toBe('r_b');
    a.resolve(jsonResponse({ run_id: 'r_a', session_id: 's_a' }));
    expect(await sendingA).toBe(true);
    expect(store.currentSessionId.value).toBe('s_b');
    expect(store.chat.value.runId).toBe('r_b');
  });

  it('forgets deleted-session notifications on the local delete path', async () => {
    store.recordNotification({
      id: 'run:gone', kind: 'run_finished', title: 'Finished', body: '',
      session_id: 's_gone', session_title: 'Gone', project_id: 'p_one',
      source: 'runtime', sticky: false,
    }, false);
    store.recordNotification({
      id: 'run:kept', kind: 'run_finished', title: 'Finished', body: '',
      session_id: 's_kept', session_title: 'Kept', project_id: 'p_one',
      source: 'runtime', sticky: false,
    }, false);
    stubFetch((url, init) => {
      if (url === '/api/sessions/s_gone' && init?.method === 'DELETE') {
        return jsonResponse({ deleted: true });
      }
      throw new Error(`unexpected ${url}`);
    });

    expect(await actions.deleteSessionById('s_gone')).toBe(true);
    expect(store.notifications.value.map((entry) => entry.id)).toEqual(['run:kept']);
    expect(store.unread.value).toEqual({ s_kept: 1 });
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
