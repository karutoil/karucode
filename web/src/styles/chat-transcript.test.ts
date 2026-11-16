// @vitest-environment node
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./chat.css', import.meta.url), 'utf8');

function rule(selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  return css.slice(start, end);
}

/** The phone section: everything from its banner comment to the next breakpoint. */
function phoneSection(): string {
  const start = css.indexOf('/* ---------- phones ----------');
  expect(start, 'the phone section exists').toBeGreaterThan(-1);
  const end = css.indexOf('@media (max-width: 1023px)', start);
  expect(end, 'the phone section ends at the next breakpoint').toBeGreaterThan(start);
  return css.slice(start, end);
}

/** One rule inside the phone section, starting at its selector. */
function phoneRule(selector: string): string {
  const phone = phoneSection();
  const start = phone.indexOf(selector);
  expect(start, `${selector} is defined for phones`).toBeGreaterThan(-1);
  const open = phone.indexOf('{', start);
  return phone.slice(start, phone.indexOf('}', open));
}

describe('transcript mock layout contract', () => {
  it('keeps the compact avatar and conversation row rhythm at every density', () => {
    // The rhythm is the scale's, not a literal: 16px between the avatar column
    // and the body, 32px between turns (§3.2).
    expect(rule('.msg')).toContain('gap: var(--space-7);');
    expect(rule('.msg')).toContain('margin: 0 0 var(--space-10);');
    expect(css).not.toMatch(/\.msg\s*\{[^}]*margin-bottom:\s*(?:22|41)px/s);
    expect(css).toContain(".date-marker::before,");
    expect(css).toContain(".date-marker::after {");
  });

  it('right-biases compact prompts without disturbing the assistant reading column', () => {
    const user = rule('.msg-user');
    expect(user).toContain('width: fit-content;');
    expect(user).toContain('max-width: min(88%, 640px);');
    expect(user).toContain('margin-left: auto;');
    expect(rule('.msg-user .msg-avatar')).toContain('order: 2;');
    expect(rule('.msg-user .msg-col')).toContain('flex: 0 1 auto;');
    expect(rule('.user-question')).toContain('padding: var(--space-5) var(--space-6);');
    expect(rule('.user-question')).toContain('background: var(--surface-raised);');
    expect(rule('.msg-assistant .markdown')).toContain('font-size: var(--fs-body);');
  });

  it('shares themed card chrome and header sizing for tools and commands', () => {
    const cards = rule('.tool-card,\n.command-card');
    expect(cards).toContain('margin: var(--space-8) 0 var(--space-7);');
    expect(cards).toContain('border: 1px solid var(--line-strong);');
    expect(cards).toContain('background: var(--surface-card);');
    expect(rule(".tool-card[data-status='error'],\n.command-card[data-status='error']")).toContain('var(--danger)');
    const headers = rule('.tool-head,\n.command-summary');
    expect(headers).toContain('height: 44px;');
    expect(headers).toContain('padding: 0 var(--space-7);');
    // B2: output ink is `--text-secondary` (10:1 in Daybreak), not
    // `--text-output`, which measured 4.27:1 there.
    expect(rule('.cmd-output')).toContain('color: var(--text-secondary);');
    expect(rule('.cmd-output[hidden]')).toContain('display: none;');
    expect(rule('.command-scroll')).toContain('overflow-y: auto;');
    // A compact command row wraps that scroller. The wrapper must not be a
    // second bounded scroller, or an expanded command shows two scroll wheels.
    expect(rule('.command-row > .cmd-output')).not.toContain('overflow: auto;');
    expect(rule('.command-row > .cmd-output')).not.toContain('max-height:');
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.tool-card,\s*\.command-card\s*\{[^}]*margin: var\(--space-6\) 0 var\(--space-6\);/);
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.tool-head,\s*\.command-summary\s*\{[^}]*min-height: 38px;/);
  });

  it('preserves transcript options and readable compact layouts on phones', () => {
    expect(css).toContain(":root[data-tool-style='rows'] .msg-body > .tool-row");
    expect(css).toContain(":root[data-grouping='turns'] .msg-body.is-folded");
    expect(css).toContain('.tool-group');
    expect(css).toContain('.process-fold');
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.msg\s*\{[^}]*gap: 0;/);
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.date-marker\s*\{[^}]*margin: var\(--space-2\) auto var\(--space-10\);/);
  });
});

