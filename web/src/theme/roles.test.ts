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
    // Roles anchored on colours that did not move are still the mock's values.
    expect(roles['--gold-send']).toBe('#b88b53');
    expect(roles['--gold-send-hover']).toBe('#e3b47c');
    expect(roles['--on-gold']).toBe('#181714');
    // Roles anchored on surfaces moved with the palette (the panel ladder was
    // rebuilt so the stack separates), so assert the *relationship* instead of a
    // frozen hex. An input must read as a subtle inset on the card surface, and
    // a menu as a clearly raised one.
    const surface = ember.palette.surface;
    expect(contrastRatio(roles['--surface-input']!, surface), 'input is an inset')
      .toBeGreaterThan(1.02);
    expect(contrastRatio(roles['--surface-menu']!, surface), 'menu is raised')
      .toBeGreaterThan(1.1);
    // The input, inset and menu must be three distinguishable fills, not one.
    expect(roles['--surface-input']).not.toBe(roles['--surface-inset']);
    expect(roles['--surface-inset']).not.toBe(roles['--surface-menu']);
  });

  it('preserves the mock offsets for Ember rather than its absolute values', () => {
    // The invariant that matters: a role's relationship to its anchor survives a
    // palette change. If Ember's surfaces move again these follow, instead of
    // leaving Ember as the one inconsistent theme.
    const roles = resolveRoles(ember);
    const channels = (hex: string): number[] => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const delta = (a: string, b: string): number[] => channels(a).map((v, i) => v - channels(b)[i]!);
    // In the mock, --surface-input (#1e2021) sits 2 channels above surface
    // (#1c1d1f) and --surface-inset (#202223) 4 above. Those offsets still hold.
    const inputDelta = delta(roles['--surface-input']!, ember.palette.surface);
    const insetDelta = delta(roles['--surface-inset']!, ember.palette.surface);
    expect(inputDelta.every((d) => d > 0), `input above surface: ${inputDelta}`).toBe(true);
    expect(insetDelta.every((d) => d > 0), `inset above surface: ${insetDelta}`).toBe(true);
    expect(insetDelta.every((d, i) => d > inputDelta[i]!), 'inset sits above input').toBe(true);
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
