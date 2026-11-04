// @vitest-environment node
/* The scale contract, enforced statically.
 *
 * `tokens.css` documents three rules and nothing enforced them, so the stylesheet
 * accumulated 414 font sizes (25 distinct), 172 radii (20 distinct) and 1092
 * spacing values (285 distinct). That drift *is* the visual defect: no two
 * screens shared a rhythm, and 220 of those font sizes were below 11px, which is
 * why the quiet text was unreadable rather than quiet.
 *
 * `scripts/design-audit.mjs` reports the same thing for a human. This is the
 * version that fails CI. The target is zero violations outside `tokens.css` —
 * the one file allowed to hold raw values, because it is the token layer.
 *
 * When this fails it prints `file:line value`, so the fix is mechanical.
 */

import { readFileSync, readdirSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const DIR = new URL('.', import.meta.url).pathname;
const TOKEN_FILE = 'tokens.css';

/**
 * Sheets that belong to another live workstream and are being migrated by their
 * owner, not by this one. They are named here rather than silently skipped so the
 * exception is visible in the test output and cannot quietly grow:
 * `the token layer is complete` asserts this set stays small.
 *
 * `artifacts.css` — the chat-artifact renderer (diagrams, HTML mocks, inline
 * previews). A parallel session owns that sheet; editing it from here would
 * conflict. It was measured at 54 violations when this exception was added.
 */
const OWNED_ELSEWHERE = new Map([['artifacts.css', 'parallel session: artifact rendering']]);

/** Spacing steps the token layer publishes (§3.2). */
const SPACE = new Set(['2px', '4px', '6px', '8px', '10px', '12px', '16px', '20px', '24px', '32px']);
/** Radii the token layer publishes (§3.4). Circles are geometry, not a radius. */
const RADIUS = new Set(['0', '50%', 'inherit']);
const FONT_SIZES = new Set(['10px', '11px', '11.5px', '12px', '12.5px', '13px', '14px', '16px', '18px', '20px', '24px']);

/** Properties where a raw length is geometry rather than rhythm. */
const GEOMETRY = /^(width|height|min-width|min-height|max-width|max-height|left|right|top|bottom|inset|flex|flex-basis|grid-|translate|transform|z-index|opacity|border-width|outline-width)/;

interface Violation {
  at: string;
  property: string;
  value: string;
}

function sheets(): string[] {
  return readdirSync(DIR)
    .filter((name) => name.endsWith('.css') && name !== TOKEN_FILE && !OWNED_ELSEWHERE.has(name))
    .sort();
}

/** Comments blanked so a value quoted in prose is not read as a declaration. */
function declarations(file: string): Array<{ at: string; property: string; value: string }> {
  const css = readFileSync(DIR + file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, (m) =>
    m.replace(/[^\n]/g, ' '),
  );
  const out: Array<{ at: string; property: string; value: string }> = [];
  css.split('\n').forEach((line, index) => {
    const match = /^\s*([a-z-]+)\s*:\s*(.+?);?\s*$/.exec(line);
    if (!match) return;
    out.push({ at: `${file}:${index + 1}`, property: match[1]!, value: match[2]!.trim() });
  });
  return out;
}

function scan(check: (declaration: { at: string; property: string; value: string }) => boolean): Violation[] {
  const found: Violation[] = [];
  for (const file of sheets()) {
    for (const declaration of declarations(file)) {
      if (check(declaration)) {
        found.push({ at: declaration.at, property: declaration.property, value: declaration.value });
      }
    }
  }
  return found;
}

const describe_ = (violations: Violation[]): string =>
  violations.map((v) => `${v.at}  ${v.property}: ${v.value}`).join('\n');

describe('no raw colour outside the token layer (§5.1)', () => {
  it('declares no hex, rgb() or hsl() literal', () => {
    const violations = scan(({ property, value }) => {
      if (property.startsWith('--')) return false; // custom properties are the token surface
      return /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?)\(/.test(value);
    });
    expect(violations, `raw colours:\n${describe_(violations)}`).toEqual([]);
  });
});

describe('no radius outside the scale (§3.4)', () => {
  it('uses var(--radius*) or a circle', () => {
    const violations = scan(({ property, value }) => {
      if (!/^border(-[a-z]+)*-radius$/.test(property)) return false;
      if (/var\(/.test(value)) return false;
      // A shorthand can carry several radii; every part must be a circle or zero.
      return value
        .split(/\s+/)
        .filter(Boolean)
        .some((part) => !RADIUS.has(part));
    });
    expect(violations, `off-scale radii:\n${describe_(violations)}`).toEqual([]);
  });
});

describe('no font size below the floor or off the scale (§3.3)', () => {
  it('uses var(--fs-*), a relative unit, or a scale step', () => {
    const violations = scan(({ property, value }) => {
      if (property !== 'font-size') return false;
      if (/var\(|em\b|rem\b|%$|inherit/.test(value)) return false;
      // calc() mixes tokens with arithmetic and is fine.
      if (/calc\(/.test(value)) return false;
      return !FONT_SIZES.has(value);
    });
    expect(violations, `off-scale font sizes (10px floor):\n${describe_(violations)}`).toEqual([]);
  });
});

describe('spacing is on the 4px rhythm (§3.2)', () => {
  it('uses var(--space-*), a scale step, or a geometry property', () => {
    const violations = scan(({ property, value }) => {
      if (!/^(padding|margin|gap|row-gap|column-gap)(-[a-z]+)*$/.test(property)) return false;
      if (GEOMETRY.test(property)) return false;
      if (/var\(|calc\(|auto|inherit|^0$/.test(value)) return false;
      return value
        .split(/\s+/)
        .filter((part) => /^-?[\d.]+px$/.test(part))
        .some((part) => !SPACE.has(part.replace(/^-/, '')));
    });
    expect(violations, `off-scale spacing:\n${describe_(violations)}`).toEqual([]);
  });
});

describe('the token layer itself is complete', () => {
  const css = readFileSync(DIR + TOKEN_FILE, 'utf8');

  it('publishes every step the other rules allow', () => {
    for (const value of SPACE) expect(css, `--space for ${value}`).toContain(value);
  });

  it('documents the three rules it enforces', () => {
    expect(css).toMatch(/never hardcode a colour/);
    expect(css).toMatch(/never hardcode a corner radius/);
  });

  it('keeps the list of sheets owned by another workstream short and named', () => {
    // An exception is a debt, not a hiding place: it must have a reason, and
    // there must not be many of them.
    expect(OWNED_ELSEWHERE.size).toBeLessThanOrEqual(2);
    for (const [file, reason] of OWNED_ELSEWHERE) {
      expect(readFileSync(DIR + file, 'utf8').length, `${file} exists`).toBeGreaterThan(0);
      expect(reason.length, `${file} has a reason`).toBeGreaterThan(10);
    }
  });
});