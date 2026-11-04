/* Fidelity role layer.
 *
 * The v3 mock was produced by flattening literal colours into per-component
 * declarations, so its stylesheet uses ~300 bespoke shades that are *not* in
 * the palette (`#3c3d3d` composer border, `#202223` inset cards, `#b88b53`
 * send button, `#8e9b95` tool output …). Mapping those onto the nearest palette
 * token is what made the first pass read "close but not identical": every
 * border came out a step too dim and every surface a step too dark.
 *
 * This module gives those shades names, once:
 *
 *   - `EMBER_ROLES` holds the mock's literal value for every role (the mock is
 *     the Ember theme, so these are exact);
 *   - every other theme — and every light theme — derives the same role by
 *     applying Ember's *offset* to its own palette colour, mirrored in light
 *     mode (`surface + #000004` becomes `surface - #000004`).
 *
 * The result is a stable, theme-correct set of roles: Ember is pixel-identical
 * to the mock, the other seven built-ins keep their own character, and a user's
 * custom theme cannot end up with a dark input box in a light workspace.
 */
import { hslToHex, relativeLuminance, rgbToHsl } from './color';
import type { ResolvedPalette, ResolvedTheme, ThemeMode } from './types';

/** Palette colours a role can sit on. */
type Anchor =
  | 'bg'
  | 'rail'
  | 'sidebar'
  | 'inspector'
  | 'surface'
  | 'raised'
  | 'hover'
  | 'line'
  | 'lineStrong'
  | 'text'
  | 'textSecondary'
  | 'muted'
  | 'faint'
  | 'accent'
  | 'accentHi'
  | 'accentBg'
  | 'accentLine'
  | 'success'
  | 'danger'
  | 'info';

interface RoleSpec {
  /** CSS custom property, also the public name components use. */
  property: string;
  /** Palette colour the mock's value is a variation of. */
  anchor: Anchor;
  /** Literal value in the mock (Ember, dark). */
  ember: string;
  /**
   * `alpha` roles are neutral overlays (a white hairline, a black scrim): they
   * must NOT follow the palette, only the mode.
   */
  kind?: 'colour' | 'alpha';
  /** Value to use in light mode for `alpha` roles (defaults to `ember`). */
  emberLight?: string;
}

