import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { ModelControls, ModelSwitcher, REASONING_OPTIONS, ReasoningSelector, filterModelOptions, modelOptions } from './ModelSwitcher';
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
          { id: 'model-b', name: 'Model B', context_window: 200000, tool_call: false },
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
    limits: {
      max_iterations: 32,
      max_run_secs: 1800,
          max_tool_output_bytes: 24000,
      max_command_output_bytes: 32000,
      command_timeout_secs: 120,
      max_command_timeout_secs: 600,
      max_file_read_bytes: 200000,
      search_max_results: 200,
      context_max_chars: 240000,
      max_tool_calls_per_turn: 8,
      max_turn_retries: 2,
      turn_retry_delay_ms: 30000,
      parallel_tool_calls: true,
    },
    approvals: { mode: 'ask', allow_commands: [], auto_approve_paths: [], timeout_secs: 300 },
    subagents: {
      max_concurrent: 3,
      max_iterations: 16,
        allow_recursive: false,
      max_depth: 1,
      timeout_secs: 900,
      allow_background: true,
      max_background: 8,
      wait_timeout_secs: 600,
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

  it('opens on the filter and walks the list from there, restoring the trigger after selection', async () => {
    const { calls } = installModelFetch();
    openSession();
    render(<ModelSwitcher />);
    const trigger = screen.getByRole('button', { name: /Model for this session: Model B/i });
    fireEvent.click(trigger);

    const options = screen.getAllByRole('option');
    expect(options.map((option) => option.getAttribute('tabindex'))).toEqual(['-1', '0', '-1']);
    // A desktop picker opens with the caret in the filter: typing a name is the
    // fastest way through a long list, and the field is the one place a keystroke
    // narrows it. The roving tab stop still starts on the current model.
    const filter = screen.getByRole('searchbox', { name: 'Filter models' });
    await waitFor(() => expect(document.activeElement).toBe(filter));

    // ArrowDown crosses from the field into the list, one row past the current
    // model, and from there the walk stays inside the list.
    fireEvent.keyDown(filter, { key: 'ArrowDown' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /Tiny/ })));
    expect(screen.getAllByRole('option').map((option) => option.getAttribute('tabindex'))).toEqual(['-1', '-1', '0']);
    fireEvent.keyDown(document.activeElement!, { key: 'Home' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /Model A/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'End' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /Tiny/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /Model B/ })));

    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    const tiny = await waitFor(() => screen.getByRole('option', { name: /Tiny/ }));
    expect(document.activeElement).toBe(tiny);
    fireEvent.click(tiny);
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it('closes on Escape without changing the selection', async () => {
    const { fetchMock } = installModelFetch();
    openSession();
    render(<ModelSwitcher />);
    const trigger = screen.getByRole('button', { name: /Model for this session: Model B/i });
    fireEvent.click(trigger);
    expect(screen.queryByRole('listbox')).not.toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(document.activeElement).toBe(trigger);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('disables every model and reasoning action while this session’s pin is pending', async () => {
    const { fetchMock } = installModelFetch();
    const session = openSession({ provider: 'gateway', model: 'model-a', reasoning_effort: 'high' });
    render(<ModelControls />);
    const modelTrigger = screen.getByRole('button', { name: /Model for this session: Model A/i });
    const reasoningTrigger = screen.getByRole('button', { name: /Reasoning effort: High/i });
    fireEvent.click(modelTrigger);
    fireEvent.click(reasoningTrigger);
    expect(screen.getAllByRole('listbox')).toHaveLength(2);

    act(() => {
      store.sessionModelMutationPending.value = session.id;
    });
    await waitFor(() => {
      expect((modelTrigger as HTMLButtonElement).disabled).toBe(true);
      expect((reasoningTrigger as HTMLButtonElement).disabled).toBe(true);
    });
    for (const option of screen.getAllByRole('option')) {
      expect((option as HTMLButtonElement).disabled).toBe(true);
    }
    expect((screen.getByRole('button', { name: /Follow install default/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: /Manage providers and models/ }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole('option', { name: /Tiny/ }));
    fireEvent.click(screen.getByRole('option', { name: /^Minimal/ }));
    expect(fetchMock).not.toHaveBeenCalled();
    act(() => {
      store.sessionModelMutationPending.value = null;
    });
  });
});

describe('ModelSwitcher picker: search, facts and grouping', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('renders a context window as tokens, never as a message count', () => {
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));

    // The bug this pins: the row used `formatCount`, which is the *message*
    // counter, so a 1,000,000-token window read "1000000 messages ctx".
    expect(screen.queryByText(/messages ctx/)).toBeNull();
    expect(screen.getByText('128K ctx')).not.toBeNull();
    expect(screen.getByText('200K ctx')).not.toBeNull();
    // The exact figure is still available, in the chip's tooltip.
    expect(screen.getByText('200K ctx').getAttribute('title')).toBe('200,000 tokens of context');
  });

  it('names the provider on every row, even when the install has one profile', () => {
    const single = twoProfileConfig();
    store.config.value = { ...single, providers: [single.providers[0]!] };
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));

    // A model id alone does not say which endpoint will answer, and the old
    // picker hid the provider entirely when there was only one profile.
    const group = screen.getByRole('group', { name: 'Gateway' });
    expect(group.textContent).toContain('Gateway');
  });

  it('carries the facts in the row’s accessible name, not only in the chips', () => {
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));

    expect(
      screen.getByRole('option', { name: 'Model B on Gateway, 200K context, no tool calling' }),
    ).not.toBeNull();
    expect(screen.getByRole('option', { name: 'Model A on Gateway, 128K context, reasoning' })).not.toBeNull();
  });

  it('filters by model name, id or provider and counts what is left', async () => {
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));
    const filter = screen.getByRole('searchbox', { name: 'Filter models' });
    expect(screen.getByText('3 models')).not.toBeNull();

    fireEvent.input(filter, { target: { value: 'tiny' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));
    expect(screen.getByText('1 of 3')).not.toBeNull();
    expect(screen.getByRole('option', { name: /Tiny/ })).not.toBeNull();

    // The provider is part of the haystack, so "local" reaches the same row.
    fireEvent.input(filter, { target: { value: 'local' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));
    expect(screen.getByRole('option', { name: /Tiny/ })).not.toBeNull();
  });

  it('says so when nothing matches, and clears the filter from there', async () => {
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));
    const filter = screen.getByRole('searchbox', { name: 'Filter models' });

    fireEvent.input(filter, { target: { value: 'zzz' } });
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(screen.getByText(/No model matches/)).not.toBeNull();
    expect(screen.getByText('0 of 3')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Clear filter' }));
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(3));
    expect((filter as HTMLInputElement).value).toBe('');
  });

  it('takes the first match on Enter in the filter', async () => {
    const { calls } = installModelFetch();
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));
    const filter = screen.getByRole('searchbox', { name: 'Filter models' });

    fireEvent.input(filter, { target: { value: 'model-a' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));
    fireEvent.keyDown(filter, { key: 'Enter' });

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({ provider: 'gateway', model: 'model-a' });
  });

  it('clears the filter on Escape before it closes the menu', async () => {
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));
    const filter = screen.getByRole('searchbox', { name: 'Filter models' });

    fireEvent.input(filter, { target: { value: 'tiny' } });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));

    // The first Escape is the query's: the list comes back, the menu stays.
    fireEvent.keyDown(filter, { key: 'Escape' });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(3));
    expect(screen.queryByRole('listbox')).not.toBeNull();
    expect((filter as HTMLInputElement).value).toBe('');

    // The second Escape has nothing left to clear, so it closes the menu.
    fireEvent.keyDown(filter, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
  });

  it('says there are no models at all, rather than blaming a filter', () => {
    const bare = twoProfileConfig();
    bare.providers = [{ ...bare.providers[0]!, models: [] }];
    store.config.value = bare;
    openSession();
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Model for this session/ }));

    // A fresh install is not a query that matched nothing: there is nothing to
    // clear, and the fix is the footer's own door into Settings.
    expect(screen.getByText(/No models configured yet/)).not.toBeNull();
    expect(screen.queryByText(/No model matches/)).toBeNull();
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(screen.getByText('0 models')).not.toBeNull();
  });

  it('starts a reopened menu with an empty filter', async () => {
    openSession();
    render(<ModelSwitcher />);
    const trigger = screen.getByRole('button', { name: /Model for this session/ });
    fireEvent.click(trigger);
    fireEvent.input(screen.getByRole('searchbox', { name: 'Filter models' }), {
      target: { value: 'tiny' },
    });
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(1));

    fireEvent.click(trigger);
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    fireEvent.click(trigger);
    await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(3));
    expect((screen.getByRole('searchbox', { name: 'Filter models' }) as HTMLInputElement).value).toBe('');
  });
});

