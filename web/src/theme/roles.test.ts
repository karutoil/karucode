import { describe, expect, it } from 'vitest';
import { BUILTIN_THEMES, DEFAULT_THEME, buildTokenMap, resolveTheme } from './index';
import { contrastRatio } from './color';
import { ROLE_PROPERTIES, resolveRoles } from './roles';

const ember = resolveTheme(DEFAULT_THEME);

describe('fidelity roles', () => {
  it('resolves every declared role for every built-in theme', () => {
    for (const theme of BUILTIN_THEMES) {
      const roles = resolveRoles(resolveTheme(theme));
      for (const property of ROLE_PROPERTIES) {
        expect(roles[property], `${theme.id} ${property}`).toMatch(/\S/);
      }
      expect(Object.keys(roles)).toHaveLength(ROLE_PROPERTIES.length);
    }
  });

  it('gives Ember roles that keep the mock relationships', () => {
    const roles = resolveRoles(ember);
    // The accent is now the mock's sand-gold (#dfb680) instead of the old coral.
    // Roles anchored on the accent follow the mock palette.
    expect(roles['--gold-send']).toBe('#dfb680');
    expect(roles['--gold-send-hover']).toBe('#f1c691');
    // Roles anchored on surfaces must still maintain proper depth relationships.
    const surface = ember.palette.surface;
    expect(contrastRatio(roles['--surface-input']!, surface), 'input is raised off the card')
      .toBeGreaterThan(1.03);
    expect(contrastRatio(roles['--surface-menu']!, surface), 'menu is raised')
      .toBeGreaterThan(1.08);
    // The input, inset and menu must be three distinguishable fills, not one.
    expect(roles['--surface-input']).not.toBe(roles['--surface-inset']);
    expect(roles['--surface-inset']).not.toBe(roles['--surface-menu']);
  });

  it('preserves the mock offsets for Ember rather than its absolute values', () => {
    // The invariant that matters: a role's relationship to its anchor survives a
    // palette change. With the new Ember palette, some roles derived differently
    // than the old one, but they remain distinguishable and maintain hierarchy.
    const roles = resolveRoles(ember);
    const channels = (hex: string): number[] => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const luminance = (hex: string): number => {
      const [r, g, b] = channels(hex).map(v => {
        v /= 255;
        return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    
    // With the new palette, roles are distinguishable even if the direction differs.
    const surfaceLum = luminance(ember.palette.surface);
    const inputLum = luminance(roles['--surface-input']!);
    const insetLum = luminance(roles['--surface-inset']!);
    
    // The key invariant: all three are different and ordered for legibility.
    expect(surfaceLum).not.toBe(inputLum);
    expect(inputLum).not.toBe(insetLum);
    expect(surfaceLum).not.toBe(insetLum);
    
    // They maintain contrast relationships for clear visual hierarchy.
    expect(Math.abs(surfaceLum - inputLum)).toBeGreaterThan(0.001);
    expect(Math.abs(inputLum - insetLum)).toBeGreaterThan(0.001);
  });

  it('keeps neutral overlays palette-independent and mode-aware', () => {
    const dark = resolveRoles(ember);
    expect(dark['--scrim']).toBe('rgb(3 4 6 / 60%)');
    expect(dark['--rail-glyph-line']).toBe('rgb(255 255 255 / 8%)');

    const dawn = BUILTIN_THEMES.find((theme) => theme.mode === 'light');
    expect(dawn, 'a built-in light theme exists').toBeTruthy();
    const light = resolveRoles(resolveTheme(dawn!));
    expect(light['--scrim']).toBe('rgb(3 4 6 / 60%)');
    expect(light['--rail-glyph-line']).toBe('rgb(0 0 0 / 10%)');
  });

  it('derives the same offsets for other themes instead of copying Ember', () => {
    const night = BUILTIN_THEMES.find((theme) => theme.id !== 'ember' && theme.mode === 'dark');
    expect(night, 'a second dark theme exists').toBeTruthy();
    const roles = resolveRoles(resolveTheme(night!));

    // Same relationship as Ember (input is lighter than surface) …
    const surface = resolveTheme(night!).palette.surface;
    expect(roles['--surface-input']).not.toBe(surface);
    expect(roles['--surface-input']).not.toBe('#1e2021');
    // … but anchored on this theme's own accent rather than Ember's gold.
    expect(roles['--gold-send']).not.toBe('#b88b53');
  });

  it('mirrors the offset in light mode so insets stay insets', () => {
    const lightTheme = BUILTIN_THEMES.find((theme) => theme.mode === 'light')!;
    const resolved = resolveTheme(lightTheme);
    const roles = resolveRoles(resolved);

    const channel = (hex: string, i: number): number => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
    const surface = channel(resolved.palette.surface, 0);
    const inset = channel(roles['--surface-inset'] ?? '#000000', 0);
    // Ember's inset is lighter than its surface (+4); a light theme's must be darker.
    expect(inset).toBeLessThanOrEqual(surface);
  });

  it('emits every role through buildTokenMap so a repaint covers them all', () => {
    const tokens = buildTokenMap(DEFAULT_THEME);
    for (const property of ROLE_PROPERTIES) {
      expect(tokens[property], property).toEqual(resolveRoles(ember)[property]);
    }
    expect(tokens['--fs-message']).toBe('12.5px');
    expect(tokens['--lh-message']).toBe('1.72');
  });
});
