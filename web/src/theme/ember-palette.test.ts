// @vitest-environment node
/* Ember, the default theme: the properties that must hold.
 *
 * This file used to assert that Ember matched `karucode-v3-themes.html`
 * byte-for-byte, colour by colour. That contract was the bug. The mock's chrome
 * is cool grey (`#111213` is `h≈210`) under a warm amber accent (`h≈34`), so
 * "match the mock exactly" meant "ship two hue families on one screen" — and
 * because `roles.ts` also returned the mock's *literal* role values for Ember,
 * the cards and inputs came out blue-slate at 20-24% saturation while the shell
 * around them sat at 5%. That is the clash the audit in
 * `docs/internal/DESIGN-SYSTEM.md` §1.1 measures.
 *
 * Byte-fidelity to a mock is not a property worth protecting. What is worth
 * protecting is stated here instead: Ember is warm, its ladder is visible, its
 * ink is legible, and its accent is the brand colour and nothing else.
 */

import { describe, expect, it } from 'vitest';
import { DEFAULT_THEME, findBuiltin } from './builtins';
import { contrastRatio, rgbToHsl } from './color';
import { resolveRoles } from './roles';
import { resolveTheme } from './tokens';

const ember = resolveTheme(DEFAULT_THEME);
const SURFACES = ['bg', 'rail', 'sidebar', 'inspector', 'surface', 'raised', 'hover'] as const;

describe('Ember is a warm graphite theme', () => {
  it('is the default theme', () => {
    expect(DEFAULT_THEME.id).toBe('ember');
    expect(DEFAULT_THEME.mode).toBe('dark');
    expect(findBuiltin('ember')).toBeDefined();
  });

  it('leans warm, not cool: every shell surface has red >= blue', () => {
    // "Warm carbon and sand" is the theme's own description. A cool grey shell
    // under an amber accent is the mismatch this whole change exists to remove.
    for (const key of SURFACES) {
      const hex = ember.palette[key];
      const r = parseInt(hex.slice(1, 3), 16);
      const b = parseInt(hex.slice(5, 7), 16);
      expect(r, `${key}=${hex} should lean warm (r=${r} b=${b})`).toBeGreaterThan(b);
    }
  });

  it('keeps the chrome near-neutral (saturation under 8%)', () => {
    // Warm, but not *tinted*: the accent is the only chromatic voice in the shell.
    for (const key of SURFACES) {
      expect(rgbToHsl(ember.palette[key]).s, `${key} saturation`).toBeLessThanOrEqual(0.08);
    }
  });

  it('sits in one hue family, within 12 degrees', () => {
    const hues = SURFACES.map((key) => rgbToHsl(ember.palette[key]))
      .filter((hsl) => hsl.s >= 0.03)
      .map((hsl) => hsl.h);
    const spread = Math.max(...hues) - Math.min(...hues);
    expect(spread, `hues ${hues.map((h) => Math.round(h)).join(', ')}`).toBeLessThanOrEqual(12);
  });
});

describe('Ember has a visible elevation ladder', () => {
  it('steps at least 2.5 lightness points per elevation level', () => {
    const order = ['bg', 'surface', 'raised', 'hover'] as const;
    const l = order.map((key) => rgbToHsl(ember.palette[key]).l * 100);
    for (let i = 1; i < l.length; i += 1) {
      expect(Math.abs(l[i]! - l[i - 1]!), `${order[i - 1]} → ${order[i]}`).toBeGreaterThanOrEqual(2.5);
    }
  });

  it('separates the side columns from the canvas', () => {
    const canvas = rgbToHsl(ember.palette.bg).l * 100;
    for (const key of ['rail', 'sidebar', 'inspector'] as const) {
      expect(Math.abs(rgbToHsl(ember.palette[key]).l * 100 - canvas), `${key} vs bg`)
        .toBeGreaterThanOrEqual(1.2);
    }
  });
});

describe('Ember keeps the brand colour intact', () => {
  it('sends with the accent itself, not a desaturated approximation', () => {
    // The accent sits at ~60% chroma. A cap in `resolveSaturation()` used to
    // clamp every non-text role to 40%, so `--gold-send` came out #d2b997 — a
    // washed-out tan — instead of the brand amber. The button has to be the brand.
    const roles = resolveRoles(ember);
    expect(roles['--gold-send']).toBe(DEFAULT_THEME.colors.accent);
    expect(roles['--ring-fill']).toBe(DEFAULT_THEME.colors.accent);
    expect(roles['--row-accent-ink']).toBe(DEFAULT_THEME.colors.accent);
  });

  it('keeps ink on the accent fill legible', () => {
    expect(contrastRatio(ember.palette.onAccent, ember.palette.accentText)).toBeGreaterThanOrEqual(
      4.5,
    );
  });
});

describe('Ember surfaces carry no second design system', () => {
  it('paints the transcript card from its own surface, not a blue literal', () => {
    // The regression that started this: `--surface-card` was the literal
    // #13181f (h≈215, s≈24%) inside a shell at s≈5%. A role cut from a surface
    // must continue that surface's hue and hue only.
    const roles = resolveRoles(ember);
    const card = rgbToHsl(roles['--surface-card']!);
    const surface = rgbToHsl(ember.palette.surface);
    expect(Math.abs(card.h - surface.h)).toBeLessThanOrEqual(12);
    expect(card.s).toBeLessThanOrEqual(surface.s + 0.06);
  });

  it('keeps the input, inset and menu as three distinguishable fills', () => {
    const roles = resolveRoles(ember);
    const fills = new Set([roles['--surface-input'], roles['--surface-inset'], roles['--surface-menu']]);
    expect(fills.size).toBe(3);
  });
});