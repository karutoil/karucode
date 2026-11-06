/* The eight built-in themes.
 *
 * Values mirror the v3 design mock (`karucode-v3-themes.html`): Ember is
 * byte-for-byte the palette that `styles/tokens.css` ships as the first-paint
 * fallback, so applying it before React renders cannot shift the UI.
 *
 * Built-ins are immutable. "Save as new" copies one into the user's custom
 * library; editing a built-in starts a draft.
 */

import { mix } from './color';
import type { Theme, ThemeColors, ThemeMode } from './types';

/** Ember, the default dark palette (see `styles/tokens.css`).
 *
 * **Warm** graphite under a sand-gold accent. That adjective is load-bearing and
 * was not true before: the previous values (`#111213`, `#1c1d1f`, …) lean blue
 * (`h≈210`), while the accent is amber (`h≈34`), so the shell was cool grey and
 * the brand was warm — two families on one screen before any component CSS ran.
 * These values sit at `h≈32` with 5% saturation, so the chrome now agrees with
 * the accent.
 *
 * The lightness ladder is `docs/internal/DESIGN-SYSTEM.md` §3.1: canvas 5.5%,
 * rail 7.6%, side columns 9.5%, cards 13%, raised 16.5%, hover 20%. Each
 * elevation step is ≥3.5 points, which is the threshold at which a step is
 * visible at all — the old ladder spanned 7%→14% in 1-2 point steps and read as
 * one flat field.
 */
const DARK_DEFAULTS: ThemeColors = {
  bg: '#0f0e0d',
  rail: '#141312',
  sidebar: '#1a1918',
  inspector: '#181716',
  surface: '#23211f',
  raised: '#2c2a28',
  hover: '#363330',
  border: '#423f3b',
  text: '#f2f0ee',
  muted: '#ada49a',
  accent: '#dfb680',
  success: '#88baa0',
  danger: '#e49e91',
  info: '#92b3d8',
  warn: '#f1c691',
  syntaxKeyword: '#dfb680',
  syntaxString: '#88baa0',
  syntaxComment: '#8b8d8c',
};

/** Daybreak, the default light palette (the mock's warm-paper mode). */
const LIGHT_DEFAULTS: ThemeColors = {
  bg: '#f6f4f1',
  rail: '#fafaf7',
  sidebar: '#f3eee7',
  inspector: '#f4f1eb',
  surface: '#ffffff',
  raised: '#ede9e0',
  hover: '#e5e0d7',
  border: '#d6d0c6',
  text: '#1c1a17',
  muted: '#6a655c',
  accent: '#bb5f2c',
  success: '#1c8465',
  danger: '#bb4a42',
  info: '#3968c9',
  warn: '#96700f',
  syntaxKeyword: '#bb5f2c',
  syntaxString: '#1c8465',
  syntaxComment: '#6f6a61',
};

export interface BuiltinSpec {
  id: string;
  name: string;
  description: string;
  mode: ThemeMode;
  colors: Partial<ThemeColors>;
}

export function defaultColors(mode: ThemeMode): ThemeColors {
  return { ...(mode === 'light' ? LIGHT_DEFAULTS : DARK_DEFAULTS) };
}

/**
 * Fill a partial palette. Missing syntax colours follow the accent, success and
 * muted values so an imported two-colour theme still highlights code sensibly.
 */
export function completeColors(partial: Partial<ThemeColors>, mode: ThemeMode): ThemeColors {
  const base = defaultColors(mode);
  const colors: ThemeColors = { ...base, ...partial };
  if (partial.syntaxKeyword === undefined) colors.syntaxKeyword = colors.accent;
  if (partial.syntaxString === undefined) {
    colors.syntaxString = mode === 'light' ? colors.success : mix(colors.success, colors.text, 0.28);
  }
  if (partial.syntaxComment === undefined) colors.syntaxComment = colors.muted;
  return colors;
}

function builtin(spec: BuiltinSpec): Theme {
  return {
    id: spec.id,
    name: spec.name,
    description: spec.description,
    mode: spec.mode,
    density: 'comfortable',
    font: 'system',
    mono: 'default',
    rounding: 10,
    colors: completeColors(spec.colors, spec.mode),
    customCSS: '',
    // Workbench reading density is the built-in starting point: compact calls,
    // completed work folded by turn, and a quiet reasoning chip. Full cards and
    // no grouping remain explicit theme options; stored choices are not changed.
    toolStyle: 'rows',
    grouping: 'turns',
    thinking: 'collapsed',
  };
}

