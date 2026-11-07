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

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { cssDeclarations } from '../../scripts/css-declarations.mjs';

const DIR = new URL('.', import.meta.url).pathname;
const TOKEN_FILE = 'tokens.css';

/**
 * Sheets that belong to another live workstream and are being migrated by their
 * owner, not by this one. They are named here rather than silently skipped so the
 * exception is visible in the test output and cannot quietly grow:
 * `the token layer is complete` asserts this set stays small.
 *
 * `artifacts.css` used to be listed here. It is now clean against every rule
 * below, so its exemption was a stale debt that hid any regression in the
 * artifact card's chrome; the map is empty and the size assertion keeps it that
 * way unless a real reason is written down.
 */
const OWNED_ELSEWHERE = new Map<string, string>();

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

interface Declaration {
  at: string;
  property: string;
  value: string;
}

/**
 * Declarations from complete rule bodies: own line, after `{`, or after `;`.
 * The shared parser blanks comments/strings, preserves source line numbers and
 * joins multi-line values for the same readable diagnostics as the old reader.
 */
function parseDeclarations(file: string, source: string): Declaration[] {
  return cssDeclarations(source).map((declaration) => ({
    at: `${file}:${declaration.line}`,
    property: declaration.property,
    value: declaration.value.replace(/\s+/g, ' ').trim(),
  }));
}

