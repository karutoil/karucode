import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { emptyChatState } from './chat';
import * as actions from './actions';
import * as store from './store';
import { detailFixture, jsonResponse, errorResponse, queuedFixture, resetAppState } from '../test/fixtures';

/**
 * What these cover, and why at this level.
 *
 * The reducer and the composer each have their own suite; these drive the *action*
 * layer with a stubbed `fetch`, so what is asserted is the actual request that
 * leaves the browser (the query string, the body shape) and what the store looks
 * like afterwards. That is where a recall could silently do the wrong thing -
 * append the rendered message body to the draft instead of the typed text, or
 * keep a row the server has already applied.
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

function runningSession(): void {
  store.currentSessionId.value = 's_ab12';
  store.sessions.value = [];
  store.chat.value = {
    ...emptyChatState('s_ab12'),
    sessionId: 's_ab12',
    running: true,
    runStatus: 'running',
    queued: [queuedFixture({ id: 'steer_1' })],
  };
}

beforeEach(() => {
  resetAppState();
});

afterEach(() => {
  globalThis.fetch = realFetch;
  vi.restoreAllMocks();
});

describe('queueChatMessage', () => {
  it('posts the mode, the rendered content and the chips a recall needs', async () => {
    runningSession();
    const fetchSpy = stubFetch(() =>
      jsonResponse(
        {
          mode: 'steer',
          session_id: 's_ab12',
          steer_id: 'steer_2',
          queued: true,
          queued_message: queuedFixture({ id: 'steer_2', draft: 'summarise' }),
        },
        202,
      ),
    );

    const accepted = await actions.queueChatMessage('steer', {
      content: 'summarise\n\nAttached 1 file:\n\n### notes.md (inlined, 7 B)',
      draft: 'summarise',
      attachments: [
        { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
      ],
    });

    expect(accepted).toBe(true);
    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/sessions/s_ab12/messages?mode=steer');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body as string);
    // The model gets the rendered body; the chip keeps the draft and its source.
    expect(body.content).toContain('### notes.md (inlined, 7 B)');
    expect(body.draft).toBe('summarise');
    expect(body.attachments).toEqual([
      { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
    ]);

    // The acceptance carries the entry, so the panel is right before the frame.
    expect(store.chat.value.queued.map((entry) => entry.id)).toEqual(['steer_1', 'steer_2']);
  });

  it('sends a follow-up with its own mode', async () => {
    runningSession();
    const fetchSpy = stubFetch(() =>
      jsonResponse(
        { mode: 'follow_up', session_id: 's_ab12', follow_up_id: 'followup_2', queued: true },
        202,
      ),
    );

    await actions.queueChatMessage('follow_up', { content: 'then run the tests', draft: 'then run the tests' });

    const [url] = fetchSpy.mock.calls[0] as [string];
    expect(url).toBe('/api/sessions/s_ab12/messages?mode=follow_up');
  });

  it('does not add the entry twice when the queue frame echoes the acceptance', async () => {
    runningSession();
    stubFetch(() =>
      jsonResponse(
        {
          mode: 'steer',
          session_id: 's_ab12',
          steer_id: 'steer_1',
          queued: true,
          queued_message: queuedFixture({ id: 'steer_1' }),
        },
        202,
      ),
    );

    await actions.queueChatMessage('steer', { content: 'also check the fixtures' });
    expect(store.chat.value.queued.map((entry) => entry.id)).toEqual(['steer_1']);
  });

  it('refuses to queue when nothing is running: a send is what that is', async () => {
    store.currentSessionId.value = 's_ab12';
    store.chat.value = { ...emptyChatState('s_ab12'), sessionId: 's_ab12', running: false };
    const fetchSpy = stubFetch(() => jsonResponse({}));

    expect(await actions.queueChatMessage('steer', { content: 'hello' })).toBe(false);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('explains a 409 instead of pretending the message is queued', async () => {
    runningSession();
    const fetchSpy = stubFetch((url) => {
      if (url.includes('/messages?mode=steer')) {
        return errorResponse('bad_request', 'session s_ab12 has no run in flight', 409);
      }
      return jsonResponse({ ...detailFixture, running: false });
    });

    const accepted = await actions.queueChatMessage('steer', { content: 'too late' });

    expect(accepted).toBe(false);
    expect(store.toasts.value.at(-1)?.message).toContain('finished before the message could be queued');
    // The queue on screen was wrong the moment the 409 arrived, so the session is
    // refetched rather than left showing a run that has already ended.
    await vi.waitFor(() =>
      expect(fetchSpy.mock.calls.some(([url]) => String(url) === '/api/sessions/s_ab12')).toBe(true),
    );
  });
});

describe('recallQueuedMessage', () => {
  it('takes the row out and hands the draft and chips to the composer', async () => {
    runningSession();
    const fetchSpy = stubFetch(() =>
      jsonResponse({
        session_id: 's_ab12',
        recalled: queuedFixture({
          id: 'steer_1',
          draft: 'summarise',
          attachments: [
            { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
          ],
        }),
      }),
    );

    expect(await actions.recallQueuedMessage('steer_1')).toBe(true);

    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/sessions/s_ab12/queues/steer_1');
    expect(init.method).toBe('DELETE');

    // The pending row is gone…
    expect(store.chat.value.queued).toEqual([]);
    // …and the composer gets the *draft*, not the rendered body it would have to
    // un-render, plus the chip so the next send is the same request.
    expect(store.composerHandoff.value).toMatchObject({
      text: 'summarise',
      attachments: [
        { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
      ],
    });
    expect(store.recallingQueued.value).toEqual({});
  });

  it('falls back to the content when the server sent no draft', async () => {
    runningSession();
    stubFetch(() =>
      jsonResponse({
        session_id: 's_ab12',
        recalled: { ...queuedFixture({ id: 'steer_1', draft: '' }), content: 'plain text' },
      }),
    );

    await actions.recallQueuedMessage('steer_1');
    await vi.waitFor(() => expect(store.composerHandoff.value?.text).toBe('plain text'));
  });

  it('says the message was already applied on a 404, and drops the stale row', async () => {
    runningSession();
    const fetchSpy = stubFetch((url) => {
      if (url.includes('/queues/')) {
        return errorResponse('not_found', 'no queued message `steer_1`', 404);
      }
      return jsonResponse({ ...detailFixture, running: true, queued_steers: [] });
    });

    expect(await actions.recallQueuedMessage('steer_1')).toBe(false);
    expect(store.toasts.value.at(-1)?.message).toBe('That message was already applied to the run.');
    // Nothing is handed to the composer: reviving a message the model already saw
    // would be a lie, and the draft stays exactly as the user left it.
    expect(store.composerHandoff.value).toBeNull();
    await vi.waitFor(() =>
      expect(fetchSpy.mock.calls.some(([url]) => String(url) === '/api/sessions/s_ab12')).toBe(true),
    );
  });

  it('fires once even when the button is clicked twice', async () => {
    runningSession();
    const fetchSpy = stubFetch(() =>
      jsonResponse({ session_id: 's_ab12', recalled: queuedFixture({ id: 'steer_1' }) }),
    );

    const first = actions.recallQueuedMessage('steer_1');
    const second = actions.recallQueuedMessage('steer_1');
    expect(await second).toBe(false);
    expect(await first).toBe(true);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });
});

describe('observing the queue', () => {
  it('seeds a reload from the snapshot so nothing queued is lost', async () => {
    const detail = {
      ...detailFixture,
      running: true,
      queued_steers: [queuedFixture({ id: 'steer_9' })],
      queued_follow_ups: [queuedFixture({ id: 'followup_9', kind: 'follow_up' })],
    };
    stubFetch((url) => {
      if (url.startsWith('/api/sessions/s_ab12')) return jsonResponse(detail);
      return jsonResponse({});
    });

    await actions.openSession('s_ab12');

    expect(store.chat.value.queued.map((entry) => entry.id)).toEqual(['steer_9', 'followup_9']);
  });
});
