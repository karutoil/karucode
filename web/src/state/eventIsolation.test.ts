import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EventStreamOptions } from '../api/events';
import type { KarucodeEvent, RawEvent } from '../api/types';
import { resetAppState, summaryFixture } from '../test/fixtures';

let streamOptions: EventStreamOptions | null = null;
vi.mock('../api/events', () => ({
  startEventStream: vi.fn((options: EventStreamOptions) => {
    streamOptions = options;
    return { stop: vi.fn(), getState: () => 'idle' };
  }),
}));

import * as actions from './actions';
import * as store from './store';

function emit(event: Partial<KarucodeEvent> & { type: string; session_id: string }): void {
  if (!streamOptions) throw new Error('stream not installed');
  streamOptions.onEvent({ seq: 1, ts_ms: 1, ...event } as unknown as RawEvent);
}

beforeEach(() => {
  resetAppState();
  store.boot.value = 'ready';
  store.currentProjectId.value = 'p_one';
  actions.startStream();
});

afterEach(() => {
  actions.stopStream();
  resetAppState();
  streamOptions = null;
  vi.restoreAllMocks();
});

describe('global session creation events', () => {
  it('neither inserts nor auto-opens an unknown session from the global stream', () => {
    // SessionCreated has no project_id. Until the scoped list refetch proves this
    // is one of ours, inserting it assigns an invented null project and opening it
    // can navigate to a different project's chat.
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    store.sessions.value = [{ ...summaryFixture, project_id: 'p_one' }];
    store.currentSessionId.value = null;

    emit({ type: 'session_created', session_id: 's_foreign', title: 'Foreign project work' });

    expect(store.sessions.value.map((session) => session.id)).not.toContain('s_foreign');
    expect(store.currentSessionId.value).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it('still folds a known local creation echo into its existing row', () => {
    store.sessions.value = [{ ...summaryFixture, id: 's_local', title: 'Before echo', project_id: 'p_one' }];
    store.currentSessionId.value = 's_local';

    emit({ type: 'session_created', session_id: 's_local', title: 'Created locally' });

    expect(store.sessions.value).toHaveLength(1);
    expect(store.sessions.value[0]?.title).toBe('Created locally');
    expect(store.currentSessionId.value).toBe('s_local');
  });
});
