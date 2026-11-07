import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import { pluginUiPreferences, resetPluginUiPreferences } from '../state/pluginUiPreferences';
import { PluginControls } from './PluginControls';
import { isUiHidden, UI_HIDE_INSPECTOR_TAB_AGENTS, UI_SLOT_COMPOSER_TOOLBAR } from './uiContributions';

/**
 * Settings → Controls in the interface.
 *
 * The panel is the only way back from either direction of a plugin's request, so
 * what matters is that the *declared* list is what it draws (a switched-off
 * control must keep its switch), that a switch changes the effective view every
 * component reads, and that a hidden built-in can be put back.
 */

function withUi(ui: unknown): void {
  store.config.value = { ...configFixture, plugin_ui: ui as never };
}

const BUTTON = {
  plugin: 'kc-focus',
  version: '1.0.0',
  kind: 'button',
  id: 'focus',
  hosted_id: 'plugin__kc-focus__focus',
  slot: UI_SLOT_COMPOSER_TOOLBAR,
  label: 'Focus mode',
  action: 'focus',
};

const COMMAND = {
  plugin: 'kc-todo',
  version: '0.2.0',
  kind: 'command',
  id: 'plan',
  hosted_id: 'plugin__kc-todo__plan',
  label: 'Ask about the plan',
  message: 'Summarise the plan.',
};

const HIDDEN = { target: UI_HIDE_INSPECTOR_TAB_AGENTS, plugins: ['kc-focus'] };

describe('controls in the interface', () => {
  beforeEach(() => {
    resetAppState();
    resetPluginUiPreferences();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
    resetPluginUiPreferences();
  });

  it('says so when no plugin contributes anything', () => {
    withUi(undefined);
    render(<PluginControls />);
    expect(screen.getByText(/No plugin adds controls to the interface/)).toBeTruthy();
    expect(screen.queryByRole('switch')).toBeNull();
  });

  it('lists each contributed control under the plugin that made it', () => {
    withUi({ contributions: [BUTTON, COMMAND], hidden: [HIDDEN] });
    render(<PluginControls />);

    expect(screen.getByText('kc-focus')).toBeTruthy();
    expect(screen.getByText('kc-todo')).toBeTruthy();
    const focus = screen.getByRole('switch', { name: 'Focus mode' });
    expect(focus.getAttribute('aria-checked')).toBe('true');
    expect(screen.getByText(/kc-focus · button · composer/)).toBeTruthy();
    expect(screen.getByText(/kc-todo · command · command palette/)).toBeTruthy();
  });

  it('switches a control off without removing the switch that brings it back', () => {
    withUi({ contributions: [BUTTON], hidden: [] });
    render(<PluginControls />);

    fireEvent.click(screen.getByRole('switch', { name: 'Focus mode' }));

    // The override applies to the effective view every component draws from…
    expect(store.pluginUi.value.contributions).toEqual([]);
    expect(store.pluginUi.value.bySlot[UI_SLOT_COMPOSER_TOOLBAR]).toBeUndefined();
    // …while the settings list still describes what the plugin declared.
    expect(store.pluginUiDeclared.value.contributions).toHaveLength(1);
    expect(screen.getByRole('switch', { name: 'Focus mode' }).getAttribute('aria-checked')).toBe(
      'false',
    );

    fireEvent.click(screen.getByRole('switch', { name: 'Focus mode' }));
    expect(store.pluginUi.value.contributions).toHaveLength(1);
    expect(pluginUiPreferences.value.disabled).toEqual([]);
  });

  it('puts a built-in back that a plugin hid, naming the plugin', () => {
    withUi({ contributions: [], hidden: [HIDDEN] });
    render(<PluginControls />);

    expect(store.pluginUi.value.hidden[UI_HIDE_INSPECTOR_TAB_AGENTS]).toEqual(['kc-focus']);
    const restore = screen.getByRole('switch', { name: 'Show the Agents tab again' });
    expect(restore.getAttribute('aria-checked')).toBe('false');
    expect(screen.getByText('Hidden by kc-focus')).toBeTruthy();

    fireEvent.click(restore);

    expect(isUiHidden(store.pluginUi.value, UI_HIDE_INSPECTOR_TAB_AGENTS)).toBe(false);
    expect(pluginUiPreferences.value.shown).toEqual([UI_HIDE_INSPECTOR_TAB_AGENTS]);
  });

  it('offers a reset only once something is overridden, and clears it', () => {
    withUi({ contributions: [BUTTON], hidden: [HIDDEN] });
    render(<PluginControls />);
    const reset = () => screen.queryByRole('button', { name: /Reset to what the plugins ask for/ });
    expect(reset()).toBeNull();

    fireEvent.click(screen.getByRole('switch', { name: 'Focus mode' }));
    expect(reset()).toBeTruthy();
    fireEvent.click(reset() as HTMLElement);

    expect(pluginUiPreferences.value).toEqual({ disabled: [], shown: [] });
    expect(screen.getByRole('switch', { name: 'Focus mode' }).getAttribute('aria-checked')).toBe(
      'true',
    );
  });
});
