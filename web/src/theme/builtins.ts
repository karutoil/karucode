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

/** Ember, the default dark palette (see `styles/tokens.css`). */
const DARK_DEFAULTS: ThemeColors = {
  bg: '#111213',
  rail: '#141516',
  sidebar: '#18191a',
  inspector: '#171819',
  surface: '#1c1d1f',
  raised: '#222326',
  hover: '#27292b',
  border: '#343638',
  text: '#eae9e5',
  muted: '#8b8d8c',
  accent: '#dfb680',
  success: '#88baa0',
  danger: '#e49e91',
  info: '#92b3d8',
  warn: '#d9a441',
  syntaxKeyword: '#dfb680',
  syntaxString: '#a5cbb4',
  syntaxComment: '#878e88',
};

/** Daybreak, the default light palette. */
const LIGHT_DEFAULTS: ThemeColors = {
  bg: '#f7f8fb',
  rail: '#e9edf5',
  sidebar: '#f0f2f8',
  inspector: '#f0f2f8',
  surface: '#ffffff',
  raised: '#e8edf6',
  hover: '#dfe6f1',
  border: '#cdd5e2',
  text: '#1b283a',
  muted: '#667487',
  accent: '#536ee6',
  success: '#227e55',
  danger: '#b94251',
  info: '#286cb5',
  warn: '#9a6413',
  syntaxKeyword: '#536ee6',
  syntaxString: '#227e55',
  syntaxComment: '#667487',
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
      bg: '#0c1221',
      rail: '#0d1526',
      sidebar: '#101b2d',
      inspector: '#0f1a2b',
      surface: '#162238',
      raised: '#1d2c45',
      hover: '#263852',
      border: '#34435e',
      text: '#e9efff',
      muted: '#9daac4',
      accent: '#83a8ff',
      success: '#6fd5a4',
      danger: '#f28e99',
      info: '#8cc7fa',
    },
  }),
  builtin({
    id: 'forest',
    name: 'Forest',
    description: 'Moss and eucalyptus',
    mode: 'dark',
    colors: {
      bg: '#101814',
      rail: '#111b16',
      sidebar: '#17221c',
      inspector: '#142018',
      surface: '#1c2a22',
      raised: '#25382d',
      hover: '#304334',
      border: '#3b5143',
      text: '#e5f0e6',
      muted: '#9cad9c',
      accent: '#a6d791',
      success: '#8dccac',
      danger: '#e8a18f',
      info: '#92c1cb',
    },
  }),
  builtin({
    id: 'sakura',
    name: 'Sakura',
    description: 'Muted plum and rose',
    mode: 'dark',
    colors: {
      bg: '#181317',
      rail: '#1c151a',
      sidebar: '#231b22',
      inspector: '#1e171e',
      surface: '#2d222b',
      raised: '#392b36',
      hover: '#44333f',
      border: '#5c4656',
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
      bg: '#0e1010',
      rail: '#111313',
      sidebar: '#161818',
      inspector: '#141616',
      surface: '#1d2020',
      raised: '#272b2b',
      hover: '#303636',
      border: '#444b4b',
      text: '#f0f2f1',
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
      rail: '#eae5dd',
      sidebar: '#f1ece3',
      inspector: '#f2eee7',
      surface: '#fffdf8',
      raised: '#e8e1d6',
      hover: '#dfd6c9',
      border: '#ccc2b3',
      text: '#302b26',
      muted: '#7b7064',
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
      bg: '#f0f9f7',
      rail: '#e1eeec',
      sidebar: '#e9f5f2',
      inspector: '#e9f5f3',
      surface: '#ffffff',
      raised: '#dbede8',
      hover: '#cbe4dc',
      border: '#bad5cd',
      text: '#18362e',
      muted: '#627e72',
      accent: '#147f70',
      success: '#21825a',
      danger: '#ad4a55',
      info: '#317f9c',
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