import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { ModelControls, ModelSwitcher, ReasoningSelector } from './ModelSwitcher';
import type { Config, SessionModelSelection, SessionSummary } from '../api/types';
import { jsonResponse, resetAppState, summaryFixture } from '../test/fixtures';
import * as store from '../state/store';

function twoProfileConfig(): Config {
  return {
    workspace: '/home/me/project',
    model: {
      provider: 'openai_compatible',
      base_url: 'https://gw.example/v1',
      model: 'model-b',
      temperature: 0.2,
      max_tokens: null,
      top_p: null,
      request_timeout_secs: 300,
      parallel_tool_calls: true,
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
          { id: 'model-b', name: 'Model B', context_window: 200000, tool_call: false },
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
    limits: {
      max_iterations: 32,
      max_run_secs: 1800,
      auto_continue: true,
      max_continues: 50,
      max_tool_output_bytes: 24000,
      max_command_output_bytes: 32000,
      command_timeout_secs: 120,
      max_command_timeout_secs: 600,
      max_file_read_bytes: 200000,
      search_max_results: 200,
      context_max_chars: 240000,
      max_tool_calls_per_turn: 8,
    },
    approvals: { mode: 'ask', allow_commands: [], auto_approve_paths: [], timeout_secs: 300 },
    subagents: {
      max_concurrent: 3,
      max_iterations: 16,
      max_continues: 0,
      allow_recursive: false,
      max_depth: 1,
      timeout_secs: 900,
    },
    plugins: [],
    mcp_servers: [],
    settings_locked: false,
    version: '0.1.0',
  };
}

/** Seed one open session, optionally with a model pin. */
function openSession(selection: SessionModelSelection | null = null): SessionSummary {
  const summary: SessionSummary = { ...summaryFixture, model: selection };
  store.sessions.value = [summary];
  store.currentSessionId.value = summary.id;
  return summary;
}

/** Mock fetch that answers PATCH /api/sessions/:id/model and records the bodies. */
function installModelFetch() {
  const calls: { url: string; body: Record<string, unknown> }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url === `/api/sessions/${summaryFixture.id}/model` && init?.method === 'PATCH') {
      const body = JSON.parse(String(init.body)) as Record<string, unknown>;
      calls.push({ url, body });
      const selection =
        typeof body.provider === 'string' && typeof body.model === 'string'
          ? {
              provider: body.provider,
              model: body.model,
              ...(typeof body.reasoning_effort === 'string'
                ? { reasoning_effort: body.reasoning_effort }
                : {}),
            }
          : null;
      return jsonResponse({
        id: summaryFixture.id,
        title: summaryFixture.title,
        workspace: summaryFixture.workspace,
        project_id: null,
        model: selection,
        created_at: 1,
        updated_at: 2,
      });
    }
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${url}` } }, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, calls };
}

describe('ModelSwitcher (per-session)', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('shows the install-wide model when the session carries no pin', () => {
    openSession();
    render(<ModelSwitcher />);
    const button = screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i });
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText(/this session/)).toBeNull();

    fireEvent.click(button);
    const options = screen.getAllByRole('option');
    expect(options.map((option) => option.textContent)).toEqual([
      expect.stringContaining('Model A'),
      expect.stringContaining('Model B'),
      expect.stringContaining('Tiny'),
    ]);
    expect(options[1]?.getAttribute('aria-selected')).toBe('true');
    // A profile without a key is called out instead of pretending it works.
    expect(screen.getByText('no key')).not.toBeNull();
    // Nothing to reset: the session already follows the default.
    expect(screen.queryByRole('button', { name: 'Follow install default' })).toBeNull();
  });

  it('pins the session to the picked model via PATCH /api/sessions/:id/model', async () => {
    const { calls } = installModelFetch();
    openSession();
    render(<ModelSwitcher />);

    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Model B/i }));
    fireEvent.click(screen.getByRole('option', { name: /Tiny/ }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'local', model: 'tiny' });
    await waitFor(() =>
      expect(store.sessions.value[0]?.model).toEqual({ provider: 'local', model: 'tiny' }),
    );
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(store.toasts.value.some((toast) => toast.message.includes('Tiny'))).toBe(true);
  });

  it('keeps the pinned reasoning effort when switching models', async () => {
    const { calls } = installModelFetch();
    openSession({ provider: 'gateway', model: 'model-a', reasoning_effort: 'high' });
    render(<ModelSwitcher />);

    // The pin is visible as a per-session marker.
    expect(screen.getByText(/this session/)).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Model A/i }));
    fireEvent.click(screen.getByRole('option', { name: /Model B/ }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'gateway', model: 'model-b', reasoning_effort: 'high' });
  });

  it('follows the install default again when the pin is cleared', async () => {
    const { calls } = installModelFetch();
    openSession({ provider: 'local', model: 'tiny' });
    render(<ModelSwitcher />);

    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Tiny/i }));
    fireEvent.click(screen.getByRole('button', { name: 'Follow install default' }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({});
    await waitFor(() => expect(store.sessions.value[0]?.model).toBeNull());
    expect(store.toasts.value.some((toast) => toast.message.includes('install default'))).toBe(true);
  });

  it('closes on Escape without changing the selection', async () => {
    const { fetchMock } = installModelFetch();
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Model B/i }));
    expect(screen.queryByRole('listbox')).not.toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe('ReasoningSelector', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('sends the picked effort, pinning the install-wide selection first', async () => {
    const { calls } = installModelFetch();
    openSession();
    render(<ReasoningSelector />);

    fireEvent.click(screen.getByRole('button', { name: /Reasoning effort: Default/i }));
    fireEvent.click(screen.getByRole('option', { name: /High/ }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'gateway', model: 'model-b', reasoning_effort: 'high' });
  });

  it('spells "off" as reasoning_effort: none', async () => {
    const { calls } = installModelFetch();
    openSession({ provider: 'gateway', model: 'model-b', reasoning_effort: 'high' });
    render(<ReasoningSelector />);

    expect(screen.getByRole('button', { name: /Reasoning effort: High/i })).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Reasoning effort: High/i }));
    fireEvent.click(screen.getByRole('option', { name: /^Off/ }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'gateway', model: 'model-b', reasoning_effort: 'none' });
  });

  it('resets to the endpoint default without dropping the model pin', async () => {
    const { calls } = installModelFetch();
    openSession({ provider: 'gateway', model: 'model-a', reasoning_effort: 'low' });
    render(<ReasoningSelector />);

    fireEvent.click(screen.getByRole('button', { name: /Reasoning effort: High|Reasoning effort: Low|Reasoning effort: Minimal|Reasoning effort: Default/i }));
    fireEvent.click(screen.getByRole('option', { name: /Default/ }));

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'gateway', model: 'model-a' });
  });

  it('stays hidden when the effective model cannot reason', () => {
    openSession({ provider: 'local', model: 'tiny' });
    const { container } = render(<ReasoningSelector />);
    expect(container.firstChild).toBeNull();
  });
});

describe('ModelControls', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('renders both selectors only when a session is open', () => {
    const { container, unmount } = render(<ModelControls />);
    // No session yet: nothing at all.
    expect(container.firstChild).toBeNull();
    unmount();

    openSession();
    render(<ModelControls />);
    expect(screen.getByRole('button', { name: /Model for this session/ })).not.toBeNull();
    expect(screen.getByRole('button', { name: /Reasoning effort:/ })).not.toBeNull();
  });
});
