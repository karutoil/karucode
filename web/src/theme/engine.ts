// @kc-doc: themes
// @kc-title: Theme the workspace
// @kc-summary: Built-in palettes, live colour and density editing, fonts and custom CSS, saved in the browser and shareable as a theme JSON file.
// @kc-tags: theme, themes, appearance, colors, density, fonts, css, customization, ui
// @kc-applies-to: ui:ThemeStudio, storage:karucode.theme-studio.v3, file:web/src/theme/engine.ts, file:web/src/components/ThemeStudio.tsx, cmd:npm run dev
//
// # Open the Theme studio
//
// Three entry points, all opening the same right-hand drawer:
//
// * the palette button at the bottom of the project rail;
// * the theme button in the workspace header (it shows the active theme name);
// * `Ctrl/Cmd+Shift+T`, or Settings → **Appearance**.
//
// The drawer edits a live preview: colours, density and custom CSS apply to the
// whole workspace immediately, before you save anything.
//
// # What a theme controls
//
// * **Palette** — 8 built-in themes (Ember, Nocturne, Forest, Sakura, Graphite,
//   Daybreak, Paper, Mint) plus 18 editable colours: canvas, project rail,
//   sidebar, inspector, cards, elevated surface, hover surface, borders, primary
//   and secondary text, accent, success, danger, info, warning and three syntax
//   colours. Separators, muted text, accent tints and shadows are derived from
//   those 18, so every surface stays coherent instead of drifting.
// * **Interface** — dark or light mode, `compact` / `comfortable` / `spacious`
//   density (spacing only), an interface font and a code font chosen from
//   locally available stacks (nothing is downloaded), and a 0–22 px corner
//   radius.
// * **Advanced** — custom CSS applied live after the application stylesheet, for
//   effects the controls do not cover.
//
// Every component reads CSS custom properties (`--theme-bg`, `--accent`, …), so
// a theme repaints the UI without a reload.
//
// # Saving, sharing and resetting
//
// The selection, your saved custom themes and the unsaved draft live in
// `localStorage` under `karucode.theme-studio.v3`. That is per browser profile:
// themes are not stored in the karucode config, do not follow you to another
// machine and are not shared between browsers. Up to 60 custom themes are kept.
//
// * **Save as new** stores the current draft under a name you type.
// * **Export** downloads `karucode-theme.json` (`format: "karucode-theme"`,
//   `version: 1`, plus `mode`, `density`, `font`, `mono`, `rounding`, `colors` and
//   `customCSS`).
// * **Import** reads such a file (maximum 150 KB) and rejects it with a toast if
//   a colour is not a 6-digit hex value, the mode/density/font is unknown, or the
//   file is larger than the limit.
// * **Reset colours** restores the palette of the theme you started from;
//   deleting a custom theme falls back to Ember. Clearing the storage key resets
//   everything to the defaults.
//
// # Gotchas
//
// * Custom CSS is injected verbatim — it is your own browser, but an imported
//   theme file can hide UI or load remote resources. Only import themes you
//   trust.
// * A light theme sets `color-scheme: light`, which changes native form controls
//   and scrollbars too; contrast is reported in the footer and warns below the
//   4.5:1 AA ratio for body text.
// * The engine writes `data-theme`, `data-mode` and `data-density` on `<html>`;
//   component CSS may branch on those, so a theme with a broken value can look
//   worse than the default without any error.

/* Theme engine entry point.
 *
 * The engine owns one applied theme. It:
 *   - resolves the palette into `--theme-*` custom properties on <html>,
 *   - sets `data-theme` / `data-mode` / `data-density`,
 *   - injects custom CSS into a dedicated <style> element,
 *   - persists the selection, the user's custom themes and the current draft.
 *
 * Public surface used by the rest of the app:
 *   `initTheme()` (main.tsx), `activeThemeName` (shell header),
 *   `activeTheme` (Theme Studio) and the mutators below (Theme Studio).
 * Nothing outside `src/theme` should read storage directly.
 */

import { computed, signal } from '@preact/signals';
import { contrastRatio } from './color';
import {
  BUILTIN_THEMES,
  DEFAULT_THEME,
  completeColors,
  modePreset,
} from './builtins';
import {
  MAX_CUSTOM_THEMES,
  clearPersistedState,
  loadPersistedState,
  makeThemeId,
  parseThemeFile,
  sanitizeCustomCSS,
  sanitizeThemeName,
  savePersistedState,
} from './storage';
import { buildTokenMap, clampRounding, resolveTheme } from './tokens';
import type {
  ResolvedTheme,
  Theme,
  ThemeColorsPatch,
  ThemeDensity,
  ThemeFont,
  ThemeMode,
  ThemeMono,
  ThemeOptionsPatch,
} from './types';

const CUSTOM_CSS_STYLE_ID = 'kc-theme-custom-css';

/** Id of the applied theme (`draft` while the palette has unsaved edits). */
export const activeThemeId = signal<string>(DEFAULT_THEME.id);
/** The user's saved themes, newest last. */
export const customThemes = signal<Theme[]>([]);
/** Saved theme the current draft was derived from; also the "reset" target. */
export const baseThemeId = signal<string>(DEFAULT_THEME.id);
/** True while the applied palette differs from any saved theme. */
export const draftEdited = signal<boolean>(false);
/** Set when the browser refuses to persist (private mode, quota). */
export const autosaveError = signal<string | null>(null);

