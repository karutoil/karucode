#!/usr/bin/env node
/**
 * Static design-system audit for `web/src/styles`.
 *
 * The stylesheet documents three rules in `tokens.css`:
 *
 *   - never hardcode a colour; use a `--theme-*` token or a public alias;
 *   - never hardcode a corner radius; use `--radius`, `--radius-sm`, `--radius-lg`;
 *   - text/font sizes come from `--fs-*`; spacing from the `--space-*` scale.
 *
 * Nothing enforced them, so ~300 bespoke shades and eleven radii accumulated.
 * This script reports every place that breaks a rule, with `file:line`, and
 * groups by rule so a reviewer can see the shape of the problem.
 *
 * Usage:
 *   node scripts/design-audit.mjs            # summary
 *   node scripts/design-audit.mjs --detail   # every violation
 *   node scripts/design-audit.mjs --rule colour
 *
 * How this relates to `src/styles/scale.test.ts`: that test is the floor CI
 * enforces — it fails on a raw colour, an off-scale radius, a font size below
 * the 10px floor, and spacing that is not a step on the 4px rhythm. This report
 * is the stricter target: it also flags *on-scale* literals (`gap: 12px` is legal
 * but loses the density knob, which rewrites `--space-*` and nothing else). Both
 * allow the same three geometry radii (`0`, `50%`, `inherit`) and the same
 * computed lengths (`calc()`, `env()`, `min()`), because those are measurements
 * rather than design decisions — the two used to disagree, and the audit's
 * version was the one that pushed somebody to put `--radius-pill` on a circle.
 */

import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { cssDeclarations } from './css-declarations.mjs';

const STYLE_DIR = new URL('../src/styles/', import.meta.url).pathname;

/**
 * `tokens.css` is the one file allowed to carry raw values: it is the token
 * layer, and the theme engine rewrites its `--theme-*` block at runtime.
 */
const TOKEN_FILES = new Set(['tokens.css']);

/** Declarations that legitimately take a raw length even when authored by hand. */
const RAW_OK_PROPERTIES = new Set([
  'width',
  'height',
  'min-width',
  'min-height',
  'max-width',
  'max-height',
  'left',
  'right',
  'top',
  'bottom',
  'inset',
  'z-index',
  'opacity',
  'grid-template-columns',
  'grid-template-rows',
  'grid-column',
  'grid-row',
  'flex',
  'flex-basis',
  'translate',
  'transform',
]);

/**
 * Radii that are geometry rather than a corner style, and therefore not a
 * violation: a circle (`50%` on an avatar, a dot, a spinner), a square (`0`), and
 * `inherit` (a child that must follow its parent's corner). `scale.test.ts`
 * allows exactly these three, and `tokens.css` §3.4 says "50% is for circles
 * only" — so reporting them here was the audit disagreeing with the rule it
 * exists to enforce, and the only way to clear the report was to write
 * `var(--radius-pill)` (999px) on something that has to be round.
 *
 * Everything else must be one of the published corners: `--radius`,
 * `--radius-sm`, `--radius-lg`, `--radius-pill`, `--radius-input`,
 * `--radius-prompt`.
 */
const RADIUS_GEOMETRY = new Set(['0', '50%', 'inherit']);

/**
 * A value a function computes is a measurement, not a hand-picked step:
 * `calc(var(--space-6) + env(safe-area-inset-bottom))`, `env(safe-area-inset-
 * bottom)`, `min(17vh, 150px)` for an overlay that stays centred in a tall or a
 * short window. There is no token for "the notch" or "17% of the viewport", and
 * inventing one would freeze a real measurement at a build-time number. These are
 * skipped by the spacing rule, the same way `scale.test.ts` skips `var(`/`calc(`.
 */
