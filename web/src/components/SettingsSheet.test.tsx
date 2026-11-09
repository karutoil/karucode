import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { SettingsSheet } from './SettingsSheet';
import type {
  CatalogLookupResponse,
  CatalogResponse,
  Config,
  ConfigPatch,
  PluginDiagnostics,
  PluginRescanReport,
  ProviderTestResult,
} from '../api/types';
import { configFixture, jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import * as THEME from '../theme';
import {
  BUILTIN_THEMES,
  DENSITY_SCALES,
  FONT_STACKS,
  MAX_ROUNDING,
  MIN_ROUNDING,
  MONO_STACKS,
  THEME_DENSITIES,
  THEME_FONTS,
  THEME_GROUPING_CHOICES,
  THEME_MODES,
  THEME_MONO_FONTS,
  THEME_STORAGE_KEY,
  THEME_THINKING_CHOICES,
  THEME_TOOL_STYLE_CHOICES,
  THEME_WORKING_CHOICES,
  isDarkColor,
  resolvePalette,
} from '../theme';

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
            // Not declared: the install-wide default applies. The edit dialog is
            // where a user makes this explicit.
            vision: null,
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
      // A vision model, so the picker's prefill is observable in the saved patch.
      vision: true,
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
      vision: false,
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
      vision: true,
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
      // A text-only match: proves a refresh does not invent vision.
      vision: false,
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

function installFetch(
  config: Config,
  options: { testResult?: ProviderTestResult; plugins?: PluginDiagnostics[] } = {},
) {
  const patches: ConfigPatch[] = [];
  /** POST paths the sheet sent to the plugin lifecycle routes, in order. */
  const pluginPosts: string[] = [];
  let pluginListCalls = 0;
  /** What a plugin *contributes* lives in `/api/config`, so this counts re-reads. */
  let configReads = 0;
  let plugins = options.plugins ?? [];
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
    if (url === '/api/config' && method === 'GET') {
      configReads += 1;
      return jsonResponse(current);
    }
    if (url === '/api/config' && method === 'PATCH') {
      const body: ConfigPatch =
        typeof init?.body === 'string' ? (JSON.parse(init.body) as ConfigPatch) : {};
      patches.push(body);
      current = applyPatches(current, body);
      return jsonResponse(current);
    }
    if (url === '/api/plugins') {
      pluginListCalls += 1;
      return jsonResponse({ plugins });
    }
    if (url.startsWith('/api/plugins/') && method === 'POST') {
      pluginPosts.push(url);
      if (url === '/api/plugins/reload') {
        const report: PluginRescanReport = {
          added: [],
          removed: [],
          restarted: [],
          unchanged: plugins.map((plugin) => plugin.name),
        };
        return jsonResponse({ report, plugins });
      }
      const name = decodeURIComponent(url.split('/')[3] ?? '');
      const found = plugins.find((plugin) => plugin.name === name);
      if (!found) {
        return jsonResponse(
          { error: { code: 'not_found', message: `no plugin ${name}` } },
          404,
        );
      }
      // A reload replaces the process, so the pid the row was showing is stale.
      plugins = plugins.map((plugin) =>
        plugin.name === name ? { ...plugin, pid: 9999, status: 'ready' } : plugin,
      );
      return jsonResponse(plugins.find((plugin) => plugin.name === name));
    }
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
  return {
    fetchMock,
    patches,
    pluginPosts,
    pluginListCalls: () => pluginListCalls,
    configReads: () => configReads,
  };
}

function renderSheet(config: Config) {
  store.config.value = config;
  store.settingsOpen.value = true;
  const result = render(<SettingsSheet />);
  // The sheet is a Dialog, and a Dialog renders through a portal into
  // `document.body` so no ancestor can clip it (see Dialog.tsx). Query the
  // document, not the render container, for anything inside the sheet.
  return { ...result, container: document.body };
}

/** Open one of the sheet's tabs. */
async function openTab(name: string): Promise<HTMLElement> {
  const tab = await screen.findByRole('tab', { name });
  fireEvent.click(tab);
  return tab;
}

/** The panel the sheet is currently showing. Panels stay mounted, so every query
 * for sheet content has to be scoped to this one or it will find the same label
 * in two tabs. */
function visiblePanel(): HTMLElement {
  const panel = document.querySelector('.settings-panels > .tab-panel:not([hidden])');
  if (!(panel instanceof HTMLElement)) throw new Error('no visible settings panel');
  return panel;
}

/**
 * Open the tab's Advanced disclosure.
 *
 * The disclosure is a native `<details>`, which jsdom keeps in the DOM but a real
 * browser hides while closed — the tests click the way a user does rather than
 * asserting on markup that only exists in jsdom.
 */
async function openAdvanced(): Promise<HTMLDetailsElement> {
  const details = visiblePanel().querySelector('details.settings-advanced');
  if (!(details instanceof HTMLDetailsElement)) throw new Error('no Advanced block on this tab');
  const summary = details.querySelector('summary');
  if (summary === null) throw new Error('Advanced block has no summary');
  fireEvent.click(summary);
  return details as HTMLDetailsElement;
}

/** The Advanced disclosure of a named panel. */
function advancedOf(panelId: string): HTMLDetailsElement {
  const details = document.querySelector(`#settings-panel-${panelId} details.settings-advanced`);
  if (!(details instanceof HTMLDetailsElement)) {
    throw new Error(`tab ${panelId} has no Advanced disclosure`);
  }
  return details;
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

/** Switch to the tab that owns `label`, then settle the form there. */
async function settleOnTab(
  tab: string,
  label: string,
  rendered: Config,
): Promise<HTMLInputElement> {
  await openTab(tab);
  return settleSheet(label, rendered);
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
    await openTab('Models');

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
    await openTab('Models');

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
    await openTab('Models');

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
    expect(patch?.approvals?.mode).toBe('ask');
    expect(patch?.subagents?.max_concurrent).toBe(3);
  });

  it('adds a picked models.dev model with prefilled metadata', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await openTab('Models');

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
      // models.dev says this one takes images, so the picker prefills it.
      vision: true,
      cost_input: 0.1,
      cost_output: 0.4,
      catalog_provider: 'openrouter',
    });

    // The picker closes and the new model shows up in the profile's library.
    await waitFor(() => expect(screen.queryByRole('dialog', { name: /models.dev/ })).toBeNull());
    const updatedGateway = await profileCard('Gateway');
    expect(within(updatedGateway).getAllByText('Qwen3 32B')).toHaveLength(2);
    expect(within(updatedGateway).getByText('qwen/qwen3-32b')).not.toBeNull();
    // The prefilled vision flag is what the row shows.
    expect(within(updatedGateway).getByText('vision')).not.toBeNull();
  });

  it('detects metadata when adding a gateway-prefixed model id by hand', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await openTab('Models');

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
      vision: false,
      cost_input: 0.07,
      cost_output: 0.28,
      catalog_provider: 'deepseek',
    });

    const updatedGateway = await profileCard('Gateway');
    expect(within(updatedGateway).getByText('DeepSeek V4.1 Flash')).not.toBeNull();
    expect(within(updatedGateway).getByText('a6api/ali/deepseek-v4.1-flash-1017')).not.toBeNull();
    expect(within(updatedGateway).getByText('via deepseek')).not.toBeNull();
    // The prefilled per-model answer is visible on the row, not just in the patch.
    expect(within(updatedGateway).getByText('no images')).not.toBeNull();
  });

  it('refreshes one model’s metadata from the catalog, keeping the custom name', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await openTab('Models');

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
      vision: true,
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
    await openTab('Models');

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
    await openTab('Models');

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: 'Edit Qwen3 32B on Gateway' }));

    const dialog = await screen.findByRole('dialog', { name: 'Edit qwen3-32b' });
    fireEvent.input(within(dialog).getByLabelText('Context window'), { target: { value: '128000' } });
    fireEvent.input(within(dialog).getByLabelText('Output cost / M tokens'), { target: { value: '0.9' } });
    fireEvent.change(within(dialog).getByLabelText('Reasoning'), { target: { value: 'false' } });
    // The per-model image flag is editable here; this is the switch that makes
    // read_file show the model a picture.
    fireEvent.change(within(dialog).getByLabelText('Image input (vision)'), {
      target: { value: 'true' },
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save model' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.providers?.upsert?.[0]?.models?.[0]).toEqual({
      id: 'qwen3-32b',
      name: 'Qwen3 32B',
      context_window: 128000,
      max_output_tokens: 8192,
      tool_call: true,
      reasoning: false,
      vision: true,
      cost_input: null,
      cost_output: 0.9,
      catalog_provider: null,
    });
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Edit qwen3-32b' })).toBeNull());
  });

  it('leaves the per-model image flag undeclared when the dialog is left alone', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await openTab('Models');

    const gateway = await profileCard('Gateway');
    fireEvent.click(within(gateway).getByRole('button', { name: 'Edit Qwen3 32B on Gateway' }));

    const dialog = await screen.findByRole('dialog', { name: 'Edit qwen3-32b' });
    // Untouched, the field must stay "unknown" rather than silently becoming a
    // text-only declaration that would override the install-wide default.
    expect((within(dialog).getByLabelText('Image input (vision)') as HTMLSelectElement).value).toBe('');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save model' }));

    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.providers?.upsert?.[0]?.models?.[0]?.vision).toBeNull();
  });

  it('rejects an out-of-range context window before saving', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await openTab('Models');

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
    await openTab('Models');

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

  it('saves the subagent turn ceiling and offers no-ceiling as the default', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    // There is no continuation to tune any more: a subagent is bounded by one
    // number (turns, 0 = no ceiling) and its wall clock.
    await settleOnTab('Agent', 'Max concurrent', config);
    await openAdvanced();
    const panel = visiblePanel();
    const turns = within(panel).getByLabelText('Turns per subagent (0 = no ceiling)') as HTMLInputElement;
    expect(turns.min).toBe('0');

    fireEvent.input(turns, { target: { value: '200' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.subagents?.max_iterations).toBe(200);
    expect(patches[0]?.subagents).not.toHaveProperty('max_continues');
  });

  it('rejects a subagent turn ceiling above the server bound', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    await settleOnTab('Agent', 'Max concurrent', config);
    await openAdvanced();
    const field = screen.getByLabelText('Turns per subagent (0 = no ceiling)') as HTMLInputElement;
    fireEvent.input(field, { target: { value: '10001' } });
    expect(field.value).toBe('10001');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText('Iterations must be 0 (no ceiling) or 1–10000.')).not.toBeNull();
    expect(patches).toHaveLength(0);
  });

  /**
   * `0` is the "no ceiling" spelling the server documents, and `255` is the
   * deepest the `u8` depth field holds. The form used to reject both: it capped
   * concurrency at 32 and depth at 10, so an operator asking for the widest
   * setting got a validation error that the server would have accepted.
   */
  it('round-trips 0 concurrency and depth 255 as an unbounded install', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    await settleOnTab('Agent', 'Max concurrent', config);
    await openAdvanced();
    const panel = visiblePanel();

    const concurrent = within(panel).getByLabelText('Max concurrent') as HTMLInputElement;
    const background = within(panel).getByLabelText('Live background agents') as HTMLInputElement;
    const depth = within(panel).getByLabelText('Max depth') as HTMLInputElement;

    // The inputs themselves advertise the wider range, so a browser's own
    // spinners do not refuse a value the server accepts.
    expect(concurrent.min).toBe('0');
    expect(concurrent.max).toBe('1024');
    expect(background.min).toBe('0');
    expect(background.max).toBe('1024');
    expect(depth.max).toBe('255');

    fireEvent.input(concurrent, { target: { value: '0' } });
    fireEvent.input(background, { target: { value: '0' } });
    fireEvent.input(depth, { target: { value: '255' } });
    expect(concurrent.value).toBe('0');
    expect(depth.value).toBe('255');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.subagents?.max_concurrent).toBe(0);
    expect(patches[0]?.subagents?.max_background).toBe(0);
    expect(patches[0]?.subagents?.max_depth).toBe(255);
    // Nothing was rejected locally, so no error text is on screen.
    expect(visiblePanel().querySelector('.field-error')).toBeNull();
  });

  it('still rejects a concurrency value above the server ceiling', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    await settleOnTab('Agent', 'Max concurrent', config);
    await openAdvanced();
    const panel = visiblePanel();
    fireEvent.input(within(panel).getByLabelText('Max concurrent'), { target: { value: '1025' } });

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(
      await screen.findByText('Concurrency must be 0 (no ceiling) or 1–1024.'),
    ).not.toBeNull();
    expect(patches).toHaveLength(0);
  });

  it('exposes every toggle as a labelled, described switch and saves the flip', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    await openTab('Models');
    await openAdvanced();
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

  it('saves the install-wide image default from its own switch', async () => {
    const config = twoProfileConfig();
    config.model.supports_images = false;
    const { patches } = installFetch(config);
    renderSheet(config);

    await openTab('Models');
    await openAdvanced();
    const images = await screen.findByRole('switch', { name: 'Send images to models' });
    expect(images.getAttribute('aria-checked')).toBe('false');
    // The description has to explain the interaction with a model's own flag,
    // otherwise "off" looks like it disables a vision model too.
    const describedBy = images.getAttribute('aria-describedby');
    expect(document.getElementById(describedBy!)?.textContent).toContain('overrides it');

    fireEvent.click(images);
    expect(images.getAttribute('aria-checked')).toBe('true');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.model?.supports_images).toBe(true);
  });

  it('does not send a temperature unless the toggle is on', async () => {
    // An invented sampling value pushed a reasoning model into repeating itself
    // instead of acting, so the field is opt-in and an empty box must not block
    // saving. Default: not sent at all.
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    const { container } = renderSheet(config);

    await openTab('Models');
    await openAdvanced();
    const toggle = await screen.findByRole('switch', { name: 'Send a temperature' });
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    const input = container.querySelector('#temperature') as HTMLInputElement;
    expect(input.disabled, 'the number box is inert while the toggle is off').toBe(true);

    // An empty box is fine while nothing is sent.
    fireEvent.input(input, { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.model?.send_temperature).toBe(false);
    expect('temperature' in (patches[0]?.model ?? {})).toBe(false);
  });

  it('sends the temperature when the toggle is on', async () => {
    const config = twoProfileConfig();
    config.model.send_temperature = true;
    const { patches } = installFetch(config);
    const { container } = renderSheet(config);

    await openTab('Models');
    await openAdvanced();
    const toggle = await screen.findByRole('switch', { name: 'Send a temperature' });
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    const input = container.querySelector('#temperature') as HTMLInputElement;
    expect(input.disabled).toBe(false);

    fireEvent.input(input, { target: { value: '0.4' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]?.model?.send_temperature).toBe(true);
    expect(patches[0]?.model?.temperature).toBe(0.4);
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

    await openTab('General');
    const appearance = await screen.findByRole('region', { name: 'Appearance' });
    expect(within(appearance).getByText(/stored in this browser/i)).not.toBeNull();
    // The General tab is where the theme is actually chosen now, not only a door
    // into the Studio: the sheet's most-used appearance settings belong here.
    expect(within(appearance).getByLabelText('Theme')).not.toBeNull();
    expect(within(appearance).getByLabelText('Density')).not.toBeNull();

    fireEvent.click(within(appearance).getByRole('button', { name: 'Theme studio' }));

    expect(store.themeStudioOpen.value).toBe(true);
    expect(store.settingsOpen.value).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
    // The appearance control is type="button" inside the settings form, so it
    // must never submit a config patch.
    expect(patches).toHaveLength(0);
  });

  it('links Theme, Density, and Colour mode to their existing Field hints', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    await openTab('General');
    const appearance = await screen.findByRole('region', { name: 'Appearance' });
    const controls = [
      ['Theme', 'settings-theme-hint', 'Applies immediately. Saving this sheet is not required.'],
      ['Density', 'settings-density-hint', 'Moves type size and spacing together.'],
      ['Colour mode', 'settings-mode-hint', 'Re-bases the surfaces, keeping your accent.'],
    ] as const;

    for (const [label, hintId, hintText] of controls) {
      const control = within(appearance).getByLabelText(label);
      expect(control.getAttribute('aria-describedby')).toBe(hintId);
      expect(document.getElementById(hintId)?.textContent).toBe(hintText);
    }
  });

  it('renders the Notifications section inside the sheet', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    await openTab('Notifications');
    const section = await screen.findByRole('region', { name: 'Notifications' });
    expect(within(section).getByRole('switch', { name: 'Notify me about runs' })).toBeTruthy();
    // The device half is its own section now, on the same tab: a server switch and
    // a browser switch must never be side by side without that boundary.
    const device = await screen.findByRole('region', { name: 'This browser' });
    expect(within(device).getByRole('switch', { name: 'Play a sound' })).toBeTruthy();
  });
});