/* prettier-ignore */
const ROLES: readonly RoleSpec[] = [
  /* ---- composer / inputs ---------------------------------------------- */
  { property: '--surface-input', anchor: 'surface', ember: '#1b212a' },
  { property: '--line-input', anchor: 'lineStrong', ember: '#333c48' },
  { property: '--focus-input', anchor: 'accentLine', ember: '#58452e' },
  { property: '--input-ink', anchor: 'text', ember: '#e9ebf0' },
  { property: '--input-placeholder', anchor: 'faint', ember: '#6d7683' },
  { property: '--surface-plus-hover', anchor: 'hover', ember: '#252c37' },
  { property: '--surface-hover-strong', anchor: 'hover', ember: '#222933' },
  { property: '--line-hair', anchor: 'line', ember: '#252c37' },
  { property: '--gold-send', anchor: 'accent', ember: '#dfb680' },
  { property: '--gold-send-hover', anchor: 'accentHi', ember: '#f1c691' },
  { property: '--on-gold', anchor: 'bg', ember: '#1c0f06' },
  { property: '--foot-ink', anchor: 'faint', ember: '#6d7683' },
  { property: '--foot-strong', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--ring-fill', anchor: 'accent', ember: '#dfb680' },
  { property: '--ring-track', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--chip-surface', anchor: 'raised', ember: '#1d232c' },
  { property: '--chip-line', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--chip-ink', anchor: 'textSecondary', ember: '#b7bdc8' },

  /* ---- menus / popovers ------------------------------------------------ */
  { property: '--surface-menu', anchor: 'raised', ember: '#20262f' },
  { property: '--line-menu', anchor: 'lineStrong', ember: '#333b46' },
  { property: '--text-menu-title', anchor: 'muted', ember: '#8b92a1' },
  { property: '--text-menu-item', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--text-menu-meta', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--row-accent-bg', anchor: 'accentBg', ember: '#32291e' },
  { property: '--row-accent-ink', anchor: 'accent', ember: '#dfb680' },

  /* ---- inspector cards & fields ---------------------------------------- */
  { property: '--surface-inset', anchor: 'surface', ember: '#1f2630' },
  { property: '--line-inset', anchor: 'line', ember: '#252c37' },
  { property: '--line-card', anchor: 'lineStrong', ember: '#2b333e' },
  { property: '--line-divider', anchor: 'line', ember: '#222831' },
  { property: '--line-keycap', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--line-overlay', anchor: 'lineStrong', ember: '#333b46' },
  { property: '--text-caption', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--text-subtle', anchor: 'faint', ember: '#6d7683' },
  { property: '--text-kicker', anchor: 'muted', ember: '#8b92a1' },
  { property: '--text-kicker-panel', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--tab-active-text', anchor: 'text', ember: '#e9ebf0' },
  { property: '--tab-hover', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--row-selected-bg', anchor: 'accentBg', ember: '#32291e' },
  { property: '--row-selected-text', anchor: 'accent', ember: '#dfb680' },
  { property: '--surface-preview', anchor: 'surface', ember: '#1b212a' },
  { property: '--line-preview', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--icon-folder', anchor: 'accent', ember: '#c79a6b' },
  { property: '--icon-rust', anchor: 'accent', ember: '#d8a178' },
  { property: '--icon-file', anchor: 'muted', ember: '#8b92a1' },
  { property: '--icon-md', anchor: 'info', ember: '#8fb3f7' },
  { property: '--icon-path', anchor: 'accent', ember: '#a08a72' },
  { property: '--icon-quiet', anchor: 'faint', ember: '#6d7683' },
  { property: '--icon-pill-down', anchor: 'faint', ember: '#6d7683' },
  { property: '--btn-accent-bg', anchor: 'accentBg', ember: '#32291e' },
  { property: '--btn-accent-bg-hover', anchor: 'accentBg', ember: '#3a2b1d' },
  { property: '--btn-accent-line', anchor: 'accentLine', ember: '#58452e' },
  { property: '--btn-accent-text', anchor: 'accent', ember: '#dfb680' },
  { property: '--toggle-track', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--toggle-knob', anchor: 'faint', ember: '#6d7683' },
  { property: '--toggle-on-track', anchor: 'accent', ember: '#dfb680' },
  { property: '--toggle-on-knob', anchor: 'accentHi', ember: '#f1c691' },

  /* ---- transcript & cards ---------------------------------------------- */
  { property: '--surface-card', anchor: 'surface', ember: '#13181f' },
  { property: '--line-card-strong', anchor: 'lineStrong', ember: '#2b333e' },
  { property: '--surface-command', anchor: 'surface', ember: '#13181f' },
  { property: '--line-command', anchor: 'lineStrong', ember: '#2b333e' },
  { property: '--surface-command-hover', anchor: 'hover', ember: '#171c23' },
  { property: '--surface-command-output', anchor: 'bg', ember: '#0a0d11' },
  { property: '--text-command', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--text-command-output', anchor: 'success', ember: '#8fdcb7' },
  { property: '--surface-symbol', anchor: 'raised', ember: '#212831' },
  { property: '--line-symbol', anchor: 'line', ember: '#232a33' },
  { property: '--text-symbol', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--text-output', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--text-output-num', anchor: 'faint', ember: '#6d7683' },
  { property: '--text-expand', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--text-body', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--text-date', anchor: 'faint', ember: '#6d7683' },
  { property: '--text-user', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--surface-user', anchor: 'surface', ember: '#101318' },
  { property: '--surface-avatar', anchor: 'raised', ember: '#252c37' },
  { property: '--line-avatar', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--text-avatar', anchor: 'textSecondary', ember: '#b7bdc8' },
  { property: '--surface-avatar-agent', anchor: 'accentBg', ember: '#32291e' },
  { property: '--line-avatar-agent', anchor: 'accentLine', ember: '#58452e' },
  { property: '--surface-diff', anchor: 'surface', ember: '#0a0d11' },
  { property: '--line-diff', anchor: 'lineStrong', ember: '#232a33' },
  { property: '--line-diff-head', anchor: 'line', ember: '#222831' },
  { property: '--surface-diff-add', anchor: 'success', ember: '#0f2a20' },
  { property: '--surface-diff-del', anchor: 'danger', ember: '#2e1a1a' },
  { property: '--surface-diff-ctx', anchor: 'surface', ember: '#0d1014' },
  { property: '--text-review', anchor: 'accent', ember: '#dfb680' },
  { property: '--surface-review', anchor: 'accentBg', ember: '#32291e' },
  { property: '--surface-review-hi', anchor: 'accentBg', ember: '#32291e' },

  /* ---- empty state ------------------------------------------------------ */
  { property: '--text-empty-title', anchor: 'text', ember: '#e9ebf0' },
  { property: '--text-empty-copy', anchor: 'muted', ember: '#9aa3b2' },
  { property: '--text-empty-symbol', anchor: 'accent', ember: '#dfb680' },

  /* ---- toasts ----------------------------------------------------------- */
  { property: '--surface-toast', anchor: 'raised', ember: '#252c37' },
  { property: '--line-toast', anchor: 'lineStrong', ember: '#3a4350' },
  { property: '--text-toast', anchor: 'text', ember: '#e9ebf0' },

  /* ---- rail ------------------------------------------------------------- */
  { property: '--rail-glyph-text', anchor: 'muted', ember: '#8b92a1' },
  { property: '--rail-add-line', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--rail-add-line-hover', anchor: 'accentLine', ember: '#58452e' },
  { property: '--rail-avatar-bg', anchor: 'raised', ember: '#252c37' },
  { property: '--rail-avatar-line', anchor: 'lineStrong', ember: '#2c333d' },
  { property: '--rail-avatar-text', anchor: 'textSecondary', ember: '#b7bdc8' },

  /* ---- accent family ---------------------------------------------------- */
  { property: '--accent-faint', anchor: 'accentLine', ember: '#58452e' },
  { property: '--accent-surface', anchor: 'accentBg', ember: '#32291e' },
  { property: '--accent-text-soft', anchor: 'accentHi', ember: '#f1c691' },
  { property: '--accent-hint', anchor: 'accent', ember: '#c08c62' },
  { property: '--field-focus-line', anchor: 'accentLine', ember: '#58452e' },
  { property: '--item-active-to', anchor: 'accentBg', ember: '#32291e' },
  { property: '--meta-text', anchor: 'muted', ember: '#8b92a1' },

  /* ---- neutral overlays: mode-dependent, palette-independent ------------ */
  { property: '--rail-glyph-line', anchor: 'text', ember: 'rgb(255 255 255 / 8%)', kind: 'alpha', emberLight: 'rgb(0 0 0 / 10%)' },
  { property: '--inset-hairline', anchor: 'text', ember: 'rgb(255 255 255 / 3%)', kind: 'alpha', emberLight: 'rgb(255 255 255 / 60%)' },
  { property: '--scrim', anchor: 'bg', ember: 'rgb(3 4 6 / 60%)', kind: 'alpha' },
  { property: '--scrim-soft', anchor: 'bg', ember: 'rgb(0 0 0 / 13%)', kind: 'alpha' },
  { property: '--shadow-composer', anchor: 'bg', ember: '0 12px 34px rgb(0 0 0 / 10%)', kind: 'alpha' },
  { property: '--shadow-dock', anchor: 'bg', ember: 'inset 0 1px 0 rgb(255 255 255 / 3%), 0 16px 40px -18px rgb(0 0 0 / 85%)', kind: 'alpha', emberLight: '0 1px 2px rgb(24 20 14 / 4%), 0 16px 34px -22px rgb(24 20 14 / 40%)' },
  { property: '--shadow-menu', anchor: 'bg', ember: '0 24px 60px -20px rgb(0 0 0 / 85%)', kind: 'alpha', emberLight: '0 20px 60px rgb(15 23 42 / 14%)' },
  { property: '--shadow-overlay', anchor: 'bg', ember: '0 30px 90px -24px rgb(0 0 0 / 85%)', kind: 'alpha', emberLight: '0 30px 90px rgb(15 23 42 / 18%)' },
];

/** Ember's palette values, used only to compute the offsets below.
 * These match the mock's :root variables from karucode-v3-themes.html exactly.
 */
const EMBER_PALETTE: Record<Anchor, string> = {
  bg: '#111213',
  rail: '#141516',
  sidebar: '#18191a',
  inspector: '#171819',
  surface: '#1c1d1f',
  raised: '#222326',
  hover: '#27292b',
  line: '#2a2c2f',
  lineStrong: '#343638',
  text: '#eae9e5',
  textSecondary: '#b4b3ae',
  muted: '#8b8d8c',
  faint: '#676b6b',
  accent: '#dfb680',
  accentHi: '#f1c691',
  accentBg: '#32291e',
  accentLine: '#58452e',
  success: '#88baa0',
  danger: '#e49e91',
  info: '#92b3d8',
};

/** Properties the stylesheet may reference, in declaration order. */
export const ROLE_PROPERTIES: readonly string[] = ROLES.map((role) => role.property);

/** Role property -> the palette colour it is cut from. Used by the tests that
 *  police "no role may be far more chromatic than its anchor". */
export const ROLE_ANCHORS: Readonly<Record<string, Anchor>> = Object.fromEntries(
  ROLES.map((role) => [role.property, role.anchor]),
);

/**
 * Any other theme: reproduce the mock's *contrast relationship* to the anchor,
 * not its per-channel offset.
 *
 * The offset approach breaks at the ends of the ramp. The mock's card is one
 * channel darker than its surface — a visible 1.008 step on near-black — but the
 * same "one channel" applied to a pure-white surface clamps to nothing, so light
 * themes' cards, diff rows and review blocks collapsed onto the panel at exactly
 * 1.000. Reproducing the ratio in X = luminance + 0.05 (where contrast is
 * linear) keeps each role's distance whatever the anchor is, so a recess stays a
 * recess in both modes and in every theme.
 */
function derivedValue(spec: RoleSpec, palette: ResolvedPalette, mode: ThemeMode): string {
  if (spec.kind === 'alpha') return spec.emberLight && mode === 'light' ? spec.emberLight : spec.ember;
  const anchorEmber = EMBER_PALETTE[spec.anchor];
  if (!anchorEmber || !/^#[0-9a-f]{6}$/i.test(anchorEmber)) return spec.ember;

  const anchor = palette[spec.anchor] ?? anchorEmber;
  const mockLum = relativeLuminance(spec.ember);
  const emberLum = relativeLuminance(anchorEmber);

  // The mock's relationship, as a signed distance in log(X) — the space where a
  // contrast ratio is a constant offset. Positive means "lighter than anchor".
  const delta = Math.log(mockLum + 0.05) - Math.log(emberLum + 0.05);

  // A role that sits exactly on its anchor *is* the anchor. Skipping the HSL
  // round-trip matters: it is lossy at 8 bits, so `--gold-send` came back as
  // #dfb67f when the accent is #dfb680. The Send button must be the brand colour,
  // not a rounding of it.
  if (Math.abs(delta) < 0.004) return anchor;

  // A role that sits LIGHTER than a near-white surface has nowhere to go, so
  // express the same distance as a slight darkening instead — which is how every
  // light design system signals elevation on white.
  const anchorLumRaw = relativeLuminance(anchor);
  const darker = delta > 0 && anchorLumRaw > 0.85;
  const step = darker ? -delta : delta;

  const anchorLum = clampLuminance(anchorLumRaw);
  const target = Math.exp(Math.log(anchorLum + 0.05) + step) - 0.05;

  // Hierarchy guard. Preserving a *ratio* compounds when the anchor itself got
  // lighter: Ember's --text-body sits 1.84x its muted colour, and muted moved up,
  // which pushed body text to near-white and flattened every level together.
  //
  // On a dark theme a secondary role must stay dimmer than primary text; on a
  // light theme it must stay *darker*. So the cap runs in the direction that
  // keeps contrast below primary text.
  const textLum = relativeLuminance(palette.text);
  const bounded =
    mode === 'dark' ? Math.min(target, textLum * 0.62) : Math.max(target, textLum * 1.6);

  // Inherit the ANCHOR's hue and saturation, not the mock literal's.
  //
  // This was the bug behind "the new themes look like shit": `spec.ember` is the
  // v3 mock's *neutral grey* (#1e2021, #1b1d1e …), so keeping "the role's own
  // hue" painted Ember's cool grey into every theme. Sakura's cards came out
  // 33-129° off its own plum surface, i.e. blue-grey panels inside a plum shell.
  // A fill must continue the surface it is cut from; only its lightness moves.
  const anchorHsl = rgbToHsl(anchor);
  return hslToHex(
    anchorHsl.h,
    resolveSaturation(spec, spec.ember, anchorHsl.s),
    lightnessForLuminance(anchorHsl.h, resolveSaturation(spec, spec.ember, anchorHsl.s), bounded),
  );
}

/**
 * How saturated a role's fill should be.
 *
 * Surfaces continue the anchor's tint; text and icon roles keep the mock's own
 * chroma (they are deliberately near-neutral warm/cool greys) so they stay
 * legible instead of picking up the theme hue.
 *
 * Everything else inherits the anchor's saturation *unchanged*. It used to be
 * capped at 0.4 here, which quietly desaturated the accent family: `--gold-send`
 * came out `#d2b997` instead of the brand amber `#dfb680`, because Ember's accent
 * sits at ~60% chroma and the cap clamped it to 40%. A role that is *cut from the
 * accent* has to carry the accent's chroma, or the Send button is not the brand
 * colour. The "coloured slab" risk the old cap guarded against does not apply:
 * the value is already the anchor's own saturation, not an invented one.
 */
function resolveSaturation(spec: RoleSpec, mockLiteral: string, anchorSaturation: number): number {
  const isText = /^--(text|icon|line|input|foot|chip-ink|meta)/.test(spec.property);
  if (isText) return Math.min(0.16, rgbToHsl(mockLiteral).s);
  return anchorSaturation;
}

/**
 * Clamp an anchor luminance into the range a fill can actually occupy, so the
 * derivation always has room to step (pure black offers none below, pure white
 * none above).
 */
function clampLuminance(value: number): number {
  return Math.min(0.97, Math.max(0.008, value));
}

/** HSL lightness for `hue`/`saturation` that yields `target` relative luminance. */
function lightnessForLuminance(hue: number, saturation: number, target: number): number {
  const wanted = Math.min(0.97, Math.max(0.002, target));
  let lo = 0;
  let hi = 1;
  for (let step = 0; step < 40; step += 1) {
    const mid = (lo + hi) / 2;
    if (relativeLuminance(hslToHex(hue, saturation, mid)) < wanted) lo = mid;
    else hi = mid;
  }
  return (lo + hi) / 2;
}

/**
 * Resolve every role for one theme.
 *
 * **Every** theme goes through `derivedValue()`, Ember included.
 *
 * It used to be the other way round: `id === 'ember'` returned the mock's
 * literals verbatim. That is what made the default UI clash with itself — the
 * literals are blue-slate neutrals (`#1b212a`, `#13181f`, `#1f2630`, `#252c37`)
 * while Ember's own palette is a warm graphite, so the cards and inputs sat in a
 * second design system at four times the chroma of the chrome around them. The
 * literals are still load-bearing, but only as the *offset* each role keeps from
 * its anchor: `ember` is read as "this far from the anchor in log-luminance",
 * then re-expressed in the anchor's own hue and saturation.
 *
 * Keeping Ember on the derived path is what makes `roles.test.ts` assert
 * relationships rather than raw hexes, and it means retuning a palette can never
 * leave a stale blue card behind.
 */
export function resolveRoles(theme: ResolvedTheme): Record<string, string> {
  const out: Record<string, string> = {};
  for (const spec of ROLES) {
    out[spec.property] = derivedValue(spec, theme.palette, theme.mode);
  }
  return out;
}
