import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  applyUiPreferences,
  isHideOverridden,
  isUiHidden,
  NO_UI_PREFERENCES,
  parsePluginUi,
  slotContributions,
  UI_HIDE_INSPECTOR_TAB_AGENTS,
  UI_SLOT_COMPOSER_TOOLBAR,
} from '../components/uiContributions';
import {
  hasUiOverrides,
  PLUGIN_UI_PREFERENCES_KEY,
  pluginUiPreferences,
  readPluginUiPreferences,
  resetPluginUiPreferences,
  setContributionEnabled,
  setHideOverridden,
} from './pluginUiPreferences';

/**
 * The person's overrides.
 *
 * Two things must hold, and they are the reason this suite exists rather than a
 * couple of assertions inside the settings test: a stored value is never trusted
 * (a corrupted entry must not break the interface), and applying a preference must
 * be a pure transform of the same model components already read, so no component
 * has to learn a second way of being told "not now".
 */

const PAYLOAD = {
  contributions: [
    {
      plugin: 'kc-focus',
      version: '1.0.0',
      kind: 'button',
      id: 'focus',
      hosted_id: 'plugin__kc-focus__focus',
      slot: UI_SLOT_COMPOSER_TOOLBAR,
      label: 'Focus mode',
      action: 'focus',
    },
    {
      plugin: 'kc-todo',
      version: '0.2.0',
      kind: 'command',
      id: 'plan',
      hosted_id: 'plugin__kc-todo__plan',
      label: 'Ask about the plan',
      message: 'Summarise the plan.',
    },
  ],
  hidden: [{ target: UI_HIDE_INSPECTOR_TAB_AGENTS, plugins: ['kc-focus'] }],
};

/** A storage stand-in, so these tests never touch the jsdom global. */
function memoryStorage(seed: Record<string, string> = {}): Storage {
  const map = new Map(Object.entries(seed));
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key: string) => map.get(key) ?? null,
    key: (index: number) => [...map.keys()][index] ?? null,
    removeItem: (key: string) => void map.delete(key),
    setItem: (key: string, value: string) => void map.set(key, value),
  } as Storage;
}

describe('reading stored overrides', () => {
  it('is empty when nothing was ever stored', () => {
    expect(readPluginUiPreferences(memoryStorage())).toEqual(NO_UI_PREFERENCES);
    expect(readPluginUiPreferences(undefined)).toEqual(NO_UI_PREFERENCES);
  });

  it('never trusts what it finds', () => {
    for (const stored of [
      'not json',
      'null',
      '[]',
      '"a string"',
      JSON.stringify({ disabled: 'nope', shown: 42 }),
      JSON.stringify({ disabled: [1, 2, null], shown: ['', '  '] }),
    ]) {
      const prefs = readPluginUiPreferences(memoryStorage({ [PLUGIN_UI_PREFERENCES_KEY]: stored }));
      expect(prefs.disabled, stored).toEqual([]);
      expect(prefs.shown, stored).toEqual([]);
    }
  });

  it('keeps well-formed entries and de-duplicates them', () => {
    const prefs = readPluginUiPreferences(
      memoryStorage({
        [PLUGIN_UI_PREFERENCES_KEY]: JSON.stringify({
          disabled: ['plugin__a__b', 'plugin__a__b', 'plugin__c__d'],
          shown: [UI_HIDE_INSPECTOR_TAB_AGENTS],
        }),
      }),
    );
    expect(prefs.disabled).toEqual(['plugin__a__b', 'plugin__c__d']);
    expect(prefs.shown).toEqual([UI_HIDE_INSPECTOR_TAB_AGENTS]);
  });

  it('caps the list so a huge value cannot grow forever', () => {
    const many = Array.from({ length: 400 }, (_, index) => `plugin__p__${index}`);
    const prefs = readPluginUiPreferences(
      memoryStorage({ [PLUGIN_UI_PREFERENCES_KEY]: JSON.stringify({ disabled: many }) }),
    );
    expect(prefs.disabled.length).toBeLessThanOrEqual(256);
  });
});

describe('the override setters', () => {
  beforeEach(() => {
    resetPluginUiPreferences();
  });

  afterEach(() => {
    resetPluginUiPreferences();
  });

  it('switches one control off and back on', () => {
    setContributionEnabled('plugin__kc-focus__focus', false);
    expect(pluginUiPreferences.value.disabled).toEqual(['plugin__kc-focus__focus']);
    // Idempotent: a second click on the same state changes nothing.
    setContributionEnabled('plugin__kc-focus__focus', false);
    expect(pluginUiPreferences.value.disabled).toEqual(['plugin__kc-focus__focus']);

    setContributionEnabled('plugin__kc-focus__focus', true);
    expect(pluginUiPreferences.value.disabled).toEqual([]);
  });

  it('overrides one hide and clears the override again', () => {
    setHideOverridden(UI_HIDE_INSPECTOR_TAB_AGENTS, true);
    expect(isHideOverridden(pluginUiPreferences.value, UI_HIDE_INSPECTOR_TAB_AGENTS)).toBe(true);
    setHideOverridden(UI_HIDE_INSPECTOR_TAB_AGENTS, false);
    expect(pluginUiPreferences.value.shown).toEqual([]);
  });

  it('reports whether anything is overridden', () => {
    expect(hasUiOverrides(pluginUiPreferences.value)).toBe(false);
    setContributionEnabled('plugin__kc-focus__focus', false);
    expect(hasUiOverrides(pluginUiPreferences.value)).toBe(true);
    resetPluginUiPreferences();
    expect(hasUiOverrides(pluginUiPreferences.value)).toBe(false);
    expect(pluginUiPreferences.value).toEqual(NO_UI_PREFERENCES);
  });
});

describe('applying overrides to a payload', () => {
  const declared = parsePluginUi(PAYLOAD);

  it('changes nothing when there are no overrides', () => {
    expect(applyUiPreferences(declared, NO_UI_PREFERENCES)).toBe(declared);
  });

  it('drops a switched-off contribution from its slot and from the palette', () => {
    const ui = applyUiPreferences(declared, { disabled: ['plugin__kc-focus__focus'], shown: [] });
    expect(slotContributions(ui, UI_SLOT_COMPOSER_TOOLBAR)).toEqual([]);
    // The other plugin's command is untouched.
    expect(ui.commands.map((c) => c.id)).toEqual(['plan']);
    // The full list is still one entry shorter, so nothing downstream counts twice.
    expect(ui.contributions).toHaveLength(1);
  });

  it('draws a hidden built-in again when the person overrode the hide', () => {
    const ui = applyUiPreferences(declared, {
      disabled: [],
      shown: [UI_HIDE_INSPECTOR_TAB_AGENTS],
    });
    expect(isUiHidden(ui, UI_HIDE_INSPECTOR_TAB_AGENTS)).toBe(false);
    // A target that was not overridden is still hidden.
    const partial = applyUiPreferences(declared, { disabled: [], shown: ['composer.attach'] });
    expect(isUiHidden(partial, UI_HIDE_INSPECTOR_TAB_AGENTS)).toBe(true);
  });

  it('keeps the catalogues, which describe the app and not the plugins', () => {
    const ui = applyUiPreferences(declared, { disabled: ['plugin__kc-focus__focus'], shown: [] });
    expect(ui.slots).toEqual(declared.slots);
    expect(ui.hideable).toEqual(declared.hideable);
  });
});