const currentTheme = signal<Theme>(DEFAULT_THEME);

/** The applied theme with every derived token resolved. */
export const activeTheme = computed<ResolvedTheme>(() => resolveTheme(currentTheme.value));

/** Label for the shell header. Drafts are labelled rather than numbered. */
export const activeThemeName = computed<string>(() =>
  draftEdited.value ? 'Unsaved theme' : currentTheme.value.name,
);

/** Built-ins first, then the user's library. */
export const themeLibrary = computed<Theme[]>(() => [
  ...BUILTIN_THEMES,
  ...customThemes.value,
]);

export const themeContrast = computed<number>(() =>
  contrastRatio(activeTheme.value.colors.text, activeTheme.value.colors.bg),
);

export function isCustomThemeId(id: string): boolean {
  return customThemes.value.some((theme) => theme.id === id);
}

/** Find a saved theme by id; unknown ids fall back to Ember. */
export function getTheme(id: string): Theme {
  const builtin = BUILTIN_THEMES.find((theme) => theme.id === id);
  if (builtin) return builtin;
  const custom = customThemes.value.find((theme) => theme.id === id);
  if (custom) return custom;
  if (id === 'draft' && draftEdited.value) return currentTheme.value;
  return DEFAULT_THEME;
}

function lookupSavedTheme(id: string): Theme | undefined {
  return (
    BUILTIN_THEMES.find((theme) => theme.id === id) ??
    customThemes.value.find((theme) => theme.id === id)
  );
}

function ensureCustomCSSElement(): HTMLStyleElement | null {
  if (typeof document === 'undefined' || !document.head) return null;
  const existing = document.getElementById(CUSTOM_CSS_STYLE_ID);
  if (existing instanceof HTMLStyleElement) return existing;
  const style = document.createElement('style');
  style.id = CUSTOM_CSS_STYLE_ID;
  style.dataset.themeCustom = 'true';
  document.head.appendChild(style);
  return style;
}

/**
 * Paint a theme. Exported so tests and non-signal callers can force a repaint;
 * user-facing code goes through the mutators below.
 */
export function applyTheme(theme: Theme): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;

  for (const [property, value] of Object.entries(buildTokenMap(theme))) {
    root.style.setProperty(property, value);
  }

  root.dataset.theme = theme.id;
  root.dataset.mode = theme.mode;
  root.dataset.density = theme.density;
  root.style.colorScheme = theme.mode;

  const meta =
    document.querySelector('meta[name="theme-color"]') ??
    document.head?.appendChild(document.createElement('meta'));
  if (meta instanceof HTMLMetaElement) {
    if (!meta.getAttribute('name')) meta.setAttribute('name', 'theme-color');
    meta.setAttribute('content', theme.colors.bg);
  }

  const style = ensureCustomCSSElement();
  if (style) style.textContent = theme.customCSS;
}

function persist(): void {
  const dirty = draftEdited.value;
  const ok = savePersistedState({
    selected: dirty ? baseThemeId.value : currentTheme.value.id,
    baseId: baseThemeId.value,
    custom: customThemes.value,
    draft: dirty ? currentTheme.value : null,
  });
  autosaveError.value = ok ? null : 'Changes are live, but this browser blocked saving them.';
}

function commit(theme: Theme, dirty: boolean): void {
  currentTheme.value = theme;
  draftEdited.value = dirty;
  activeThemeId.value = theme.id;
  applyTheme(theme);
  persist();
}

/** Turn the applied theme into an editable draft, derived from the same base. */
function beginDraft(): Theme {
  const base = currentTheme.value;
  baseThemeId.value = base.id;
  return { ...base, id: 'draft', name: 'Untitled theme', description: undefined };
}

/**
 * Apply the persisted theme and paint it. Safe to call more than once (it
 * re-reads storage); `main.tsx` calls it exactly once, before the first render.
 */
export function initTheme(): void {
  const stored = loadPersistedState();

  if (stored) {
    customThemes.value = stored.custom;
    baseThemeId.value = stored.baseId;

    if (stored.draft) {
      draftEdited.value = true;
      currentTheme.value = stored.draft;
      activeThemeId.value = 'draft';
      applyTheme(stored.draft);
      autosaveError.value = null;
      return;
    }

    const selected = lookupSavedTheme(stored.selected) ?? DEFAULT_THEME;
    draftEdited.value = false;
    baseThemeId.value = selected.id;
    currentTheme.value = selected;
    activeThemeId.value = selected.id;
    applyTheme(selected);
    autosaveError.value = null;
    return;
  }

  customThemes.value = [];
  baseThemeId.value = DEFAULT_THEME.id;
  draftEdited.value = false;
  currentTheme.value = DEFAULT_THEME;
  activeThemeId.value = DEFAULT_THEME.id;
  applyTheme(DEFAULT_THEME);
  autosaveError.value = null;
}

