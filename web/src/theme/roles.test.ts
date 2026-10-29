import { describe, expect, it } from 'vitest';
import { BUILTIN_THEMES, DEFAULT_THEME, buildTokenMap, resolveTheme } from './index';
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

  it('gives Ember the mock literals verbatim', () => {
    const roles = resolveRoles(ember);
    expect(roles['--surface-input']).toBe('#1e2021');
    expect(roles['--line-input']).toBe('#3c3d3d');
    expect(roles['--focus-input']).toBe('#866c50');
    expect(roles['--gold-send']).toBe('#b88b53');
    expect(roles['--gold-send-hover']).toBe('#e3b47c');
    expect(roles['--on-gold']).toBe('#181714');
    expect(roles['--surface-inset']).toBe('#202223');
    expect(roles['--surface-menu']).toBe('#252628');
    expect(roles['--line-overlay']).toBe('#4a4c4c');
    expect(roles['--accent-faint']).toBe('#484038');
    expect(roles['--row-selected-bg']).toBe('#343027');
    expect(roles['--text-output']).toBe('#8e9b95');
  });

  it('keeps neutral overlays palette-independent and mode-aware', () => {
    const dark = resolveRoles(ember);
    expect(dark['--scrim']).toBe('rgb(0 0 0 / 47%)');
    expect(dark['--rail-glyph-line']).toBe('rgb(255 255 255 / 8%)');

    const dawn = BUILTIN_THEMES.find((theme) => theme.mode === 'light');
    expect(dawn, 'a built-in light theme exists').toBeTruthy();
    const light = resolveRoles(resolveTheme(dawn!));
    expect(light['--scrim']).toBe('rgb(0 0 0 / 47%)');
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
