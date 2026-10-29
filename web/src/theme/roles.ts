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
  { property: '--surface-input', anchor: 'surface', ember: '#1e2021' },
  { property: '--line-input', anchor: 'lineStrong', ember: '#3c3d3d' },
  { property: '--focus-input', anchor: 'accentLine', ember: '#866c50' },
  { property: '--input-ink', anchor: 'text', ember: '#e4e5df' },
  { property: '--input-placeholder', anchor: 'muted', ember: '#898d89' },
  { property: '--surface-plus-hover', anchor: 'hover', ember: '#2e3031' },
  { property: '--surface-hover-strong', anchor: 'hover', ember: '#303233' },
  { property: '--line-hair', anchor: 'line', ember: '#3c3e3d' },
  { property: '--gold-send', anchor: 'accent', ember: '#b88b53' },
  { property: '--gold-send-hover', anchor: 'accent', ember: '#e3b47c' },
  { property: '--on-gold', anchor: 'bg', ember: '#181714' },
  { property: '--foot-ink', anchor: 'faint', ember: '#6f7670' },
  { property: '--foot-strong', anchor: 'muted', ember: '#989e96' },
  { property: '--ring-fill', anchor: 'accent', ember: '#c69e6e' },
  { property: '--ring-track', anchor: 'lineStrong', ember: '#494b4b' },
  { property: '--chip-surface', anchor: 'line', ember: '#242627' },
  { property: '--chip-line', anchor: 'lineStrong', ember: '#3a3b3a' },
  { property: '--chip-ink', anchor: 'muted', ember: '#acb5b0' },

  /* ---- menus / popovers ------------------------------------------------ */
  { property: '--surface-menu', anchor: 'surface', ember: '#252628' },
  { property: '--line-menu', anchor: 'lineStrong', ember: '#434547' },
  { property: '--text-menu-title', anchor: 'muted', ember: '#848a87' },
  { property: '--text-menu-item', anchor: 'text', ember: '#c9c9c5' },
  { property: '--text-menu-meta', anchor: 'muted', ember: '#8b908c' },
  { property: '--row-accent-bg', anchor: 'accentBg', ember: '#33302b' },
  { property: '--row-accent-ink', anchor: 'accent', ember: '#e9d7be' },

  /* ---- inspector cards & fields ---------------------------------------- */
  { property: '--surface-inset', anchor: 'surface', ember: '#202223' },
  { property: '--line-inset', anchor: 'line', ember: '#303334' },
  { property: '--line-card', anchor: 'lineStrong', ember: '#363838' },
  { property: '--line-divider', anchor: 'line', ember: '#2b2e30' },
  { property: '--line-keycap', anchor: 'lineStrong', ember: '#393b3c' },
  { property: '--line-overlay', anchor: 'lineStrong', ember: '#4a4c4c' },
  { property: '--text-caption', anchor: 'muted', ember: '#a2a69f' },
  { property: '--text-subtle', anchor: 'faint', ember: '#777d76' },
  { property: '--text-kicker', anchor: 'muted', ember: '#808781' },
  { property: '--text-kicker-panel', anchor: 'muted', ember: '#9c9d97' },
  { property: '--tab-active-text', anchor: 'accent', ember: '#e9cfab' },
  { property: '--tab-hover', anchor: 'text', ember: '#dedbd5' },
  { property: '--row-selected-bg', anchor: 'accentBg', ember: '#343027' },
  { property: '--row-selected-text', anchor: 'accent', ember: '#f1d8b4' },
  { property: '--surface-preview', anchor: 'surface', ember: '#1e2020' },
  { property: '--line-preview', anchor: 'lineStrong', ember: '#343633' },
  { property: '--icon-folder', anchor: 'accent', ember: '#c5a57b' },
  { property: '--icon-rust', anchor: 'accent', ember: '#d8a178' },
  { property: '--icon-file', anchor: 'muted', ember: '#a8a38b' },
  { property: '--icon-md', anchor: 'info', ember: '#9badbf' },
  { property: '--icon-path', anchor: 'accent', ember: '#ad9d81' },
  { property: '--icon-quiet', anchor: 'muted', ember: '#8a8e8b' },
  { property: '--icon-pill-down', anchor: 'faint', ember: '#777c77' },
  { property: '--btn-accent-bg', anchor: 'accentBg', ember: '#2a2722' },
  { property: '--btn-accent-bg-hover', anchor: 'accentBg', ember: '#403222' },
  { property: '--btn-accent-line', anchor: 'accentLine', ember: '#4b3c2c' },
  { property: '--btn-accent-text', anchor: 'accent', ember: '#d7bb94' },
  { property: '--toggle-track', anchor: 'lineStrong', ember: '#454847' },
  { property: '--toggle-knob', anchor: 'text', ember: '#aeb2ae' },
  { property: '--toggle-on-track', anchor: 'accent', ember: '#917451' },
  { property: '--toggle-on-knob', anchor: 'text', ember: '#f8e2c6' },

  /* ---- transcript & cards ---------------------------------------------- */
  { property: '--surface-card', anchor: 'surface', ember: '#1b1d1e' },
  { property: '--line-card-strong', anchor: 'lineStrong', ember: '#313436' },
  { property: '--surface-command', anchor: 'surface', ember: '#202222' },
  { property: '--line-command', anchor: 'lineStrong', ember: '#363838' },
  { property: '--surface-command-hover', anchor: 'hover', ember: '#282929' },
  { property: '--surface-command-output', anchor: 'bg', ember: '#181a1b' },
  { property: '--text-command', anchor: 'text', ember: '#bdc4bb' },
  { property: '--text-command-output', anchor: 'success', ember: '#8eac97' },
  { property: '--surface-symbol', anchor: 'raised', ember: '#272b2b' },
  { property: '--line-symbol', anchor: 'line', ember: '#3a3e3c' },
  { property: '--text-symbol', anchor: 'success', ember: '#9eb6a6' },
  { property: '--text-output', anchor: 'muted', ember: '#8e9b95' },
  { property: '--text-output-num', anchor: 'faint', ember: '#6e7672' },
  { property: '--text-expand', anchor: 'text', ember: '#babdb7' },
  { property: '--text-body', anchor: 'muted', ember: '#bfc1bc' },
  { property: '--text-date', anchor: 'faint', ember: '#686e6b' },
  { property: '--text-user', anchor: 'text', ember: '#dadbd6' },
  { property: '--surface-user', anchor: 'surface', ember: '#212224' },
  { property: '--surface-avatar', anchor: 'raised', ember: '#2d302d' },
  { property: '--line-avatar', anchor: 'lineStrong', ember: '#444945' },
  { property: '--text-avatar', anchor: 'text', ember: '#d7dcd7' },
  { property: '--surface-avatar-agent', anchor: 'accentBg', ember: '#302a24' },
  { property: '--line-avatar-agent', anchor: 'accentLine', ember: '#5a4630' },
  { property: '--surface-diff', anchor: 'surface', ember: '#1a1c1d' },
  { property: '--line-diff', anchor: 'lineStrong', ember: '#353737' },
  { property: '--line-diff-head', anchor: 'line', ember: '#2d3030' },
  { property: '--surface-diff-add', anchor: 'success', ember: '#1d3027' },
  { property: '--surface-diff-del', anchor: 'danger', ember: '#362626' },
  { property: '--surface-diff-ctx', anchor: 'surface', ember: '#1b1d1f' },
  { property: '--text-review', anchor: 'accent', ember: '#e3c39d' },
  { property: '--surface-review', anchor: 'accentBg', ember: '#362c20' },
  { property: '--surface-review-hi', anchor: 'accentBg', ember: '#4c3925' },

  /* ---- empty state ------------------------------------------------------ */
  { property: '--text-empty-title', anchor: 'text', ember: '#eae8e4' },
  { property: '--text-empty-copy', anchor: 'muted', ember: '#909590' },
  { property: '--text-empty-symbol', anchor: 'accent', ember: '#e5b67d' },

  /* ---- toasts ----------------------------------------------------------- */
  { property: '--surface-toast', anchor: 'raised', ember: '#292b2a' },
  { property: '--line-toast', anchor: 'lineStrong', ember: '#54544c' },
  { property: '--text-toast', anchor: 'text', ember: '#dfdfd7' },

  /* ---- rail ------------------------------------------------------------- */
  { property: '--rail-glyph-text', anchor: 'muted', ember: '#9c9e9f' },
  { property: '--rail-add-line', anchor: 'lineStrong', ember: '#3e4141' },
  { property: '--rail-add-line-hover', anchor: 'accent', ember: '#6e655b' },
  { property: '--rail-avatar-bg', anchor: 'surface', ember: '#31352f' },
  { property: '--rail-avatar-line', anchor: 'lineStrong', ember: '#475144' },
  { property: '--rail-avatar-text', anchor: 'success', ember: '#cfdfcd' },

  /* ---- accent family ---------------------------------------------------- */
  { property: '--accent-faint', anchor: 'accentLine', ember: '#484038' },
  { property: '--accent-surface', anchor: 'accentBg', ember: '#27241f' },
  { property: '--accent-text-soft', anchor: 'accentHi', ember: '#f0d5ac' },
  { property: '--accent-hint', anchor: 'accent', ember: '#ab987a' },
  { property: '--field-focus-line', anchor: 'accentLine', ember: '#655038' },
  { property: '--item-active-to', anchor: 'accentBg', ember: '#252321' },
  { property: '--meta-text', anchor: 'muted', ember: '#858983' },

  /* ---- neutral overlays: mode-dependent, palette-independent ------------ */
  { property: '--rail-glyph-line', anchor: 'text', ember: 'rgb(255 255 255 / 8%)', kind: 'alpha', emberLight: 'rgb(0 0 0 / 10%)' },
  { property: '--inset-hairline', anchor: 'text', ember: 'rgb(255 255 255 / 3%)', kind: 'alpha', emberLight: 'rgb(255 255 255 / 60%)' },
  { property: '--scrim', anchor: 'bg', ember: 'rgb(0 0 0 / 47%)', kind: 'alpha' },
  { property: '--scrim-soft', anchor: 'bg', ember: 'rgb(0 0 0 / 13%)', kind: 'alpha' },
  { property: '--shadow-composer', anchor: 'bg', ember: '0 12px 34px rgb(0 0 0 / 10%)', kind: 'alpha' },
  { property: '--shadow-menu', anchor: 'bg', ember: '0 20px 60px rgb(0 0 0 / 60%)', kind: 'alpha', emberLight: '0 20px 60px rgb(15 23 42 / 14%)' },
  { property: '--shadow-overlay', anchor: 'bg', ember: '0 30px 90px rgb(0 0 0 / 69%)', kind: 'alpha', emberLight: '0 30px 90px rgb(15 23 42 / 18%)' },
];

