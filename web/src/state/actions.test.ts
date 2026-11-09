import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { emptyChatState } from './chat';
import { emptyUsage } from '../lib/sessionInfo';
import * as actions from './actions';
import * as store from './store';
import type { SessionInfo } from '../api/types';
import { approvalFixture, detailFixture, jsonResponse, errorResponse, resetAppState } from '../test/fixtures';

/**
 * What these cover.
 *
 * Both are request-shaping bugs in the action layer rather than in a component:
 * the store ends up describing something the server never said. The queue and
 * composer suites already stub `fetch` this way, so the same style is used here -
 * what is asserted is the request that leaves the browser and the state that is
 * left behind when it fails.
 */

const realFetch = globalThis.fetch;

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

/** A deferred response: lets a test hold one read open while another starts. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}


/** A `GET /api/sessions/:id/info` body, with the one figure these tests read. */
function sessionInfo(sessionId: string, inputTokens: number): SessionInfo {
  const totals = { ...emptyUsage(), calls: 1, input_tokens: inputTokens, output_tokens: 10 };
  return {
    session: {
      id: sessionId,
      title: sessionId,
      workspace: '/work',
      project_id: null,
      created_at: 0,
      updated_at: 0,
    },
    running: true,
    stats: {
      messages_total: 1,
      messages_main: 1,
      messages_subagent: 0,
      messages_by_role: { user: 1 },
      messages_main_by_role: { user: 1 },
      tool_calls_total: 0,
      tool_calls: {},
      runs_by_status: { running: 1 },
      runs_total: 1,
      working_ms: 0,
    },
    usage: { main: totals, by_agent: {}, total: totals },
    context: { model: 'gpt-5', window: 128_000, used: inputTokens },
    agents: [],
    messages: {
      total: 1,
      main: 1,
      subagent: 0,
      by_role: { user: 1 },
      main_by_role: { user: 1 },
    },
    tools: { total: 0, by_name: [], truncated: false },
    runs: { total: 1, by_status: { running: 1 }, working_ms: 0, recent: [] },
  };
}

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  globalThis.fetch = realFetch;
  resetAppState();
  vi.restoreAllMocks();
});

