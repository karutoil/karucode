/**
 * "New session" is a conversation the client holds until there is something to
 * put in it.
 *
 * Hitting the button used to `POST /api/sessions` immediately, so every press -
 * and every change of mind - left an empty row behind. These tests pin the new
 * contract: the press writes nothing, the first message creates the session, and
 * everything the reader chose before that (the model pin) travels with it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDetail } from '../api/types';
import { detailFixture, errorResponse, jsonResponse, resetAppState, summaryFixture } from '../test/fixtures';
import * as actions from './actions';
import * as store from './store';

const realFetch = globalThis.fetch;

interface FetchMock {
  mock: { calls: [RequestInfo | URL, RequestInit | undefined][] };
}

function stubFetch(
  handler: (url: string, init?: RequestInit) => Response | Promise<Response>,
): FetchMock {
  const impl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    return handler(url, init);
  });
  globalThis.fetch = impl as unknown as typeof fetch;
  return impl as unknown as FetchMock;
}

/** The `POST /api/sessions` answers the server gives for a fresh, untitled row. */
function createdSession(id: string): Response {
  return jsonResponse({
    ...detailFixture.session,
    id,
    title: '',
    project_id: 'p_one',
    workspace: '/home/me/project',
  });
}

function detail(id: string): SessionDetail {
  return { ...detailFixture, session: { ...detailFixture.session, id, project_id: 'p_one' } };
}

function requests(fetchMock: FetchMock, match: (url: string) => boolean): number {
  return fetchMock.mock.calls.filter(([input]) => match(String(input))).length;
}

