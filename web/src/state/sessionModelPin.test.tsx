import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { EventStreamOptions } from '../api/events';
import type { Config, KarucodeEvent, RawEvent, Session } from '../api/types';
import { jsonResponse, resetAppState } from '../test/fixtures';
import { ModelSwitcher } from '../components/ModelSwitcher';

const realFetch = globalThis.fetch;

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function sessionWithModel(model: Session['model']): Session {
  return {
    id: 's_one', title: 'Pinned work', workspace: '/work', project_id: 'p1',
    created_at: 0, updated_at: 1, model,
  };
}

/**
 * The open session's model pin, kept outside every session list.
 *
 * The composer's pills used to read the pin from the session's row in a list,
 * which made them disappear whenever the row was not there - a fresh control
 * chat, or a project-less session after a list refresh. The pin now lives in
 * its own signal, and these cover the three places that know it: the session
 * read, the PATCH that sets it, and the `session_model_changed` event.
 */

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

/** One provider with two models: enough for the pills to name something. */
function testConfig(): Config {
  return {
    workspace: '/work',
    model: {
      provider: 'gateway',
      base_url: 'https://gw.example/v1',
      model: 'model-b',
      temperature: 0.2,
      send_temperature: false,
      max_tokens: null,
      top_p: null,
      request_timeout_secs: 300,
      parallel_tool_calls: true,
      supports_images: false,
      api_key_set: true,
    },
    providers: [
      {
        id: 'gateway',
        name: 'Gateway',
        provider: 'openai_compatible',
        base_url: 'https://gw.example/v1',
        models: [
          { id: 'model-a', name: 'Model A', context_window: 128000, reasoning: true },
          { id: 'model-b', name: 'Model B', context_window: 200000 },
        ],
        request_timeout_secs: 300,
        api_key_set: true,
        aggregator: false,
      },
      {
        id: 'local',
        name: 'Local',
        provider: 'openai_compatible',
        base_url: 'http://127.0.0.1:8000/v1',
        models: [{ id: 'tiny', name: 'Tiny', reasoning: false }],
        request_timeout_secs: 120,
        api_key_set: false,
        aggregator: false,
      },
    ],
    active: { provider: 'gateway', model: 'model-b' },
    limits: {} as Config['limits'],
    approvals: { mode: 'ask', allow_commands: [], auto_approve_paths: [], timeout_secs: 300 },
    subagents: {} as Config['subagents'],
    plugins: [],
    mcp_servers: [],
    settings_locked: false,
    version: '0.1.0',
  };
}

beforeEach(() => {
  resetAppState();
  store.boot.value = 'ready';
  actions.startStream();
});

afterEach(() => {
  actions.stopStream();
  resetAppState();
  globalThis.fetch = realFetch;
  streamOptions = null;
  vi.restoreAllMocks();
});

