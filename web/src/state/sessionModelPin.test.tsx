import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/preact';
import type { EventStreamOptions } from '../api/events';
import type { Config, KarucodeEvent, RawEvent } from '../api/types';
import { resetAppState } from '../test/fixtures';
import { ModelSwitcher } from '../components/ModelSwitcher';

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
      },
      {
        id: 'local',
        name: 'Local',
        provider: 'openai_compatible',
        base_url: 'http://127.0.0.1:8000/v1',
        models: [{ id: 'tiny', name: 'Tiny', reasoning: false }],
        request_timeout_secs: 120,
        api_key_set: false,
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
});