/** The session list read a send schedules; answered so it cannot close the chat. */
function listRoute(id: string): Response {
  return jsonResponse({ sessions: [{ ...summaryFixture, id, project_id: 'p_one' }] });
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

describe('starting a conversation without creating it', () => {
  it('writes nothing when New session is pressed', () => {
    const fetchMock = stubFetch(() => {
      throw new Error('New session must not talk to the server');
    });

    actions.startDraftSession();

    expect(store.currentSessionId.value).toBeNull();
    expect(store.draftSession.value).toMatchObject({ projectId: 'p_one', model: null });
    expect(fetchMock.mock.calls).toHaveLength(0);
  });

  it('keeps one draft when the button is pressed again', () => {
    actions.startDraftSession();
    const first = store.draftSession.value;
    actions.startDraftSession();

    // A second press is "focus the box", not a second conversation: pressing it
    // twice must not discard anything already typed.
    expect(store.draftSession.value).toBe(first);
  });

  it('lets the composer be typed in before the session exists', () => {
    store.health.value = {
      status: 'ok',
      version: '0.1.0',
      uptime_secs: 1,
      workspace: '/home/me/project',
      projects: [],
      auth_required: false,
      provider_configured: true,
      db: 'ok',
    };
    expect(store.composerDisabledReason.value).toBe('Create or select a session to start.');

    actions.startDraftSession();

    expect(store.composerDisabledReason.value).toBeNull();
  });

  it('abandons the draft when another session is opened', async () => {
    stubFetch((url) => {
      if (url === '/api/sessions/s_other') return jsonResponse(detail('s_other'));
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    await actions.openSession('s_other');

    expect(store.draftSession.value).toBeNull();
    expect(store.currentSessionId.value).toBe('s_other');
  });

  it('drops the draft when the project changes', async () => {
    stubFetch((url) => {
      if (url === '/api/sessions?project=p_two') return jsonResponse({ sessions: [] });
      if (url.startsWith('/api/agents')) return jsonResponse({ agents: [] });
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    await actions.switchProject('p_two');

    expect(store.draftSession.value).toBeNull();
    expect(store.currentProjectId.value).toBe('p_two');
  });
});

describe('the first message creates the session', () => {
  it('creates it, then sends the message into it', async () => {
    const bodies: Record<string, unknown>[] = [];
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') {
        bodies.push(JSON.parse(String(init.body)) as Record<string, unknown>);
        return createdSession('s_new');
      }
      if (url === '/api/sessions/s_new/messages') {
        expect(JSON.parse(String(init?.body)).content).toBe('hello there');
        return jsonResponse({ run_id: 'r_new', session_id: 's_new' });
      }
      if (url === '/api/sessions?project=p_one') return listRoute('s_new');
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    expect(await actions.sendChatMessage('hello there')).toBe(true);

    // Exactly one session written, and it is the one the message went into.
    expect(requests(fetchMock, (url) => url === '/api/sessions')).toBe(1);
    expect(bodies[0]).toEqual({ project: 'p_one' });
    expect(store.currentSessionId.value).toBe('s_new');
    expect(store.draftSession.value).toBeNull();
    expect(store.readLastSessionId()).toBe('s_new');
    // The message is on screen as the reader's own turn, carrying its run.
    const messages = store.chat.value.items.filter((item) => item.kind === 'message');
    expect(messages[messages.length - 1]).toMatchObject({
      role: 'user',
      content: 'hello there',
      runId: 'r_new',
    });
    // The new row is in the rail immediately; the first message is what titles it.
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s_new']);
  });

  it('creates the session already pinned to the model chosen for the draft', async () => {
    const bodies: Record<string, unknown>[] = [];
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') {
        bodies.push(JSON.parse(String(init.body)) as Record<string, unknown>);
        return jsonResponse({
          ...detailFixture.session,
          id: 's_new',
          project_id: 'p_one',
          model: { provider: 'gateway', model: 'big', reasoning_effort: 'high' },
        });
      }
      if (url === '/api/sessions/s_new/messages') {
        return jsonResponse({ run_id: 'r_new', session_id: 's_new' });
      }
      if (url === '/api/sessions?project=p_one') return listRoute('s_new');
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    // The pills work before the session exists: the choice is held in the draft.
    expect(await actions.setSessionModelSelection('gateway', 'big', 'high')).toBe(true);
    expect(store.modelSelection.value).toEqual({
      providerId: 'gateway',
      modelId: 'big',
      pinned: true,
      effort: 'high',
    });

    expect(await actions.sendChatMessage('hello there')).toBe(true);

    // One request, not a create followed by a PATCH: the pin rides along.
    expect(bodies[0]).toEqual({
      project: 'p_one',
      model: { provider: 'gateway', model: 'big', reasoning_effort: 'high' },
    });
    expect(requests(fetchMock, (url) => url.includes('/model'))).toBe(0);
    // …and the pills now read the session's own pin, which is the same one.
    expect(store.modelSelection.value).toMatchObject({ modelId: 'big', pinned: true });
  });

  it('follows the install default again when the draft pin is cleared', async () => {
    actions.startDraftSession();
    await actions.setSessionModelSelection('gateway', 'big', null);
    expect(store.modelSelection.value.pinned).toBe(true);

    const fetchMock = stubFetch(() => {
      throw new Error('clearing a draft pin must not talk to the server');
    });
    expect(await actions.clearSessionModelSelection()).toBe(true);

    expect(store.draftSession.value?.model).toBeNull();
    expect(store.modelSelection.value.pinned).toBe(false);
    expect(fetchMock.mock.calls).toHaveLength(0);
  });

  it('keeps the draft and sends nothing when the session cannot be created', async () => {
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') {
        return errorResponse('offline', 'the server is down', 500);
      }
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    expect(await actions.sendChatMessage('hello there')).toBe(false);

    // The text is still in the composer: sending again is the whole retry.
    expect(store.draftSession.value).not.toBeNull();
    expect(store.currentSessionId.value).toBeNull();
    expect(store.chat.value.items).toHaveLength(0);
    expect(requests(fetchMock, (url) => url.includes('/messages'))).toBe(0);
    expect(store.toasts.value.some((toast) => toast.message === 'the server is down')).toBe(true);
  });

  it('creates one session when a send and an attachment ask at the same moment', async () => {
    let answerCreate!: (response: Response) => void;
    const creating = new Promise<Response>((resolve) => {
      answerCreate = resolve;
    });
    const fetchMock = stubFetch((url, init) => {
      if (url === '/api/sessions' && init?.method === 'POST') return creating;
      if (url === '/api/sessions/s_new/messages') {
        return jsonResponse({ run_id: 'r_new', session_id: 's_new' });
      }
      if (url === '/api/sessions?project=p_one') return listRoute('s_new');
      throw new Error(`unexpected ${url}`);
    });
    actions.startDraftSession();

    // A binary attachment lives under the session's directory, so it asks for the
    // session too. Both callers must share one create, not race two.
    const sending = actions.sendChatMessage('hello there');
    const attaching = actions.materializeDraftSession();
    answerCreate(createdSession('s_new'));

    expect(await attaching).toBe('s_new');
    expect(await sending).toBe(true);
    expect(requests(fetchMock, (url) => url === '/api/sessions')).toBe(1);
  });

  it('does not create anything for a lone attachment when no draft is open', async () => {
    stubFetch(() => {
      throw new Error('nothing should be created');
    });

    expect(await actions.materializeDraftSession()).toBeNull();
  });
});