/** Built-in themes, in gallery order. */
export const BUILTIN_THEMES: readonly Theme[] = [
  builtin({
    id: 'ember',
    name: 'Ember',
    description: 'Warm carbon and sand',
    mode: 'dark',
    colors: DARK_DEFAULTS,
  }),
  builtin({
    id: 'nocturne',
    name: 'Nocturne',
    description: 'Deep blue and periwinkle',
    mode: 'dark',
    colors: {
      bg: '#070b15',
      rail: '#0a101d',
      sidebar: '#0d1625',
      inspector: '#0d1625',
      surface: '#121d30',
      raised: '#19263b',
      hover: '#203046',
      border: '#2c3f5c',
      text: '#e9efff',
      muted: '#9daac4',
      accent: '#83a8ff',
      success: '#6fd5a4',
      danger: '#f28e99',
      info: '#8cc7fa',
      syntaxKeyword: '#83a8ff',
      syntaxString: '#8ed8b2',
      syntaxComment: '#8a8f9e',
    },
  }),
  builtin({
    id: 'forest',
    name: 'Forest',
    description: 'Moss and eucalyptus',
    mode: 'dark',
    colors: {
      bg: '#0b110e',
      rail: '#0f1813',
      sidebar: '#141e19',
      inspector: '#131f17',
      surface: '#1a2820',
      raised: '#223229',
      hover: '#2b3b2e',
      border: '#3a4e41',
      text: '#f7faf7',
      muted: '#a2b1a2',
      accent: '#a6d791',
      success: '#8dccac',
      danger: '#e8a18f',
      info: '#8fb2d8',
    },
  }),
  builtin({
    id: 'sakura',
    name: 'Sakura',
    description: 'Muted plum and rose',
    mode: 'dark',
    colors: {
      bg: '#100c0f',
      rail: '#161114',
      sidebar: '#1c161c',
      inspector: '#1c161c',
      surface: '#261c25',
      raised: '#30242d',
      hover: '#3a2c36',
      border: '#4d3b47',
      text: '#f9ebf2',
      muted: '#bca7b5',
      accent: '#f1a7cb',
      success: '#a4d2b0',
      danger: '#f29b9c',
      info: '#abb9eb',
    },
  }),
  builtin({
    id: 'graphite',
    name: 'Graphite',
    description: 'Neutral monochrome',
    mode: 'dark',
    colors: {
      bg: '#0d0f0f',
      rail: '#131414',
      sidebar: '#181a1a',
      inspector: '#181a1a',
      surface: '#1f2323',
      raised: '#282c2c',
      hover: '#303636',
      border: '#3d4545',
      text: '#f4f6f5',
      muted: '#9baba9',
      accent: '#d4dbe0',
      success: '#9cc9ae',
      danger: '#ee9d9d',
      info: '#91b8d3',
    },
  }),
  builtin({
    id: 'daybreak',
    name: 'Daybreak',
    description: 'Soft daylight and cobalt',
    mode: 'light',
    colors: LIGHT_DEFAULTS,
  }),
  builtin({
    id: 'paper',
    name: 'Paper',
    description: 'Warm parchment and copper',
    mode: 'light',
    colors: {
      bg: '#f8f5ef',
      rail: '#faf9f7',
      sidebar: '#f3eee7',
      inspector: '#f4f0eb',
      surface: '#ffffff',
      raised: '#eee8e0',
      hover: '#e6dfd5',
      border: '#cfc6b8',
      text: '#2a2621',
      muted: '#605752',
      accent: '#a05b32',
      success: '#407b55',
      danger: '#a54946',
      info: '#4879a0',
    },
  }),
  builtin({
    id: 'mint',
    name: 'Mint',
    description: 'Fresh sea glass',
    mode: 'light',
    colors: {
      bg: '#eff8f6',
      rail: '#f7fafa',
      sidebar: '#e6f3f0',
      inspector: '#e9f5f3',
      surface: '#ffffff',
      raised: '#dfefea',
      hover: '#d3e8e2',
      border: '#b6d2ca',
      text: '#142d26',
      muted: '#506358',
      accent: '#147f70',
      success: '#21825a',
      danger: '#ad4a55',
      info: '#3d7fa8',
      syntaxKeyword: '#147f70',
      syntaxString: '#a0522d',
      syntaxComment: '#556168',
    },
  }),
];

export const DEFAULT_THEME: Theme = BUILTIN_THEMES[0] as Theme;

export function findBuiltin(id: string): Theme | undefined {
  return BUILTIN_THEMES.find((theme) => theme.id === id);
}

/** The preset a mode switch re-bases onto (mirrors the design mock). */
export function modePreset(mode: ThemeMode): Theme {
  return findBuiltin(mode === 'light' ? 'daybreak' : 'ember') ?? DEFAULT_THEME;
}