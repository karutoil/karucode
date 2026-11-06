// @vitest-environment node
/* Stylesheet contract for the accent as a *fill*.
 *
 * The palette's `--accent` is tuned to be legible as text on a canvas. In light
 * mode that makes it a dark ink (Daybreak: #bb5f2c is 4.39:1 on white) — fine for
 * a label, wrong for a button, because a button needs the reverse relationship:
 * light text on a fill. Painting Send, the assistant avatar and the rail logo
 * with `--accent` gave 3.6:1 white-on-coral: a primary action that fails AA.
 *
 * The engine already derives the pair that works — `accentText` (the accent
 * nudged until it passes as small text) and `onAccent` (readable ink over it) —
 * so the stylesheet must use `--accent-fill`/`--on-accent` for filled controls
 * and leave `--accent` for text, glyphs and hairlines.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (name: string): string => readFileSync(new URL(name, import.meta.url), 'utf8');
const tokens = read('./tokens.css');
const composer = read('./composer.css');
const chat = read('./chat.css');
const rail = read('./rail.css');

/** Declarations of one rule, matched by its full selector text. */
function declarations(css: string, selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const found: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
  }
  return found;
}

/** Raw body of one rule, for multi-line values the declaration parser skips. */
function ruleBody(css: string, selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

describe('accent fill: a primary action is readable in every theme', () => {
  it('exposes the fill and its ink as first-paint tokens', () => {
    // The engine writes `--theme-*` at runtime; the static block has to carry the
    // same two so the first frame is not an empty `var()`.
    expect(tokens).toMatch(/--theme-accent-text:\s*#[0-9a-f]{6};/);
    expect(tokens).toMatch(/--theme-on-accent:\s*#[0-9a-f]{6};/);
    expect(tokens).toMatch(/--accent-fill:\s*var\(--theme-accent-text\);/);
    expect(tokens).toMatch(/--on-accent:\s*var\(--theme-on-accent\);/);
  });

  it('paints the send button with the fill pair, not the text accent', () => {
    const send = declarations(composer, '.button-send');
    expect(send['background']).toBe('var(--accent-fill)');
    expect(send['color']).toBe('var(--on-accent)');
  });

  it('paints the assistant avatar with the fill pair', () => {
    // The avatar's background is a multi-line gradient, so match the whole rule.
    const block = ruleBody(chat, '.msg-avatar.agent');
    expect(block).toContain('color: var(--on-accent);');
    expect(block).toContain('var(--accent-fill)');
    expect(block).not.toMatch(/var\(--accent\)/);
  });

  it('paints the rail logo with the fill pair', () => {
    const block = ruleBody(rail, '.brand-mark');
    expect(block).toContain('color: var(--on-accent);');
    expect(block).toContain('var(--accent-fill)');
  });

  it('keeps text accents on the role meant for text', () => {
    // The model pill is small mono type on an accent *tint*. `--accent-text` is
    // the AA-safe ink against the canvas; a tint lifts the ground under it, where
    // the pair measured 3.62:1 in Daybreak. `--accent-on-tint` is the same accent
    // mixed toward primary text for exactly that case, so it is the stricter of
    // the two answers and the one this pill must use.
    const pill = declarations(chat, '.model-pill');
    expect(pill['color']).toBe('var(--accent-on-tint)');
  });

  it('defines one ink for accent-on-tint and uses it wherever a tint is painted', () => {
    expect(tokens).toMatch(/--accent-on-tint:\s*color-mix\(in srgb, var\(--accent-fill\)/);
    // A bare `--accent` on `--accent-bg` was the 3.62:1 chip the audit measured.
    // Nothing may reintroduce that pair: scan every rule body that paints one of
    // the accent tints for an accent-coloured ink.
    const tints = /background(-color)?:\s*var\(--(accent-bg|accent-surface|row-accent-bg|row-selected-bg|btn-accent-bg|btn-accent-bg-hover|theme-accent-bg)\)/;
    const offenders: string[] = [];
    for (const name of ['chat.css', 'composer.css', 'git.css', 'inspector.css', 'layout.css', 'overlays.css', 'rail.css', 'settings.css', 'goals.css', 'todo.css', 'control-shared.css', 'control-rail.css', 'control-overview.css', 'control-session.css']) {
      const css = read(`./${name}`);
      for (const match of css.matchAll(/\{[^{}]*\}/g)) {
        const body = match[0];
        if (!tints.test(body)) continue;
        // Split on `;` and read the *property*, not a substring: `border-color`
        // and `background-color` both end in "color" and are not ink.
        const inks = body
          .split(';')
          .map((decl) => decl.trim())
          .filter((decl) => /^color:\s*var\(--(accent|accent-fill|accent-text|accent-text-soft|accent-hi)\)$/.test(decl));
        if (inks.length > 0) {
          offenders.push(`${name}: ${body.replace(/\s+/g, ' ').slice(0, 90)}`);
        }
      }
    }
    expect(offenders, 'accent ink on an accent tint').toEqual([]);
  });

  it('paints the control plane’s project initials with the AA accent role', () => {
    // The tones are how the control plane tells projects apart. Tone 3 is the
    // accent tile, and its initials were `--accent` on `--accent-bg`: 3.61:1 in
    // Daybreak. Initials are text, so they take the same role as every other
    // accent label on a tint.
    // The glyph is shared by all three regions, so it lives in the shared sheet.
    const glyph = ruleBody(read('./control-shared.css'), ".cp-glyph[data-tone='3']");
    expect(glyph).toContain('color: var(--accent-on-tint);');
    expect(glyph).not.toMatch(/color:\s*var\(--accent\);/);
  });
});