describe('the open session model pin', () => {
  it('is cleared when nothing is open', () => {
    store.rememberSessionModel('s_one', { provider: 'p', model: 'm' });
    expect(store.openSessionModel.value).toEqual({
      sessionId: 's_one',
      model: { provider: 'p', model: 'm' },
    });
    store.rememberSessionModel(null, null);
    expect(store.openSessionModel.value).toBeNull();
  });

  it('follows a session_model_changed event for the open session', () => {
    store.currentSessionId.value = 's_one';
    emit({
      type: 'session_model_changed',
      session_id: 's_one',
      model: { provider: 'local', model: 'tiny' },
    });
    expect(store.openSessionModel.value).toEqual({
      sessionId: 's_one',
      model: { provider: 'local', model: 'tiny' },
    });
    expect(store.modelSelection.value).toEqual({
      providerId: 'local',
      modelId: 'tiny',
      pinned: true,
      effort: null,
    });
  });

  it('ignores a session_model_changed event for another session', () => {
    store.currentSessionId.value = 's_one';
    emit({
      type: 'session_model_changed',
      session_id: 's_other',
      model: { provider: 'local', model: 'tiny' },
    });
    expect(store.openSessionModel.value).toBeNull();
  });

  it('claims no pin while the pin is unknown', () => {
    // A session that has been opened but not yet read has no known pin, so the
    // pill must not say "this session" on a guess.
    store.currentSessionId.value = 's_one';
    store.rememberSessionModel('s_one', null);
    expect(store.modelSelection.value.pinned).toBe(false);
  });

  it('does not flip while the pin is still being read', () => {
    // The reported flicker: opening a chat recorded "no pin" before the read
    // answered, so the pill showed the install default and then changed value.
    // `undefined` is "not read yet" and falls through to what is already known,
    // so nothing changes until the server answers.
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';
    store.sessions.value = [{
      id: 's_one',
      title: 'Pinned work',
      workspace: '/p',
      project_id: 'p1',
      model: { provider: 'local', model: 'tiny' },
      created_at: 0,
      updated_at: 0,
      message_count: 0,
      running: false,
    }];
    const before = store.modelSelection.value;
    store.rememberSessionModel('s_one', undefined);
    const reading = store.modelSelection.value;
    expect(reading).toEqual(before);
    expect(reading.pinned).toBe(true);
    expect(reading.modelId).toBe('tiny');
  });

  it('treats a known "no pin" as a fact, not as an unknown', () => {
    // The server said this session follows the default; a stale list row must
    // not re-assert a pin over that answer.
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';
    store.sessions.value = [{
      id: 's_one',
      title: 'Stale row',
      workspace: '/p',
      project_id: 'p1',
      model: { provider: 'local', model: 'tiny' },
      created_at: 0,
      updated_at: 0,
      message_count: 0,
      running: false,
    }];
    store.rememberSessionModel('s_one', null);
    expect(store.modelSelection.value.pinned).toBe(false);
  });

  it('renames the pill when a session_model_changed event lands', async () => {
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';
    render(<ModelSwitcher />);
    expect(screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i })).not.toBeNull();

    emit({
      type: 'session_model_changed',
      session_id: 's_one',
      model: { provider: 'local', model: 'tiny' },
    });

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Model for this session: Tiny on Local/i })).not.toBeNull(),
    );
  });

  it('locks the component caller while a newer action owns an older patch response', async () => {
    const oldModel = deferred<Response>();
    const newModel = deferred<Response>();
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { model?: string };
      return body.model === 'model-a' ? oldModel.promise : newModel.promise;
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';
    store.rememberSessionModel('s_one', { provider: 'gateway', model: 'model-b' });
    render(<ModelSwitcher />);
    const selections = vi.spyOn(actions, 'setSessionModelSelection');

    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i }));
    fireEvent.click(screen.getByRole('option', { name: /Model A/ }));
    await waitFor(() => expect(selections).toHaveBeenCalledTimes(1));
    const trigger = screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i });
    expect((trigger as HTMLButtonElement).disabled).toBe(true);
    // The component deliberately locks every action during a pending write. The
    // action layer still has to order concurrent programmatic/session callers.
    void actions.setSessionModelSelection('local', 'tiny');
    await waitFor(() => expect(selections).toHaveBeenCalledTimes(2));
    expect(store.sessionModelMutationPending.value).toBe('s_one');

    oldModel.resolve(jsonResponse(sessionWithModel({ provider: 'gateway', model: 'model-a' })));
    expect(await selections.mock.results[0]?.value).toBe(false);
    expect(store.sessionModelMutationPending.value).toBe('s_one');
    expect(store.modelSelection.value.modelId).toBe('model-b');

    newModel.resolve(jsonResponse(sessionWithModel({ provider: 'local', model: 'tiny' })));
    expect(await selections.mock.results[1]?.value).toBe(true);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Model for this session: Tiny on Local/i })).not.toBeNull(),
    );
    expect(store.sessionModelMutationPending.value).toBeNull();
  });

  it('discards an older choice that answers after the newer choice completed', async () => {
    const oldModel = deferred<Response>();
    const newModel = deferred<Response>();
    globalThis.fetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { model?: string };
      return body.model === 'model-a' ? oldModel.promise : newModel.promise;
    }) as unknown as typeof fetch;
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';

    const oldChoice = actions.setSessionModelSelection('gateway', 'model-a');
    const newChoice = actions.setSessionModelSelection('local', 'tiny');
    newModel.resolve(jsonResponse(sessionWithModel({ provider: 'local', model: 'tiny' })));
    expect(await newChoice).toBe(true);
    oldModel.resolve(jsonResponse(sessionWithModel({ provider: 'gateway', model: 'model-a' })));
    expect(await oldChoice).toBe(false);
    expect(store.modelSelection.value).toMatchObject({
      providerId: 'local', modelId: 'tiny', pinned: true,
    });
    expect(store.toasts.value.filter((toast) => toast.message.includes('Model A'))).toHaveLength(0);
  });

  it('keeps the newer default clear when an older pin answers late', async () => {
    const oldModel = deferred<Response>();
    const clearing = deferred<Response>();
    globalThis.fetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { model?: string };
      return body.model === 'model-a' ? oldModel.promise : clearing.promise;
    }) as unknown as typeof fetch;
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';
    store.rememberSessionModel('s_one', { provider: 'local', model: 'tiny' });

    const oldChoice = actions.setSessionModelSelection('gateway', 'model-a');
    const latestClear = actions.clearSessionModelSelection();
    clearing.resolve(jsonResponse(sessionWithModel(null)));
    expect(await latestClear).toBe(true);
    oldModel.resolve(jsonResponse(sessionWithModel({ provider: 'gateway', model: 'model-a' })));
    expect(await oldChoice).toBe(false);
    expect(store.modelSelection.value.pinned).toBe(false);
    expect(store.modelSelection.value.modelId).toBe('model-b');
  });

  it('keeps the newer pin when an older clear/default response arrives late', async () => {
    const oldClear = deferred<Response>();
    const newModel = deferred<Response>();
    globalThis.fetch = vi.fn(async (_input: RequestInfo| URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { model?: string };
      return body.model === 'model-a' ? newModel.promise : oldClear.promise;
    }) as unknown as typeof fetch;
    store.config.value = testConfig();
    store.currentSessionId.value = 's_one';

    const staleClear = actions.clearSessionModelSelection();
    const latestChoice = actions.setSessionModelSelection('gateway', 'model-a', 'high');
    newModel.resolve(jsonResponse(sessionWithModel({ provider: 'gateway', model: 'model-a', reasoning_effort: 'high' })));
    expect(await latestChoice).toBe(true);
    oldClear.resolve(jsonResponse(sessionWithModel(null)));
    expect(await staleClear).toBe(false);
    expect(store.modelSelection.value).toMatchObject({
      providerId: 'gateway', modelId: 'model-a', pinned: true, effort: 'high',
    });
  });
});
