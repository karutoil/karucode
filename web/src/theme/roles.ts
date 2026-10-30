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
  { property: '--surface-input', anchor: 'surface', ember: '#2d3032' },
  { property: '--line-input', anchor: 'lineStrong', ember: '#424343' },
  { property: '--focus-input', anchor: 'accentLine', ember: '#b8a289' },
  { property: '--input-ink', anchor: 'text', ember: '#e4e5e0' },
  { property: '--input-placeholder', anchor: 'muted', ember: '#a4a7a4' },
  { property: '--surface-plus-hover', anchor: 'hover', ember: '#3d4041' },
  { property: '--surface-hover-strong', anchor: 'hover', ember: '#3f4142' },
  { property: '--line-hair', anchor: 'line', ember: '#464847' },
  { property: '--gold-send', anchor: 'accent', ember: '#b88b53' },
  { property: '--gold-send-hover', anchor: 'accent', ember: '#e3b47c' },
  { property: '--on-gold', anchor: 'bg', ember: '#181714' },
  { property: '--foot-ink', anchor: 'faint', ember: '#838a84' },
  { property: '--foot-strong', anchor: 'muted', ember: '#b6bab5' },
  { property: '--ring-fill', anchor: 'accent', ember: '#c69e6d' },
  { property: '--ring-track', anchor: 'lineStrong', ember: '#4f5252' },
  { property: '--chip-surface', anchor: 'line', ember: '#2f3233' },
  { property: '--chip-line', anchor: 'lineStrong', ember: '#404240' },
  { property: '--chip-ink', anchor: 'muted', ember: '#ced3d1' },

  /* ---- menus / popovers ------------------------------------------------ */
  { property: '--surface-menu', anchor: 'surface', ember: '#333537' },
  { property: '--line-menu', anchor: 'lineStrong', ember: '#494b4e' },
  { property: '--text-menu-title', anchor: 'muted', ember: '#a0a4a2' },
  { property: '--text-menu-item', anchor: 'text', ember: '#c9c9c5' },
  { property: '--text-menu-meta', anchor: 'muted', ember: '#a7aba7' },
  { property: '--row-accent-bg', anchor: 'accentBg', ember: '#4d4941' },
  { property: '--row-accent-ink', anchor: 'accent', ember: '#e9d7be' },

  /* ---- inspector cards & fields ---------------------------------------- */
  { property: '--surface-inset', anchor: 'surface', ember: '#303236' },
  { property: '--line-inset', anchor: 'line', ember: '#3a3e3f' },
  { property: '--line-card', anchor: 'lineStrong', ember: '#3c3e3e' },
  { property: '--line-divider', anchor: 'line', ember: '#35393c' },
  { property: '--line-keycap', anchor: 'lineStrong', ember: '#3f4243' },
  { property: '--line-overlay', anchor: 'lineStrong', ember: '#505353' },
  { property: '--text-caption', anchor: 'muted', ember: '#c1c3bf' },
  { property: '--text-subtle', anchor: 'faint', ember: '#8c918b' },
  { property: '--text-kicker', anchor: 'muted', ember: '#9ba09c' },
  { property: '--text-kicker-panel', anchor: 'muted', ember: '#b9b9b5' },
  { property: '--tab-active-text', anchor: 'accent', ember: '#e9cfaa' },
  { property: '--tab-hover', anchor: 'text', ember: '#dedbd4' },
  { property: '--row-selected-bg', anchor: 'accentBg', ember: '#4f493b' },
  { property: '--row-selected-text', anchor: 'accent', ember: '#f1d8b3' },
  { property: '--surface-preview', anchor: 'surface', ember: '#2e2f34' },
  { property: '--line-preview', anchor: 'lineStrong', ember: '#3b3d39' },
  { property: '--icon-folder', anchor: 'accent', ember: '#c5a57b' },
  { property: '--icon-rust', anchor: 'accent', ember: '#d8a177' },
  { property: '--icon-file', anchor: 'muted', ember: '#c4c0b0' },
  { property: '--icon-md', anchor: 'info', ember: '#9badbf' },
  { property: '--icon-path', anchor: 'accent', ember: '#ad9d81' },
  { property: '--icon-quiet', anchor: 'muted', ember: '#a5a9a6' },
  { property: '--icon-pill-down', anchor: 'faint', ember: '#8c908c' },
  { property: '--btn-accent-bg', anchor: 'accentBg', ember: '#464139' },
  { property: '--btn-accent-bg-hover', anchor: 'accentBg', ember: '#5e4a32' },
  { property: '--btn-accent-line', anchor: 'accentLine', ember: '#80664b' },
  { property: '--btn-accent-text', anchor: 'accent', ember: '#d7bb95' },
  { property: '--toggle-track', anchor: 'lineStrong', ember: '#4b4e4d' },
  { property: '--toggle-knob', anchor: 'text', ember: '#aeb2ae' },
  { property: '--toggle-on-track', anchor: 'accent', ember: '#917451' },
  { property: '--toggle-on-knob', anchor: 'text', ember: '#f8e2c6' },

  /* ---- transcript & cards ---------------------------------------------- */
  { property: '--surface-card', anchor: 'surface', ember: '#28292c' },
  { property: '--line-card-strong', anchor: 'lineStrong', ember: '#373b3d' },
  { property: '--surface-command', anchor: 'surface', ember: '#303236' },
  { property: '--line-command', anchor: 'lineStrong', ember: '#3c3e3e' },
  { property: '--surface-command-hover', anchor: 'hover', ember: '#393b3b' },
  { property: '--surface-command-output', anchor: 'bg', ember: '#181a1c' },
  { property: '--text-command', anchor: 'text', ember: '#bdc4ba' },
  { property: '--text-command-output', anchor: 'success', ember: '#8fac97' },
  { property: '--surface-symbol', anchor: 'raised', ember: '#38393f' },
  { property: '--line-symbol', anchor: 'line', ember: '#444846' },
  { property: '--text-symbol', anchor: 'success', ember: '#9db6a6' },
  { property: '--text-output', anchor: 'muted', ember: '#adb6b2' },
  { property: '--text-output-num', anchor: 'faint', ember: '#828a86' },
  { property: '--text-expand', anchor: 'text', ember: '#babdb7' },
  { property: '--text-body', anchor: 'muted', ember: '#d5cec6' },
  { property: '--text-date', anchor: 'faint', ember: '#7b827e' },
  { property: '--text-user', anchor: 'text', ember: '#dadbd6' },
  { property: '--surface-user', anchor: 'surface', ember: '#303236' },
  { property: '--surface-avatar', anchor: 'raised', ember: '#3b3f3b' },
  { property: '--line-avatar', anchor: 'lineStrong', ember: '#4a4f4b' },
  { property: '--text-avatar', anchor: 'text', ember: '#d7dcd7' },
  { property: '--surface-avatar-agent', anchor: 'accentBg', ember: '#4d443a' },
  { property: '--line-avatar-agent', anchor: 'accentLine', ember: '#91714e' },
  { property: '--surface-diff', anchor: 'surface', ember: '#272a2b' },
  { property: '--line-diff', anchor: 'lineStrong', ember: '#3b3e3e' },
  { property: '--line-diff-head', anchor: 'line', ember: '#373a3a' },
  { property: '--surface-diff-add', anchor: 'success', ember: '#1d2f27' },
  { property: '--surface-diff-del', anchor: 'danger', ember: '#352626' },
  { property: '--surface-diff-ctx', anchor: 'surface', ember: '#27292c' },
  { property: '--text-review', anchor: 'accent', ember: '#e3c39c' },
  { property: '--surface-review', anchor: 'accentBg', ember: '#544532' },
  { property: '--surface-review-hi', anchor: 'accentBg', ember: '#6a5034' },

  /* ---- empty state ------------------------------------------------------ */
  { property: '--text-empty-title', anchor: 'text', ember: '#eae8e3' },
  { property: '--text-empty-copy', anchor: 'muted', ember: '#adb1ad' },
  { property: '--text-empty-symbol', anchor: 'accent', ember: '#e5b67c' },

  /* ---- toasts ----------------------------------------------------------- */
  { property: '--surface-toast', anchor: 'raised', ember: '#38393f' },
  { property: '--line-toast', anchor: 'lineStrong', ember: '#5a5a52' },
  { property: '--text-toast', anchor: 'text', ember: '#dfdfd8' },

  /* ---- rail ------------------------------------------------------------- */
  { property: '--rail-glyph-text', anchor: 'muted', ember: '#b9bbbc' },
  { property: '--rail-add-line', anchor: 'lineStrong', ember: '#444848' },
  { property: '--rail-add-line-hover', anchor: 'accent', ember: '#6f655b' },
  { property: '--rail-avatar-bg', anchor: 'surface', ember: '#3d423b' },
  { property: '--rail-avatar-line', anchor: 'lineStrong', ember: '#4d5749' },
  { property: '--rail-avatar-text', anchor: 'success', ember: '#cfdfcc' },

  /* ---- accent family ---------------------------------------------------- */
  { property: '--accent-faint', anchor: 'accentLine', ember: '#796c5e' },
  { property: '--accent-surface', anchor: 'accentBg', ember: '#443e36' },
  { property: '--accent-text-soft', anchor: 'accentHi', ember: '#edcc9b' },
  { property: '--accent-hint', anchor: 'accent', ember: '#ab9879' },
  { property: '--field-focus-line', anchor: 'accentLine', ember: '#9d7c57' },
  { property: '--item-active-to', anchor: 'accentBg', ember: '#413e3a' },
  { property: '--meta-text', anchor: 'muted', ember: '#a0a49f' },

  /* ---- neutral overlays: mode-dependent, palette-independent ------------ */
  { property: '--rail-glyph-line', anchor: 'text', ember: 'rgb(255 255 255 / 8%)', kind: 'alpha', emberLight: 'rgb(0 0 0 / 10%)' },
  { property: '--inset-hairline', anchor: 'text', ember: 'rgb(255 255 255 / 3%)', kind: 'alpha', emberLight: 'rgb(255 255 255 / 60%)' },
  { property: '--scrim', anchor: 'bg', ember: 'rgb(0 0 0 / 47%)', kind: 'alpha' },
  { property: '--scrim-soft', anchor: 'bg', ember: 'rgb(0 0 0 / 13%)', kind: 'alpha' },
  { property: '--shadow-composer', anchor: 'bg', ember: '0 12px 34px rgb(0 0 0 / 10%)', kind: 'alpha' },
  { property: '--shadow-dock', anchor: 'bg', ember: '0 18px 44px rgb(0 0 0 / 38%), 0 2px 8px rgb(0 0 0 / 26%)', kind: 'alpha', emberLight: '0 18px 44px rgb(15 23 42 / 18%), 0 2px 8px rgb(15 23 42 / 10%)' },
  { property: '--shadow-menu', anchor: 'bg', ember: '0 20px 60px rgb(0 0 0 / 60%)', kind: 'alpha', emberLight: '0 20px 60px rgb(15 23 42 / 14%)' },
  { property: '--shadow-overlay', anchor: 'bg', ember: '0 30px 90px rgb(0 0 0 / 69%)', kind: 'alpha', emberLight: '0 30px 90px rgb(15 23 42 / 18%)' },
];

