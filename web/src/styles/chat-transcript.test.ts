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

  it('shrinks a long model id without losing the status chip or the rewind', () => {
    // 320-390px: an unbounded model id pushed the tag, the throughput badge and
    // the rewind button off the metadata row.
    const top = phoneRule('.msg-assistant .msg-top');
    expect(top).toContain('flex-wrap: wrap;');
    expect(top).toContain('height: auto;');
    const pill = phoneRule('.msg-assistant .model-pill');
    expect(pill).toContain('min-width: 0;');
    expect(pill).toContain('overflow: hidden;');
    expect(pill).toContain('text-overflow: ellipsis;');
    expect(pill).toContain('white-space: nowrap;');
    // Nothing in the metadata is hidden to make room...
    expect(phoneSection()).not.toMatch(/\.msg-(?:top|author|time)\b[^{]*\{[^}]*display:\s*none/);
    // ...and on a phone the rewind control is not left at desktop's hover-only
    // opacity, because there is no hover.
    expect(css).toMatch(/@media \(hover: none\)[\s\S]*?\.rewind-button \{[^}]*opacity: 1;/);
  });
});
