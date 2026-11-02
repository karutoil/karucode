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
 * These are the mock's own values: a near-black cool canvas (`#08090b`), a
 * four-step chrome ladder, and a coral accent. The accent is warm against a cool
 * shell on purpose — it is the only saturated colour in the frame, so it reads as
 * "the thing you press" without any weight of its own.
 */
const DARK_DEFAULTS: ThemeColors = {
  bg: '#08090b',
  rail: '#0b0d10',
  sidebar: '#0e1116',
  inspector: '#11151a',
  surface: '#171c23',
  raised: '#1d232c',
  hover: '#252c37',
  border: '#333c48',
  text: '#e9ebf0',
  muted: '#9aa3b2',
  accent: '#eaa06d',
  success: '#7fd6b4',
  danger: '#f2938b',
  info: '#8fb3f7',
  warn: '#f0c26b',
  syntaxKeyword: '#eaa06d',
  syntaxString: '#8fdcb7',
  syntaxComment: '#7c8494',
};

/** Daybreak, the default light palette (the mock's warm-paper mode). */
const LIGHT_DEFAULTS: ThemeColors = {
  bg: '#f4f2ee',
  rail: '#faf9f6',
  sidebar: '#f7f4ef',
  inspector: '#faf9f6',
  surface: '#ffffff',
  raised: '#f7f5f1',
  hover: '#eeebe5',
  border: '#ded9d0',
  text: '#1c1a17',
  muted: '#6f6a61',
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
    // The mock's own transcript: full cards, every call in place, a quiet
    // reasoning chip. The view options exist so a long session can be folded.
    toolStyle: 'cards',
    grouping: 'none',
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
      bg: '#0c1221',
      rail: '#101a30',
      sidebar: '#142238',
      inspector: '#172843',
      surface: '#1d2d4b',
      raised: '#223350',
      hover: '#263953',
      border: '#34435e',
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
      bg: '#101814',
      rail: '#14201a',
      sidebar: '#1a2720',
      inspector: '#1d2e22',
      surface: '#22342a',
      raised: '#26392e',
      hover: '#304334',
      border: '#3b5143',
      text: '#e5f0e6',
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
      bg: '#181317',
      rail: '#231a20',
      sidebar: '#2a2029',
      inspector: '#312531',
      surface: '#382a36',
      raised: '#3f2f3b',
      hover: '#453440',
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
      rail: '#171919',
      sidebar: '#1e2121',
      inspector: '#242828',
      surface: '#292e2e',
      raised: '#2f3333',
      hover: '#323838',
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
      sidebar: '#e7decf',
      inspector: '#e1d8c9',
      surface: '#fffdf8',
      raised: '#e8e1d6',
      hover: '#dfd6c9',
      border: '#ccc2b3',
      text: '#302b26',
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
      bg: '#f0f9f7',
      rail: '#e1eeec',
      sidebar: '#d2eae4',
      inspector: '#c7e5e0',
      surface: '#ffffff',
      raised: '#dbede8',
      hover: '#cbe4dc',
      border: '#bad5cd',
      text: '#18362e',
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