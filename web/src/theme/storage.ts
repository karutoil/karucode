/* Persistence and import/export for the Theme Studio.
 *
 * Everything crossing the storage/import boundary is untrusted: localStorage
 * may hold an older shape, a user may edit the JSON by hand, and an imported
 * file can contain anything. `readTheme()` is the single validator — it clamps
 * numbers, whitelists knobs, validates every hex colour and caps custom CSS, so
 * the engine can assume a well-formed `Theme`.
 */

import { normalizeHex } from './color';
import { BUILTIN_THEMES, completeColors } from './builtins';
import { clampRounding } from './tokens';
import {
  THEME_DENSITIES,
  THEME_FONTS,
  THEME_GROUPINGS,
  THEME_MONO_FONTS,
  THEME_COLOR_KEYS,
  THEME_THINKING_MODES,
  THEME_TOOL_STYLES,
  type Theme,
  type ThemeColors,
  type ThemeDensity,
  type ThemeFont,
  type ThemeGrouping,
  type ThemeMode,
  type ThemeMono,
  type ThemeThinking,
  type ThemeToolStyle,
} from './types';

export const THEME_STORAGE_KEY = 'karucode.theme-studio.v3';
export const THEME_FORMAT = 'karucode-theme';
export const THEME_FORMAT_VERSION = 1;
export const MAX_CUSTOM_THEMES = 60;
export const MAX_CUSTOM_CSS = 40_000;
export const MAX_IMPORT_BYTES = 150_000;
export const MAX_THEME_NAME = 50;

export interface PersistedThemeState {
  version: 1;
  /** Id of the saved theme (or the base theme while a draft is active). */
  selected: string;
  /** Id the draft was derived from; drives "Reset colours". */
  baseId: string;
  custom: Theme[];
  draft: Theme | null;
}

export class ThemeFileError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ThemeFileError';
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === 'string' && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

/** Cap the editor payload and neutralise a stray `</style>` before it is inlined. */
export function sanitizeCustomCSS(value: unknown): string {
  if (typeof value !== 'string') return '';
  return value
    .replace(/<\/style\s*>/gi, '')
    .replace(/<\/style/gi, '')
    .slice(0, MAX_CUSTOM_CSS);
}

export function sanitizeThemeName(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const name = value.replace(/\s+/g, ' ').trim().slice(0, MAX_THEME_NAME);
  return name.length > 0 ? name : null;
}