/** Switch to a built-in or saved theme, discarding the draft. */
export function selectTheme(id: string): boolean {
  const theme = lookupSavedTheme(id);
  if (!theme) return false;
  baseThemeId.value = theme.id;
  commit(theme, false);
  return true;
}

/**
 * Edit the applied theme. The first edit promotes it to a draft; every edit is
 * applied and persisted immediately (the studio says "Autosaved" and means it).
 */
export function updateTheme(patch: ThemeOptionsPatch): void {
  const base = draftEdited.value ? currentTheme.value : beginDraft();
  const { colors: colorPatch, customCSS, rounding, name, ...rest } = patch;

  const next: Theme = {
    ...base,
    ...rest,
    colors: { ...base.colors, ...(colorPatch ?? {}) },
  };

  if (name !== undefined) {
    const cleanName = sanitizeThemeName(name);
    if (cleanName) next.name = cleanName;
  }
  if (customCSS !== undefined) next.customCSS = sanitizeCustomCSS(customCSS);
  if (rounding !== undefined) next.rounding = clampRounding(rounding);

  commit(next, true);
}

export function updateColors(colors: ThemeColorsPatch): void {
  updateTheme({ colors });
}

export function setThemeMode(mode: ThemeMode): void {
  if (currentTheme.value.mode === mode) return;
  const base = draftEdited.value ? currentTheme.value : beginDraft();
  const preset = modePreset(mode);
  // Keep the user's accent (the one colour that defines the identity), re-base
  // the surfaces so the result is legible in the new mode.
  const colors = {
    ...preset.colors,
    accent: base.colors.accent,
    syntaxKeyword: base.colors.accent,
  };
  commit({ ...base, mode, colors }, true);
}

export function setDensity(density: ThemeDensity): void {
  updateTheme({ density });
}

export function setFont(font: ThemeFont): void {
  updateTheme({ font });
}

export function setMono(mono: ThemeMono): void {
  updateTheme({ mono });
}

export function setRounding(rounding: number): void {
  updateTheme({ rounding });
}

export function setCustomCSS(customCSS: string): void {
  updateTheme({ customCSS });
}

/** Restore the palette of the theme the draft was derived from. */
export function resetColors(): void {
  const base = lookupSavedTheme(baseThemeId.value) ?? DEFAULT_THEME;
  // Nothing to undo: the applied theme already is the base palette.
  if (!draftEdited.value && currentTheme.value.id === base.id) return;
  updateTheme({ colors: base.colors });
}

function uniqueThemeName(name: string): string {
  const used = new Set(themeLibrary.value.map((theme) => theme.name.toLowerCase()));
  if (!used.has(name.toLowerCase())) return name;
  for (let index = 2; index < 100; index += 1) {
    const candidate = `${name} (${index})`.slice(0, 50);
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
  return name;
}

/** Save the applied palette as a new custom theme. Returns null for a blank name. */
export function saveThemeAsNew(name: string): Theme | null {
  const cleanName = sanitizeThemeName(name);
  if (!cleanName) return null;

  const saved: Theme = {
    ...currentTheme.value,
    id: makeThemeId(),
    name: uniqueThemeName(cleanName),
    description: currentTheme.value.description ?? 'Custom theme',
  };
  customThemes.value = [...customThemes.value, saved].slice(-MAX_CUSTOM_THEMES);
  baseThemeId.value = saved.id;
  commit(saved, false);
  return saved;
}

/** Delete a saved custom theme. Built-ins and drafts cannot be deleted. */
export function deleteCustomTheme(id?: string): boolean {
  const targetId = id ?? (draftEdited.value ? baseThemeId.value : currentTheme.value.id);
  const target = customThemes.value.find((theme) => theme.id === targetId);
  if (!target) return false;

  customThemes.value = customThemes.value.filter((theme) => theme.id !== targetId);

  if (currentTheme.value.id === targetId) {
    baseThemeId.value = DEFAULT_THEME.id;
    commit(DEFAULT_THEME, false);
  } else if (draftEdited.value && baseThemeId.value === targetId) {
    baseThemeId.value = DEFAULT_THEME.id;
    commit(currentTheme.value, true);
  } else {
    persist();
  }
  return true;
}

/** Validate and add a theme from an imported file. Throws `ThemeFileError`. */
export function importTheme(text: string): Theme {
  const parsed = parseThemeFile(text);
  const imported: Theme = {
    ...parsed,
    id: makeThemeId(),
    name: uniqueThemeName(parsed.name),
    colors: completeColors(parsed.colors, parsed.mode),
  };
  customThemes.value = [...customThemes.value, imported].slice(-MAX_CUSTOM_THEMES);
  baseThemeId.value = imported.id;
  commit(imported, false);
  return imported;
}

/** Drop every custom theme and return to Ember. */
export function resetTheme(): void {
  customThemes.value = [];
  baseThemeId.value = DEFAULT_THEME.id;
  draftEdited.value = false;
  currentTheme.value = DEFAULT_THEME;
  activeThemeId.value = DEFAULT_THEME.id;
  applyTheme(DEFAULT_THEME);
  clearPersistedState();
  autosaveError.value = null;
}