import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDetail } from '../api/types';
import { NOW, detailFixture, jsonResponse, resetAppState, storedMessage, summaryFixture } from '../test/fixtures';
import { emptyChatState } from './chat';
import * as actions from './actions';
import * as store from './store';

/**
 * Recovery paths that only exist because the SSE stream never replays: a hole in
 * the sequence, or an explicit `stream_lagged` marker, has to repair the open
 * session without destroying what is already on screen.
 */

let push: (payload: Record<string, unknown>) => void;
let getSessionCalls = 0;
let detailToReturn: SessionDetail;

function sseStream(): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      push = (payload) => {
        controller.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(payload)}\n\n`));
      };
    },
  });
  return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

function installRouter(): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
      if (url.startsWith('/api/events')) return sseStream();
      if (url === '/api/sessions' || url.startsWith('/api/sessions?')) {
        return jsonResponse({ sessions: [summaryFixture] });
      }
      if (url === '/api/sessions/s_ab12') {
        getSessionCalls += 1;
        return jsonResponse(detailToReturn);
      }
      return jsonResponse({ error: { code: 'not_found', message: url } }, 404);
    }),
  );
}

function transcriptText(): string {
  return store.chat.value.items
    .map((item) => (item.kind === 'message' ? item.content : ''))
    .join('|');
}

function runningDetail(): SessionDetail {
  const run = detailFixture.runs[0];
  return {
    ...detailFixture,
    // Mid-answer: the assistant turn is not persisted yet, so the snapshot only
    // knows the earlier preamble.
    runs: run ? [{ ...run, status: 'running', finished_at: null }] : [],
    running: true,
  };
}

/**
 * The same mid-answer snapshot, but the server has already persisted a tool call
 * whose `tool_call_started` frame was one of the dropped ones.
 */
function runningDetailWithGapTool(): SessionDetail {
  const base = runningDetail();
  return {
    ...base,
    messages: [
      ...base.messages,
      storedMessage({
        id: 5,
        role: 'assistant',
        content: '',
        run_id: 'r_x',
        created_at: NOW - 80_000,
        tool_calls: [{ id: 'call_gap', name: 'read_file', arguments: '{"path":"src/gap.rs"}' }],
      }),
      storedMessage({
        id: 6,
        role: 'tool',
        content: 'contents of the file read during the hole',
        tool_call_id: 'call_gap',
        name: 'read_file',
        created_at: NOW - 79_000,
      }),
    ],
  };
}

beforeEach(() => {
  resetAppState();
  getSessionCalls = 0;
  detailToReturn = detailFixture;
  installRouter();
  store.boot.value = 'ready';
  store.sessions.value = [summaryFixture];
  store.currentSessionId.value = 's_ab12';
  store.chat.value = emptyChatState('s_ab12');
  actions.startStream();
});

afterEach(() => {
  resetAppState();
  vi.unstubAllGlobals();
});

async function openRun(answer = 'the streamed answer'): Promise<void> {
  await vi.waitFor(() => expect(typeof push).toBe('function'), { timeout: 3000 });
  push({ seq: 1, ts_ms: NOW, session_id: 's_ab12', run_id: 'r_x', type: 'run_started', prompt: 'Go' });
  push({ seq: 2, ts_ms: NOW, session_id: 's_ab12', run_id: 'r_x', type: 'text_delta', delta: answer });
  await vi.waitFor(() => expect(transcriptText()).toContain(answer), { timeout: 3000 });
}

describe('stream recovery', () => {
  it('refetches on a lag marker without deleting the answer that is still streaming', async () => {
    detailToReturn = runningDetail();
    await openRun();
    const callsBefore = getSessionCalls;

    push({ seq: 6, ts_ms: NOW, session_id: null, type: 'stream_lagged', skipped: 3 });

    await vi.waitFor(() => expect(getSessionCalls).toBeGreaterThan(callsBefore), { timeout: 3000 });
    // The snapshot has no streaming message, so a reseed here would delete it.
    expect(transcriptText()).toContain('the streamed answer');
    expect(store.chat.value.running).toBe(true);
    // …and the marker is visible rather than telling the user to reload.
    const notice = store.chat.value.items.find((item) => item.kind === 'notice');
    expect(notice?.kind === 'notice' ? notice.message : '').toContain('3 update(s) were dropped');
  });

  it('clears a stuck run when the lag swallowed the terminal event', async () => {
    detailToReturn = runningDetail();
    await openRun();
    // The run finished on the server, but `run_finished` was one of the lost frames.
    detailToReturn = detailFixture;

    push({ seq: 9, ts_ms: NOW, session_id: null, type: 'stream_lagged', skipped: 2 });

    await vi.waitFor(() => expect(store.chat.value.running).toBe(false), { timeout: 3000 });
    // The persisted answer replaces the transcript, because no run is in flight.
    expect(transcriptText()).toContain('The login handler is unfinished.');
  });

  it('refetches on a bare seq hole even without a marker', async () => {
    detailToReturn = detailFixture;
    await vi.waitFor(() => expect(typeof push).toBe('function'), { timeout: 3000 });
    push({ seq: 1, ts_ms: NOW, session_id: 's_ab12', run_id: 'r_x', type: 'run_started', prompt: 'Go' });
    push({ seq: 2, ts_ms: NOW, session_id: 's_ab12', run_id: 'r_x', type: 'text_delta', delta: 'x' });
    const callsBefore = getSessionCalls;

    push({ seq: 8, ts_ms: NOW, session_id: 's_ab12', run_id: 'r_x', type: 'text_delta', delta: 'y' });

    await vi.waitFor(() => expect(getSessionCalls).toBeGreaterThan(callsBefore), { timeout: 3000 });
  });

  it('surfaces a tool card that arrived during the hole without losing the live answer', async () => {
    // The hole swallowed `tool_call_started`/`tool_call_finished` for `call_gap`.
    // The server has the call persisted; the client never saw the frames.
    detailToReturn = runningDetailWithGapTool();
    await openRun();
    const callsBefore = getSessionCalls;

    push({ seq: 6, ts_ms: NOW, session_id: null, type: 'stream_lagged', skipped: 2 });

    await vi.waitFor(() => expect(getSessionCalls).toBeGreaterThan(callsBefore), { timeout: 3000 });

    // The card the lost frames would have added is back…
    const gapTool = store.chat.value.items.find(
      (item) => item.kind === 'tool' && item.callId === 'call_gap',
    );
    expect(gapTool).toBeDefined();
    expect(gapTool?.kind === 'tool' ? gapTool.output : '').toContain('read during the hole');
    // …and the answer that is still streaming was not deleted to get it.
    expect(transcriptText()).toContain('the streamed answer');
    expect(store.chat.value.running).toBe(true);
  });
});
