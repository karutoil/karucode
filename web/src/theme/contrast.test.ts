// @vitest-environment node
/* The design system's hard floors, as a test.
 *
 * "The UI feels washed out and the designs clash" is measurable, and these are
 * the measurements. Every assertion here failed against the stylesheet as it was
 * in November 2026:
 *
 *   - 30 of 85 text nodes on the default screen were below 4.5:1, and they were
 *     the 9-10px ones (a `Ctrl N` shortcut at 3.14:1, session metadata at 3.41:1);
 *   - the shell painted near-neutral chrome at 4-6% saturation beside "fidelity
 *     role" surfaces at 20-24% saturation in a different hue family, which is
 *     what "designs are clashing" looks like numerically;
 *   - adjacent surfaces in the elevation ladder differed by 1-2 lightness points,
 *     below the threshold where a step is visible at all.
 *
 * The floors are the contract in `docs/internal/DESIGN-SYSTEM.md` §4. A theme
 * that fails one is a bug in `roles.ts` or in the palette. Do not lower a floor
 * to make a test pass.
 */

import { describe, expect, it } from 'vitest';
import { BUILTIN_THEMES, DEFAULT_THEME } from './builtins';
import { contrastRatio, rgbToHsl } from './color';
import { ROLE_ANCHORS, resolveRoles } from './roles';
import { resolveTheme } from './tokens';
import type { ResolvedTheme } from './types';

/** Every surface a piece of text can legitimately sit on. */
function surfaces(theme: ResolvedTheme): Array<[string, string]> {
  const p = theme.palette;
  return [
    ['bg', p.bg],
    ['rail', p.rail],
    ['sidebar', p.sidebar],
    ['inspector', p.inspector],
    ['surface', p.surface],
    ['raised', p.raised],
    ['hover', p.hover],
  ];
}

const themes = BUILTIN_THEMES.map((theme) => resolveTheme(theme));

