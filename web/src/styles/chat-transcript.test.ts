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
