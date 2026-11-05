// @vitest-environment node
/* The artifact card's static layout contract.
 *
 * The component tests (`ArtifactCard.test.tsx`) drive the behaviour - publishing,
 * failing, copying, expanding. These pin the three rules the card's CSS has to
 * keep, because each of them is a size or ink decision that a component test in
 * jsdom cannot see:
 *
 * 1. An HTML artifact honours the same height cap as every other kind on a
 *    phone; the expanded dialog is the one place it may exceed it.
 * 2. Captions and notices are read, so their ink is `--muted`, not the faintest
 *    ink the theme publishes.
 * 3. The cap itself is defined once, on the card, and lowered for phones.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./artifacts.css', import.meta.url), 'utf8');

/** The whole `@media (max-width: 640px)` block, matched by brace depth. */
function phoneBlock(): string {
  const start = css.indexOf('@media (max-width: 640px)');
  expect(start, 'the phone block exists').toBeGreaterThan(-1);
  let depth = 0;
  for (let index = css.indexOf('{', start); index < css.length; index += 1) {
    if (css[index] === '{') depth += 1;
    else if (css[index] === '}') {
      depth -= 1;
      if (depth === 0) return css.slice(start, index + 1);
    }
  }
  throw new Error('unterminated phone block');
}

function rule(selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

function phoneRule(selector: string): string {
  const phone = phoneBlock();
  // Exact selector boundary: `.artifact` must not match `.artifact-head`.
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`${escaped}\\s*\\{`).exec(phone);
  expect(match, `${selector} is defined for phones`).not.toBeNull();
  const open = phone.indexOf('{', match!.index);
  return phone.slice(match!.index, phone.indexOf('}', open));
}

describe('artifact frame height', () => {
  it('caps a phone frame at the card height every other kind honours', () => {
    // The frame's height is set inline from the document (or the 420px default),
    // and a mock may report up to 720px. Without this cap one HTML artifact
    // filled the screen and pushed the answer below it.
    expect(phoneRule('.artifact-frame')).toContain('max-height: var(--artifact-max-h);');
  });

  it('defines the cap once on the card and lowers it for phones', () => {
    expect(rule('.artifact')).toContain('--artifact-max-h: 520px;');
    expect(phoneRule('.artifact')).toContain('--artifact-max-h: 380px;');
    // Every non-frame kind already sizes to the cap; the frame is the new one.
    expect(rule('.artifact-markdown')).toContain('max-height: var(--artifact-max-h);');
    expect(rule('.artifact-table-wrap')).toContain('max-height: var(--artifact-max-h);');
    expect(rule('.artifact-image')).toContain('max-height: var(--artifact-max-h);');
  });

  it('lets the expanded view opt out: the reader asked for it', () => {
    const expanded = rule('.artifact-expanded .artifact-frame');
    expect(expanded).toContain('max-height: none;');
    expect(expanded).toContain('min-height: 60vh;');
    // The other kinds opt out of their cap in full screen too.
    expect(css).toMatch(
      /\.artifact-expanded \.artifact-svg,[\s\S]*?\.artifact-markdown \{\s*max-height: none;/,
    );
  });
});

describe('artifact caption and notice ink', () => {
  it('sets the table caption in --muted, not the faintest ink', () => {
    // "N more rows are not shown" is part of reading the table.
    expect(rule('.artifact-caption')).toContain('color: var(--muted);');
    expect(rule('.artifact-caption')).not.toContain('var(--faint)');
  });

  it('sets notes and notices in --muted, not the faintest ink', () => {
    // "Rendered in an isolated frame…", "shown as source only", "cannot be
    // previewed here": each says what the artifact is or what is missing.
    const notices = css.slice(
      css.indexOf('\n.artifact-note,') + 1,
      css.indexOf('\n}', css.indexOf('\n.artifact-note,')),
    );
    expect(notices).toContain('color: var(--muted);');
    expect(notices).not.toContain('var(--faint)');
  });
});