describe('SettingsSheet appearance controls', () => {
  /* The General tab is the second front door to the theme engine: it exposes the
   * same options as the Theme Studio but writes them straight into the sheet, so
   * every one of them has to apply to <html>, persist in the browser and — the
   * part that is easy to get wrong — never travel to the server as a config
   * patch. Appearance is stored per browser profile, so a PATCH here would either
   * be rejected by the server or silently rewrite someone's config.
   *
   * Table-driven on purpose: the failure mode is a single option wired to the
   * wrong mutator (Code font setting the UI font, Radius writing density), which
   * one example per control would not catch across the whole matrix. */

  const THEME_KEY = THEME_STORAGE_KEY;

  function persistedDraft(): Record<string, unknown> | null {
    const raw = localStorage.getItem(THEME_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { draft?: Record<string, unknown> | null };
    return parsed.draft ?? null;
  }

  function htmlToken(property: string): string {
    return document.documentElement.style.getPropertyValue(property);
  }

  beforeEach(() => {
    resetAppState();
    localStorage.clear();
    THEME.resetTheme();
  });

  afterEach(() => {
    resetAppState();
    localStorage.clear();
    THEME.resetTheme();
    vi.unstubAllGlobals();
  });

  it('offers all eight built-in themes and applies any of them locally', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    const select = (await settleOnTab('General', 'Theme', twoProfileConfig())) as unknown as HTMLSelectElement;
    const options = Array.from(select.options)
      .map((option) => option.value)
      .filter((value) => value !== 'draft');
    expect(options).toEqual(BUILTIN_THEMES.map((theme) => theme.id));

    for (const theme of BUILTIN_THEMES) {
      fireEvent.change(select, { target: { value: theme.id } });
      expect(document.documentElement.dataset.theme, theme.id).toBe(theme.id);
      expect(document.documentElement.dataset.mode, theme.id).toBe(theme.mode);
      expect(htmlToken('--theme-bg'), theme.id).toBe(resolvePalette(theme).bg);
      expect(htmlToken('--theme-text'), theme.id).toBe(resolvePalette(theme).text);
      const stored = JSON.parse(localStorage.getItem(THEME_KEY) ?? '{}') as { selected?: string };
      expect(stored.selected, theme.id).toBe(theme.id);
      expect(patches, theme.id).toHaveLength(0);
    }
  });

  it('applies both colour modes, re-basing the palette for the chosen one', async () => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await settleOnTab('General', 'Theme', twoProfileConfig());
    const mode = screen.getByLabelText('Colour mode');

    // The default theme is dark, so the light flip comes first: it is the one that
    // re-bases Ember's near-black surfaces, and flipping straight back has to
    // restore a dark canvas rather than keep them.
    for (const value of ['light', 'dark'] as const) {
      fireEvent.change(mode, { target: { value } });
      expect(document.documentElement.dataset.mode, value).toBe(value);
      expect(document.documentElement.style.colorScheme, value).toBe(value);
      expect(isDarkColor(THEME.activeTheme.value.colors.bg), value).toBe(value === 'dark');
      expect(THEME.activeTheme.value.colors.accent, value).toBe(THEME.DEFAULT_THEME.colors.accent);
      expect(persistedDraft()?.mode, value).toBe(value);
      expect(patches, value).toHaveLength(0);
    }

    THEME.initTheme();
    expect(document.documentElement.dataset.mode).toBe('dark');
  });

  it.each(THEME_DENSITIES.map((value) => [value] as const))(
    'applies the %s density to the scale, not only to an attribute',
    async (value) => {
      const { patches } = installFetch(twoProfileConfig());
      renderSheet(twoProfileConfig());
      await settleOnTab('General', 'Theme', twoProfileConfig());

      fireEvent.change(screen.getByLabelText('Density'), { target: { value } });

      const scale = DENSITY_SCALES[value];
      expect(document.documentElement.dataset.density).toBe(value);
      expect(htmlToken('--fs-body')).toBe(`${scale.body}px`);
      expect(htmlToken('--fs-small')).toBe(`${scale.small}px`);
      expect(htmlToken('--lh-body')).toBe(String(scale.lineHeight));
      expect(htmlToken('--theme-density')).toBe(String(scale.scale));
      // Spacing moves with the type step (that is what "density" means here).
      expect(htmlToken('--space-4')).toBe(`${Math.round(8 * scale.space * 100) / 100}px`);
      expect(persistedDraft()?.density).toBe(value);
      expect(patches).toHaveLength(0);

      // A reload re-applies it: persistence, not just live state.
      THEME.initTheme();
      expect(document.documentElement.dataset.density).toBe(value);
      expect(htmlToken('--fs-body')).toBe(`${scale.body}px`);
    },
  );

  it.each(
    THEME_TOOL_STYLE_CHOICES.flatMap((toolStyle) =>
      THEME_GROUPING_CHOICES.flatMap((grouping) =>
        THEME_WORKING_CHOICES.flatMap((working) =>
          THEME_THINKING_CHOICES.map((thinking) => [
            `${toolStyle.value}/${grouping.value}/${working.value}/${thinking.value}`,
            toolStyle.value,
            grouping.value,
            working.value,
            thinking.value,
          ] as const),
        ),
      ),
    ),
  )(
    'applies the %s transcript combination',
    async (_label, toolStyle, grouping, working, thinking) => {
      const { patches } = installFetch(twoProfileConfig());
      renderSheet(twoProfileConfig());
      await settleOnTab('General', 'Theme', twoProfileConfig());

      fireEvent.change(screen.getByLabelText('Tool calls'), { target: { value: toolStyle } });
      fireEvent.change(screen.getByLabelText('Grouping'), { target: { value: grouping } });
      fireEvent.change(screen.getByLabelText('Working steps'), { target: { value: working } });
      fireEvent.change(screen.getByLabelText('Reasoning blocks'), { target: { value: thinking } });

      expect(document.documentElement.dataset.toolStyle).toBe(toolStyle);
      expect(document.documentElement.dataset.grouping).toBe(grouping);
      expect(document.documentElement.dataset.working).toBe(working);
      expect(document.documentElement.dataset.thinking).toBe(thinking);
      expect(persistedDraft()).toMatchObject({ toolStyle, grouping, working, thinking });
      expect(patches).toHaveLength(0);

      THEME.initTheme();
      expect(document.documentElement.dataset.toolStyle).toBe(toolStyle);
      expect(document.documentElement.dataset.grouping).toBe(grouping);
      expect(document.documentElement.dataset.working).toBe(working);
      expect(document.documentElement.dataset.thinking).toBe(thinking);
    },
  );

  it('names the transcript options the way the Theme Studio does', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await settleOnTab('General', 'Theme', twoProfileConfig());

    const valuesOf = (label: string): string[] =>
      Array.from((screen.getByLabelText(label) as HTMLSelectElement).options).map(
        (option) => option.value,
      );
    expect(valuesOf('Tool calls')).toEqual(THEME_TOOL_STYLE_CHOICES.map((c) => c.value));
    expect(valuesOf('Grouping')).toEqual(THEME_GROUPING_CHOICES.map((c) => c.value));
    expect(valuesOf('Working steps')).toEqual(THEME_WORKING_CHOICES.map((c) => c.value));
    expect(valuesOf('Reasoning blocks')).toEqual(THEME_THINKING_CHOICES.map((c) => c.value));
    expect(valuesOf('Density')).toEqual([...THEME_DENSITIES]);
    expect(valuesOf('Colour mode')).toEqual([...THEME_MODES]);
  });

  it.each([
    ...THEME_FONTS.map((value) => ['Interface font', value, FONT_STACKS[value]] as const),
    ...THEME_MONO_FONTS.map((value) => ['Code font', value, MONO_STACKS[value]] as const),
  ])('applies the %s option "%s"', async (label, value, stack) => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await settleOnTab('General', 'Theme', twoProfileConfig());
    await openAdvanced();

    fireEvent.change(screen.getByLabelText(label), { target: { value } });

    const property = label === 'Code font' ? '--font-mono' : '--font-ui';
    expect(htmlToken(property)).toBe(stack);
    const draft = persistedDraft() as { font?: string; mono?: string } | null;
    expect(draft?.[label === 'Code font' ? 'mono' : 'font']).toBe(value);
    expect(patches).toHaveLength(0);

    THEME.initTheme();
    expect(htmlToken(property)).toBe(stack);
  });

  it.each([MIN_ROUNDING, MAX_ROUNDING])('applies the %i px corner radius endpoint', async (value) => {
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await settleOnTab('General', 'Theme', twoProfileConfig());
    await openAdvanced();

    fireEvent.input(screen.getByLabelText('Corner rounding (px)'), { target: { value: String(value) } });

    expect(htmlToken('--radius')).toBe(`${value}px`);
    expect(htmlToken('--radius-sm')).toBe(`${Math.max(0, value - 4)}px`);
    expect(htmlToken('--radius-md')).toBe(`${Math.max(0, value - 2)}px`);
    expect(htmlToken('--radius-xs')).toBe(`${Math.max(0, value - 6)}px`);
    expect(htmlToken('--radius-lg')).toBe(`${value + 4}px`);
    expect(persistedDraft()?.rounding).toBe(value);
    expect(patches).toHaveLength(0);
  });

  it('never sends an appearance change to the server, even after Save', async () => {
    /* Belt and braces on the whole block: the Appearance section is the one part
     * of this sheet that is not config, so a save after touching every control
     * must still produce at most the untouched-config patch — no theme keys, and
     * nothing at all when nothing else changed. */
    const { patches } = installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());
    await settleOnTab('General', 'Theme', twoProfileConfig());
    await openAdvanced();

    fireEvent.change(screen.getByLabelText('Density'), { target: { value: 'compact' } });
    fireEvent.change(screen.getByLabelText('Colour mode'), { target: { value: 'light' } });
    fireEvent.change(screen.getByLabelText('Code font'), { target: { value: 'classic' } });
    fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'forest' } });
    expect(patches).toHaveLength(0);

    const save = screen.getByRole('button', { name: 'Save changes' });
    fireEvent.click(save);
    await waitFor(() => expect(patches.length).toBeLessThanOrEqual(1));
    for (const patch of patches) {
      const keys = Object.keys(patch as Record<string, unknown>);
      for (const themeKey of ['theme', 'density', 'mode', 'font', 'mono', 'rounding']) {
        expect(keys, `config patch leaked ${themeKey}`).not.toContain(themeKey);
      }
    }
  });
});