const COMPUTED = /\b(calc|env|min|max|clamp)\(/;

/** Font sizes that declare no size of their own: they copy the parent's step. */
const RELATIVE_FONT_SIZE = /var\(|em\b|rem\b|%$|inherit|calc\(/;


/** Font sizes the token layer publishes. */
const FONT_SCALE = ['--fs-body', '--fs-small', '--fs-micro', '--fs-message'];

function files() {
  return readdirSync(STYLE_DIR)
    .filter((name) => name.endsWith('.css'))
    .sort();
}

/** Declarations wherever they start in a rule body, with their original lines. */
function declarations(file, css) {
  return cssDeclarations(css).map((declaration) => ({
    at: `${file}:${declaration.line}`,
    property: declaration.property,
    value: declaration.value.replace(/\s+/g, ' ').trim(),
  }));
}

function collect() {
  const out = { colour: [], radius: [], fontSize: [], spacing: [], borderColour: [] };

  for (const file of files()) {
    const raw = readFileSync(join(STYLE_DIR, file), 'utf8');
    const isTokenFile = TOKEN_FILES.has(file);

    for (const { at, property, value } of declarations(file, raw)) {
      if (isTokenFile) continue;

      // ---- raw colours ----
      const colour = /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?)\(/.exec(value);
      if (colour) {
        const entry = { at, property, value: value.trim() };
        if (/^border|^outline|^box-shadow/.test(property)) out.borderColour.push(entry);
        else out.colour.push(entry);
      }

      // ---- radii ----
      if (/^border(-[a-z]+)*-radius$/.test(property) && !/var\(/.test(value)) {
        // A shorthand can carry several radii; every part must be a circle, a
        // square or `inherit`, exactly as `scale.test.ts` requires.
        const parts = value.split(/\s+/).filter(Boolean);
        if (parts.some((part) => !RADIUS_GEOMETRY.has(part))) {
          out.radius.push({ at, property, value: value.trim() });
        }
      }

      // ---- font sizes ----
      if (property === 'font-size' && !RELATIVE_FONT_SIZE.test(value)) {
        out.fontSize.push({ at, property, value: value.trim() });
      }

      // ---- spacing off the 4/7/10/14/18/24/30 scale ----
      if (/^(padding|margin|gap|row-gap|column-gap)(-[a-z]+)*$/.test(property)) {
        if (
          /\d+px/.test(value) &&
          !/var\(/.test(value) &&
          !COMPUTED.test(value) &&
          !/^(auto|inherit|0)$/.test(value) &&
          !RAW_OK_PROPERTIES.has(property)
        ) {
          out.spacing.push({ at, property, value: value.trim() });
        }
      }
    }
  }
  return out;
}

function summarise(results) {
  const pad = (n) => String(n).padStart(5);
  console.log('karucode stylesheet audit — violations of the documented token rules\n');
  for (const [rule, list] of Object.entries(results)) {
    console.log(`${pad(list.length)}  ${rule}`);
  }
  console.log(`\n${pad(Object.values(results).reduce((a, b) => a + b.length, 0))}  total`);

  for (const [rule, list] of Object.entries(results)) {
    if (!list.length) continue;
    const byFile = new Map();
    for (const v of list) {
      const file = v.at.split(':')[0];
      byFile.set(file, (byFile.get(file) ?? 0) + 1);
    }
    const top = [...byFile.entries()].sort((a, b) => b[1] - a[1]);
    console.log(`\n${rule} by file (top 6):`);
    for (const [file, n] of top.slice(0, 6)) console.log(`   ${String(n).padStart(4)}  ${file}`);
  }

  // The most telling single number: how many distinct values are in play.
  for (const [rule, list] of Object.entries(results)) {
    if (!list.length) continue;
    const distinct = new Set(list.map((v) => v.value));
    console.log(`\n${rule}: ${distinct.size} distinct declared values`);
    const counts = new Map();
    for (const v of list) counts.set(v.value, (counts.get(v.value) ?? 0) + 1);
    console.log(
      '   ' +
        [...counts.entries()]
          .sort((a, b) => b[1] - a[1])
          .slice(0, 18)
          .map(([v, n]) => `${v}×${n}`)
          .join('  '),
    );
  }
}

const ruleIndex = process.argv.indexOf('--rule');
const detail = process.argv.includes('--detail');
const results = collect();

if (detail || ruleIndex > -1) {
  const wanted = ruleIndex > -1 ? process.argv[ruleIndex + 1] : null;
  for (const [rule, list] of Object.entries(results)) {
    if (wanted && rule !== wanted) continue;
    for (const v of list) console.log(`${v.at}  ${v.property}: ${v.value}`);
  }
} else {
  summarise(results);
}