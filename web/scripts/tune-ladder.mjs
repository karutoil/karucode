#!/usr/bin/env node
/**
 * Snap each built-in theme's surface ladder onto the contract's steps.
 *
 * `docs/internal/DESIGN-SYSTEM.md` §3.1 asks for an elevation ladder whose steps
 * are actually visible (≥2.5 HSL lightness points) and whose side columns are
 * distinguishable from the canvas. The eight built-ins were authored by hand and
 * do not satisfy it — several steps are 1.7-2.3 points, which is why the UI read
 * as one flat field.
 *
 * This script does not invent colours. It keeps every theme's own hue and
 * saturation and only moves lightness onto the target ladder, then reports the
 * resulting hex values (and the contrast those values produce) so the change can
 * be pasted into `src/theme/builtins.ts` and reviewed.
 *
 * Usage: node scripts/tune-ladder.mjs
 */

const TARGETS = {
  dark: {
    bg: 5.5,
    rail: 7.6,
    sidebar: 9.8,
    inspector: 9.8,
    surface: 13.0,
    raised: 16.5,
    hover: 20.0,
  },
  light: {
    // Light elevation is inverted: the card is the brightest plane, and the
    // chrome and popovers step *down*. The shadow tokens carry the lift.
    surface: 100,
    rail: 97.5,
    inspector: 93.8,
    bg: 95.5,
    sidebar: 92.8,
    raised: 90.5,
    hover: 87.0,
  },
};

function hexToRgb(hex) {
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
}

function rgbToHsl([r, g, b]) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  let h = 0;
  let s = 0;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
  }
  return { h, s, l };
}

function hslToHex(h, s, l) {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r, g, b] =
    h < 60 ? [c, x, 0]
    : h < 120 ? [x, c, 0]
    : h < 180 ? [0, c, x]
    : h < 240 ? [0, x, c]
    : h < 300 ? [x, 0, c]
    : [c, 0, x];
  const to255 = (v) => Math.round(Math.min(255, Math.max(0, (v + m) * 255)));
  return '#' + [to255(r), to255(g), to255(b)].map((v) => v.toString(16).padStart(2, '0')).join('');
}

function luminance(hex) {
  const [r, g, b] = hexToRgb(hex).map((v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a, b) {
  const l1 = luminance(a);
  const l2 = luminance(b);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

const THEMES = {
  ember: { mode: 'dark', colors: { bg: '#111213', rail: '#141516', sidebar: '#18191a', inspector: '#171819', surface: '#1c1d1f', raised: '#222326', hover: '#27292b', text: '#eae9e5' } },
  nocturne: { mode: 'dark', colors: { bg: '#0c1221', rail: '#101a30', sidebar: '#142238', inspector: '#172843', surface: '#1d2d4b', raised: '#223350', hover: '#263953', text: '#e9efff' } },
  forest: { mode: 'dark', colors: { bg: '#101814', rail: '#14201a', sidebar: '#1a2720', inspector: '#1d2e22', surface: '#22342a', raised: '#26392e', hover: '#304334', text: '#e5f0e6' } },
  sakura: { mode: 'dark', colors: { bg: '#181317', rail: '#231a20', sidebar: '#2a2029', inspector: '#312531', surface: '#382a36', raised: '#3f2f3b', hover: '#453440', text: '#f9ebf2' } },
  graphite: { mode: 'dark', colors: { bg: '#0e1010', rail: '#171919', sidebar: '#1e2121', inspector: '#242828', surface: '#292e2e', raised: '#2f3333', hover: '#323838', text: '#f0f2f1' } },
  daybreak: { mode: 'light', colors: { bg: '#f4f2ee', rail: '#faf9f6', sidebar: '#f7f4ef', inspector: '#faf9f6', surface: '#ffffff', raised: '#f7f5f1', hover: '#eeebe5', text: '#1c1a17' } },
  paper: { mode: 'light', colors: { bg: '#f8f5ef', rail: '#eae5dd', sidebar: '#e7decf', inspector: '#e1d8c9', surface: '#fffdf8', raised: '#e8e1d6', hover: '#dfd6c9', text: '#302b26' } },
  mint: { mode: 'light', colors: { bg: '#f0f9f7', rail: '#e1eeec', sidebar: '#d2eae4', inspector: '#c7e5e0', surface: '#ffffff', raised: '#dbede8', hover: '#cbe4dc', text: '#18362e' } },
};

const ORDER = ['bg', 'rail', 'sidebar', 'inspector', 'surface', 'raised', 'hover'];

for (const [id, theme] of Object.entries(THEMES)) {
  const targets = TARGETS[theme.mode];
  console.log(`\n${id} (${theme.mode})`);
  const out = {};
  for (const key of ORDER) {
    const { h, s } = rgbToHsl(hexToRgb(theme.colors[key]));
    out[key] = hslToHex(h, s, targets[key] / 100);
  }

  // Text has to clear 11:1 on the lightest (dark mode) / darkest (light mode)
  // plane. Keep its hue and saturation, move its lightness until it does.
  const planes = Object.values(out);
  const extreme = theme.mode === 'dark'
    ? planes.reduce((a, b) => (luminance(b) > luminance(a) ? b : a))
    : planes.reduce((a, b) => (luminance(b) < luminance(a) ? b : a));
  const ink = rgbToHsl(hexToRgb(theme.colors.text));
  let text = theme.colors.text;
  for (let step = 0; step <= 100; step += 1) {
    const candidate = hslToHex(ink.h, ink.s, theme.mode === 'dark' ? Math.min(1, ink.l + step / 200) : Math.max(0, ink.l - step / 200));
    text = candidate;
    if (contrast(candidate, extreme) >= 11.2) break;
  }

  const fmt = (o) => ORDER.map((k) => `${k}: '${o[k]}'`).join(', ');
  console.log(`  ${fmt(out)}, text: '${text}'`);

  // Report the ladder and the contrast the new ink achieves, so the pasted
  // values can be reviewed without re-running the test suite.
  const l = ORDER.map((k) => Math.round(rgbToHsl(hexToRgb(out[k])).l * 1000) / 10);
  const elev = ['bg', 'surface', 'raised', 'hover'].map((k) => rgbToHsl(hexToRgb(out[k])).l * 100);
  const steps = elev.slice(1).map((v, i) => Math.round(Math.abs(v - elev[i]) * 10) / 10);
  const textRatio = Math.round(contrast(text, extreme) * 10) / 10;
  console.log(`  ladder ${l.join(' / ')}  elevation steps ${steps.join(', ')}  text-on-extreme ${textRatio}:1`);
}