/* Colour maths for the theme engine.
 *
 * Everything here is pure so the engine and its tests do not need a DOM. Inputs
 * are `#rgb` / `#rrggbb`; invalid values return `null` from `normalizeHex()` so
 * callers can reject a hex field instead of painting a broken token.
 */

export type RGB = readonly [number, number, number];

const SHORT_HEX = /^#?([0-9a-f])([0-9a-f])([0-9a-f])$/i;
const LONG_HEX = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i;

const clamp = (value: number, min: number, max: number): number =>
  Math.min(max, Math.max(min, value));

/** Normalise `#abc`, `abc`, `#AABBCC` to `#aabbcc`. Null when not a hex colour. */
export function normalizeHex(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  const short = SHORT_HEX.exec(trimmed);
  if (short) {
    const [, r, g, b] = short;
    return `#${r}${r}${g}${g}${b}${b}`.toLowerCase();
  }
  const long = LONG_HEX.exec(trimmed);
  if (long) {
    const [, r, g, b] = long;
    return `#${r}${g}${b}`.toLowerCase();
  }
  return null;
}

/** Parse a normalised hex colour. Throws on invalid input (a programming bug). */
export function hexToRgb(hex: string): RGB {
  const normalized = normalizeHex(hex);
  if (!normalized) throw new Error(`Not a hex colour: ${hex}`);
  return [
    Number.parseInt(normalized.slice(1, 3), 16),
    Number.parseInt(normalized.slice(3, 5), 16),
    Number.parseInt(normalized.slice(5, 7), 16),
  ];
}

export function rgbToHex(rgb: RGB): string {
  return `#${rgb
    .map((channel) => clamp(Math.round(channel), 0, 255).toString(16).padStart(2, '0'))
    .join('')}`;
}

/**
 * Mix two colours in sRGB. `amount` 0 returns `from`, 1 returns `to` — the same
 * convention as the mock's `blend()` helper.
 */
export function mix(from: string, to: string, amount: number): string {
  const t = clamp(amount, 0, 1);
  const a = hexToRgb(from);
  const b = hexToRgb(to);
  return rgbToHex([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
}

/** Relative luminance per WCAG 2.1. */
export function relativeLuminance(hex: string): number {
  const channels = hexToRgb(hex).map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  const [r, g, b] = channels as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio, 1–21. */
export function contrastRatio(a: string, b: string): number {
  const first = relativeLuminance(a);
  const second = relativeLuminance(b);
  const lighter = Math.max(first, second);
  const darker = Math.min(first, second);
  return (lighter + 0.05) / (darker + 0.05);
}

/** True when a colour reads as a dark surface. */
export function isDarkColor(hex: string): boolean {
  return relativeLuminance(hex) < 0.34;
}

/** Pick black or white text for a coloured button. */
export function readableTextOn(hex: string): '#0b0d0f' | '#ffffff' {
  return contrastRatio(hex, '#0b0d0f') >= contrastRatio(hex, '#ffffff') ? '#0b0d0f' : '#ffffff';
}

/** `rgba()` for translucent overlays that must follow the palette. */
export function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${clamp(alpha, 0, 1).toFixed(2)})`;
}

/** HSL form of a colour. `h` in degrees, `s` and `l` in 0–1. */
export interface HSL {
  h: number;
  s: number;
  l: number;
}

export function rgbToHsl(hex: string): HSL {
  const [r, g, b] = hexToRgb(hex).map((channel) => channel / 255) as [number, number, number];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const delta = max - min;
  if (delta === 0) return { h: 0, s: 0, l };
  const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min);
  let h: number;
  if (max === r) h = ((g - b) / delta + (g < b ? 6 : 0)) / 6;
  else if (max === g) h = ((b - r) / delta + 2) / 6;
  else h = ((r - g) / delta + 4) / 6;
  return { h: h * 360, s, l };
}

/** HSL (degrees, 0–1, 0–1) back to `#rrggbb`. */
export function hslToHex(h: number, s: number, l: number): string {
  const hue = ((h % 360) + 360) % 360;
  const sat = clamp(s, 0, 1);
  const light = clamp(l, 0, 1);
  const k = (n: number): number => (n + hue / 30) % 12;
  const a = sat * Math.min(light, 1 - light);
  const f = (n: number): number =>
    light - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return rgbToHex([f(0) * 255, f(8) * 255, f(4) * 255]);
}

/** Saturation of a colour, 0–1. Used to detect a washed-out tint. */
export function saturationOf(hex: string): number {
  return rgbToHsl(hex).s;
}