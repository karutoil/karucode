import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import { PluginSlot } from './PluginSlot';
import {
  UI_BADGE_SLOTS,
  UI_SLOT_COMPOSER_TOOLBAR,
  UI_SLOT_INSPECTOR_HEADER,
  UI_SLOT_SESSION_HEADER_CHIP,
} from './uiContributions';

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

/**
 * Chips: a `session.row.badge` / `session.header.chip` entry carries `text` and
 * `tone`, not a `label`. The regression this guards is the drawn word coming from
 * the wrong field — a chip whose payload had only `text`+`tone` used to be refused
 * by the parser (its `label` was empty) and, before that, to render an empty
 * button.
 *
 * `UI_BADGE_SLOTS` is asserted against, not restated: the slots these tests use
 * are chips only for as long as the catalogue says so.
 */
describe('plugin-contributed chips', () => {
  beforeEach(() => {
    resetAppState();
    runPluginUiAction.mockReset();
    sendMessage.mockReset();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  function chip(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      plugin: 'kc-focus',
      version: '0.1.0',
      kind: 'button',
      id: 'focus',
      hosted_id: 'plugin__kc-focus__focus',
      slot: UI_SLOT_SESSION_HEADER_CHIP,
      // Deliberately no `label`: the host sends only what the plugin declared
      // here, and a chip's content is `text`.
      text: 'Focus 25m',
      tone: 'success',
      message: 'Start a focus block.',
      ...overrides,
    };
  }

  it('draws a chip from text and tone when it has no label at all', () => {
    expect(UI_BADGE_SLOTS).toContain(UI_SLOT_SESSION_HEADER_CHIP);
    withUi({ contributions: [chip()] });
    const { container } = render(<PluginSlot slot={UI_SLOT_SESSION_HEADER_CHIP} />);

    const control = screen.getByRole('button', { name: 'Focus 25m' });
    expect(control.textContent).toContain('Focus 25m');
    expect(control.classList.contains('session-chip')).toBe(true);
    expect(control.classList.contains('session-chip-success')).toBe(true);
    expect(control.getAttribute('data-tone')).toBe('success');
    // The group is labelled for chips, not for controls that are not there.
    expect(container.querySelector('.plugin-slot')?.getAttribute('aria-label')).toBe(
      'Plugin chips',
    );
  });

  it('draws the tail of a 24-byte chip text the label cap would have refused', () => {
    // `MAX_UI_BADGE_BYTES` is 24 and `MAX_UI_LABEL_CHARS` 80, so this is inside the
    // host's own limit; the parser must not be the thing that drops it.
    const text = 'a'.repeat(24);
    withUi({ contributions: [chip({ text })] });
    render(<PluginSlot slot={UI_SLOT_SESSION_HEADER_CHIP} />);
    expect(screen.getByRole('button', { name: text })).toBeTruthy();
  });

  it('keeps the plugin text when the label is a different accessible name', () => {
    withUi({ contributions: [chip({ label: 'Focus session in progress' })] });
    const { container } = render(<PluginSlot slot={UI_SLOT_SESSION_HEADER_CHIP} />);
    // Drawn: the chip text. Announced: the label, which is the fuller sentence.
    expect(container.querySelector('.plugin-slot-label')?.textContent).toBe('Focus 25m');
    expect(screen.getByRole('button', { name: 'Focus session in progress' })).toBeTruthy();
  });

  it('falls back to a neutral tone instead of an empty class', () => {
    withUi({ contributions: [chip({ tone: 'not-a-tone' })] });
    const { container } = render(<PluginSlot slot={UI_SLOT_SESSION_HEADER_CHIP} />);
    const control = container.querySelector('.plugin-slot-button');
    expect(control?.classList.contains('session-chip-neutral')).toBe(true);
    expect(control?.className).not.toContain('session-chip- ');
  });

  it('runs an action from a chip that has one', async () => {
    runPluginUiAction.mockResolvedValue({ ok: true, message: 'Focused', refresh: [], open_url: null });
    withUi({ contributions: [chip({ message: null, action: 'focus' })] });
    render(<PluginSlot slot={UI_SLOT_SESSION_HEADER_CHIP} />);

    fireEvent.click(screen.getByRole('button', { name: 'Focus 25m' }));
    await waitFor(() => expect(runPluginUiAction).toHaveBeenCalledTimes(1));
    expect(runPluginUiAction.mock.calls[0]?.slice(0, 2)).toEqual(['kc-focus', 'focus']);
  });
});
