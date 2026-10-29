import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { SettingsSheet } from './SettingsSheet';
import type {
  CatalogLookupResponse,
  CatalogResponse,
  Config,
  ConfigPatch,
  ProviderTestResult,
} from '../api/types';
import { configFixture, jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

/* ------------------------------------------------------------------ *
 * Fixtures: two provider profiles, one of them active and keyed.
 * ------------------------------------------------------------------ */

function twoProfileConfig(): Config {
  return {
    ...configFixture,
    providers: [
      {
        id: 'default',
        name: 'Default',
        provider: 'openai_compatible',
        base_url: 'http://127.0.0.1:4000/v1',
        models: [
          {
            id: 'gpt-4o-mini',
            name: 'GPT-4o mini',
            context_window: 128000,
            max_output_tokens: 16384,
            tool_call: true,
            reasoning: false,
            cost_input: 0.15,
            cost_output: 0.6,
          },
        ],
        request_timeout_secs: 300,
        api_key_set: true,
      },
      {
        id: 'gateway',
        name: 'Gateway',
        provider: 'openai_compatible',
        base_url: 'https://api.example.test/v1',
        models: [
          {
            id: 'qwen3-32b',
            name: 'Qwen3 32B',
            context_window: 32768,
            max_output_tokens: 8192,
            tool_call: true,
            reasoning: true,
            cost_input: null,
            cost_output: null,
          },
        ],
        request_timeout_secs: 120,
        api_key_set: false,
      },
    ],
    active: { provider: 'default', model: 'gpt-4o-mini' },
  };
}

const CATALOG_PROVIDERS: CatalogResponse = {
  ok: true,
  providers: [{ id: 'openrouter', name: 'OpenRouter', model_count: 2 }],
  provider: null,
  models: [],
  fetched_at_ms: 1_710_000_000_000,
  stale: false,
  error: null,
};

const CATALOG_MODELS: CatalogResponse = {
  ok: true,
  providers: [],
  provider: { id: 'openrouter', name: 'OpenRouter', model_count: 2 },
  models: [
    {
      id: 'qwen/qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 65536,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: true,
      cost_input: 0.1,
      cost_output: 0.4,
    },
    {
      id: 'deepseek/deepseek-r1',
      name: 'DeepSeek R1',
      context_window: 131072,
      max_output_tokens: 16384,
      tool_call: false,
      reasoning: true,
      cost_input: 0.55,
      cost_output: 2.19,
    },
  ],
  fetched_at_ms: 1_710_000_000_000,
  stale: false,
  error: null,
};

/** What the server's prefix-stripping lookup would answer, per queried id. */
const LOOKUP_HITS: Record<string, CatalogLookupResponse['match']> = {
  'qwen3-32b': {
    provider: 'alibaba',
    matched_id: 'qwen3-32b',
    model: {
      id: 'qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 131072,
      max_output_tokens: 16384,
      tool_call: true,
      reasoning: true,
      cost_input: 0.2,
      cost_output: 0.6,
    },
  },
  'a6api/ali/deepseek-v4.1-flash-1017': {
    provider: 'deepseek',
    matched_id: 'deepseek-v4.1-flash-1017',
    model: {
      id: 'deepseek-v4.1-flash-1017',
      name: 'DeepSeek V4.1 Flash',
      context_window: 128000,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: false,
      cost_input: 0.07,
      cost_output: 0.28,
    },
  },
};

/* ------------------------------------------------------------------ *
 * Fetch stub: applies provider/active patches so the UI sees its writes.
 * ------------------------------------------------------------------ */

function applyPatches(config: Config, patch: ConfigPatch): Config {
  const next: Config = {
    ...config,
    providers: config.providers.map((profile) => ({ ...profile, models: [...profile.models] })),
    active: { ...config.active },
  };
  for (const incoming of patch.providers?.upsert ?? []) {
    const index = next.providers.findIndex((profile) => profile.id === incoming.id);
    if (index === -1) {
      next.providers.push({
        id: incoming.id,
        name: incoming.name ?? incoming.id,
        provider: incoming.provider ?? 'openai_compatible',
        base_url: incoming.base_url ?? '',
        models: incoming.models ?? [],
        request_timeout_secs: incoming.request_timeout_secs ?? 300,
        api_key_set: Boolean(incoming.api_key),
      });
      continue;
    }
    const existing = next.providers[index];
    if (!existing) continue;
    next.providers[index] = {
      ...existing,
      name: incoming.name ?? existing.name,
      base_url: incoming.base_url ?? existing.base_url,
      models: incoming.models ?? existing.models,
      api_key_set: incoming.clear_api_key
        ? false
        : incoming.api_key
          ? true
          : existing.api_key_set,
    };
  }
  for (const id of patch.providers?.delete ?? []) {
    next.providers = next.providers.filter((profile) => profile.id !== id);
  }
  if (patch.active) {
    next.active = {
      provider: patch.active.provider ?? next.active.provider,
      model: patch.active.model ?? next.active.model,
    };
  }
  return next;
}

function installFetch(config: Config, options: { testResult?: ProviderTestResult } = {}) {
  const patches: ConfigPatch[] = [];
  let current = config;
  const testResult: ProviderTestResult = options.testResult ?? {
    ok: true,
    models: ['gpt-4o-mini', 'qwen3-32b'],
    error: null,
    latency_ms: 42,
  };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const method = (init?.method ?? 'GET').toUpperCase();
    if (url === '/api/config' && method === 'GET') return jsonResponse(current);
    if (url === '/api/config' && method === 'PATCH') {
      const body: ConfigPatch =
        typeof init?.body === 'string' ? (JSON.parse(init.body) as ConfigPatch) : {};
      patches.push(body);
      current = applyPatches(current, body);
      return jsonResponse(current);
    }
    if (url === '/api/plugins') return jsonResponse({ plugins: [] });
    if (url === '/api/mcp') return jsonResponse({ servers: [] });
    if (url === '/api/health') {
      return jsonResponse({
        status: 'ok',
        version: '0.1.0',
        uptime_secs: 1,
        workspace: '/home/me/project',
        projects: [],
        auth_required: false,
        provider_configured: true,
        db: 'ok',
      });
    }
    if (url === '/api/provider/test') return jsonResponse(testResult);
    if (url === '/api/models/catalog') return jsonResponse(CATALOG_PROVIDERS);
    if (url === '/api/models/catalog?provider=openrouter') return jsonResponse(CATALOG_MODELS);
    if (url.startsWith('/api/models/lookup')) {
      const model = new URLSearchParams(url.split('?')[1] ?? '').get('model') ?? '';
      const response: CatalogLookupResponse = {
        ok: true,
        match: LOOKUP_HITS[model] ?? null,
        fetched_at_ms: 1_710_000_000_000,
        stale: false,
        error: null,
      };
      return jsonResponse(response);
    }
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${method} ${url}` } }, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, patches };
}

function renderSheet(config: Config) {
  store.config.value = config;
  store.settingsOpen.value = true;
  return render(<SettingsSheet />);
}

/**
 * Wait until the sheet has stopped re-seeding itself.
 *
 * The sheet paints the cached config, then `loadConfig()` resolves and replaces
 * the whole form state. A value typed before that lands is silently discarded,
 * so every test that edits a field has to let the refresh finish first.
 */
async function settleSheet(label: string, rendered: Config): Promise<HTMLInputElement> {
  await screen.findByLabelText(label);
  // The sheet paints the cached config, then `loadConfig()` resolves, stores the
  // parsed response (a fresh object) and re-seeds the whole form from it. A value
  // typed before that lands is silently discarded by `setForm(...)`, so wait for
  // the store write and the render it schedules, not for a guessed delay.
  await waitFor(() => expect(store.config.value).not.toBe(rendered));
  await new Promise((resolve) => setTimeout(resolve, 0));
  await waitFor(() =>
    expect((screen.getByLabelText(label) as HTMLInputElement).disabled).toBe(false),
  );
  return screen.getByLabelText(label) as HTMLInputElement;
}

function profileCard(name: string): Promise<HTMLElement> {
  return screen.findByRole('heading', { name }).then((heading) => {
    const card = heading.closest('.provider-card');
    if (!(card instanceof HTMLElement)) throw new Error(`no provider card for ${name}`);
    return card;
  });
}

describe('SettingsSheet providers', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('lists every provider profile with the active badge and key state', async () => {
    installFetch(twoProfileConfig());
    const { container } = renderSheet(twoProfileConfig());

    expect(await screen.findByRole('heading', { name: 'Default' })).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'Gateway' })).not.toBeNull();

    const cards = container.querySelectorAll('.provider-card');
    expect(cards).toHaveLength(2);
    const defaultCard = cards[0];
    const gatewayCard = cards[1];
    if (!(defaultCard instanceof HTMLElement) || !(gatewayCard instanceof HTMLElement)) {
      throw new Error('provider cards did not render');
    }

    expect(within(defaultCard).getByText('active')).not.toBeNull();
    expect(within(defaultCard).getByText('key set')).not.toBeNull();
    expect(within(defaultCard).getByText('http://127.0.0.1:4000/v1')).not.toBeNull();
    expect(within(defaultCard).getByText(/1 model/)).not.toBeNull();

    expect(within(gatewayCard).queryByText('active')).toBeNull();
    expect(within(gatewayCard).getByText('no key')).not.toBeNull();
    expect(within(gatewayCard).getByText('https://api.example.test/v1')).not.toBeNull();
    expect(within(gatewayCard).getByText('Qwen3 32B')).not.toBeNull();
  });

  it('posts {profile:id} to /api/provider/test and renders the inline result', async () => {
    const { fetchMock } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: /Test connection for/ }));

    expect(await within(gateway).findByText('Connection succeeded')).not.toBeNull();
    expect(within(gateway).getByText('42 ms')).not.toBeNull();
    expect(within(gateway).getByText(/2 models: gpt-4o-mini, qwen3-32b/)).not.toBeNull();
    // The other profile was not probed.
    const testCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input) === '/api/provider/test',
    );
    expect(testCalls).toHaveLength(1);
    const init = testCalls[0]?.[1];
    expect(JSON.parse(String(init?.body))).toEqual({ profile: 'gateway' });
  });

  it('shows a failing probe inline without breaking the form', async () => {
    installFetch(twoProfileConfig(), {
      testResult: { ok: false, models: undefined, error: '401 invalid key', latency_ms: 7 },
    });
    renderSheet(twoProfileConfig());

    const defaultCard = await profileCard('Default');
    fireEvent.click(within(defaultCard).getByRole('button', { name: /Test connection for/ }));

    expect(await within(defaultCard).findByText('Connection failed')).not.toBeNull();
    expect(within(defaultCard).getByText('401 invalid key')).not.toBeNull();
    expect(within(defaultCard).getByText('7 ms')).not.toBeNull();
  });

  it('saves the general settings without touching provider connection fields', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    fireEvent.click(await screen.findByRole('button', { name: 'Save changes' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    const patch = patches[0];
    expect(patch?.model).toBeDefined();
    // Connection details now belong to provider profiles, not the flat model
    // section: sending them here would overwrite the active profile.
    expect(patch?.model?.base_url).toBeUndefined();
    expect(patch?.model?.provider).toBeUndefined();
    expect(patch?.model?.model).toBeUndefined();
    expect(patch?.model?.api_key).toBeUndefined();
    expect(patch?.limits?.max_iterations).toBe(32);
    expect(patch?.limits?.auto_continue).toBe(true);
    expect(patch?.limits?.max_continues).toBe(50);
    expect(patch?.approvals?.mode).toBe('ask');
    expect(patch?.subagents?.max_concurrent).toBe(3);
    expect(patch?.subagents?.max_continues).toBe(0);
  });

  it('adds a picked models.dev model with prefilled metadata', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: /Browse models\.dev/ }));

    const picker = await screen.findByRole('dialog', { name: /models.dev: Gateway/ });
    fireEvent.click(await within(picker).findByRole('button', { name: /OpenRouter/ }));
    fireEvent.click(await within(picker).findByRole('checkbox', { name: /Qwen3 32B/ }));
    fireEvent.click(within(picker).getByRole('button', { name: /Add selected/ }));

    await waitFor(() => expect(patches).toHaveLength(1));
    const upsert = patches[0]?.providers?.upsert?.[0];
    expect(upsert?.id).toBe('gateway');
    expect(upsert?.models).toHaveLength(2);
    expect(upsert?.models?.[1]).toEqual({
      id: 'qwen/qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 65536,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: true,
      cost_input: 0.1,
      cost_output: 0.4,
      catalog_provider: 'openrouter',
    });

    // The picker closes and the new model shows up in the profile's library.
    await waitFor(() => expect(screen.queryByRole('dialog', { name: /models.dev/ })).toBeNull());
    const updatedGateway = await profileCard('Gateway');
    expect(within(updatedGateway).getAllByText('Qwen3 32B')).toHaveLength(2);
    expect(within(updatedGateway).getByText('qwen/qwen3-32b')).not.toBeNull();
  });

  it('detects metadata when adding a gateway-prefixed model id by hand', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    const input = within(gateway).getByLabelText('Add a model id');
    fireEvent.input(input, { target: { value: 'a6api/ali/deepseek-v4.1-flash-1017' } });
    fireEvent.click(within(gateway).getByRole('button', { name: 'Add model id to Gateway' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    const upsert = patches[0]?.providers?.upsert?.[0];
    expect(upsert?.id).toBe('gateway');
    // The gateway id is kept verbatim; only the metadata comes from the match.
    expect(upsert?.models?.[1]).toEqual({
      id: 'a6api/ali/deepseek-v4.1-flash-1017',
      name: 'DeepSeek V4.1 Flash',
      context_window: 128000,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: false,
      cost_input: 0.07,
      cost_output: 0.28,
      catalog_provider: 'deepseek',
    });

    const updatedGateway = await profileCard('Gateway');
    expect(within(updatedGateway).getByText('DeepSeek V4.1 Flash')).not.toBeNull();
    expect(within(updatedGateway).getByText('a6api/ali/deepseek-v4.1-flash-1017')).not.toBeNull();
    expect(within(updatedGateway).getByText('via deepseek')).not.toBeNull();
  });

  it('refreshes one model’s metadata from the catalog, keeping the custom name', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: 'Refresh metadata for Qwen3 32B on Gateway' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    const upsert = patches[0]?.providers?.upsert?.[0];
    expect(upsert?.id).toBe('gateway');
    // The custom display name survives; the metadata comes from the match.
    expect(upsert?.models?.[0]).toEqual({
      id: 'qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 131072,
      max_output_tokens: 16384,
      tool_call: true,
      reasoning: true,
      cost_input: 0.2,
      cost_output: 0.6,
      catalog_provider: 'alibaba',
    });
    const updatedGateway = await profileCard('Gateway');
    expect(within(updatedGateway).getByText('via alibaba')).not.toBeNull();
  });

  it('leaves the model alone when a refresh finds no catalog entry', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const defaultCard = await profileCard('Default');
    fireEvent.click(
      within(defaultCard).getByRole('button', { name: 'Refresh metadata for GPT-4o mini on Default' }),
    );

    await waitFor(() =>
      expect(store.toasts.value.some((toast) => toast.message.includes('No models.dev entry matches'))).toBe(
        true,
      ),
    );
    expect(patches).toHaveLength(0);
  });

  it('edits a model’s metadata through the dialog', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: 'Edit Qwen3 32B on Gateway' }));

    const dialog = await screen.findByRole('dialog', { name: 'Edit qwen3-32b' });
    fireEvent.input(within(dialog).getByLabelText('Context window'), { target: { value: '128000' } });
    fireEvent.input(within(dialog).getByLabelText('Output cost / M tokens'), { target: { value: '0.9' } });
    fireEvent.change(within(dialog).getByLabelText('Reasoning'), { target: { value: 'false' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save model' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.providers?.upsert?.[0]?.models?.[0]).toEqual({
      id: 'qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 128000,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: false,
      cost_input: null,
      cost_output: 0.9,
      catalog_provider: null,
    });
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Edit qwen3-32b' })).toBeNull());
  });

  it('rejects an out-of-range context window before saving', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: 'Edit Qwen3 32B on Gateway' }));

    const dialog = await screen.findByRole('dialog', { name: 'Edit qwen3-32b' });
    fireEvent.input(within(dialog).getByLabelText('Context window'), { target: { value: '0' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save model' }));

    await within(dialog).findByRole('alert');
    expect(patches).toHaveLength(0);
  });

  it('still adds the bare id when the catalog has no match', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const gateway = await profileCard('Gateway');
    const input = within(gateway).getByLabelText('Add a model id');
    fireEvent.input(input, { target: { value: 'a6api/ali/unlisted-9000' } });
    fireEvent.click(within(gateway).getByRole('button', { name: 'Add model id to Gateway' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    const upsert = patches[0]?.providers?.upsert?.[0];
    expect(upsert?.models?.[1]).toEqual({ id: 'a6api/ali/unlisted-9000', name: null });
  });
});

describe('SettingsSheet controls', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('lets a subagent outlive its first segment and saves both numbers', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    // A delegated task that needs hours is bounded by segments x refreshes, so
    // both numbers have to be reachable from the settings screen.
    await settleSheet('Auto-continues', config);
    // The mock config had the one-shot budget; make it a long-horizon one.
    fireEvent.input(screen.getByLabelText('Auto-continues'), { target: { value: '60' } });
    expect((screen.getByLabelText('Auto-continues') as HTMLInputElement).value).toBe('60');

    fireEvent.input(screen.getByLabelText('Turns per segment'), { target: { value: '200' } });

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.subagents?.max_iterations).toBe(200);
    expect(patches[0]?.subagents?.max_continues).toBe(60);
  });

  it('rejects an auto-continues value above the server bound', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    const field = await settleSheet('Auto-continues', config);
    fireEvent.input(field, { target: { value: '1001' } });
    expect(field.value).toBe('1001');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText('Auto-continues must be 0–1000.')).not.toBeNull();
    expect(patches).toHaveLength(0);
  });

  it('exposes every toggle as a labelled, described switch and saves the flip', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const parallel = await screen.findByRole('switch', { name: 'Parallel tool calls' });
    expect(parallel.getAttribute('aria-checked')).toBe('true');
    expect(parallel.className).toContain('toggle');
    expect(parallel.className).toContain('is-on');

    // The mock pairs the label with a hint line; both are announced.
    expect(screen.getByText('Ask the endpoint to allow multiple tool calls per turn.')).not.toBeNull();
    const describedBy = parallel.getAttribute('aria-describedby');
    expect(describedBy, 'switch is described by its hint').toBeTruthy();
    expect(document.getElementById(describedBy!)?.textContent).toContain('multiple tool calls');

    fireEvent.click(parallel);
    expect(parallel.getAttribute('aria-checked')).toBe('false');
    expect(parallel.className).not.toContain('is-on');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.model?.parallel_tool_calls).toBe(false);
  });

  it('keeps the settings sheet on the mock’s chrome classes', async () => {
    installFetch(twoProfileConfig());
    const { container } = renderSheet(twoProfileConfig());

    const sheet = await screen.findByRole('dialog', { name: 'Settings' });
    expect(sheet.className).toContain('dialog-sheet');
    expect(sheet.className).toContain('dialog-settings');
    // Title + description are the mock's `.modal-title` / `.modal-desc` pair.
    expect(within(sheet).getByText('Personalize your coding experience.')).not.toBeNull();
    expect(container.querySelectorAll('.setting-row').length).toBeGreaterThan(0);
    expect(container.querySelector('.settings-footer')).not.toBeNull();
  });
});

describe('SettingsSheet appearance', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    store.themeStudioOpen.value = false;
    vi.unstubAllGlobals();
  });

  it('opens the Theme Studio from the Appearance row without saving settings', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const appearance = await screen.findByRole('region', { name: 'Appearance' });
    expect(within(appearance).getByText(/stored in this browser/)).not.toBeNull();

    fireEvent.click(within(appearance).getByRole('button', { name: 'Theme studio' }));

    expect(store.themeStudioOpen.value).toBe(true);
    expect(store.settingsOpen.value).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
    // The appearance control is type="button" inside the settings form, so it
    // must never submit a config patch.
    expect(patches).toHaveLength(0);
  });
});