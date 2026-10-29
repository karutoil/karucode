/* Token resolution: palette inputs -> concrete CSS custom properties.
 *
 * The app's stylesheet (`styles/tokens.css`) exposes public aliases such as
 * `--bg`, `--text` and `--border` that point at `--theme-*`. Rewriting the
 * `--theme-*` block on <html> therefore repaints every component without any
 * component knowing a theme exists. `buildTokenMap()` is pure so the resolver
 * can be unit-tested without a DOM.
 */

import { contrastRatio, mix, readableTextOn } from './color';
import { resolveRoles } from './roles';
import type {
  DensityScale,
  ResolvedPalette,
  ResolvedTheme,
  Theme,
  ThemeDensity,
  ThemeFont,
  ThemeMono,
} from './types';

export const FONT_STACKS: Record<ThemeFont, string> = {
  system:
    "Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, 'Noto Sans', sans-serif",
  modern: "'Avenir Next', 'Century Gothic', ui-sans-serif, system-ui, sans-serif",
  rounded: "'Trebuchet MS', ui-rounded, 'Segoe UI', sans-serif",
  humanist: "'Segoe UI', Verdana, ui-sans-serif, sans-serif",
  classic: 'Arial, Helvetica, sans-serif',
};

export const MONO_STACKS: Record<ThemeMono, string> = {
  default: "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, ui-monospace, monospace",
  system: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  classic: "'Courier New', Courier, monospace",
};

/** Density moves type size and spacing together (and drives `data-density`). */
export const DENSITY_SCALES: Record<ThemeDensity, DensityScale> = {
  compact: { scale: 0.92, space: 0.86, body: 12, small: 11, micro: 9.5, lineHeight: 1.62 },
  comfortable: { scale: 1, space: 1, body: 13, small: 11.5, micro: 10, lineHeight: 1.5 },
  spacious: { scale: 1.06, space: 1.14, body: 13.25, small: 12, micro: 10.5, lineHeight: 1.8 },
};

const SPACE_STEPS = [4, 7, 10, 14, 18, 24, 30] as const;

export const MIN_ROUNDING = 0;
export const MAX_ROUNDING = 22;

export function clampRounding(value: number): number {
  if (!Number.isFinite(value)) return 10;
  return Math.min(MAX_ROUNDING, Math.max(MIN_ROUNDING, Math.round(value)));
}

/** Small text on the sidebar/cards must stay at AA; darken or lighten toward text. */
function readableAccent(accent: string, text: string, backgrounds: readonly string[]): string {
  const passes = (candidate: string): boolean =>
    backgrounds.every((background) => contrastRatio(candidate, background) >= 4.5);
  if (passes(accent)) return accent;
  for (let step = 1; step <= 10; step += 1) {
    const candidate = mix(accent, text, step / 10);
    if (passes(candidate)) return candidate;
  }
  return text;
}

/**
 * Every non-editable token, derived from the palette input. Keeping this in one
 * function means a colour edit can never leave a stale divider or tint behind.
 */
export function resolvePalette(theme: Theme): ResolvedPalette {
  const c = theme.colors;
  const accentBg = mix(c.surface, c.accent, 0.17);
  // The accent becomes text (eyebrow, badge, selected card) and the fill of the
  // primary button. Both uses need AA, so nudge it toward text until it passes
  // on every surface it sits on — never mutate the user's editable colour.
  const accentText = readableAccent(c.accent, c.text, [
    c.surface,
    c.sidebar,
    c.inspector,
    accentBg,
  ]);
  return {
    ...c,
    line: mix(c.surface, c.border, 0.62),
    lineSoft: mix(c.surface, c.border, 0.34),
    lineStrong: c.border,
    textSecondary: mix(c.text, c.muted, 0.4),
    faint: mix(c.muted, c.bg, 0.28),
    accentHi: mix(c.accent, c.text, 0.22),
    accentBg,
    accentLine: mix(c.border, c.accent, 0.45),
    accentText,
    onAccent: readableTextOn(accentText),
    successBg: mix(c.surface, c.success, 0.16),
    dangerBg: mix(c.surface, c.danger, 0.16),
    shadow: theme.mode === 'light' ? 'rgba(15, 23, 42, 0.16)' : 'rgba(0, 0, 0, 0.48)',
  };
}

export function resolveTheme(theme: Theme): ResolvedTheme {
  return { ...theme, palette: resolvePalette(theme) };
}

/** `palette` key -> CSS custom property consumed by the stylesheet. */
export const THEME_TOKEN_ENTRIES: ReadonlyArray<readonly [keyof ResolvedPalette, string]> = [
  ['bg', '--theme-bg'],
  ['rail', '--theme-rail'],
  ['sidebar', '--theme-sidebar'],
  ['inspector', '--theme-inspector'],
  ['surface', '--theme-surface'],
  ['raised', '--theme-raised'],
  ['hover', '--theme-hover'],
  ['line', '--theme-line'],
  ['lineSoft', '--theme-line-soft'],
  ['lineStrong', '--theme-line-strong'],
  ['text', '--theme-text'],
  ['textSecondary', '--theme-text-secondary'],
  ['muted', '--theme-muted'],
  ['faint', '--theme-faint'],
  ['accent', '--theme-accent'],
  ['accentHi', '--theme-accent-hi'],
  ['accentBg', '--theme-accent-bg'],
  ['accentLine', '--theme-accent-line'],
  ['accentText', '--theme-accent-text'],
  ['onAccent', '--theme-on-accent'],
  ['success', '--theme-success'],
  ['successBg', '--theme-success-bg'],
  ['warn', '--theme-warn'],
  ['danger', '--theme-danger'],
  ['dangerBg', '--theme-danger-bg'],
  ['info', '--theme-info'],
  ['syntaxKeyword', '--theme-syntax-keyword'],
  ['syntaxString', '--theme-syntax-string'],
  ['syntaxComment', '--theme-syntax-comment'],
  ['shadow', '--theme-shadow'],
];

/**
 * The full declaration set for a theme: palette tokens, shape (radius), type
 * stacks and the density-scaled spacing/typography scale.
 */
export function buildTokenMap(theme: Theme): Record<string, string> {
  const palette = resolvePalette(theme);
  const density = DENSITY_SCALES[theme.density];
  const rounding = clampRounding(theme.rounding);

  const tokens: Record<string, string> = {};
  for (const [key, property] of THEME_TOKEN_ENTRIES) tokens[property] = palette[key];

  // Fidelity roles (input box, inset card, menu, diff row, rail glyph …): the
  // mock's own literals for Ember, derived offsets for every other theme.
  Object.assign(tokens, resolveRoles({ ...theme, palette }));

  tokens['--radius'] = `${rounding}px`;
  tokens['--radius-sm'] = `${Math.max(0, rounding - 3)}px`;
  tokens['--radius-lg'] = `${rounding + 2}px`;
  tokens['--font-ui'] = FONT_STACKS[theme.font];
  tokens['--font-mono'] = MONO_STACKS[theme.mono];

  tokens['--fs-body'] = `${density.body}px`;
  tokens['--fs-small'] = `${density.small}px`;
  tokens['--fs-micro'] = `${density.micro}px`;
  tokens['--lh-body'] = String(density.lineHeight);
  // Message copy keeps the mock's denser rhythm whatever the density step is.
  tokens['--fs-message'] = '12.5px';
  tokens['--lh-message'] = '1.72';
  tokens['--theme-density'] = String(density.scale);

  SPACE_STEPS.forEach((step, index) => {
    tokens[`--space-${index + 1}`] = `${Math.round(step * density.space * 100) / 100}px`;
  });

  return tokens;
}