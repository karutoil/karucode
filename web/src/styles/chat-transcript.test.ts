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
    expect(rule('.msg')).toContain('gap: 14px;');
    expect(rule('.msg')).toContain('margin: 0 0 33px;');
    expect(css).not.toMatch(/\.msg\s*\{[^}]*margin-bottom:\s*(?:22|41)px/s);
    expect(css).toContain(".date-marker::before,");
    expect(css).toContain(".date-marker::after {");
  });

  it('shares themed card chrome and header sizing for tools and commands', () => {
    const cards = rule('.tool-card,\n.command-card');
    expect(cards).toContain('margin: 18px 0 15px;');
    expect(cards).toContain('border: 1px solid var(--line-card-strong);');
    expect(cards).toContain('background: var(--surface-card);');
    expect(rule(".tool-card[data-status='error'],\n.command-card[data-status='error']")).toContain('var(--danger)');
    const headers = rule('.tool-head,\n.command-summary');
    expect(headers).toContain('height: 44px;');
    expect(headers).toContain('padding: 0 15px;');
    expect(rule('.cmd-output')).toContain('color: var(--text-output);');
    expect(rule('.cmd-output[hidden]')).toContain('display: none;');
    expect(rule('.command-scroll')).toContain('overflow-y: auto;');
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.tool-card,\s*\.command-card\s*\{[^}]*margin: 13px 0 11px;/);
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.tool-head,\s*\.command-summary\s*\{[^}]*min-height: 38px;/);
  });

  it('preserves transcript options and readable compact layouts on phones', () => {
    expect(css).toContain(":root[data-tool-style='rows'] .msg-body > .tool-row");
    expect(css).toContain(":root[data-grouping='turns'] .msg-body.is-folded");
    expect(css).toContain('.tool-group');
    expect(css).toContain('.process-fold');
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.msg\s*\{[^}]*gap: 0;/);
    expect(css).toMatch(/@media \(max-width: 640px\)[\s\S]*?\.date-marker\s*\{[^}]*margin: 4px auto 30px;/);
  });
});
