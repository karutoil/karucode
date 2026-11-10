// @vitest-environment node
/* The frame contract: the few dimensions that are structural, not stylistic.
 *
 * This file used to assert that the stylesheet reproduced
 * `karucode-v3-themes.html` to the pixel — `.brand-mark` must be 34×34 with an
 * 11px radius, `.msg` must have `margin: 0 0 33px`, `.main-head` must have
 * `gap: 18px`, and so on. That is a test of a mock, not of a product: 11px and
 * 13px radii are not on the six-step radius scale, 33px and 18px are not on the
 * 4px spacing rhythm, and pinning them made every later correction a test
 * failure. It is also why the drift was invisible — the tests were asserting the
 * drift.
 *
 * So the file now guards what actually must not move:
 *
 *   - the column widths and header heights the layout depends on (§frame);
 *   - the responsive breakpoints that decide when a column collapses;
 *   - that the frame metrics are consumed as tokens, not re-typed per component.
 *
 * Everything else is owned by `scale.test.ts` (token discipline) and by each
 * component's own contract test (structure).
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

// `import.meta.url`, not `__dirname`: this suite is ESM, and the whole point of
// these files is to read the stylesheet they are next to.
const DIR = new URL('.', import.meta.url).pathname;
const read = (name: string): string => readFileSync(DIR + name, 'utf8');

const tokens = read('tokens.css');
const layout = read('layout.css');
const rail = read('rail.css');
const chat = read('chat.css');
const composer = read('composer.css');

/** The body of one top-level rule, by selector. */
function rule(css: string, selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

describe('frame metrics are declared once, in tokens.css', () => {
  it('maps the task sidebar onto the Workbench measure', () => {
    expect(tokens).toMatch(/--sidebar-w:\s*var\(--workbench-sidebar-w\)/);
  });

  it.each([
    ['--rail-w', '64px'],
    ['--workbench-sidebar-w', '232px'],
    ['--inspector-w', '342px'],
    ['--content-max', '820px'],
    ['--header-h', '66px'],
  ])('%s is %s', (property, value) => {
    expect(tokens).toMatch(new RegExp(`${property}:\\s*${value}`));
  });

  it('base radius is 10px so the first paint matches the engine default', () => {
    expect(tokens).toMatch(/--radius:\s*10px/);
    expect(tokens).toMatch(/--lh-message:\s*1\.72/);
  });
});

describe('the layout consumes the frame metrics rather than re-typing them', () => {
  it('.main-head takes its height from --header-h', () => {
    const body = rule(layout, '.main-head');
    expect(body).toContain('height: var(--header-h)');
    expect(body).toContain('min-height: var(--header-h)');
  });

  it('.chat-inner caps its measure with --content-max', () => {
    // Declared in layout.css, capped in chat.css.
    expect(`${layout}${chat}`).toMatch(/max-width:\s*calc\(var\(--content-max\)/);
  });

  it('the rail, sidebar and inspector read their widths from tokens', () => {
    const shell = `${layout}${rail}`;
    expect(shell).toMatch(/var\(--rail-w\)/);
    expect(shell).toMatch(/var\(--sidebar-w\)/);
    expect(shell).toMatch(/var\(--inspector-w\)/);
  });
});

describe('responsive breakpoints collapse the right column', () => {
  const media = (width: number): string => {
    const match = tokens.match(new RegExp(`@media\\s*\\(max-width:\\s*${width}px\\)\\s*\\{[^}]+\\}`, 's'));
    expect(match, `${width}px breakpoint`).toBeTruthy();
    return match![0];
  };

  it('narrows the sidebar and inspector at 1350px', () => {
    expect(media(1350)).toMatch(/--sidebar-w:\s*234px/);
    expect(media(1350)).toMatch(/--inspector-w:\s*305px/);
  });

  it('narrows them again at 1100px', () => {
    expect(media(1100)).toMatch(/--sidebar-w:\s*222px/);
    expect(media(1100)).toMatch(/--inspector-w:\s*300px/);
  });

  it('collapses the rail and sidebar at 1023px, reserving no column', () => {
    const block = media(1023);
    expect(block).toMatch(/--sidebar-w:\s*0px/);
    expect(block).toMatch(/--rail-w:\s*0px/);
  });
});

describe('the composer and transcript do not fight the frame', () => {
  it('the transcript keeps a readable measure inside the content column', () => {
    expect(rule(chat, '.chat-inner')).toMatch(/max-width:\s*calc\(var\(--content-max\)/);
  });

  it('the composer dock is positioned by tokens, not by magic numbers', () => {
    const dock = composer.includes('\n.composer-dock {') ? rule(composer, '.composer-dock') : composer;
    expect(dock).toMatch(/var\(--(?:space|radius|content-max)/);
  });
});