function declarations(file: string): Declaration[] {
  return parseDeclarations(file, readFileSync(DIR + file, 'utf8'));
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

/** A hand-picked colour: hex, `rgb()`/`rgba()` or `hsl()`/`hsla()`. */
const RAW_COLOUR = /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?)\(/;

describe('the declaration scanner', () => {
  it('joins a value that spans lines, so a raw colour cannot hide', () => {
    const source = [
      '.sheen {',
      '  background:',
      '    radial-gradient(120% 120% at 22% 12%, rgb(255 255 255 / 55%), transparent 42%),',
      '    linear-gradient(145deg, var(--accent), var(--danger));',
      '}',
    ].join('\n');

    const found = parseDeclarations('probe.css', source);

    expect(found).toHaveLength(1);
    expect(found[0]?.at).toBe('probe.css:2');
    expect(found[0]?.property).toBe('background');
    expect(found[0]?.value).toBe(
      'radial-gradient(120% 120% at 22% 12%, rgb(255 255 255 / 55%), transparent 42%), ' +
        'linear-gradient(145deg, var(--accent), var(--danger))',
    );
    // Which is the point: the colour rule now sees the literal on the next line.
    expect(found[0]?.value).toMatch(RAW_COLOUR);
  });

  it('reports the line the property is on, not the line the literal is on', () => {
    const found = parseDeclarations('probe.css', '.a {\n  box-shadow:\n    0 1px 0 #fff;\n}\n');
    expect(found[0]?.at).toBe('probe.css:2');
  });

  it('keeps one-line declarations as they were, without their terminator', () => {
    const found = parseDeclarations('probe.css', '.a {\n  color: var(--text);\n  padding: 0;\n}\n');
    expect(found).toEqual([
      { at: 'probe.css:2', property: 'color', value: 'var(--text)' },
      { at: 'probe.css:3', property: 'padding', value: '0' },
    ]);
  });

  it('ignores a value quoted in a comment, including a multi-line one', () => {
    const found = parseDeclarations(
      'probe.css',
      '/* the mock used\n   background: #ffffff; here */\n.a {\n  color: var(--text);\n}\n',
    );
    expect(found).toEqual([{ at: 'probe.css:4', property: 'color', value: 'var(--text)' }]);
  });

  it('reads a declaration closed by the end of its block', () => {
    const found = parseDeclarations('probe.css', '.a {\n  color: var(--text) }\n');
    expect(found).toEqual([{ at: 'probe.css:2', property: 'color', value: 'var(--text)' }]);
  });
});

describe('inline and inline-multiline declarations', () => {
  it('reads a custom property whose name carries a digit', () => {
    // The old pattern was `[a-z-]+`, which matched `--space-` and then failed on
    // the `1`, so every numbered token (`--space-1`, `--surface-2`, `--fs-label`)
    // was invisible to the audit — including the guard that checks a sheet only
    // reads tokens that exist.
    const found = parseDeclarations(
      'probe.css',
      ':root {\n  --space-1: 2px;\n  --surface-2: var(--surface-raised);\n}\n',
    );
    expect(found).toEqual([
      { at: 'probe.css:2', property: '--space-1', value: '2px' },
      { at: 'probe.css:3', property: '--surface-2', value: 'var(--surface-raised)' },
    ]);
  });

  it('reads complete rule bodies and preserves declaration start lines', () => {
    const css = [
      '/* .comment { color: #abcdef; } */',
      '.inline { color: #123456; padding: 3px; }',
      '.inline-multiline { font-size:',
      '  9px; margin: 7px; }',
      '.strings::before { content: ".fake { color: #fedcba; }"; }',
      '/* a second comment',
      '   with { padding: 13px; } */',
      '.after { border-radius: 5px; }',
    ].join('\n');

    expect(cssDeclarations(css)).toEqual([
      { property: 'color', value: '#123456', line: 2 },
      { property: 'padding', value: '3px', line: 2 },
      { property: 'font-size', value: '9px', line: 3 },
      { property: 'margin', value: '7px', line: 4 },
      { property: 'content', value: '', line: 5 },
      { property: 'border-radius', value: '5px', line: 8 },
    ]);
  });
});

describe('the avatar sheen is themed, not a literal', () => {
  it('reads the multi-line gradient as one declaration with no raw colour', () => {
    // The exact defect this scanner gained multi-line support for: `background:`
    // on one line and `rgb(255 255 255 / 55%)` on the next was invisible to the
    // audit and to this test, so the literal survived several passes.
    const sheen = declarations('chat.css').find(
      (declaration) => declaration.property === 'background' && declaration.value.includes('radial-gradient'),
    );
    expect(sheen, 'the avatar gradient is read as one declaration').toBeDefined();
    expect(sheen?.value).not.toMatch(RAW_COLOUR);
    expect(sheen?.value).toContain('color-mix(');
  });
});

describe('which sheets the rules cover', () => {
  it('scans artifacts.css rather than exempting it', () => {
    // The exemption was stale: the sheet passes the rules, and an exception is a
    // hiding place for the next raw value someone adds there.
    expect(sheets()).toContain('artifacts.css');
    expect(OWNED_ELSEWHERE.has('artifacts.css')).toBe(false);
  });

  it('scans every sheet except the token layer and named exemptions', () => {
    const all = readdirSync(DIR).filter((name) => name.endsWith('.css'));
    const unscanned = all.filter((name) => !sheets().includes(name));
    expect(unscanned.sort()).toEqual(
      all.filter((name) => name === TOKEN_FILE || OWNED_ELSEWHERE.has(name)).sort(),
    );
  });
});

describe('no raw colour outside the token layer (§5.1)', () => {
  it('declares no hex, rgb() or hsl() literal', () => {
    const violations = scan(({ property, value }) => {
      if (property.startsWith('--')) return false; // custom properties are the token surface
      return RAW_COLOUR.test(value);
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

  it('defines every custom property the sheets read', () => {
    // An undefined `var()` is invalid at computed-value time: the whole declaration
    // is dropped, so a typo'd token paints nothing and no test notices. This is the
    // defect the alias comment in tokens.css describes (seven silently empty fills),
    // so it gets a guard of its own.
    //
    // Two legitimate sources of a name that is not in a sheet: the token layer, and
    // a runtime writer (`style.setProperty`, `useHeightVariable`). Anything else is
    // a typo.
    const declared = new Set<string>();
    for (const file of readdirSync(DIR).filter((name) => name.endsWith('.css'))) {
      for (const declaration of declarations(file)) {
        if (declaration.property.startsWith('--')) declared.add(declaration.property.slice(2));
      }
    }
    const runtime = new Set<string>();
    const source = (path: string): string => readFileSync(path, 'utf8');
    const src = new URL('../', import.meta.url).pathname;
    const walk = (dir: string): void => {
      for (const name of readdirSync(dir)) {
        if (name === 'node_modules' || name.startsWith('.')) continue;
        const path = `${dir}${name}`;
        if (statSync(path).isDirectory()) walk(`${path}/`);
        else if (/\.(ts|tsx)$/.test(name) && !name.includes('.test.')) {
          for (const match of source(path).matchAll(/--([a-z0-9-]+)/g)) runtime.add(match[1]!);
        }
      }
    };
    walk(src);

    const used = new Set<string>();
    for (const file of sheets()) {
      for (const declaration of declarations(file)) {
        for (const match of declaration.value.matchAll(/var\(--([a-z0-9-]+)/g)) {
          used.add(match[1]!);
        }
      }
    }
    const missing = [...used].filter((name) => !declared.has(name) && !runtime.has(name)).sort();
    expect(missing, `undefined tokens read by a sheet:\n${missing.join('\n')}`).toEqual([]);
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