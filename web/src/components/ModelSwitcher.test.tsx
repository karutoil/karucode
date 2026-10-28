import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { ModelSwitcher } from './ModelSwitcher';
import type { Config } from '../api/types';
import { jsonResponse, resetAppState } from '../test/fixtures';
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
          { id: 'model-a', name: 'Model A', context_window: 128000 },
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
        models: [{ id: 'tiny', name: 'Tiny' }],
        request_timeout_secs: 120,
        api_key_set: false,
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
    },
    approvals: { mode: 'ask', allow_commands: [], auto_approve_paths: [], timeout_secs: 300 },
    subagents: {
      max_concurrent: 3,
      max_iterations: 16,
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

describe('ModelSwitcher', () => {
  beforeEach(() => {
    resetAppState();
    store.config.value = twoProfileConfig();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('shows the active model with its provider and lists every profile', () => {
    render(<ModelSwitcher />);
    const button = screen.getByRole('button', { name: /Active model: Model B on Gateway/i });
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(screen.getByText('Model B')).not.toBeNull();
    expect(screen.getByText('Gateway')).not.toBeNull();

    fireEvent.click(button);
    expect(button.getAttribute('aria-expanded')).toBe('true');
    const options = screen.getAllByRole('option');
    expect(options.map((option) => option.textContent)).toEqual([
      expect.stringContaining('Model A'),
      expect.stringContaining('Model B'),
      expect.stringContaining('Tiny'),
    ]);
    expect(options[1]?.getAttribute('aria-selected')).toBe('true');
    expect(options[0]?.getAttribute('aria-selected')).toBe('false');
    // A profile without a key is called out instead of pretending it works.
    expect(screen.getByText('no key')).not.toBeNull();
  });

  it('switches model through PATCH /api/config and closes the menu', async () => {
    const next = twoProfileConfig();
    next.active = { provider: 'local', model: 'tiny' };
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (url === '/api/config' && init?.method === 'PATCH') return jsonResponse(next);
      if (url === '/api/health') return jsonResponse({ status: 'ok' });
      return jsonResponse({ error: { code: 'not_found', message: `unmocked ${url}` } }, 404);
    });
    vi.stubGlobal('fetch', fetchMock);

    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Active model: Model B/i }));
    fireEvent.click(screen.getByRole('option', { name: /Tiny/ }));

    await waitFor(() => {
      expect(store.config.value?.active).toEqual({ provider: 'local', model: 'tiny' });
    });
    const patch = fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');
    expect(patch).toBeDefined();
    expect(JSON.parse(String(patch?.[1]?.body))).toEqual({
      active: { provider: 'local', model: 'tiny' },
    });
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(store.toasts.value.some((toast) => toast.message.includes('Tiny'))).toBe(true);
  });

  it('closes on Escape without changing the model', async () => {
    const fetchMock = vi.fn(async () => jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);
    render(<ModelSwitcher />);
    fireEvent.click(screen.getByRole('button', { name: /Active model: Model B/i }));
    expect(screen.queryByRole('listbox')).not.toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(fetchMock).not.toHaveBeenCalled();
    expect(store.config.value?.active).toEqual({ provider: 'gateway', model: 'model-b' });
  });
});