describe('phone and density contract', () => {
  it('pins the phone tool/command header at 38px instead of only flooring it', () => {
    const headers = phoneRule('.tool-head,');
    // The shared rule sets `height: 44px`; a `min-height` below a fixed height
    // loses, so this block has to pin the height itself.
    expect(headers).toMatch(/\n\s+height: 38px;/);
    // `min-height` stays as the documented floor for content that grows.
    expect(headers).toContain('min-height: 38px;');
    // The desktop band is unchanged.
    expect(rule('.tool-head,\n.command-summary')).toContain('height: 44px;');
  });

  it('does not add a second, hand-written vertical rhythm in compact density', () => {
    // Density already scales `--space-*` (the turn gap) and the type scale; the
    // compact-only `.msg` padding made the row tighter than the setting said.
    expect(css).toContain(":root[data-density='compact'] .chat-inner");
    expect(css).not.toMatch(/:root\[data-density='compact'\]\s*\.msg\s*\{/);
    // The row's own box stays padding-free: the gap under the turn is the rhythm.
    expect(rule('.msg')).toContain('padding: 0;');
  });

  it('keeps live status and rewind reachable on a narrow transcript', () => {
    const top = phoneRule('.msg-assistant .msg-top');
    expect(top).toContain('flex-wrap: wrap;');
    expect(top).toContain('height: auto;');
    // Nothing in the live metadata is hidden to make room...
    expect(phoneSection()).not.toMatch(/\.msg-(?:top|author|time)\b[^{]*\{[^}]*display:\s*none/);
    // ...and rewind never becomes hover-only on a touch device. The compact
    // right-biased prompt uses the same control rather than removing it.
    expect(css).toMatch(/@media \(hover: none\)[\s\S]*?\.rewind-button \{[^}]*opacity: 1;/);
  });

  it('floats a call’s rewind over its head in every transcript style', () => {
    // Rewind rides each stored row, including every tool call and command. A
    // call's head is a fixed-height flex row that already spends its width on
    // the call and its outcome, so the button is positioned over it: joining the
    // flow pushed the outcome off a narrow card and changed the head's height as
    // it faded in. The vertical offset has to match the head it overlays.
    const floated = css.match(
      /\.tool-card > \.rewind-tool,[\s\S]*?\.command-row > \.rewind-tool \{([^}]*)\}/,
    );
    expect(floated, 'the call rewind rule exists').not.toBeNull();
    expect(floated![1]).toContain('position: absolute;');
    expect(floated![1]).toContain('right: var(--space-5);');
    // The compact rows' head is 26px, the cards' is 44px: one offset cannot fit
    // both, or the pill straddles the border.
    expect(css).toMatch(
      /\.tool-row > \.rewind-tool,\s*\.command-row > \.rewind-tool \{[^}]*top: 3px;/,
    );
    expect(floated![1]).toContain('top: 11px;');
    // It is revealed by hovering the call, and the row is its containing block.
    expect(css).toMatch(/\.tool-card:hover > \.rewind-tool,[\s\S]*?opacity: 1;/);
    expect(rule('.tool-row')).toContain('position: relative;');
    expect(rule('.command-row')).toContain('position: relative;');
    // On a touch screen the pill would cover the call's own text, so it takes a
    // row of its own under the head instead of floating over it.
    expect(css).toMatch(
      /@media \(hover: none\)[\s\S]*?\.tool-card > \.rewind-tool,[\s\S]*?position: static;/,
    );
  });
});
