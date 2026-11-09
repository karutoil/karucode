// @kc-doc: themes
// @kc-title: Theme the workspace
// @kc-summary: Built-in palettes, live colour and density editing, fonts and custom CSS, the transcript view options (cards or rows, grouped calls, working steps folded or open), saved in the browser and shareable as a theme JSON file.
// @kc-tags: theme, themes, appearance, colors, density, fonts, css, customization, ui, transcript, tool-calls, grouping, thinking, working-steps, show-the-working
// @kc-applies-to: ui:ThemeStudio, ui:ChatTranscript, ui:ToolCard, ui:ToolGroup, ui:ProcessFold, ui:ThinkingBlock, storage:karucode.theme-studio.v3, file:web/src/theme/engine.ts, file:web/src/theme/types.ts, file:web/src/state/transcriptLayout.ts, file:web/src/components/ThemeStudio.tsx, file:web/src/components/ChatTranscript.tsx, file:web/src/components/ToolGroup.tsx, cmd:npm run dev
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
//   radius. Code uses the bundled JetBrains Mono by default; the other two
//   choices are the operating system's own mono stack and Courier New.
// * **Transcript** — how much of the agent's work the conversation shows at
//   once. Built-in themes start with compact rows, completed-turn grouping and
//   the working steps open; full cards, no grouping and a folded turn remain
//   available below.
// * **Advanced** — custom CSS applied live after the application stylesheet, for
//   effects the controls do not cover.
//
// Every component reads CSS custom properties (`--theme-bg`, `--accent`, …), so
// a theme repaints the UI without a reload.
//
// # Transcript view options
//
// Four settings, saved with the theme (so they export and import with it) and
// applied as `data-tool-style`, `data-grouping`, `data-working` and
// `data-thinking` on `<html>` next to `data-density`:
//
// * **Tool calls** — `rows` (built-in default) draws one quiet ~26 px line per
//   call (symbol, name, arguments, duration, status) that expands into numbered
//   output. `cards` draws the full card with a footer disclosure instead. Neither
//   successful variant spends space on raw output while closed; running cards
//   retain their live preview, returned images stay visible, and failures open to
//   their explanation. A turn with twenty calls reads as a list instead of twenty
//   blocks.
// * **Grouping** — `turns` (built-in default) puts a finished turn's working
//   steps under one summary line ("Show the working · N tool calls · 1.2 s"),
//   leaving the answer and actionable notices. `tools` groups only a run of
//   *consecutive* calls ("read_file, grep +1 more · 3 calls · 1.2 s"). `none`
//   shows every call where it happened.
// * **Working steps** — `open` (built-in default) starts that work expanded: the
//   narration, the runs of calls and the calls inside them. The summary line stays
//   as the **Hide the working** header that folds it away. `folded` is the terse
//   view, where a finished turn is one **Show the working** line to click. Either
//   way the state is per row — one turn, one run of calls — so opening one leaves
//   every other row as it was, and switching the option repaints only the rows you
//   have not clicked yourself.
// * **Reasoning** — `collapsed` (default) is a `Thought` chip that opens to the
//   reasoning. `preview` adds the first line of the thought, in italics, so a
//   glance shows what the agent was working out. `expanded` shows it in full,
//   and a running model is always shown open with a live dot.
//
// Nothing that can still change what you must do is ever folded: a streaming
// turn, a running call, a pending approval, an error, an interrupted run and a
// notice all stay on screen. `foldTurn` lifts a notice or error out of the fold
// rather than hiding it, and a group opens itself while a member is running or
// after a failure.
//
// Assistant prose written *between* tool calls is narration, not an answer, and
// is always drawn quieter (muted, smaller) than the final answer so the
// conclusion stands out. That is not optional: it is what makes a long turn
// readable.
//
// # Saving, sharing and resetting
//
// The selection, your saved custom themes and the unsaved draft live in
// `localStorage` under `karucode.theme-studio.v3`. That is per browser profile:
// themes are not stored in the karucode config, do not follow you to another
// machine and are not shared between browsers. Up to 60 custom themes are kept.
// Stored `cards` / `rows`, grouping, working-steps and reasoning values always
// win over the new built-in defaults. A saved custom theme or imported file from
// before those fields existed keeps the legacy `cards` + `none` fallback and a
// folded turn; only built-ins move to `rows` + `turns` + `open`.
//
// * **Save as new** stores the current draft under a name you type.
// * **Export** downloads `karucode-theme.json` (`format: "karucode-theme"`,
//   `version: 1`, plus `mode`, `density`, `font`, `mono`, `rounding`, `colors`,
//   `customCSS` and the four transcript options `toolStyle`, `grouping`,
//   `working` and `thinking`).
// * **Import** reads such a file (maximum 150 KB) and rejects it with a toast if
//   a colour is not a 6-digit hex value, the mode/density/font is unknown, or the
//   file is larger than the limit.
// * **Reset colours** restores the palette of the theme you started from,
//   re-based to the mode the draft is in: a theme's 18 colours belong to its
//   own mode, so resetting after an Appearance flip keeps the mode (and the
//   accent) and restores that mode's surfaces. Deleting a custom theme falls
//   back to Ember. Clearing the storage key resets everything to the defaults.
//
// # Gotchas
//
// * Custom CSS is injected verbatim — it is your own browser, but an imported
//   theme file can hide UI or load remote resources. Only import themes you
//   trust. Because the engine writes tokens inline on `<html>`, a `:root` rule
//   that overrides a `--theme-*` token needs `!important`; Copy variables emits
//   the applied palette and derived roles with that marker already included.
// * A light theme sets `color-scheme: light`, which changes native form controls
//   and scrollbars too; the footer reports the contrast of the colours actually
//   applied — the derived ones, not the raw hex you typed — and warns below the
//   4.5:1 AA ratio for body text.
// * The engine writes `data-theme`, `data-mode`, `data-density`,
//   `data-tool-style`, `data-grouping`, `data-working` and `data-thinking` on
//   `<html>`; component CSS may branch on those, so a theme with a broken value
//   can look worse than the default without any error.
// * The four transcript options are read at render time from `activeTheme`, and
//   `web/src/state/transcriptLayout.ts` owns the grouping and folding rules.
//   Changing a default there changes every user's transcript; the built-ins are
//   the place to change what a *theme* starts with.
// * A theme saved before these options existed still loads: `readTheme()` falls
//   back per field, so an unknown or missing value becomes the default rather
//   than rejecting the user's palette.

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
  ThemeColors,
  ThemeColorsPatch,
  ThemeDensity,
  ThemeFont,
  ThemeGrouping,
  ThemeMode,
  ThemeMono,
  ThemeOptionsPatch,
  ThemeThinking,
  ThemeToolStyle,
  ThemeWorking,
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

