import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import { PluginSlot } from './PluginSlot';
import { UI_SLOT_COMPOSER_TOOLBAR, UI_SLOT_INSPECTOR_HEADER } from './uiContributions';

/**
 * A plugin's button, from click to toast.
 *
 * Three properties matter and each has a test here: the control is drawn by the
 * shell (no plugin markup), a click runs exactly one request, and the attribution
 * is on the control so nobody mistakes it for a built-in. The confirmation a
 * person sees comes from the plugin's own validated answer — a failed action must
 * never read as a success.
 */

const runPluginUiAction = vi.fn();
const sendMessage = vi.fn();

vi.mock('../api/endpoints', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/endpoints')>();
  return {
    ...actual,
    runPluginUiAction: (...args: unknown[]) => runPluginUiAction(...args),
    sendMessage: (...args: unknown[]) => sendMessage(...args),
  };
});

function withUi(ui: unknown): void {
  store.config.value = { ...configFixture, plugin_ui: ui as never };
}

function button(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plugin: 'kc-todo',
    version: '0.2.0',
    kind: 'button',
    id: 'add',
    hosted_id: 'plugin__kc-todo__add',
    slot: UI_SLOT_COMPOSER_TOOLBAR,
    label: 'New task',
    title: 'Add a task to this session',
    action: 'add',
    ...overrides,
  };
}

describe('plugin-contributed buttons', () => {
  beforeEach(() => {
    resetAppState();
    runPluginUiAction.mockReset();
    sendMessage.mockReset();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  it('draws nothing at all when no plugin asked for the slot', () => {
    withUi(undefined);
    const { container } = render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);
    expect(container.innerHTML).toBe('');
  });

  it('draws a labelled button with its attribution, not plugin markup', () => {
    withUi({ contributions: [button({ label: '<b>New</b> task' })] });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);

    const control = screen.getByRole('button', { name: '<b>New</b> task' });
    expect(control.getAttribute('data-plugin')).toBe('kc-todo');
    expect(control.getAttribute('data-contribution')).toBe('add');
    // The label is text: a plugin cannot inject markup through it.
    expect(control.textContent).toContain('<b>New</b> task');
    expect(control.querySelector('b')).toBeNull();
    // The tooltip names the plugin, so a control is always attributable.
    expect(control.getAttribute('title')).toContain('kc-todo');
  });

  it('runs one action per click and reports what the plugin said', async () => {
    runPluginUiAction.mockResolvedValue({
      ok: true,
      message: 'Task added',
      refresh: [],
      open_url: null,
    });
    withUi({ contributions: [button()] });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);

    fireEvent.click(screen.getByRole('button', { name: 'New task' }));

    await waitFor(() => expect(runPluginUiAction).toHaveBeenCalledTimes(1));
    expect(runPluginUiAction.mock.calls[0]?.slice(0, 2)).toEqual(['kc-todo', 'add']);
    await waitFor(() =>
      expect(store.toasts.value.map((toast) => toast.message)).toContain('Task added'),
    );
    expect(store.toasts.value.find((toast) => toast.message === 'Task added')?.tone).toBe(
      'success',
    );
  });

  it('shows a failed action as an error instead of a success', async () => {
    runPluginUiAction.mockResolvedValue({
      ok: false,
      message: 'No plan file in this workspace',
      refresh: [],
      open_url: null,
    });
    withUi({ contributions: [button()] });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);

    fireEvent.click(screen.getByRole('button', { name: 'New task' }));
    await waitFor(() =>
      expect(
        store.toasts.value.find((toast) => toast.message === 'No plan file in this workspace')
          ?.tone,
      ).toBe('error'),
    );
  });

  it('surfaces a transport failure with the plugin in the message', async () => {
    runPluginUiAction.mockRejectedValue(new Error('plugin went away'));
    withUi({ contributions: [button()] });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);

    fireEvent.click(screen.getByRole('button', { name: 'New task' }));
    await waitFor(() => {
      const toast = store.toasts.value.find((candidate) =>
        candidate.message.includes('New task'),
      );
      expect(toast?.tone).toBe('error');
      expect(toast?.message).toContain('plugin went away');
    });
  });

  it('sends a user turn for a message contribution, without an action', async () => {
    store.currentSessionId.value = 's_1';
    sendMessage.mockResolvedValue({ run_id: 'r_1' });
    withUi({
      contributions: [
        button({
          id: 'plan',
          hosted_id: 'plugin__kc-todo__plan',
          label: 'Ask',
          title: null,
          action: null,
          message: 'Summarise the plan.',
        }),
      ],
    });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);

    fireEvent.click(screen.getByRole('button', { name: 'Ask' }));
    await waitFor(() => expect(sendMessage).toHaveBeenCalledWith('s_1', 'Summarise the plan.'));
    expect(runPluginUiAction).not.toHaveBeenCalled();
  });

  it('only draws controls for the slot it was asked about', () => {
    withUi({
      contributions: [
        button(),
        button({
          id: 'ctx',
          hosted_id: 'plugin__kc-todo__ctx',
          label: 'Context thing',
          title: 'Context thing',
          slot: UI_SLOT_INSPECTOR_HEADER,
        }),
      ],
    });
    const { container } = render(<PluginSlot slot={UI_SLOT_INSPECTOR_HEADER} />);
    expect(container.querySelectorAll('.plugin-slot-button')).toHaveLength(1);
    expect(screen.getByRole('button', { name: /Context thing/ })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /New task/ })).toBeNull();
  });

  it('draws an icon name it knows and falls back for one it does not', () => {
    withUi({
      contributions: [
        button({ id: 'a', hosted_id: 'plugin__kc-todo__a', label: 'Known', title: 'Known', icon: 'bolt' }),
        button({
          id: 'b',
          hosted_id: 'plugin__kc-todo__b',
          label: 'Unknown',
          title: 'Unknown',
          icon: 'no-such-icon',
        }),
      ],
    });
    render(<PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />);
    // Both buttons exist: an unknown icon name is a neutral glyph, not a failure.
    expect(screen.getByRole('button', { name: /Known/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Unknown/ })).toBeTruthy();
  });
});
