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