/**
 * Contrast of the *painted* ink on the painted canvas.
 *
 * Read from the resolved palette, not from `theme.colors`: `resolvePalette()`
 * nudges primary text until it clears 11:1 on every surface, so the raw inputs
 * describe a pair of colours the user typed while the palette describes the pair
 * on screen. Reporting the first number made the footer disagree with the
 * drawer it sits in (it read 4.6:1 for a draft that rendered at 12:1).
 */
export const themeContrast = computed<number>(() => {
  const { palette } = activeTheme.value;
  return contrastRatio(palette.text, palette.bg);
});

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
  // Transcript view options ride the same mechanism as density: components CSS
  // branches on the attribute, so switching a view repaints without a reload.
  root.dataset.toolStyle = theme.toolStyle;
  root.dataset.grouping = theme.grouping;
  root.dataset.working = theme.working;
  root.dataset.thinking = theme.thinking;
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

/**
 * The palette a mode requires.
 *
 * A theme's 18 colours are the palette *for its own mode* — a dark canvas in
 * `light` mode is not a palette, it is a bug the resolver then has to rescue.
 * So any operation that changes the mode without changing every colour
 * (`setThemeMode`, and `resetColors` when the base theme's mode differs)
 * re-derives the surfaces from the mode's preset and keeps only the two colours
 * that carry the theme's identity: the accent and the syntax keyword that
 * follows it.
 */
function paletteForMode(mode: ThemeMode, accent: string): ThemeColors {
  const preset = modePreset(mode);
  return { ...preset.colors, accent, syntaxKeyword: accent };
}

export function setThemeMode(mode: ThemeMode): void {
  if (currentTheme.value.mode === mode) return;
  const base = draftEdited.value ? currentTheme.value : beginDraft();
  // Keep the user's accent (the one colour that defines the identity), re-base
  // the surfaces so the result is legible in the new mode.
  commit({ ...base, mode, colors: paletteForMode(mode, base.colors.accent) }, true);
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

export function setToolStyle(toolStyle: ThemeToolStyle): void {
  updateTheme({ toolStyle });
}

export function setGrouping(grouping: ThemeGrouping): void {
  updateTheme({ grouping });
}

/** Open or fold a finished turn's working steps, transcript-wide. */
export function setWorking(working: ThemeWorking): void {
  updateTheme({ working });
}

export function setThinking(thinking: ThemeThinking): void {
  updateTheme({ thinking });
}

/**
 * Restore the palette of the theme the draft was derived from.
 *
 * The base's colours are a palette for the *base's* mode, so restoring them
 * verbatim is only right while the draft is in that mode. After a mode flip it
 * is not: "Ember → Light → Reset colours" used to paint Ember's near-black
 * surfaces under `data-mode="light"`, which is how the drawer ended up with
 * unreadable text after a reset. Reset therefore applies the same transformation
 * `setThemeMode` does, so the result is always a palette for the current mode.
 */
export function resetColors(): void {
  const base = lookupSavedTheme(baseThemeId.value) ?? DEFAULT_THEME;
  // Nothing to undo: the applied theme already is the base palette.
  if (!draftEdited.value && currentTheme.value.id === base.id) return;
  const mode = currentTheme.value.mode;
  const colors = mode === base.mode ? base.colors : paletteForMode(mode, base.colors.accent);
  updateTheme({ colors });
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