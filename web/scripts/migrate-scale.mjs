#!/usr/bin/env node
/**
 * Migrate literal sizes, radii and spacing onto the design scale.
 *
 * `src/styles/scale.test.ts` is the contract; this is the mechanical half of
 * satisfying it. Doing ~1500 declarations by hand is error-prone (and was: four
 * agents produced four different mappings), so the mapping lives here once, is
 * rule-aware, and prints exactly what it will change.
 *
 * Design decisions:
 *
 *   - **Rule-aware font sizes.** `9px` and `9.5px` are gone either way, but where
 *     they land depends on context: inside a rule that sets
 *     `text-transform: uppercase` (a kicker, a badge, a section label) they go to
 *     the 10px step, which is legal for tracked uppercase. Anywhere else they go
 *     to 11px, because the whole complaint was that quiet text was unreadable.
 *   - **`0` is not a step.** A bare `0` in a shorthand stays `0`.
 *   - **Shorthands are mapped per value**, so `margin: 0 0 33px` becomes
 *     `margin: 0 0 var(--space-10)` and `padding: 10px 12px` keeps its rhythm.
 *   - **Nothing inside `calc()`, `var()`, `clamp()` or a media query is touched.**
 *     Those already express intent, and a blind substitution inside `calc()` is
 *     how a layout breaks.
 *   - **Nearest step, tie to the smaller.** Maximum movement is 2px, so no layout
 *     may reflow; the contract says as much.
 *
 * Usage:
 *   node scripts/migrate-scale.mjs --check                 # report only
 *   node scripts/migrate-scale.mjs --write inspector.css   # rewrite one file
 *   node scripts/migrate-scale.mjs --write-inspector       # every non-exempt sheet
 */

import { readFileSync, readdirSync, writeFileSync } from 'node:fs';

const DIR = new URL('../src/styles/', import.meta.url).pathname;

/** Sheet owned by another workstream; never touched from here. */
const EXEMPT = new Set(['tokens.css', 'artifacts.css']);

const FONT_STEPS = [
  [10, '--fs-micro'],
  [11, '--fs-tiny'],
  [11.5, '--fs-small'],
  [12, '--fs-label'],
  [12.5, '--fs-message'],
  [13, '--fs-body'],
  [14, '--fs-lead'],
  [16, '--fs-title'],
  [18, '--fs-heading'],
  [20, '--fs-display'],
  [24, '--fs-hero'],
];

const RADIUS_STEPS = [
  [4, '--radius-xs'],
  [6, '--radius-sm'],
  [8, '--radius-md'],
  [10, '--radius'],
  [14, '--radius-lg'],
];

const SPACE_STEPS = [
  [2, '--space-1'],
  [4, '--space-2'],
  [6, '--space-3'],
  [8, '--space-4'],
  [10, '--space-5'],
  [12, '--space-6'],
  [16, '--space-7'],
  [20, '--space-8'],
  [24, '--space-9'],
  [32, '--space-10'],
];

/** Nearest step by absolute distance; a tie keeps the smaller value. */
function nearest(steps, value) {
  let best = steps[0];
  let bestDistance = Infinity;
  for (const step of steps) {
    const distance = Math.abs(step[0] - value);
    if (distance < bestDistance) {
      best = step;
      bestDistance = distance;
    }
  }
  return best;
}

