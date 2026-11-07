import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import { CommandPalette } from './CommandPalette';

/**
 * A plugin's palette command is a real palette row, not a special case.
 *
 * It has to sort, filter, respond to arrow keys and run on Enter exactly like a
 * built-in entry — a second, plugin-only list would drift the moment somebody
 * touched the palette. It also names its plugin, because an action a person did
 * not expect should be attributable *before* it runs.
 */

const runPluginUiAction = vi.fn();

vi.mock('../api/endpoints', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/endpoints')>();
  return {
    ...actual,
    runPluginUiAction: (...args: unknown[]) => runPluginUiAction(...args),
  };
});

function openPaletteWithCommand(): void {
  store.config.value = {
    ...configFixture,
    plugin_ui: {
      contributions: [
        {
          plugin: 'kc-todo',
          version: '0.2.0',
          kind: 'command',
          id: 'plan',
          hosted_id: 'plugin__kc-todo__plan',
          label: 'Todo: summarise the plan',
          title: 'Ask the agent about the plan',
          message: 'Summarise the plan.',
        },
      ],
    } as never,
  };
  store.paletteOpen.value = true;
  render(<CommandPalette />);
}

describe('plugin commands in the palette', () => {
  beforeEach(() => {
    resetAppState();
    runPluginUiAction.mockReset();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  it('lists a contributed command under Plugins, naming its plugin', () => {
    openPaletteWithCommand();
    const option = screen.getByRole('option', { name: /Todo: summarise the plan/ });
    expect(option.textContent).toContain('kc-todo');
    expect(screen.getByText('Plugins')).toBeTruthy();
  });

  it('filters and runs it like any other row', async () => {
    openPaletteWithCommand();
    const input = screen.getByRole('combobox', { name: 'Search commands' });

    fireEvent.input(input, { target: { value: 'summarise' } });
    await waitFor(() =>
      expect(screen.queryByRole('option', { name: /New session/ })).toBeNull(),
    );

    // Enter runs the plugin's contribution: here a `message` one, so it posts a
    // user turn rather than an action.
    store.currentSessionId.value = 's_1';
    const fetchSpy = vi.fn(async () =>
      new Response(JSON.stringify({ run_id: 'r_1' }), {
        status: 202,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchSpy);
    try {
      fireEvent.keyDown(input, { key: 'Enter' });
      await waitFor(() => expect(fetchSpy).toHaveBeenCalled());
      const body = (fetchSpy.mock.calls[0] as unknown as [string, RequestInit])[1]?.body;
      expect(String(body)).toContain('Summarise the plan.');
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('says so when a contributed command needs a session', async () => {
    openPaletteWithCommand();
    store.currentSessionId.value = null;
    fireEvent.click(screen.getByRole('option', { name: /Todo: summarise the plan/ }));
    await waitFor(() =>
      expect(store.toasts.value.map((toast) => toast.message).join(' ')).toContain(
        'Open a session',
      ),
    );
  });
});
