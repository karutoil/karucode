import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RawEvent } from '../api/types';
import {
  configFixture,
  detailFixture,
  jsonResponse,
  resetAppState,
  summaryFixture,
} from '../test/fixtures';
import * as actions from './actions';
import { resetNotificationSingletons } from './notifyWiring';
import * as store from './store';

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('bootstrap and the live stream interlock', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    resetNotificationSingletons();
    vi.unstubAllGlobals();
  });

  it('subscribes first and releases a rapid session start and notification after its snapshots', async () => {
    const sessionsRead = deferred<Response>();
    let push: ((event: RawEvent) => void) | null = null;
    let sessionsReads = 0;
    const order: string[] = [];
    const rapidSummary = {
      ...summaryFixture,
      id: 's_rapid', title: '', project_id: 'p_one', running: false,
      message_count: 0, last_run_status: null,
    };
    const run = detailFixture.runs[0]!;
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/api/events') {
        order.push('events');
        return new Response(new ReadableStream<Uint8Array>({
          start(controller) {
            push = (event) => controller.enqueue(new TextEncoder().encode(
              `id: ${event.seq}\ndata: ${JSON.stringify(event)}\n\n`,
            ));
          },
        }), {
          status: 200,
          headers: {
            'content-type': 'text/event-stream',
            'X-Karucode-Event-Epoch': 'bootstrap-process',
          },
        });
      }
      if (url === '/api/health') {
        order.push('health');
        return jsonResponse({
          status: 'ok', version: 'test', uptime_secs: 1, workspace: null,
          projects: [], auth_required: false, provider_configured: true, db: 'ok',
        });
      }
      if (url === '/api/config') {
        order.push('config');
        return jsonResponse(configFixture);
      }
      if (url === '/api/projects') {
        order.push('projects');
        return jsonResponse({ workspace: null, projects: [{
          id: 'p_one', name: 'One', path: '/one', created_at: 0, updated_at: 0,
          session_count: 1,
        }] });
      }
      if (url === '/api/sessions?project=p_one') {
        order.push('sessions');
        sessionsReads += 1;
        if (sessionsReads === 1) return sessionsRead.promise;
        return jsonResponse({ sessions: [rapidSummary] });
      }
      if (url === '/api/agents?project=p_one') return jsonResponse({ agents: [] });
      if (url === '/api/sessions/s_rapid') {
        return jsonResponse({
          ...detailFixture,
          session: rapidSummary,
          messages: [],
          runs: [{ ...run, id: 'r_rapid', status: 'running', finished_at: null }],
          agents: [], approvals: [], claims: [], running: true,
        });
      }
      throw new Error(`unexpected ${url}`);
    }));

    const booting = actions.bootstrap();
    await vi.waitFor(() => expect(sessionsReads).toBe(1));
    expect(order[0]).toBe('events');
    await vi.waitFor(() => expect(push).not.toBeNull());
    const emit = (event: RawEvent): void => {
      if (!push) throw new Error('event stream did not open');
      push(event);
    };

    // This is the previously open hole: the server has started a session and
    // emitted its notification while the authoritative list is still in flight.
    emit({ seq: 1, ts_ms: 1, session_id: 's_rapid', type: 'session_created', title: 'Rapid start' });
    emit({ seq: 2, ts_ms: 2, session_id: 's_rapid', run_id: 'r_rapid', type: 'run_started', prompt: 'Begin now' });
    emit({ seq: 3, ts_ms: 3, session_id: 's_rapid', run_id: 'r_rapid', type: 'text_delta', delta: 'live answer' });
    emit({
      seq: 4, ts_ms: 4, session_id: 's_rapid', run_id: 'r_rapid', type: 'notification',
      notification: {
        id: 'run:r_rapid', kind: 'run_finished', title: 'Run finished', body: 'Done',
        session_id: 's_rapid', session_title: 'Rapid start', project_id: 'p_one',
        source: 'runtime', sticky: false,
      },
    });
    // Delivery is buffered, not raced into empty pre-bootstrap state.
    expect(store.notifications.value).toEqual([]);
    expect(store.currentSessionId.value).toBeNull();

    sessionsRead.resolve(jsonResponse({ sessions: [rapidSummary] }));
    await booting;

    expect(store.boot.value).toBe('ready');
    expect(store.currentSessionId.value).toBe('s_rapid');
    expect(store.sessions.value[0]?.title).toBe('Rapid start');
    expect(store.chat.value.items.some(
      (item) => item.kind === 'message' && item.content === 'live answer',
    )).toBe(true);
    expect(store.notifications.value.map((entry) => entry.id)).toEqual(['run:r_rapid']);
    // The selected session is already on screen when the buffered notification
    // is reconciled, so this is read history rather than a false unread badge.
    expect(store.notifications.value[0]?.read).toBe(true);
    expect(order.filter((name) => name === 'events')).toHaveLength(1);
  });
});