describe('SettingsSheet tabs', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('splits the sheet into five labelled tabs, one panel at a time', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const tabs = await screen.findAllByRole('tab');
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      'General',
      'Models',
      'Agent',
      'Notifications',
      'Plugins & MCP',
    ]);
    // Exactly one tab is selected and reachable; the rest are one Tab away.
    expect(tabs.map((tab) => tab.getAttribute('aria-selected'))).toEqual([
      'true',
      'false',
      'false',
      'false',
      'false',
    ]);
    expect(tabs.map((tab) => tab.tabIndex)).toEqual([0, -1, -1, -1, -1]);

    // The first tab is the one that opens, and it is the only visible panel.
    expect(tabs[0]?.getAttribute('aria-controls')).toBe('settings-panel-general');
    const panels = document.querySelectorAll('.settings-panels > .tab-panel');
    expect(panels).toHaveLength(5);
    expect(panels[0]?.hasAttribute('hidden')).toBe(false);
    expect([...panels].slice(1).every((panel) => panel.hasAttribute('hidden'))).toBe(true);

    // A locked sheet still lets every tab be read, so the strip is never disabled.
    expect(tabs.every((tab) => !(tab as HTMLButtonElement).disabled)).toBe(true);
  });

  it('moves between tabs with the arrow keys and keeps the panels in sync', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const strip = await screen.findByRole('tablist', { name: 'Settings sections' });
    fireEvent.keyDown(strip, { key: 'ArrowRight' });

    const models = screen.getByRole('tab', { name: 'Models' });
    expect(models.getAttribute('aria-selected')).toBe('true');
    expect(document.getElementById('settings-panel-models')?.hasAttribute('hidden')).toBe(false);
    expect(document.getElementById('settings-panel-general')?.hasAttribute('hidden')).toBe(true);
    // The provider list is only rendered in the Models panel, so this proves the
    // panel swap moved the content and not just the highlight.
    expect(await screen.findByRole('heading', { name: 'Default' })).not.toBeNull();
  });

  it('keeps a half-typed value when the tab is switched away and back', async () => {
    const config = twoProfileConfig();
    installFetch(config);
    renderSheet(config);

    // A panel that unmounted on switch would lose this; it is why the sheet's
    // panels persist rather than being created on demand.
    const density = await settleOnTab('General', 'Theme', config);
    fireEvent.input(screen.getByLabelText('Top-p'), { target: { value: '' } });
    await openTab('Models');
    await openAdvanced();
    const topP = screen.getByLabelText('Top-p') as HTMLInputElement;
    fireEvent.input(topP, { target: { value: '0.8' } });

    await openTab('Agent');
    await openTab('Models');
    expect((screen.getByLabelText('Top-p') as HTMLInputElement).value).toBe('0.8');
    expect(density).not.toBeNull();
  });

  it('starts every Advanced block collapsed', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    // The whole point of the tabs is that the common settings fit without
    // scrolling; a disclosure that started open would put them all back.
    for (const panel of ['general', 'models', 'agent', 'notifications']) {
      expect(advancedOf(panel).open, `${panel} starts collapsed`).toBe(false);
    }
  });

  it('announces the Advanced block as a disclosure and toggles it', async () => {
    installFetch(twoProfileConfig());
    renderSheet(twoProfileConfig());

    const details = advancedOf('models');
    const summary = details.querySelector('summary') as HTMLElement;
    // It reads as a disclosure, with a line saying what is inside rather than a
    // bare "Advanced" that hides an unknown amount of setting.
    expect(summary.textContent).toContain('Advanced');
    expect(summary.textContent).toContain('Sampling');

    fireEvent.click(summary);
    expect(details.open).toBe(true);
    fireEvent.click(summary);
    expect(details.open).toBe(false);
  });

  it('takes the user to the tab and disclosure a rejected field lives in', async () => {
    const config = twoProfileConfig();
    const { patches } = installFetch(config);
    renderSheet(config);

    // A subagent's turn ceiling is validated but only exists inside the Agent
    // tab's Advanced block, so a rejected value there has to travel to it.
    await settleOnTab('Agent', 'Max concurrent', config);
    await openAdvanced();
    fireEvent.input(screen.getByLabelText('Turns per subagent (0 = no ceiling)'), {
      target: { value: '10001' },
    });
    // Start the save from a different tab, which is the real situation: the
    // footer is shared, so a user can press Save from anywhere.
    await openTab('General');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    // The error lands, said in the footer (visible from every tab) …
    expect(await screen.findByText(/1 setting needs attention in Agent/)).not.toBeNull();
    // … and the sheet has moved to the field, with its disclosure open.
    expect(screen.getByRole('tab', { name: 'Agent' }).getAttribute('aria-selected')).toBe('true');
    expect(advancedOf('agent').open).toBe(true);
    expect(await screen.findByText('Iterations must be 0 (no ceiling) or 1–10000.')).not.toBeNull();
    expect(patches).toHaveLength(0);
  });

  it('does not open Advanced for a field that is on the tab already', async () => {
    const config = twoProfileConfig();
    installFetch(config);
    renderSheet(config);

    await settleOnTab('Agent', 'Max concurrent', config);
    // The approval timeout is on the Agent tab but *outside* its fold, next to
    // the approval mode. Revealing it must not expand anything.
    fireEvent.input(screen.getByLabelText('Approval timeout (s)'), { target: { value: '0' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(await screen.findByText('Approval timeout must be 1–86400 seconds.')).not.toBeNull();
    expect(advancedOf('agent').open).toBe(false);
  });
});
/* ------------------------------------------------------------------ *
 * Plugin lifecycle: reload, enable/disable and rescan without a restart.
 * ------------------------------------------------------------------ */

function pluginDiagnostics(name: string, status = 'ready'): PluginDiagnostics {
  return {
    name,
    status,
    manifest: { name, version: '0.1.0', protocol: '1' },
    tools: [],
    error: null,
    pid: 4242,
    restart_count: 0,
  };
}

describe('SettingsSheet plugins', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('reloads one plugin and re-reads the diagnostics', async () => {
    const plugins = [pluginDiagnostics('wordy')];
    const { pluginPosts, pluginListCalls } = installFetch(twoProfileConfig(), { plugins });
    renderSheet(twoProfileConfig());

    await openTab('Plugins & MCP');
    const row = (await screen.findByText('wordy')).closest('.diagnostics-row');
    if (!(row instanceof HTMLElement)) throw new Error('no plugin row');
    const before = pluginListCalls();

    fireEvent.click(within(row).getByRole('button', { name: 'Reload' }));

    // Exactly the one plugin is reloaded, through its own route.
    await waitFor(() => expect(pluginPosts).toEqual(['/api/plugins/wordy/reload']));
    // The list is fetched again: the row must not keep showing the old process.
    await waitFor(() => expect(pluginListCalls()).toBeGreaterThan(before));
  });

  it('rescans every plugin source with one call', async () => {
    const { pluginPosts } = installFetch(twoProfileConfig(), {
      plugins: [pluginDiagnostics('wordy')],
    });
    renderSheet(twoProfileConfig());

    await openTab('Plugins & MCP');
    fireEvent.click(await screen.findByRole('button', { name: 'Rescan plugins' }));

    await waitFor(() => expect(pluginPosts).toEqual(['/api/plugins/reload']));
  });

  it('re-reads the config after a lifecycle action, so new contributions appear', async () => {
    // A plugin's tabs, buttons and hides are served by `/api/config`. Reloading a
    // process tells the sheet nothing about them, so without a config re-read,
    // installing a plugin leaves its controls invisible until a page reload.
    const { configReads, pluginPosts } = installFetch(twoProfileConfig(), {
      plugins: [pluginDiagnostics('wordy')],
    });
    renderSheet(twoProfileConfig());

    await openTab('Plugins & MCP');
    await screen.findByRole('button', { name: 'Rescan plugins' });
    const before = configReads();

    fireEvent.click(screen.getByRole('button', { name: 'Rescan plugins' }));

    await waitFor(() => expect(pluginPosts).toEqual(['/api/plugins/reload']));
    await waitFor(() => expect(configReads()).toBeGreaterThan(before));
  });

  it('disables a running plugin and enables a disabled one', async () => {
    const { pluginPosts } = installFetch(twoProfileConfig(), {
      plugins: [pluginDiagnostics('wordy'), pluginDiagnostics('muted', 'disabled')],
    });
    renderSheet(twoProfileConfig());

    await openTab('Plugins & MCP');
    const running = (await screen.findByText('wordy')).closest('.diagnostics-row');
    const stopped = screen.getByText('muted').closest('.diagnostics-row');
    if (!(running instanceof HTMLElement) || !(stopped instanceof HTMLElement)) {
      throw new Error('plugin rows did not render');
    }

    fireEvent.click(within(running).getByRole('button', { name: 'Disable' }));
    await waitFor(() => expect(pluginPosts).toEqual(['/api/plugins/wordy/enable']));

    // A disabled plugin offers the opposite action, never "Reload" by itself.
    fireEvent.click(within(stopped).getByRole('button', { name: 'Enable' }));
    await waitFor(() =>
      expect(pluginPosts).toEqual([
        '/api/plugins/wordy/enable',
        '/api/plugins/muted/enable',
      ]),
    );
  });

  it('reports the watcher state so an operator knows whether to click', async () => {
    installFetch({ ...twoProfileConfig(), plugins_watch: true, plugins_watch_secs: 3 });
    renderSheet({ ...twoProfileConfig(), plugins_watch: true, plugins_watch_secs: 3 });

    await openTab('Plugins & MCP');
    expect(
      await screen.findByText(/Plugin sources are watched every 3s; edits apply without a click\./),
    ).not.toBeNull();
  });
});
