/* Palette legibility contract for the built-in themes.
 *
 * The UI is a stack of near-neighbour surfaces (canvas → rail → sidebar →
 * inspector → cards → raised → hover). When those neighbours sit too close in
 * luminance the whole workspace reads as one flat, washed-out colour, which is
 * exactly the regression these tests exist to prevent.
 *
 * Every threshold here is a *minimum separation*, not a fixed colour, so the
 * palettes stay free to carry their own hue identity while being forced to
 * stay legible.
 */

import { describe, expect, it } from 'vitest';
import { BUILTIN_THEMES } from './builtins';
import { contrastRatio } from './color';
import { resolvePalette, resolveTheme } from './tokens';
import { resolveRoles } from './roles';
import type { Theme, ThemeColorKey } from './types';

/** Surfaces in the order they actually meet on screen. */
const LAYERS = ['bg', 'rail', 'sidebar', 'inspector', 'surface', 'raised', 'hover'] as const;

/**
 * The chrome columns all sit directly on the canvas and are divided from one
 * another by a 1px `--line`, not by tone: the rail (left), the sessions sheet and
 * the inspector (right) are three separate columns of one frame, and the mock
 * paints them at 1.020–1.024 apart. What matters is that each one is legible as
 * a column *against the canvas*, so that is what is pinned here.
 */
const CHROME_COLUMNS = ['rail', 'sidebar', 'inspector'] as const;

/**
 * Minimum contrast between adjacent layers.
 *
 * The ladder has two halves, and they are held to different floors:
 *
 * * **Chrome** (`bg → rail → sidebar → inspector`) is the mock's near-black
 *   frame. On a `#08090b` canvas a 1.07 step per column would put the *card*
 *   surface around 30% grey — the "bright grey app" the mock exists to avoid.
 *   Chrome separates at 1.017–1.04 and is told apart by its 1px `--line` and by
 *   position, which is how the mock draws it (its own steps are 1.020–1.024).
 * * **Content** (`inspector → surface → raised → hover`) is where the floor
 *   matters: cards, menus and hover states are objects that must read as
 *   distinct from what they sit on, and the washed-out regression was exactly
 *   these collapsing. Dark themes take a full 1.06 step there, light themes 1.04.
 */
const CHROME_LAYERS = 4;

const MIN_LAYER_STEP: Record<Theme['mode'], number> = { dark: 1.06, light: 1.04 };

/** Chrome columns are a frame, not objects: only a visible hairline is required. */
const MIN_CHROME_STEP = 1.015;

/** HSL saturation, 0–1. A dark UI must not be built out of pure grey. */
function hueSaturation(hex: string): number {
  return saturation(hex);
}

/** Shortest distance between two hues, in degrees (0–180). */
function hueDistance(a: string, b: string): number {
  const delta = Math.abs(hue(a) - hue(b));
  return Math.min(delta, 360 - delta);
}

function saturation(hex: string): number {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const lightness = (max + min) / 2;
  const delta = max - min;
  if (delta === 0) return 0;
  return lightness > 0.5 ? delta / (2 - max - min) : delta / (max + min);
}

/** Hue in degrees, 0–360. */
function hue(hex: string): number {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  if (delta === 0) return 0;
  let h: number;
  if (max === r) h = ((g - b) / delta + (g < b ? 6 : 0)) / 6;
  else if (max === g) h = ((b - r) / delta + 2) / 6;
  else h = ((r - g) / delta + 4) / 6;
  return h * 360;
}