describe('contrast floors (§4)', () => {
  describe.each(themes.map((theme) => [theme.id, theme] as const))('%s', (id, theme) => {
    const p = theme.palette;

    it('primary text is 11:1 on every surface', () => {
      // WCAG AAA for body text is 7:1. 11:1 is deliberately beyond it: in a dense
      // tool UI the primary voice has to stay dominant on the *lightest* surface,
      // which is where the old ladder and old ink both gave up.
      for (const [name, colour] of surfaces(theme)) {
        expect(contrastRatio(p.text, colour), `${id}: text on ${name}`).toBeGreaterThanOrEqual(11);
      }
    });

    it('secondary text is 7:1 on every surface', () => {
      for (const [name, colour] of surfaces(theme)) {
        expect(
          contrastRatio(p.textSecondary, colour),
          `${id}: text-secondary on ${name}`,
        ).toBeGreaterThanOrEqual(7);
      }
    });

    it('muted text is 4.5:1 on every surface', () => {
      // `--muted` is the *readable* quiet voice: metadata, captions, hints.
      // Anything a user must read uses this at minimum.
      for (const [name, colour] of surfaces(theme)) {
        expect(contrastRatio(p.muted, colour), `${id}: muted on ${name}`).toBeGreaterThanOrEqual(4.5);
      }
    });

    it('faint is decorative only: 3:1 against the surfaces it is used on', () => {
      for (const [name, colour] of surfaces(theme)) {
        expect(contrastRatio(p.faint, colour), `${id}: faint on ${name}`).toBeGreaterThanOrEqual(3);
      }
    });

    it('accent text is 4.5:1 on the surfaces it paints', () => {
      for (const [name, colour] of [
        ['surface', p.surface],
        ['sidebar', p.sidebar],
        ['inspector', p.inspector],
        ['accent-bg', p.accentBg],
        ['bg', p.bg],
      ] as Array<[string, string]>) {
        expect(
          contrastRatio(p.accentText, colour),
          `${id}: accent-text on ${name}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    });

    it('ink on an accent fill is 4.5:1', () => {
      expect(contrastRatio(p.onAccent, p.accentText), `${id}: on-accent over accent-fill`)
        .toBeGreaterThanOrEqual(4.5);
    });

    it('status colours are readable on the surface they tint', () => {
      for (const [label, ink, fill] of [
        ['success', p.success, p.successBg],
        ['danger', p.danger, p.dangerBg],
        ['warn', p.warn, p.bg],
        ['info', p.info, p.bg],
      ] as Array<[string, string, string]>) {
        expect(contrastRatio(ink, fill), `${id}: ${label}`).toBeGreaterThanOrEqual(4.5);
      }
    });
  });
});

describe('the elevation ladder (§3.1)', () => {
  // Depth is only legible if a step is big enough to survive an 8-bit panel and
  // a dim screen. One lightness point is not: the old ladder spanned 7%→12%
  // across five surfaces, so everything read as a single flat field.
  // Elevation is not one direction in both modes. A dark theme lifts a card off
  // its canvas by getting lighter; a light theme does the same. But the planes
  // that sit *over or inside* a card (a popover, a hovered row) step away from it,
  // and that direction has to be consistent — which is what the reviewer found
  // reversed in light mode (surface L100 → raised L90 → hover L87, i.e. "raised"
  // below the page it floats over).
  const MIN_STEP = 2.5;

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: the canvas and the card are clearly separate planes',
    (id, theme) => {
      const p = theme.palette;
      const step = Math.abs(rgbToHsl(p.surface).l - rgbToHsl(p.bg).l) * 100;
      expect(step, `${id}: bg → surface (${rgbToHsl(p.bg).l} → ${rgbToHsl(p.surface).l})`)
        .toBeGreaterThanOrEqual(MIN_STEP);
    },
  );

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: raised and hover step away from the card, one direction, 2.5+ points each',
    (id, theme) => {
      const p = theme.palette;
      const order = ['surface', 'raised', 'hover'] as const;
      const l = order.map((key) => rgbToHsl(p[key]).l * 100);
      const steps = l.slice(1).map((value, index) => value - l[index]!);
      const sign = Math.sign(steps[0]!);
      for (const [index, step] of steps.entries()) {
        expect(Math.sign(step), `${id}: step ${index + 1} reverses (${steps.join(', ')})`).toBe(sign);
        expect(Math.abs(step), `${id}: step ${index + 1} too small (${steps.join(', ')})`)
          .toBeGreaterThanOrEqual(MIN_STEP);
      }
    },
  );

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: every rail and panel is distinguishable from the canvas',
    (_id, theme) => {
      const p = theme.palette;
      const canvas = rgbToHsl(p.bg).l * 100;
      for (const key of ['rail', 'sidebar', 'inspector'] as const) {
        expect(Math.abs(rgbToHsl(p[key]).l * 100 - canvas), `${_id}: ${key} vs bg (${canvas})`)
          .toBeGreaterThanOrEqual(1.2);
      }
    },
  );
});

describe('one hue family per theme (§2)', () => {
  /* The regression this whole document exists for.
   *
   * Ember's chrome was near-neutral grey (4-6% saturation) while its cards,
   * inputs and insets were blue-slate at 20-24% saturation in a *different* hue
   * family — a second design system smuggled in through the role layer. A plain
   * hue test misses it (blue is `info`, so it is "in the palette"); the tell is
   * that a fill must continue the fill it is cut from.
   *
   * So the rule is per-role and relative: a surface role may not drift from its
   * own anchor in hue, and may not be dramatically more chromatic than it.
   */
  const SURFACE_ANCHORS = new Set(['bg', 'rail', 'sidebar', 'inspector', 'surface', 'raised', 'hover']);
  const HUE_TOLERANCE = 18;
  const CHROMA_TOLERANCE = 0.1;

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: every surface role continues the hue of its anchor',
    (id, theme) => {
      const roles = resolveRoles(theme);
      const offenders: string[] = [];

      for (const [property, anchor] of Object.entries(ROLE_ANCHORS)) {
        if (!SURFACE_ANCHORS.has(anchor)) continue;
        const value = roles[property];
        if (!value || !/^#[0-9a-f]{6}$/i.test(value)) continue;
        const role = rgbToHsl(value);
        const source = rgbToHsl(theme.palette[anchor as keyof typeof theme.palette]);
        // A near-grey has no meaningful hue and cannot clash with anything.
        if (role.s < 0.06) continue;
        const drift = Math.min(
          Math.abs(role.h - source.h),
          360 - Math.abs(role.h - source.h),
        );
        if (drift > HUE_TOLERANCE) {
          offenders.push(
            `${property}=${value} (h=${Math.round(role.h)}) vs ${anchor} (h=${Math.round(source.h)}): ${Math.round(drift)}° off`,
          );
        }
      }

      expect(offenders, `${id}: surface roles in a different hue family`).toEqual([]);
    },
  );

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: no surface role is dramatically more chromatic than its anchor',
    (id, theme) => {
      const roles = resolveRoles(theme);
      const offenders: string[] = [];

      for (const [property, anchor] of Object.entries(ROLE_ANCHORS)) {
        if (!SURFACE_ANCHORS.has(anchor)) continue;
        const value = roles[property];
        if (!value || !/^#[0-9a-f]{6}$/i.test(value)) continue;
        const role = rgbToHsl(value);
        const source = rgbToHsl(theme.palette[anchor as keyof typeof theme.palette]);
        if (role.s > source.s + CHROMA_TOLERANCE) {
          offenders.push(
            `${property}=${value} (s=${Math.round(role.s * 100)}%) vs ${anchor} (s=${Math.round(source.s * 100)}%)`,
          );
        }
      }

      expect(offenders, `${id}: surface roles far more saturated than their anchor`).toEqual([]);
    },
  );

  it.each(themes.map((theme) => [theme.id, theme] as const))(
    '%s: the palette itself stays in one hue family',
    (id, theme) => {
      // Every surface the theme paints shares a hue, give or take a lean. A
      // palette with a warm canvas and a blue inspector is the root defect.
      const hues = (['bg', 'rail', 'sidebar', 'inspector', 'surface', 'raised', 'hover'] as const)
        .map((key) => rgbToHsl(theme.palette[key]))
        .filter((hsl) => hsl.s >= 0.03)
        .map((hsl) => hsl.h);
      if (hues.length < 2) return;

      // Circular mean, then the worst deviation from it.
      const radians = hues.map((h) => (h * Math.PI) / 180);
      const mean = Math.atan2(
        radians.reduce((a, b) => a + Math.sin(b), 0),
        radians.reduce((a, b) => a + Math.cos(b), 0),
      );
      const meanDeg = ((mean * 180) / Math.PI + 360) % 360;
      for (const hue of hues) {
        const drift = Math.min(Math.abs(hue - meanDeg), 360 - Math.abs(hue - meanDeg));
        expect(drift, `${id}: surface hue ${Math.round(hue)} vs family ${Math.round(meanDeg)}`)
          .toBeLessThanOrEqual(25);
      }
    },
  );
});

describe('the default theme is the one the stylesheet promises', () => {
  it('is Ember, dark, and matches the first-paint block', () => {
    expect(DEFAULT_THEME.id).toBe('ember');
    expect(DEFAULT_THEME.mode).toBe('dark');
  });
});