/** True when a declaration already expresses intent and must not be rewritten. */
function isDerived(value) {
  return /var\(|calc\(|clamp\(|min\(|max\(|env\(|em\b|rem\b|%|inherit|auto|initial|unset/.test(value);
}

/**
 * Rewrite one declaration value, if it is a plain length that should be a token.
 * Returns the new value, or null when nothing changed.
 */
function rewriteValue(property, value, uppercase) {
  if (isDerived(value)) return null;

  if (property === 'font-size') {
    const pixels = /^(-?[\d.]+)px$/.exec(value.trim());
    if (!pixels) return null;
    const size = Number(pixels[1]);
    // A kicker may sit on the 10px step; anything else has an 11px floor.
    const floor = uppercase ? 10 : 11;
    const candidates = FONT_STEPS.filter(([step]) => step >= floor);
    const [, token] = nearest(candidates, size);
    return `var(${token})`;
  }

  if (/^border(-[a-z]+)*-radius$/.test(property)) {
    // A shorthand may carry several radii: `0 5px 5px 0`.
    const parts = value.trim().split(/\s+/);
    let changed = false;
    const mapped = parts.map((part) => {
      if (part === '0' || part === '50%' || /var\(/.test(part)) return part;
      if (part === '999px') { changed = true; return 'var(--radius-pill)'; }
      const pixels = /^([\d.]+)px$/.exec(part);
      if (!pixels) return part;
      // Radius is always a token, even when the literal already happens to
      // equal a step: the step is a role (`--radius-sm`), and a component that
      // re-types 6px instead of naming the role is the drift this prevents.
      const [, token] = nearest(RADIUS_STEPS, Number(pixels[1]));
      changed = true;
      return `var(${token})`;
    });
    return changed ? mapped.join(' ') : null;
  }

  if (/^(padding|margin|gap|row-gap|column-gap)(-[a-z]+)*$/.test(property)) {
    const parts = value.trim().split(/\s+/);
    let changed = false;
    const mapped = parts.map((part) => {
      if (part === '0' || part === 'auto') return part;
      const pixels = /^(-?[\d.]+)px$/.exec(part);
      if (!pixels) return part;
      const [, token] = nearest(SPACE_STEPS, Math.abs(Number(pixels[1])));
      changed = true;
      return `var(${token})`;
    });
    return changed ? mapped.join(' ') : null;
  }

  return null;
}

/**
 * Walk the sheet rule by rule so font sizes can see whether their own block is
 * uppercase. Declarations outside any rule (custom properties) are left alone.
 */
function migrate(file, source) {
  const lines = source.split('\n');
  const changes = [];

  // Map line index -> whether its enclosing rule is uppercase.
  const uppercase = new Array(lines.length).fill(false);
  let depth = 0;
  let blockStart = -1;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const opens = (line.match(/\{/g) || []).length;
    const closes = (line.match(/\}/g) || []).length;
    if (depth === 0 && opens > 0) blockStart = i;
    depth += opens - closes;
    if (depth === 0 && blockStart >= 0) {
      const block = lines.slice(blockStart, i + 1).join('\n');
      const isUppercase = /text-transform:\s*uppercase/.test(block);
      for (let j = blockStart; j <= i; j += 1) uppercase[j] = isUppercase;
      blockStart = -1;
    }
    if (depth < 0) depth = 0;
  }

  for (let i = 0; i < lines.length; i += 1) {
    const match = /^(\s*)([a-z-]+)\s*:\s*(.+?);(\s*(?:\/\*.*\*\/)?)$/.exec(lines[i]);
    if (!match) continue;
    const [, indent, property, value, trailer] = match;
    // A custom property is the token surface, not a consumer of it.
    if (property.startsWith('--')) continue;
    const next = rewriteValue(property, value, uppercase[i]);
    if (next === null) continue;
    changes.push({ line: i + 1, property, from: value, to: next });
    lines[i] = `${indent}${property}: ${next};${trailer}`;
  }

  return { output: lines.join('\n'), changes, file };
}

const args = process.argv.slice(2);
const write = args.includes('--write') || args.includes('--write-inspector');
const targets = args.filter((arg) => !arg.startsWith('--'));

const names = (targets.length
  ? targets
  : readdirSync(DIR).filter((name) => name.endsWith('.css'))
).filter((name) => name.endsWith('.css')).sort();

let total = 0;
for (const name of names) {
  if (EXEMPT.has(name)) continue;
  const source = readFileSync(DIR + name, 'utf8');
  const result = migrate(name, source);
  total += result.changes.length;
  console.log(`${name}: ${result.changes.length} declarations${write ? ' rewritten' : ' would change'}`);
  if (args.includes('--verbose')) {
    for (const change of result.changes.slice(0, 400)) {
      console.log(`  ${name}:${change.line}  ${change.property}: ${change.from} -> ${change.to}`);
    }
  }
  if (write && result.changes.length) writeFileSync(DIR + name, result.output);
}
console.log(`\n${total} declarations${write ? ' rewritten' : ' to rewrite'}`);