/** Ember's palette values, used only to compute the offsets below. */
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

function clampChannel(value: number): number {
  return Math.min(255, Math.max(0, Math.round(value)));
}

function parseHex(hex: string): [number, number, number] {
  const raw = hex.replace('#', '').trim();
  const full =
    raw.length === 3
      ? raw
          .split('')
          .map((c) => c + c)
          .join('')
      : raw.slice(0, 6);
  return [
    parseInt(full.slice(0, 2), 16) || 0,
    parseInt(full.slice(2, 4), 16) || 0,
    parseInt(full.slice(4, 6), 16) || 0,
  ];
}

function toHex(rgb: [number, number, number]): string {
  return (
    '#' +
    rgb
      .map((channel) => clampChannel(channel).toString(16).padStart(2, '0'))
      .join('')
  );
}

/** Ember itself: the mock's own values, unchanged. */
function emberValue(spec: RoleSpec, mode: ThemeMode): string {
  if (spec.kind === 'alpha') {
    return mode === 'light' ? (spec.emberLight ?? spec.ember) : spec.ember;
  }
  return spec.ember;
}

/**
 * Any other theme: keep Ember's per-channel offset, mirrored in light mode so an
 * inset stays an inset instead of turning into a highlight.
 */
function derivedValue(spec: RoleSpec, palette: ResolvedPalette, mode: ThemeMode): string {
  if (spec.kind === 'alpha') return spec.emberLight && mode === 'light' ? spec.emberLight : spec.ember;
  const anchorEmber = EMBER_PALETTE[spec.anchor];
  if (!anchorEmber || !/^#[0-9a-f]{6}$/i.test(anchorEmber)) return spec.ember;
  const base = parseHex(anchorEmber);
  const target = parseHex(spec.ember);
  const own = parseHex(palette[spec.anchor] ?? anchorEmber);
  const sign = mode === 'light' ? -1 : 1;
  return toHex([
    own[0] + sign * (target[0] - base[0]),
    own[1] + sign * (target[1] - base[1]),
    own[2] + sign * (target[2] - base[2]),
  ]);
}

/** Resolve every role for one theme: exact for Ember, derived elsewhere. */
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