describe('modelOptions / filterModelOptions', () => {
  it('flattens every profile and model, keeping config order', () => {
    const options = modelOptions(twoProfileConfig());
    expect(options.map((option) => `${option.providerId}:${option.modelId}`)).toEqual([
      'gateway:model-a',
      'gateway:model-b',
      'local:tiny',
    ]);
    expect(options[0]).toMatchObject({
      providerName: 'Gateway',
      providerKeySet: true,
      modelName: 'Model A',
      contextWindow: 128000,
      reasoning: true,
    });
    expect(options[2]).toMatchObject({ providerName: 'Local', providerKeySet: false, modelName: 'Tiny' });
  });

  it('is empty without a config, so a caller never has to guard first', () => {
    expect(modelOptions(null)).toEqual([]);
  });

  it('matches every word anywhere in the name, id or provider', () => {
    const options = modelOptions(twoProfileConfig());
    expect(filterModelOptions(options, '').map((option) => option.modelId)).toEqual([
      'model-a',
      'model-b',
      'tiny',
    ]);
    expect(filterModelOptions(options, 'gateway').map((option) => option.modelId)).toEqual([
      'model-a',
      'model-b',
    ]);
    expect(filterModelOptions(options, 'MODEL-B').map((option) => option.modelId)).toEqual(['model-b']);
    // Two words narrow rather than widen: "model" alone reaches two rows.
    expect(filterModelOptions(options, 'model tiny')).toEqual([]);
    expect(filterModelOptions(options, '  ').map((option) => option.modelId)).toEqual([
      'model-a',
      'model-b',
      'tiny',
    ]);
  });

  it('does not match capability words, so "tools" cannot reach a model without them', () => {
    const options = modelOptions(twoProfileConfig());
    // Model B is the one with `tool_call: false`; its row says "no tools", and a
    // search for "tools" must not be answered with it.
    expect(filterModelOptions(options, 'tools')).toEqual([]);
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

  it('supports keyboard navigation and restores its trigger on selection', async () => {
    const { calls } = installModelFetch();
    openSession();
    render(<ReasoningSelector />);
    const trigger = screen.getByRole('button', { name: /Reasoning effort: Default/i });
    fireEvent.click(trigger);
    const options = screen.getAllByRole('option');
    expect(options.map((option) => option.getAttribute('tabindex'))).toEqual(['0', '-1', '-1', '-1', '-1', '-1', '-1']);
    await waitFor(() => expect(document.activeElement).toBe(options[0]));

    fireEvent.keyDown(options[0]!, { key: 'End' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /^Max/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'Home' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /^Default/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowUp' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /^Max/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /^Default/ })));
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('option', { name: /^Off/ })));

    fireEvent.click(document.activeElement!);
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(document.activeElement).toBe(trigger);
    expect(screen.queryByRole('listbox')).toBeNull();
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

describe('ModelSwitcher stability (the pills never blink out)', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('renders for an open session that has no row in any list', () => {
    // The bug: a brand-new control chat has no row until the overview
    // snapshot catches up, so the pills used to vanish for seconds.
    store.currentSessionId.value = 'c_fresh';
    render(<ModelControls />);
    expect(screen.getByRole('button', { name: /Model for this session/ })).not.toBeNull();
    expect(screen.getByRole('button', { name: /Reasoning effort:/ })).not.toBeNull();
  });

  it('keeps rendering after the session list is replaced', async () => {
    openSession();
    const { container } = render(<ModelControls />);
    const button = screen.getByRole('button', { name: /Model for this session/ });
    // A run event replaces `sessions.value` with the project's own list.
    store.sessions.value = [];
    await waitFor(() => expect(container.firstChild).not.toBeNull());
    expect(screen.getByRole('button', { name: /Model for this session/ })).toBe(button);
  });

  it('shows the install default and then the pinned model without unmounting', async () => {
    store.currentSessionId.value = 'c_fresh';
    render(<ModelSwitcher />);
    const button = screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i });
    expect(screen.queryByText(/this session/)).toBeNull();

    store.rememberSessionModel('c_fresh', { provider: 'local', model: 'tiny' });

    // Same button element: the pill changed its label, it did not remount.
    await waitFor(() => expect(button.getAttribute('aria-label')).toMatch(/Tiny/));
    expect(screen.getByRole('button', { name: /Model for this session/ })).toBe(button);
    expect(screen.getByText(/this session/)).not.toBeNull();
  });

  it('puts the session back on the install default when the pin is cleared', async () => {
    const { calls } = installModelFetch();
    store.currentSessionId.value = summaryFixture.id;
    store.rememberSessionModel(summaryFixture.id, { provider: 'local', model: 'tiny' });
    render(<ModelSwitcher />);
    expect(screen.getByRole('button', { name: /Model for this session: Tiny/i })).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Model for this session: Tiny/i }));
    fireEvent.click(screen.getByRole('button', { name: 'Follow install default' }));

    await waitFor(() => expect(calls).toHaveLength(1));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i })).not.toBeNull(),
    );
    expect(screen.queryByText(/this session/)).toBeNull();
  });

  it('says so honestly when no model is configured at all', () => {
    const bare = twoProfileConfig();
    bare.active = { provider: 'gateway', model: '' };
    bare.providers = [{ ...bare.providers[0]!, models: [] }];
    store.config.value = bare;
    store.currentSessionId.value = 'c_fresh';
    render(<ModelSwitcher />);
    const button = screen.getByRole('button', { name: /Model for this session/ });
    expect(button.getAttribute('title')).toBe('No model configured yet');
    expect(button.textContent).toContain('—');
  });

  it('lets a draft choose its model without a session to PATCH', async () => {
    // "New session" creates nothing until the first message, so there is no id to
    // write a pin to. The pills stay mounted and the choice is held in the draft,
    // where `sendChatMessage` picks it up for the create request.
    const fetchMock = vi.fn(async () => {
      throw new Error('a draft pin must not reach the server');
    });
    vi.stubGlobal('fetch', fetchMock);
    store.draftSession.value = { id: 5, projectId: null, model: null };

    render(<ModelSwitcher />);
    const button = screen.getByRole('button', { name: /Model for this session: Model B on Gateway/i });
    expect(screen.queryByText(/this session/)).toBeNull();

    fireEvent.click(button);
    fireEvent.click(screen.getByRole('option', { name: /Model A/ }));

    await waitFor(() => expect(store.draftSession.value?.model).toEqual({ provider: 'gateway', model: 'model-a' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Model for this session: Model A/i })).not.toBeNull(),
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

/**
 * The phone tier: one pill carries the model and the effort, and the effort's
 * choices are a group in the model sheet.
 *
 * The alternative - two pills at 108px/92px caps - rendered as "Claud…" and
 * "Def…" on a 390px screen, and the second sheet had nowhere to open: it was
 * clipped by the pill row it lived in. These tests pin the shape that replaces
 * them, not the geometry (that is `styles/composer.test.ts` and the browser).
 */
describe('ModelSwitcher on a phone', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
    store.phoneViewport.value = true;
  });

  afterEach(() => {
    store.phoneViewport.value = false;
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('shows the model and the reasoning effort in one control', () => {
    openSession({ provider: 'gateway', model: 'model-a', reasoning_effort: 'high' });
    render(<ModelControls />);

    const trigger = screen.getByRole('button', {
      name: /Model and reasoning for this session: Model A on Gateway, reasoning High/i,
    });
    // Both facts are on the button, not just in its accessible name.
    expect(trigger.textContent).toContain('Model A');
    expect(trigger.textContent).toContain('High');
    // There is no second pill to hit, and no second sheet to open.
    expect(screen.queryByRole('button', { name: /Reasoning effort:/ })).toBeNull();
  });

  it('offers the same efforts, in the same order, inside the model sheet', async () => {
    const { calls } = installModelFetch();
    openSession({ provider: 'gateway', model: 'model-a' });
    render(<ModelSwitcher />);

    fireEvent.click(screen.getByRole('button', { name: /Model and reasoning for this session/ }));
    const group = screen.getByRole('group', { name: 'Reasoning effort for this session' });
    expect(within(group).getAllByRole('button').map((button) => button.textContent)).toEqual(
      REASONING_OPTIONS.map((option) => option.label),
    );
    // The current value is a pressed toggle, not a listbox selection: the model
    // list owns arrow-key traversal and must keep it.
    expect(within(group).getByRole('button', { name: 'Default' }).getAttribute('aria-pressed')).toBe('true');

    fireEvent.click(within(group).getByRole('button', { name: 'Minimal' }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]?.body).toEqual({
      provider: 'gateway',
      model: 'model-a',
      reasoning_effort: 'minimal',
    });
    // Choosing closes the sheet, so the pill can show the new value.
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
  });

  it('keeps the group out of a model that cannot reason', () => {
    openSession({ provider: 'local', model: 'tiny' });
    render(<ModelSwitcher />);

    fireEvent.click(screen.getByRole('button', { name: /Model and reasoning for this session/ }));
    expect(screen.queryByRole('group', { name: 'Reasoning effort for this session' })).toBeNull();
    expect(screen.getAllByRole('option')).toHaveLength(3);
  });

  it('returns to two pills when the viewport leaves the phone tier', () => {
    openSession({ provider: 'gateway', model: 'model-a' });
    render(<ModelControls />);
    expect(screen.queryByRole('button', { name: /Reasoning effort:/ })).toBeNull();

    act(() => {
      store.phoneViewport.value = false;
    });
    expect(screen.getByRole('button', { name: /Model for this session:/ })).not.toBeNull();
    expect(screen.getByRole('button', { name: /Reasoning effort:/ })).not.toBeNull();
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

/**
 * The list is exported so any other surface that offers a reasoning effort —
 * the control plane's launch box — renders this table instead of a copy of it.
 * These tests pin what a second reader is allowed to assume about it.
 */
describe('REASONING_OPTIONS', () => {
  it('is the server’s own set of efforts, with "no preference" first', () => {
    // `default` is not a wire value: it means "send nothing", which is why
    // `ReasoningSelector` stores `null` for it.
    expect(REASONING_OPTIONS.map((option) => option.value)).toEqual([
      'default',
      'none',
      'minimal',
      'low',
      'medium',
      'high',
      'max',
    ]);
    expect(REASONING_OPTIONS[0]?.label).toBe('Default');
  });

  it('gives every entry a label and a description, so a picker can render all three fields', () => {
    for (const option of REASONING_OPTIONS) {
      expect(option.label.trim()).not.toBe('');
      expect(option.description.trim()).not.toBe('');
    }
    // A `<select>` shows the label; the description is what the popover shows.
    expect(REASONING_OPTIONS.find((option) => option.value === 'none')?.label).toBe('Off');
  });

  it('has no duplicate wire values', () => {
    const values = REASONING_OPTIONS.map((option) => option.value);
    expect(new Set(values).size).toBe(values.length);
  });

  it('renders exactly these options, in this order, in the popover', () => {
    resetAppState();
    store.config.value = twoProfileConfig();
    store.currentSessionId.value = summaryFixture.id;
    const { container } = render(<ReasoningSelector />);

    fireEvent.click(screen.getByRole('button', { name: /Reasoning effort:/ }));
    expect(
      [...container.querySelectorAll('.model-switcher-option-name')].map((node) => node.textContent),
    ).toEqual(REASONING_OPTIONS.map((option) => option.label));
    // The description line is the option's explanation of the wire value.
    expect(
      [...container.querySelectorAll('.model-switcher-option-id')].map((node) => node.textContent),
    ).toEqual(REASONING_OPTIONS.map((option) => option.description));
  });
});