export function makeThemeId(): string {
  return `custom-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

const ID_PATTERN = /^[a-z0-9][a-z0-9-]{0,63}$/i;

export function isValidThemeId(value: unknown): value is string {
  return typeof value === 'string' && value !== 'draft' && ID_PATTERN.test(value);
}

interface ReadOptions {
  /** Persisted entries must carry a stable id; imports may omit one. */
  requireId?: boolean;
}

/** Validate one theme. Returns a `Theme` or a human-readable error message. */
function readTheme(input: unknown, options: ReadOptions = {}): Theme | string {
  if (!isRecord(input)) return 'A theme must be an object.';

  let id: string;
  if (isValidThemeId(input.id)) {
    id = input.id;
  } else {
    if (options.requireId) return 'A saved theme is missing a valid id.';
    id = makeThemeId();
  }

  const name = sanitizeThemeName(input.name);
  if (!name) return 'A theme needs a name of 1–50 characters.';

  if (!isRecord(input.colors)) return `“${name}” is missing its colours.`;
  const rawColors = input.colors;
  const colors: Partial<ThemeColors> = {};
  for (const key of THEME_COLOR_KEYS) {
    const raw = rawColors[key];
    if (raw === undefined) continue;
    const hex = normalizeHex(raw);
    if (!hex) return `“${rawColors[key] as string}” is not a valid hex colour for ${key}.`;
    colors[key] = hex;
  }

  const mode = oneOf<ThemeMode>(input.mode, ['dark', 'light'], 'dark');
  const density = oneOf<ThemeDensity>(input.density, THEME_DENSITIES, 'comfortable');
  const font = oneOf<ThemeFont>(input.font, THEME_FONTS, 'system');
  const mono = oneOf<ThemeMono>(input.mono, THEME_MONO_FONTS, 'default');
  // Older saved themes and files predate the view options: an unknown or
  // missing value falls back to the defaults instead of failing the theme.
  const toolStyle = oneOf<ThemeToolStyle>(input.toolStyle, THEME_TOOL_STYLES, 'cards');
  const grouping = oneOf<ThemeGrouping>(input.grouping, THEME_GROUPINGS, 'none');
  const thinking = oneOf<ThemeThinking>(input.thinking, THEME_THINKING_MODES, 'collapsed');
  const rounding =
    typeof input.rounding === 'number' && Number.isFinite(input.rounding)
      ? clampRounding(input.rounding)
      : 10;
  const description =
    typeof input.description === 'string' ? input.description.slice(0, 140) : '';

  const theme: Theme = {
    id,
    name,
    mode,
    density,
    font,
    mono,
    rounding,
    colors: completeColors(colors, mode),
    customCSS: sanitizeCustomCSS(input.customCSS),
    toolStyle,
    grouping,
    thinking,
  };
  if (description) theme.description = description;
  return theme;
}

/** Lenient validator used when loading the store: corrupt entries are skipped. */
export function sanitizeTheme(input: unknown): Theme | null {
  const result = readTheme(input, { requireId: true });
  return typeof result === 'string' ? null : result;
}

/** Strict parser for an imported `.theme.json` file. Throws `ThemeFileError`. */
export function parseThemeFile(text: string): Theme {
  if (typeof text !== 'string' || text.trim().length === 0) {
    throw new ThemeFileError('The theme file is empty.');
  }
  if (text.length > MAX_IMPORT_BYTES) {
    throw new ThemeFileError('That theme file is too large (limit 150 KB).');
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new ThemeFileError('That file is not valid JSON.');
  }
  if (!isRecord(parsed)) throw new ThemeFileError('That file is not a theme.');

  if (parsed.format !== undefined && parsed.format !== THEME_FORMAT) {
    throw new ThemeFileError('That JSON is not a karucode theme file.');
  }

  const withName: Record<string, unknown> = {
    ...parsed,
    name: sanitizeThemeName(parsed.name) ?? 'Imported theme',
  };
  // Imported files never get to choose their id; ids stay unique in the library.
  delete withName.id;

  const result = readTheme(withName, { requireId: false });
  if (typeof result === 'string') throw new ThemeFileError(result);
  return result;
}

/** Stable, download-friendly JSON for a theme (ids are intentionally omitted). */
export function exportThemeJSON(theme: Theme): string {
  const document = {
    format: THEME_FORMAT,
    version: THEME_FORMAT_VERSION,
    name: theme.name,
    description: theme.description ?? '',
    mode: theme.mode,
    density: theme.density,
    font: theme.font,
    mono: theme.mono,
    rounding: theme.rounding,
    colors: { ...theme.colors },
    customCSS: theme.customCSS,
    toolStyle: theme.toolStyle,
    grouping: theme.grouping,
    thinking: theme.thinking,
  };
  return `${JSON.stringify(document, null, 2)}\n`;
}

export function themeFileSlug(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return slug || 'theme';
}

/** Read the persisted studio state. Never throws; corrupt data yields null. */
export function loadPersistedState(): PersistedThemeState | null {
  if (typeof localStorage === 'undefined') return null;
  let raw: string | null = null;
  try {
    raw = localStorage.getItem(THEME_STORAGE_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isRecord(parsed)) return null;

  const custom: Theme[] = [];
  const seen = new Set<string>(BUILTIN_THEMES.map((theme) => theme.id));
  const rawCustom = Array.isArray(parsed.custom) ? parsed.custom : [];
  for (const entry of rawCustom.slice(0, MAX_CUSTOM_THEMES)) {
    const theme = sanitizeTheme(entry);
    if (!theme || seen.has(theme.id)) continue;
    seen.add(theme.id);
    custom.push(theme);
  }

  let draft: Theme | null = null;
  if (parsed.draft !== undefined && parsed.draft !== null) {
    const candidate = readTheme(parsed.draft, { requireId: false });
    if (typeof candidate !== 'string') draft = { ...candidate, id: 'draft' };
  }

  const selected = typeof parsed.selected === 'string' ? parsed.selected : 'ember';
  const baseId = typeof parsed.baseId === 'string' ? parsed.baseId : selected;

  return { version: 1, selected, baseId, custom, draft };
}

export function savePersistedState(state: Omit<PersistedThemeState, 'version'>): boolean {
  if (typeof localStorage === 'undefined') return false;
  try {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({ version: 1, ...state, custom: state.custom.slice(0, MAX_CUSTOM_THEMES) }),
    );
    return true;
  } catch {
    // Private mode or quota exceeded: the live theme still works, it just will
    // not survive a reload. The studio surfaces the failure via `autosaveError`.
    return false;
  }
}

export function clearPersistedState(): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.removeItem(THEME_STORAGE_KEY);
  } catch {
    // Nothing to do.
  }
}