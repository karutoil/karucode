// @vitest-environment node
/* Visual contract for tool calls as one family.
 *
 * A tool invocation and a shell command are different kinds of work, but on the
 * transcript they are sibling events: both need the same header rhythm, panel
 * surface, status language and expandable output. This pins the shared chrome
 * while deliberately leaving each one's interaction and content structure to
 * its component tests.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./chat.css', import.meta.url), 'utf8');

function declarations(selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const result: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) result[match[1]] = match[2];
  }
  return result;
}

function sharedDeclarations(first: string, second: string): Record<string, string> {
  const marker = `\n${first},\n${second} {`;
  const start = css.indexOf(marker);
  expect(start, `${first} and ${second} share a rule`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const result: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) result[match[1]] = match[2];
  }
  return result;
}

describe('tool calls: one visual language', () => {
  it('shares one themed card shell and one error treatment', () => {
    const shell = sharedDeclarations('.tool-card', '.command-card');
    expect(shell['border']).toBe('1px solid var(--line-strong)');
    // §3.4: the card is one radius, and it is the token's, not a literal.
    expect(shell['border-radius']).toBe('var(--radius)');
    expect(shell['background']).toBe('var(--surface-card)');
    expect(css).toMatch(/\.tool-card\[data-status='error'\],[\s\S]*?\.command-card\[data-status='error'\]/);
  });

  it('aligns call headers, status, and output typography', () => {
    expect(css).toMatch(/\.tool-head,\s*\n\.command-summary \{/);
    for (const selector of ['.tool-body', '.cmd-output']) {
      const body = declarations(selector);
      expect(body['color'], selector).toBe('var(--text-secondary)');
      expect(body['font-family'], selector).toBe('var(--font-mono)');
      expect(body['font-size'], selector).toBe('var(--fs-tiny)');
      expect(body['line-height'], selector).toBe('1.8');
      // The well is the console rung: a step *below* the card in Ember (-4.3
      // HSL points) and in Daybreak (-3.0). `--surface-sunken` resolves to a
      // line tone and sits 6 points *above* an Ember card, so a console read as
      // a second card there. See the recess ladder in chat.css.
      expect(body['background'], selector).toBe('var(--surface-command-output)');
    }
    expect(declarations('.command-status.is-ok')['color']).toBe('var(--success)');
    expect(declarations('.command-status.is-error')['color']).toBe('var(--danger)');
  });

  it('keeps cards and command headers compact on phones', () => {
    const phone = css.slice(css.indexOf('@media (max-width: 640px)'));
    expect(phone).toMatch(/\.tool-card,\s*\n\s*\.command-card \{[^}]*margin: var\(--space-6\) 0 var\(--space-6\)/s);
    expect(phone).toMatch(/\.tool-head,\s*\n\s*\.command-summary \{[^}]*min-height: 38px/s);
    expect(phone).toMatch(/\.cmd-output \{[^}]*padding: var\(--space-4\) var\(--space-5\)/s);
  });
});
