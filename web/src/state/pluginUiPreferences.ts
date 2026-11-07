import { signal } from '@preact/signals';
import { NO_UI_PREFERENCES, type PluginUiPreferences } from '../components/uiContributions';

/**
 * The person's overrides for what plugins draw, persisted per browser.
 *
 * A plugin can add a control and can ask the app to hide one of its own. Without
 * this, the only way out of a control somebody does not want — or of a built-in
 * a plugin took away — is disabling the whole plugin, which throws away its tools
 * and hooks too. So the overrides live here: one on/off per contributed control,
 * one "draw it again" per hidden built-in.
 *
 * Stored in `localStorage` rather than the server's config on purpose: it is a
 * preference about *this* browser, exactly like the theme and the rail state, and
 * it needs no approval, no round trip and no config reload. The trade-off, which
 * the settings screen states, is that it does not follow you to another machine.
 */

export const PLUGIN_UI_PREFERENCES_KEY = 'karucode.plugin-ui.v1';

/** Cap on stored entries, so a corrupted or hostile value cannot grow forever. */
const MAX_ENTRIES = 256;

function readList(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const entry of raw.slice(0, MAX_ENTRIES)) {
    if (typeof entry === 'string' && entry.trim() !== '' && !out.includes(entry)) {
      out.push(entry);
    }
  }
  return out;
}

/**
 * Read the stored overrides.
 *
 * Never throws and never trusts what it finds: a value written by an older build,
 * a half-finished migration or a curious user becomes "no overrides" rather than a
 * rendering bug.
 */
export function readPluginUiPreferences(storage: Storage | undefined = safeStorage()): PluginUiPreferences {
  if (!storage) return NO_UI_PREFERENCES;
  try {
    const text = storage.getItem(PLUGIN_UI_PREFERENCES_KEY);
    if (!text) return NO_UI_PREFERENCES;
    const parsed: unknown = JSON.parse(text);
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return NO_UI_PREFERENCES;
    }
    const record = parsed as Record<string, unknown>;
    return {
      disabled: readList(record.disabled),
      shown: readList(record.shown),
    };
  } catch {
    return NO_UI_PREFERENCES;
  }
}

function safeStorage(): Storage | undefined {
  try {
    return typeof localStorage === 'undefined' ? undefined : localStorage;
  } catch {
    // A browser with storage disabled throws on access, not just on write.
    return undefined;
  }
}

function writePluginUiPreferences(
  prefs: PluginUiPreferences,
  storage: Storage | undefined = safeStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(PLUGIN_UI_PREFERENCES_KEY, JSON.stringify(prefs));
  } catch {
    // Storage full or read-only: the in-memory signal still applies for this
    // session, which is better than refusing the click.
  }
}

/** The live overrides. Components read this through `store.pluginUi`. */
export const pluginUiPreferences = signal<PluginUiPreferences>(readPluginUiPreferences());

function commit(next: PluginUiPreferences): void {
  pluginUiPreferences.value = next;
  writePluginUiPreferences(next);
}

/** Switch one contributed control on or off (`hosted_id` from the payload). */
export function setContributionEnabled(hostedId: string, enabled: boolean): void {
  const current = pluginUiPreferences.value;
  const isDisabled = current.disabled.includes(hostedId);
  if (enabled ? !isDisabled : isDisabled) return;
  const disabled = enabled
    ? current.disabled.filter((id) => id !== hostedId)
    : [...current.disabled, hostedId];
  commit({ ...current, disabled });
}

/**
 * Override a plugin's hide request for one built-in control.
 *
 * `overridden: true` means "draw it again": the app stops honouring the plugin's
 * request for that target until this is cleared.
 */
export function setHideOverridden(target: string, overridden: boolean): void {
  const current = pluginUiPreferences.value;
  const already = current.shown.includes(target);
  if (already === overridden) return;
  const shown = overridden
    ? [...current.shown, target]
    : current.shown.filter((name) => name !== target);
  commit({ ...current, shown });
}

/** Clear every override, back to "whatever the plugins declared". */
export function resetPluginUiPreferences(): void {
  commit(NO_UI_PREFERENCES);
}

/**
 * True when the person has overridden at least one thing.
 *
 * The settings screen uses it to decide whether a "Reset" action is worth
 * offering.
 */
export function hasUiOverrides(prefs: PluginUiPreferences): boolean {
  return prefs.disabled.length > 0 || prefs.shown.length > 0;
}