describe('refreshSessionInfo', () => {
  it('coalesces a burst for the same session into one read', async () => {
    store.currentSessionId.value = 's_ab12';
    const one = deferred<Response>();
    const fetchMock = stubFetch(() => one.promise);

    const first = actions.refreshSessionInfo();
    const second = actions.refreshSessionInfo();
    // Both callers are waiting on the same request, not two of the same request.
    expect(fetchMock).toHaveBeenCalledTimes(1);

    one.resolve(jsonResponse(sessionInfo('s_ab12', 1_000)));
    await Promise.all([first, second]);
    expect(store.sessionInfoFor.value).toBe('s_ab12');
    expect(store.sessionInfoLoading.value).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('does not answer a read for session B with session A\'s in-flight read', async () => {
    // The bug: a single `sessionInfoInFlight` flag meant B's call was handed A's
    // promise. That promise requests A's URL, the "did I move session" guard then
    // drops the result, and B never loads - an empty Session tab with no error.
    store.currentSessionId.value = 's_a';
    const aRead = deferred<Response>();
    const requested: string[] = [];
    stubFetch((url) => {
      requested.push(url);
      if (url.includes('/s_a/')) return aRead.promise;
      return jsonResponse(sessionInfo('s_b', 2_000));
    });

    const aCall = actions.refreshSessionInfo();
    // The user switches sessions while A's read is still open.
    store.currentSessionId.value = 's_b';
    expect(store.sessionInfoLoading.value).toBe(true);
    const bCall = actions.refreshSessionInfo();
    await bCall;

    expect(requested).toEqual(
      expect.arrayContaining(['/api/sessions/s_a/info', '/api/sessions/s_b/info']),
    );
    // B has its own numbers, not A's and not nothing.
    expect(store.sessionInfoFor.value).toBe('s_b');
    expect(store.sessionInfo?.value?.usage.total.input_tokens).toBe(2_000);

    aRead.resolve(jsonResponse(sessionInfo('s_a', 1_000)));
    await aCall;
    // A's late answer must not overwrite B's tab, and must not clear B's state.
    expect(store.sessionInfoFor.value).toBe('s_b');
    expect(store.sessionInfo?.value?.usage.total.input_tokens).toBe(2_000);
  });

  it('does not let a superseded read report "not loading" for the current one', async () => {
    // The second half of the bug: A's `finally` set `sessionInfoLoading = false`
    // unconditionally, so B's caller saw a finished spinner for a read that had
    // not started returning anything.
    store.currentSessionId.value = 's_a';
    const aRead = deferred<Response>();
    const bRead = deferred<Response>();
    stubFetch((url) => (url.includes('/s_a/') ? aRead.promise : bRead.promise));

    const aCall = actions.refreshSessionInfo();
    store.currentSessionId.value = 's_b';
    const bCall = actions.refreshSessionInfo();
    expect(store.sessionInfoLoading.value).toBe(true);

    // A finishes first, while B is still open.
    aRead.resolve(jsonResponse(sessionInfo('s_a', 1_000)));
    await aCall;
    expect(store.sessionInfoLoading.value).toBe(true);

    bRead.resolve(jsonResponse(sessionInfo('s_b', 2_000)));
    await bCall;
    expect(store.sessionInfoLoading.value).toBe(false);
    expect(store.sessionInfoFor.value).toBe('s_b');
  });
});

describe('resolveApproval', () => {
  function pendingApproval() {
    store.currentSessionId.value = 's_ab12';
    store.chat.value = {
      ...emptyChatState('s_ab12'),
      running: true,
      runStatus: 'running',
      items: [
        {
          kind: 'approval',
          id: 'approval:ap_1',
          at: 1_000,
          approval: { ...approvalFixture, agent_id: null, status: 'pending', decision: null },
        },
      ],
    };
  }

  it('refetches the session when the decision could not be delivered', async () => {
    // A failed resolve used to leave `approval.status === 'pending'`, so the card
    // kept telling the reader the run was waiting for a decision it had already
    // tried to give. Only the server knows whether it is still pending, so the
    // state comes from a refetch rather than from a local guess.
    pendingApproval();
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/approvals/ap_1') {
        expect(init?.method).toBe('POST');
        return errorResponse('internal', 'the approval channel closed', 500);
      }
      // The refetch: the server says nothing is pending any more.
      return jsonResponse({ ...detailFixture, approvals: [], running: true });
    });

    const ok = await actions.resolveApproval('ap_1', 'allow_once');
    expect(ok).toBe(false);
    // The toast is still the immediate feedback.
    expect(store.toasts.value.at(-1)?.message).toContain('the approval channel closed');

    // ...and the transcript was asked to re-read the session.
    await vi.waitFor(() => {
      expect(fetchMock.mock.calls.some(([input]) => String(input) === '/api/sessions/s_ab12')).toBe(
        true,
      );
    });
    // Nothing was invented locally: the card is not marked expired or resolved.
    const item = store.chat.value.items.find((entry) => entry.kind === 'approval');
    expect(item?.kind === 'approval' ? item.approval.status : null).toBe('pending');
  });

  it('marks the approval expired on a 404 without refetching twice', async () => {
    // The expired path is already correct and must keep working: the server said
    // the approval does not exist, so the card can say so immediately.
    pendingApproval();
    stubFetch((url) => {
      if (url === '/api/approvals/ap_1') {
        return errorResponse('not_found', 'approval ap_1', 404);
      }
      return jsonResponse({ ...detailFixture, approvals: [], running: true });
    });

    const ok = await actions.resolveApproval('ap_1', 'deny');
    expect(ok).toBe(false);
    const item = store.chat.value.items.find((entry) => entry.kind === 'approval');
    expect(item?.kind === 'approval' ? item.approval.status : null).toBe('expired');
  });
});
describe('the notification centre’s actions', () => {
  const entry = {
    id: 'run:r_9',
    sessionId: 's_other',
    projectId: 'p_docs',
  };

  /** A notification about another project's session, as the server reported it. */
  function recordNotification(): void {
    store.recordNotification(
      {
        id: entry.id,
        kind: 'run_finished',
        title: 'Run finished',
        body: 'Docs rebuilt.',
        session_id: entry.sessionId,
        session_title: 'Rebuild the docs',
        project_id: 'p_docs',
        source: 'runtime',
        sticky: false,
      },
      false,
    );
  }

  it('opens the overlay, and only the overlay', () => {
    store.settingsOpen.value = true;
    actions.openNotificationCenter();
    expect(store.notificationCenterOpen.value).toBe(true);
    expect(store.settingsOpen.value).toBe(false);

    actions.toggleNotificationCenter();
    expect(store.notificationCenterOpen.value).toBe(false);
    actions.toggleNotificationCenter();
    expect(store.notificationCenterOpen.value).toBe(true);
    actions.closeNotificationCenter();
    expect(store.notificationCenterOpen.value).toBe(false);
  });

  it('switches project before opening a session that lives in another one', async () => {
    // The rail only ever holds one project's sessions, so opening a session from
    // the centre without moving the project would leave the rail describing one
    // project while the chat showed another.
    store.currentProjectId.value = 'p_karucode';
    store.currentSessionId.value = 's_here';
    recordNotification();
    store.notificationCenterOpen.value = true;

    const requested: string[] = [];
    stubFetch((url) => {
      requested.push(url);
      if (url === '/api/sessions/s_other') {
        return jsonResponse({
          ...detailFixture,
          session: { ...detailFixture.session, id: 's_other', project_id: 'p_docs' },
        });
      }
      if (url.startsWith('/api/sessions?')) {
        return jsonResponse({ sessions: [] });
      }
      if (url.startsWith('/api/agents')) {
        return jsonResponse({ agents: [] });
      }
      return jsonResponse({});
    });

    await actions.openNotification(entry);

    // Marking read is part of opening: the badge clears without a second gesture.
    expect(store.unreadTotal.value).toBe(0);
    expect(store.notificationCenterOpen.value).toBe(false);
    expect(store.currentProjectId.value).toBe('p_docs');
    expect(store.currentSessionId.value).toBe('s_other');
    // The project's sessions were read *before* the session itself, which is the
    // order the rail depends on.
    expect(requested).toContain('/api/sessions?project=p_docs');
    expect(requested).toContain('/api/sessions/s_other');
    expect(requested.indexOf('/api/sessions?project=p_docs')).toBeLessThan(
      requested.indexOf('/api/sessions/s_other'),
    );
  });

  it('routes the toast one-argument Open path through the notification project', async () => {
    store.currentProjectId.value = 'p_karucode';
    store.currentSessionId.value = 's_here';
    recordNotification();
    stubFetch((url) => {
      if (url === '/api/sessions/s_other') {
        return jsonResponse({
          ...detailFixture,
          session: { ...detailFixture.session, id: 's_other', project_id: 'p_docs' },
        });
      }
      if (url.startsWith('/api/sessions?')) return jsonResponse({ sessions: [] });
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      return jsonResponse({});
    });

    // This is exactly ToastRegion's call shape; the notification record supplies
    // the missing project and focusSession delegates to openSessionInProject.
    vi.spyOn(window, 'focus').mockImplementation(() => undefined);
    actions.focusSession(entry.sessionId);
    await vi.waitFor(() => expect(store.currentSessionId.value).toBe('s_other'));
    expect(store.currentProjectId.value).toBe('p_docs');
  });

  it('does not switch project for a session already in the open one', async () => {
    store.currentProjectId.value = 'p_docs';
    store.currentSessionId.value = 's_elsewhere';
    recordNotification();

    const requested: string[] = [];
    stubFetch((url) => {
      requested.push(url);
      if (url === '/api/sessions/s_other') {
        return jsonResponse({
          ...detailFixture,
          session: { ...detailFixture.session, id: 's_other', project_id: 'p_docs' },
        });
      }
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      return jsonResponse({});
    });

    await actions.openNotification(entry);

    expect(store.currentProjectId.value).toBe('p_docs');
    expect(store.currentSessionId.value).toBe('s_other');
    expect(requested).not.toContain('/api/sessions?project=p_docs');
  });

  it('marks a notice about no session read without navigating anywhere', async () => {
    store.recordNotification(
      {
        id: 'plugin:kc-todo:x',
        kind: 'info',
        title: 'Todo list updated',
        body: '',
        session_id: '',
        session_title: '',
        source: 'kc-todo',
        sticky: false,
      },
      false,
    );
    const fetchMock = stubFetch(() => jsonResponse({}));

    await actions.openNotification({ id: 'plugin:kc-todo:x', sessionId: '', projectId: null });

    expect(store.unreadTotal.value).toBe(0);
    expect(store.notificationCenterOpen.value).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe('transcript pagination', () => {
  it('loads older session pages by exclusive cursor and retains the current tail', async () => {
    const tail = detailFixture.messages[0]!;
    const older = { ...tail, id: tail.id - 1, content: 'older page', created_at: tail.created_at - 1 };
    const fetchMock = stubFetch((url) => jsonResponse({ ...detailFixture, messages: url.includes('before=') ? [older] : [tail], messages_truncated: !url.includes('before='), messages_before: url.includes('before=') ? null : tail.id }));
    await actions.openSession(detailFixture.session.id, { force: true });
    expect(store.sessionHistory.value?.before).toBe(tail.id);
    await actions.loadOlderSession();
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes(`before=${tail.id}`))).toBe(true);
    expect(store.sessionHistory.value?.messages.map((m) => m.id)).toEqual([older.id, tail.id]);
    expect(store.chat.value.items.some((item) => item.kind === 'message' && item.content === 'older page')).toBe(true);
    expect(store.chat.value.items.some((item) => item.id === 'notice:history-truncated')).toBe(false);
  });

  it('loads agent pages and reports failures without dropping the cursor', async () => {
    const agent = (await import('../test/fixtures')).wireAgentSummary('a_page');
    const message = { ...detailFixture.messages[0]!, agent_id: agent.id };
    let fail = true;
    stubFetch((url) => {
      if (url.includes('before=') && fail) return errorResponse('error', 'page failed', 500);
      return jsonResponse({ agent, messages: [url.includes('before=') ? { ...message, id: message.id - 1 } : message], messages_truncated: !url.includes('before='), messages_before: url.includes('before=') ? null : message.id, messages_limit: 2000 });
    });
    await actions.openAgentDetail(agent.id);
    await actions.loadOlderAgent();
    expect(store.agentHistory.value?.error).toBeTruthy();
    expect(store.agentHistory.value?.before).toBe(message.id);
    fail = false;
    await actions.loadOlderAgent();
    expect(store.agentHistory.value?.messages.map((m) => m.id)).toEqual([message.id - 1, message.id]);
    expect(store.agentHistory.value?.truncated).toBe(false);
  });
  it('replaces cached history and cursor on refresh after a gap or rewind', async () => {
    const base = detailFixture.messages[0]!;
    let messages = [10, 11].map((id) => ({ ...base, id }));
    let before: number | null = 10;
    stubFetch((url) => jsonResponse({ ...detailFixture, messages: url.includes('before=') ? [{ ...base, id: 9 }] : messages, messages_truncated: before !== null, messages_before: url.includes('before=') ? null : before }));
    await actions.openSession(detailFixture.session.id, { force: true });
    await actions.loadOlderSession();
    messages = [20, 21].map((id) => ({ ...base, id })); before = 20;
    await actions.openSession(detailFixture.session.id, { silent: true });
    expect(store.sessionHistory.value?.messages.map((m) => m.id)).toEqual([20, 21]);
    expect(store.sessionHistory.value?.before).toBe(20);
    messages = [{ ...base, id: 5 }]; before = null;
    await actions.openSession(detailFixture.session.id, { silent: true });
    expect(store.sessionHistory.value?.messages.map((m) => m.id)).toEqual([5]);
    expect(store.sessionHistory.value?.before).toBeNull();
  });

  it('keeps pages the reader loaded when a silent refresh returns a newer tail', async () => {
    const base = detailFixture.messages[0]!;
    const rows = (ids: number[]) => ids.map((id) => ({ ...base, id }));
    let reads = 0;
    stubFetch((url) => {
      if (url.includes('before=')) {
        return jsonResponse({ ...detailFixture, messages: rows([9]), messages_truncated: false, messages_before: null });
      }
      reads += 1;
      return jsonResponse(
        reads === 1
          ? { ...detailFixture, messages: rows([10, 11]), messages_truncated: true, messages_before: 10 }
          : { ...detailFixture, messages: rows([10, 11, 12]), messages_truncated: true, messages_before: 10 },
      );
    });
    await actions.openSession(detailFixture.session.id, { force: true });
    await actions.loadOlderSession();
    expect(store.sessionHistory.value?.messages.map((m) => m.id)).toEqual([9, 10, 11]);
    expect(store.sessionHistory.value?.before).toBeNull();

    // A reconnect or resync refetches the tail. The rows the reader paged back
    // to (id 9) must survive, and the union is complete, so the marker says so.
    await actions.openSession(detailFixture.session.id, { silent: true });
    expect(store.sessionHistory.value?.messages.map((m) => m.id)).toEqual([9, 10, 11, 12]);
    expect(store.sessionHistory.value?.before).toBeNull();
    expect(store.sessionHistory.value?.truncated).toBe(false);
    expect(store.chat.value.items.some((item) => item.id === 'notice:history-truncated')).toBe(false);
  });

  it('unblocks an older-page loader when a same-session refresh fails', async () => {
    const base = detailFixture.messages[0]!;
    const page = deferred<Response>();
    let fail = false;
    stubFetch((url) => url.includes('before=') ? page.promise : fail ? errorResponse('error', 'failed', 500) : jsonResponse({ ...detailFixture, messages: [base], messages_truncated: true, messages_before: base.id }));
    await actions.openSession(detailFixture.session.id, { force: true });
    const pending = actions.loadOlderSession(); fail = true;
    await actions.openSession(detailFixture.session.id, { silent: true });
    expect(store.sessionHistory.value?.loading).toBe(false);
    page.resolve(jsonResponse({ ...detailFixture, messages: [], messages_truncated: false, messages_before: null }));
    await pending;
    expect(store.sessionHistory.value?.before).toBe(base.id);
  });

  it('keeps live tool state when a page overlaps its persisted call', async () => {
    const base = detailFixture.messages[0]!;
    const call = { ...base, id: 10, role: 'assistant', content: '', tool_calls: [{ id: 'c1', name: 'example', arguments: '{}' }] };
    stubFetch((url) => jsonResponse({ ...detailFixture, messages: url.includes('before=') ? [{ ...base, id: 9 }] : [call], messages_truncated: !url.includes('before='), messages_before: url.includes('before=') ? null : 10 }));
    await actions.openSession(detailFixture.session.id, { force: true });
    store.chat.value = { ...store.chat.value, items: store.chat.value.items.map((item) => item.id === 'tool:c1' && item.kind === 'tool' ? { ...item, status: 'running', output: 'live output' } : item) };
    await actions.loadOlderSession();
    expect(store.chat.value.items.find((item) => item.id === 'tool:c1')).toMatchObject({ status: 'running', output: 'live output' });
  });

  it('ignores stale agent detail and page responses after reopening the same agent', async () => {
    const agent = (await import('../test/fixtures')).wireAgentSummary('a_race');
    const base = { ...detailFixture.messages[0]!, agent_id: agent.id };
    const old = deferred<Response>(); const page = deferred<Response>(); let reads = 0;
    stubFetch((url) => {
      if (url.includes('before=')) return page.promise;
      if (++reads === 1) return old.promise;
      return jsonResponse({ agent, messages: [base], messages_truncated: true, messages_before: base.id });
    });
    const first = actions.openAgentDetail(agent.id);
    await actions.openAgentDetail(agent.id);
    const pendingPage = actions.loadOlderAgent();
    await actions.openAgentDetail(agent.id);
    old.resolve(errorResponse('error', 'stale failure', 500));
    page.resolve(jsonResponse({ agent, messages: [{ ...base, id: base.id - 1 }], messages_truncated: false, messages_before: null }));
    await Promise.all([first, pendingPage]);
    expect(store.agentDetailError.value).toBeNull();
    expect(store.agentHistory.value?.messages.map((m) => m.id)).toEqual([base.id]);
    expect(store.agentHistory.value?.before).toBe(base.id);
  });

});