/** Ember's palette values, used only to compute the offsets below. */
const EMBER_PALETTE: Record<Anchor, string> = {
  bg: '#111213',
  rail: '#191a1c',
  sidebar: '#202223',
  inspector: '#27282a',
  surface: '#2c2d31',
  raised: '#313337',
  hover: '#36393b',
  line: '#35373a',
  lineStrong: '#3a3d3f',
  text: '#eae9e5',
  muted: '#a6a8a7',
  faint: '#7c7e7e',
  accent: '#dfb680',
  accentHi: '#e1c196',
  accentBg: '#544128',
  accentLine: '#84735c',
  success: '#88baa0',
  danger: '#e49e91',
  info: '#92b3d8',
};

/** Properties the stylesheet may reference, in declaration order. */
export const ROLE_PROPERTIES: readonly string[] = ROLES.map((role) => role.property);

/** Ember itself: the mock's own values, unchanged. */
function emberValue(spec: RoleSpec, mode: ThemeMode): string {
  if (spec.kind === 'alpha') {
    return mode === 'light' ? (spec.emberLight ?? spec.ember) : spec.ember;
  }
  return spec.ember;
}

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
 * Surfaces continue the anchor's tint. Text and icon roles keep the mock's own
 * chroma (they are deliberately near-neutral warm/cool greys), so they stay
 * legible instead of picking up the theme hue. Either way the value is capped so
 * a vivid surface cannot turn a card into a coloured slab.
 */
function resolveSaturation(spec: RoleSpec, mockLiteral: string, anchorSaturation: number): number {
  const isText = /^--(text|icon|line|input|foot|chip-ink|meta)/.test(spec.property);
  if (isText) return Math.min(0.16, rgbToHsl(mockLiteral).s);
  return Math.min(anchorSaturation, 0.4);
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
 * Ember uses its literals verbatim (they ARE the mock, kept in step with its
 * palette); every other theme derives them from the same relationships. Keeping
 * Ember on the literal path means it must be rebased whenever its palette moves,
 * which is why roles.test.ts asserts the relationships rather than raw hexes.
 */
export function resolveRoles(theme: ResolvedTheme): Record<string, string> {
  const mode = theme.mode;
  const isEmber = theme.id === 'ember';
  const out: Record<string, string> = {};
  for (const spec of ROLES) {
    out[spec.property] = isEmber
      ? emberValue(spec, mode)
      : derivedValue(spec, theme.palette, mode);
  }
  return out;
}
