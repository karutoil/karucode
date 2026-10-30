import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { BUILTIN_THEMES, DEFAULT_THEME, completeColors } from './builtins';
import { contrastRatio, normalizeHex } from './color';
import { DENSITY_SCALES, buildTokenMap, clampRounding, resolvePalette } from './tokens';

describe('built-in themes', () => {
  it('ships eight themes with unique ids', () => {
    expect(BUILTIN_THEMES).toHaveLength(8);
    expect(new Set(BUILTIN_THEMES.map((theme) => theme.id)).size).toBe(8);
    expect(BUILTIN_THEMES.map((theme) => theme.id)).toEqual([
      'ember',
      'nocturne',
      'forest',
      'sakura',
      'graphite',
      'daybreak',
      'paper',
      'mint',
    ]);
  });

  it('covers both modes and keeps every colour a valid hex', () => {
    expect(BUILTIN_THEMES.some((theme) => theme.mode === 'dark')).toBe(true);
    expect(BUILTIN_THEMES.some((theme) => theme.mode === 'light')).toBe(true);

    for (const theme of BUILTIN_THEMES) {
      for (const [key, value] of Object.entries(theme.colors)) {
        expect(normalizeHex(value), `${theme.id}.${key}`).toBe(value);
      }
      expect(theme.rounding).toBe(10);
      expect(theme.customCSS).toBe('');
    }
  });

  it('keeps body text at AA contrast on every built-in surface', () => {
    for (const theme of BUILTIN_THEMES) {
      expect(contrastRatio(theme.colors.text, theme.colors.bg), theme.id).toBeGreaterThanOrEqual(
        4.5,
      );
    }
  });

  it('keeps accent text and primary buttons at AA contrast', () => {
    for (const theme of BUILTIN_THEMES) {
      const palette = resolvePalette(theme);
      for (const surface of [palette.surface, palette.sidebar, palette.accentBg]) {
        expect(contrastRatio(palette.accentText, surface), `${theme.id} accent on ${surface}`)
          .toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(palette.onAccent, palette.accentText), `${theme.id} on-accent`)
        .toBeGreaterThanOrEqual(4.5);
    }
  });

  it('uses the token stylesheet values for Ember (no first-paint shift)', () => {
    expect(DEFAULT_THEME.id).toBe('ember');
    // `styles/tokens.css` paints the first frame before the engine runs, so the
    // canvas and text it hardcodes must match the built-in exactly.
    expect(DEFAULT_THEME.colors.bg).toBe('#111213');
    expect(DEFAULT_THEME.colors.text).toBe('#eae9e5');
    expect(DEFAULT_THEME.colors.accent).toBe('#dfb680');
    expect(DEFAULT_THEME.colors.syntaxString).toBe('#a5cbb4');
    // Surfaces are asserted against the stylesheet itself below, so the two
    // cannot drift apart.
    expect(DEFAULT_THEME.colors.surface).toBe('#2c2d31');
  });

  it('keeps every Ember surface in sync with the first-paint stylesheet', () => {
    // The real invariant: applying Ember must not repaint anything, which means
    // every literal in tokens.css has to equal the built-in theme's value.
    // `tokens.css` is only readable from node (jsdom reports an http URL), and
    // the stylesheet contract lives in styles/theme-studio.test.ts.
    const css = readFileSync('src/styles/tokens.css', 'utf8');
    const declared: Record<string, string> = {};
    for (const match of css.matchAll(/--theme-([a-z-]+):\s*(#[0-9a-f]{6});/g)) {
      declared[match[1]!] = match[2]!;
    }
    const expected: Record<string, keyof typeof DEFAULT_THEME.colors> = {
      bg: 'bg',
      rail: 'rail',
      sidebar: 'sidebar',
      inspector: 'inspector',
      surface: 'surface',
      raised: 'raised',
      hover: 'hover',
      text: 'text',
      muted: 'muted',
      accent: 'accent',
      success: 'success',
      danger: 'danger',
      info: 'info',
      warn: 'warn',
    };
    for (const [token, key] of Object.entries(expected)) {
      expect(declared[token], `--theme-${token} in tokens.css`).toBe(DEFAULT_THEME.colors[key]);
    }
  });
});

describe('completeColors', () => {
  it('derives syntax colours from the palette for partial themes', () => {
    const colors = completeColors({ accent: '#ff0000', success: '#00ff00', muted: '#888888' }, 'light');
    expect(colors.syntaxKeyword).toBe('#ff0000');
    expect(colors.syntaxString).toBe('#00ff00');
    expect(colors.syntaxComment).toBe('#888888');
  });

  it('falls back to the mode defaults for missing surfaces', () => {
    const dark = completeColors({}, 'dark');
    const light = completeColors({}, 'light');
    expect(dark.bg).toBe('#111213');
    expect(light.bg).toBe('#f7f8fb');
    expect(light.text).toBe('#1b283a');
  });
});

describe('token resolution', () => {
  it('derives the full palette with valid colours', () => {
    const palette = resolvePalette(DEFAULT_THEME);
    for (const [key, value] of Object.entries(palette)) {
      expect(value, key).toBeTruthy();
      if (key !== 'shadow') expect(normalizeHex(value), key).toBe(value);
    }
    expect(palette.lineStrong).toBe(DEFAULT_THEME.colors.border);
    expect(palette.accentHi).not.toBe(palette.accent);
    expect(palette.successBg).not.toBe(palette.surface);
  });

  it('maps every token the stylesheet consumes', () => {
    const tokens = buildTokenMap(DEFAULT_THEME);
    const expected = [
      '--theme-bg',
      '--theme-rail',
      '--theme-sidebar',
      '--theme-inspector',
      '--theme-surface',
      '--theme-raised',
      '--theme-hover',
      '--theme-line',
      '--theme-line-soft',
      '--theme-line-strong',
      '--theme-text',
      '--theme-text-secondary',
      '--theme-muted',
      '--theme-faint',
      '--theme-accent',
      '--theme-accent-hi',
      '--theme-accent-bg',
      '--theme-accent-line',
      '--theme-success',
      '--theme-success-bg',
      '--theme-warn',
      '--theme-danger',
      '--theme-danger-bg',
      '--theme-info',
      '--theme-syntax-keyword',
      '--theme-syntax-string',
      '--theme-syntax-comment',
      '--theme-shadow',
      '--radius',
      '--radius-sm',
      '--radius-lg',
      '--font-ui',
      '--font-mono',
      '--fs-body',
      '--space-4',
    ];
    for (const property of expected) expect(tokens[property], property).toBeTruthy();
  });

  it('clamps the radius and scales the derived radii with it', () => {
    expect(clampRounding(-4)).toBe(0);
    expect(clampRounding(9.6)).toBe(10);
    expect(clampRounding(100)).toBe(22);
    expect(clampRounding(Number.NaN)).toBe(10);

    const tokens = buildTokenMap({ ...DEFAULT_THEME, rounding: 0 });
    expect(tokens['--radius']).toBe('0px');
    expect(tokens['--radius-sm']).toBe('0px');
    expect(tokens['--radius-lg']).toBe('2px');
  });

  it('applies fonts and density', () => {
    const compact = buildTokenMap({ ...DEFAULT_THEME, density: 'compact' });
    const spacious = buildTokenMap({ ...DEFAULT_THEME, density: 'spacious' });
    expect(compact['--fs-body']).toBe(`${DENSITY_SCALES.compact.body}px`);
    expect(compact['--fs-body']).not.toBe(spacious['--fs-body']);
    expect(compact['--space-4']).not.toBe(spacious['--space-4']);

    const classic = buildTokenMap({ ...DEFAULT_THEME, font: 'classic', mono: 'classic' });
    expect(classic['--font-ui']).toContain('Arial');
    expect(classic['--font-mono']).toContain('Courier New');
  });

  it('uses a lighter shadow for light mode', () => {
    const light = BUILTIN_THEMES.find((theme) => theme.mode === 'light');
    expect(light).toBeTruthy();
    const dark = resolvePalette(DEFAULT_THEME).shadow;
    const bright = resolvePalette(light!).shadow;
    expect(bright).not.toBe(dark);
    expect(bright).toContain('rgba(15, 23, 42');
  });
});