describe('built-in palette legibility', () => {
  it('separates every adjacent surface layer', () => {
    for (const theme of BUILTIN_THEMES) {
      const floor = MIN_LAYER_STEP[theme.mode];
      // Content layers: cards, menus and hover states are objects on a surface.
      for (let index = CHROME_LAYERS; index < LAYERS.length; index += 1) {
        const lower = theme.colors[LAYERS[index - 1]!];
        const upper = theme.colors[LAYERS[index]!];
        const ratio = contrastRatio(lower, upper);
        expect(
          ratio,
          `${theme.id}: ${LAYERS[index - 1]} → ${LAYERS[index]} is ${ratio.toFixed(3)}:1, ` +
            `needs ≥ ${floor}:1 or the two surfaces read as one colour`,
        ).toBeGreaterThanOrEqual(floor);
      }
    }
  });

  it('lifts every chrome column off the canvas', () => {
    for (const theme of BUILTIN_THEMES) {
      for (const column of CHROME_COLUMNS) {
        const ratio = contrastRatio(theme.colors.bg, theme.colors[column]);
        expect(
          ratio,
          `${theme.id}: ${column} on the canvas is ${ratio.toFixed(3)}:1, ` +
            `needs ≥ ${MIN_CHROME_STEP}:1 or the column dissolves into the background`,
        ).toBeGreaterThanOrEqual(MIN_CHROME_STEP);
      }
    }
  });

  it('draws borders that are actually visible against their surface', () => {
    for (const theme of BUILTIN_THEMES) {
      const ratio = contrastRatio(theme.colors.border, theme.colors.surface);
      expect(ratio, `${theme.id}: border on surface`).toBeGreaterThanOrEqual(1.25);
    }
  });

  it('keeps body and secondary text readable on cards, not just on the canvas', () => {
    for (const theme of BUILTIN_THEMES) {
      const { text, muted } = theme.colors;
      for (const layer of ['bg', 'surface', 'raised'] as const) {
        const background = theme.colors[layer];
        expect(contrastRatio(text, background), `${theme.id}: text on ${layer}`)
          .toBeGreaterThanOrEqual(7);
        expect(contrastRatio(muted, background), `${theme.id}: muted on ${layer}`)
          .toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it('keeps the accent legible as a label on every surface it is used on', () => {
    for (const theme of BUILTIN_THEMES) {
      const palette = resolvePalette(theme);
      // The raw accent is used for fills and glyphs; accentText is the form the
      // engine derives when small accent text needs to reach AA.
      for (const layer of ['bg', 'surface', 'raised', 'sidebar'] as const) {
        const ratio = contrastRatio(palette.accentText, theme.colors[layer]);
        expect(ratio, `${theme.id}: accentText on ${layer}`).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(palette.accent, theme.colors.surface), `${theme.id}: accent on surface`)
        .toBeGreaterThanOrEqual(4);
    }
  });

  it('gives dark surfaces a deliberate cast instead of dead neutral grey', () => {
    for (const theme of BUILTIN_THEMES) {
      if (theme.mode !== 'dark') continue;
      // Ember is "warm carbon" and Graphite is deliberately monochrome, so both
      // sit at a subtle 4-6% cast — compare Radix dark (5.9%) and VS Code
      // (1.3%): a faint tint is normal and intentional. The tinted themes must
      // carry a real hue. What this guards against is a surface that is exactly
      // 0% (a pure grey) or so faint it reads as one.
      const floor = theme.id === 'ember' || theme.id === 'graphite' ? 0.035 : 0.1;
      for (const layer of ['sidebar', 'surface', 'raised'] as const) {
        const sat = saturation(theme.colors[layer]);
        expect(sat, `${theme.id}: ${layer} saturation`).toBeGreaterThanOrEqual(floor);
      }
    }
  });

  it('keeps status colours distinguishable from each other', () => {
    for (const theme of BUILTIN_THEMES) {
      // The semantic job is that success reads green, danger reads red and info
      // reads blue — not that they stay far from the accent. A cobalt-accented
      // theme legitimately has a blue info colour; Ember's warm palette puts
      // its salmon danger and gold accent close on the wheel by design.
      // 40° is the floor: below that a success and an info chip read as the
      // same green-teal, which is the "one colour" complaint on status pills.
      for (const [a, b] of [
        ['success', 'danger'],
        ['success', 'info'],
        ['danger', 'info'],
      ] as const) {
        const delta = Math.abs(hue(theme.colors[a]) - hue(theme.colors[b]));
        const distance = Math.min(delta, 360 - delta);
        expect(distance, `${theme.id}: ${a} vs ${b} hue`).toBeGreaterThanOrEqual(40);
      }
    }
  });

  it('separates the syntax colours so highlighted code is not monochrome', () => {
    // Syntax tokens carry meaning through hue, not lightness: keyword and string
    // often sit at the same luminance (Ember: gold keyword, mint string) and are
    // told apart by hue alone. Comparing contrast here would be the wrong axis.
    const pairs: ReadonlyArray<readonly [ThemeColorKey, ThemeColorKey]> = [
      ['syntaxKeyword', 'syntaxString'],
      ['syntaxString', 'syntaxComment'],
      ['syntaxKeyword', 'syntaxComment'],
    ];
    for (const theme of BUILTIN_THEMES) {
      const colors = theme.colors;
      for (const [a, b] of pairs) {
        const delta = Math.abs(hue(colors[a]) - hue(colors[b]));
        const hueDistance = Math.min(delta, 360 - delta);
        const lightnessDistance = contrastRatio(colors[a], colors[b]);
        // A *comment* is meant to recede, so it is allowed to sit close in hue
        // as long as it is clearly dimmer. Anything else needs a distinct hue.
        const isComment = a === 'syntaxComment' || b === 'syntaxComment';
        const minHue = isComment ? 20 : 40;
        const minLightness = isComment ? 1.15 : 1.3;
        expect(
          hueDistance >= minHue || lightnessDistance >= minLightness,
          `${theme.id}: ${a} vs ${b} (hue ${hueDistance.toFixed(0)}°, lightness ${lightnessDistance.toFixed(2)})`,
        ).toBe(true);
      }
    }
  });
});

describe('derived role layer legibility', () => {
  it('lifts structural roles off the surface they are painted on', () => {
    for (const theme of BUILTIN_THEMES) {
      const resolved = resolveTheme(theme);
      const roles = resolveRoles(resolved);
      const { surface } = resolved.palette;

      // An input box, a menu and an inset card have to be visible as separate
      // objects, otherwise the composer and the panels vanish into the canvas —
      // the "washed out" complaint at its most literal. Calibration: GitHub's
      // input sits 1.085 off its card, VS Code's menu 1.089. A *subtle* inset
      // (1.01+) is correct for an input. A menu must read as clearly raised in
      // dark mode; on a near-white surface it can only darken slightly, which is
      // how every light design system expresses elevation.
      expect(contrastRatio(roles['--surface-input']!, surface), `${theme.id}: input`).toBeGreaterThan(1.01);
      expect(contrastRatio(roles['--surface-inset']!, surface), `${theme.id}: inset`).toBeGreaterThan(1.01);
      const menuFloor = theme.mode === 'dark' ? 1.1 : 1.01;
      expect(contrastRatio(roles['--surface-menu']!, surface), `${theme.id}: menu`).toBeGreaterThan(menuFloor);
    }
  });

  it('keeps accent and status tints coloured, not washed to grey', () => {
    // Regression: accentBg/statusBg used to be a plain sRGB mix of surface and
    // accent. Once the surfaces were lifted off near-black the mix collapsed
    // toward the surface's own hue and the tint went grey (Ember: 8.8%
    // saturation), so selected rows and status pills lost their colour — the
    // "washed out" complaint in its second form.
    for (const theme of BUILTIN_THEMES) {
      const palette = resolvePalette(theme);
      const accentSat = saturation(theme.colors.accent);
      for (const [name, tint] of [
        ['accentBg', palette.accentBg],
        ['successBg', palette.successBg],
        ['dangerBg', palette.dangerBg],
      ] as const) {
        const source = name === 'accentBg' ? theme.colors.accent : theme.colors[name === 'successBg' ? 'success' : 'danger'];
        const tintSat = saturation(tint);
        // The tint keeps a real fraction of its source's chroma …
        expect(tintSat, `${theme.id}: ${name} saturation`).toBeGreaterThan(
          saturation(source) * 0.25,
        );
        // … and, where the accent is itself saturated, is not a grey.
        if (accentSat > 0.3) {
          expect(tintSat, `${theme.id}: ${name} not grey`).toBeGreaterThan(0.06);
        }
        // It must also read as a distinct surface, not blend into the card.
        // Calibration: GitHub's input sits 1.085 off its card. A deliberately
        // muted tint (Graphite's near-neutral danger) can sit just under that,
        // so 1.06 is the floor — below it the tint is invisible.
        expect(contrastRatio(tint, palette.surface), `${theme.id}: ${name} vs surface`)
          .toBeGreaterThan(1.06);
      }
    }
  });

  it('keeps every surface role off the panel it is painted on', () => {
    // Regression: roles were derived by applying Ember's per-channel offset to
    // the theme's own anchor. "One channel darker" is a visible 1.008 step on
    // near-black but clamps to nothing on a pure-white surface, so light themes'
    // cards, diff rows and review blocks collapsed onto the panel at exactly
    // 1.000 — surfaces you could not see at all. The derivation now preserves
    // the contrast *ratio*, which survives both ends of the ramp.
    const SURFACE_ROLES = [
      '--surface-input',
      '--surface-inset',
      '--surface-preview',
      '--surface-card',
      '--surface-command',
      '--surface-symbol',
      '--surface-user',
      '--surface-diff',
      '--surface-diff-ctx',
      '--surface-toast',
      '--chip-surface',
      '--surface-avatar',
    ] as const;

    for (const theme of BUILTIN_THEMES) {
      const resolved = resolveTheme(theme);
      const roles = resolveRoles(resolved);
      const { surface } = resolved.palette;
      for (const role of SURFACE_ROLES) {
        const ratio = contrastRatio(roles[role]!, surface);
        expect(ratio, `${theme.id}: ${role} vs surface`).toBeGreaterThan(1.02);
      }
    }
  });

  it('keeps secondary text below primary text in the hierarchy', () => {
    // Regression: preserving role *ratios* compounded when an anchor got
    // lighter. Ember's --text-body is 1.84x its muted colour, so once muted
    // moved up, body text rose to near-white and every text level collapsed into
    // one brightness — "it feels like one colour" for the type ramp.
    const SECONDARY = [
      '--text-body',
      '--text-command',
      '--text-output',
      '--text-caption',
      '--text-kicker',
      '--text-menu-item',
      '--text-empty-copy',
      '--text-subtle',
    ] as const;
    for (const theme of BUILTIN_THEMES) {
      const resolved = resolveTheme(theme);
      const roles = resolveRoles(resolved);
      const { surface, text } = resolved.palette;
      const primary = contrastRatio(text, surface);
      for (const role of SECONDARY) {
        const ratio = contrastRatio(roles[role]!, surface);
        expect(
          ratio,
          `${theme.id}: ${role} (${ratio.toFixed(2)}) must stay under primary text (${primary.toFixed(2)})`,
        ).toBeLessThan(primary * 0.97);
      }
    }
  });

  it('keeps every derived role in its anchor colour\u2019s hue family', () => {
    // Regression, and the bug behind "the new themes look like shit": roles were
    // derived from the mock literal's own hue, and those literals are neutral
    // grey. So every theme inherited Ember's cool grey — Sakura ended up with
    // blue-grey cards 33-129\u00b0 off its own plum surface, i.e. a plum shell wrapped
    // around a grey transcript. A fill must continue the surface it is cut from.
    const groups: Array<[string, readonly string[]]> = [
      ['surface', ['--surface-card', '--surface-command', '--surface-input', '--surface-inset', '--surface-preview', '--surface-user']],
      ['raised', ['--surface-symbol', '--surface-toast']],
    ];

    for (const theme of BUILTIN_THEMES) {
      const resolved = resolveTheme(theme);
      const roles = resolveRoles(resolved);
      const palette = resolved.palette;
      for (const [anchor, roleNames] of groups) {
        const anchorColor = palette[anchor as keyof typeof palette] as string;
        // A near-neutral anchor has no meaningful hue to continue.
        if (hueSaturation(anchorColor) < 0.05) continue;
        for (const role of roleNames) {
          const distance = hueDistance(roles[role]!, anchorColor);
          expect(
            distance,
            `${theme.id}: ${role} is ${distance.toFixed(0)}\u00b0 off ${anchor} (${anchorColor})`,
          ).toBeLessThan(30);
        }
      }
    }
  });

  it('keeps the derived line and faint roles visible but subordinate', () => {
    for (const theme of BUILTIN_THEMES) {
      const palette = resolvePalette(theme);
      // Lines must be visible on a card …
      expect(contrastRatio(palette.line, palette.surface), `${theme.id}: line`).toBeGreaterThan(1.12);
      expect(contrastRatio(palette.lineSoft, palette.surface), `${theme.id}: lineSoft`).toBeGreaterThan(1.04);
      // … and faint text must still be readable, not decorative.
      expect(contrastRatio(palette.faint, palette.surface), `${theme.id}: faint`).toBeGreaterThan(2);
    }
  });
});
