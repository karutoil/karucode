/* Shared theme types.
 *
 * A theme is a *palette input*: the eighteen colours a user can edit plus the
 * interface knobs (mode, density, fonts, radius, custom CSS). Every remaining
 * token (derived lines, translucent backgrounds, shadows) is computed from
 * those inputs by `resolveTheme()` in `tokens.ts`, so editing one colour keeps
 * the whole surface coherent.
 */

export type ThemeMode = 'dark' | 'light';
export type ThemeDensity = 'compact' | 'comfortable' | 'spacious';
export type ThemeFont = 'system' | 'modern' | 'rounded' | 'humanist' | 'classic';
export type ThemeMono = 'default' | 'system' | 'classic';

export const THEME_MODES: readonly ThemeMode[] = ['dark', 'light'];
export const THEME_DENSITIES: readonly ThemeDensity[] = ['compact', 'comfortable', 'spacious'];
export const THEME_FONTS: readonly ThemeFont[] = [
  'system',
  'modern',
  'rounded',
  'humanist',
  'classic',
];
export const THEME_MONO_FONTS: readonly ThemeMono[] = ['default', 'system', 'classic'];

/** The editable palette. Everything else in `ResolvedPalette` is derived. */
export interface ThemeColors {
  bg: string;
  rail: string;
  sidebar: string;
  inspector: string;
  surface: string;
  raised: string;
  hover: string;
  border: string;
  text: string;
  muted: string;
  accent: string;
  success: string;
  danger: string;
  info: string;
  warn: string;
  syntaxKeyword: string;
  syntaxString: string;
  syntaxComment: string;
}

export type ThemeColorKey = keyof ThemeColors;

export const THEME_COLOR_KEYS: readonly ThemeColorKey[] = [
  'bg',
  'rail',
  'sidebar',
  'inspector',
  'surface',
  'raised',
  'hover',
  'border',
  'text',
  'muted',
  'accent',
  'success',
  'danger',
  'info',
  'warn',
  'syntaxKeyword',
  'syntaxString',
  'syntaxComment',
];

/** Type sizes and spacing multipliers for one density step. */
export interface DensityScale {
  /** Multiplier exposed as `--theme-density` for CSS that wants it. */
  scale: number;
  space: number;
  body: number;
  small: number;
  micro: number;
  lineHeight: number;
}

/** A stored (built-in, custom or draft) theme. `colors` is always complete. */
export interface Theme {
  id: string;
  name: string;
  description?: string;
  mode: ThemeMode;
  density: ThemeDensity;
  font: ThemeFont;
  mono: ThemeMono;
  /** Corner radius in px; clamped to 0–22. */
  rounding: number;
  colors: ThemeColors;
  customCSS: string;
}

/** A theme ready to paint: palette inputs plus every derived token. */
export interface ResolvedTheme extends Theme {
  palette: ResolvedPalette;
}

export interface ResolvedPalette extends ThemeColors {
  line: string;
  lineSoft: string;
  lineStrong: string;
  textSecondary: string;
  faint: string;
  accentHi: string;
  accentBg: string;
  accentLine: string;
  /** Accent adjusted until it reads as text on the sidebar and cards (AA). */
  accentText: string;
  /** Text colour that is legible on a solid accent fill (primary buttons). */
  onAccent: string;
  successBg: string;
  dangerBg: string;
  shadow: string;
}

/** A partial palette patch, as produced by the colour controls. */
export type ThemeColorsPatch = Partial<ThemeColors>;

/** Fields the Theme Studio can change without touching the palette. */
export interface ThemeOptionsPatch {
  name?: string;
  description?: string;
  mode?: ThemeMode;
  density?: ThemeDensity;
  font?: ThemeFont;
  mono?: ThemeMono;
  rounding?: number;
  customCSS?: string;
  colors?: ThemeColorsPatch;
}

/** A colour control in the Theme Studio. */
export interface ThemeColorField {
  key: ThemeColorKey;
  label: string;
  hint: string;
}

export interface ThemeColorGroup {
  id: string;
  title: string;
  description: string;
  fields: readonly ThemeColorField[];
}

export const THEME_COLOR_GROUPS: readonly ThemeColorGroup[] = [
  {
    id: 'backgrounds',
    title: 'Colours',
    description: 'Make the workspace yours, from the project rail to the editor surface.',
    fields: [
      { key: 'bg', label: 'Canvas', hint: 'App background' },
      { key: 'rail', label: 'Project rail', hint: 'Far-left rail' },
      { key: 'sidebar', label: 'Session sidebar', hint: 'Session list' },
      { key: 'inspector', label: 'Inspector', hint: 'Right-hand panel' },
      { key: 'surface', label: 'Cards & panels', hint: 'Cards, inputs, sheets' },
      { key: 'raised', label: 'Elevated surface', hint: 'Menus and raised cards' },
      { key: 'hover', label: 'Hover surface', hint: 'Hovered rows and buttons' },
      { key: 'border', label: 'Borders', hint: 'Dividers and outlines' },
    ],
  },
  {
    id: 'accents',
    title: 'Text & accents',
    description: 'Click a swatch or type a hex code. Changes appear instantly.',
    fields: [
      { key: 'text', label: 'Primary text', hint: 'Body copy and headings' },
      { key: 'muted', label: 'Secondary text', hint: 'Labels and metadata' },
      { key: 'accent', label: 'Accent', hint: 'Selection, focus and brand' },
      { key: 'success', label: 'Success', hint: 'Passed checks and completions' },
      { key: 'danger', label: 'Error', hint: 'Failures and destructive actions' },
      { key: 'info', label: 'Info', hint: 'Links and informational states' },
    ],
  },
  {
    id: 'syntax',
    title: 'Syntax highlighting',
    description: 'Fine-tune code colours independently of the interface palette.',
    fields: [
      { key: 'syntaxKeyword', label: 'Keywords', hint: 'Keyword and operator colour' },
      { key: 'syntaxString', label: 'Types & values', hint: 'Strings, types and literals' },
      { key: 'syntaxComment', label: 'Comments', hint: 'Comments and doc strings' },
    ],
  },
];

export const THEME_FONT_LABELS: Record<ThemeFont, string> = {
  system: 'System / Inter',
  modern: 'Modern',
  rounded: 'Rounded',
  humanist: 'Humanist',
  classic: 'Classic',
};

export const THEME_MONO_LABELS: Record<ThemeMono, string> = {
  default: 'SF Mono / Consolas',
  system: 'System mono',
  classic: 'Courier New',
};