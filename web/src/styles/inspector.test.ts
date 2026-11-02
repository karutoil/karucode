// @vitest-environment node
/* Stylesheet contract for the inspector tab strip.
 *
 * Plugin panels add one tab each (`GET /api/plugins` -> `panels`), so the strip
 * can overflow. It used to hide its own scrollbar - `scrollbar-width: none`
 * plus a `display: none` `::-webkit-scrollbar` - while its tabs were `flex: 1 1
 * 0` with `min-width: 0`, so extra plugin tabs were squeezed until their labels
 * overlapped and there was no bar to scroll with. Component tests run with
 * `css: false`, so this suite reads `inspector.css` directly and pins the
 * behaviour that keeps overflow reachable.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const cssSource = readFileSync(new URL('./inspector.css', import.meta.url), 'utf8');

/** Declarations of a single, non-nested rule. */
function cssDeclarations(selector: string): Record<string, string> {
  const start = cssSource.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined in inspector.css`).toBeGreaterThan(-1);
  const end = cssSource.indexOf('\n}', start);
  const declarations: Record<string, string> = {};
  for (const line of cssSource.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) declarations[match[1]] = match[2];
  }
  return declarations;
}

describe('inspector tab strip stylesheet', () => {
  const strip = cssDeclarations('.tabs');
  const tab = cssDeclarations('.tab');

  it('scrolls horizontally instead of hiding the overflow', () => {
    expect(strip['overflow-x']).toBe('auto');
    // A vertical scroller inside the strip would eat the tab underline.
    expect(strip['overflow-y']).toBe('hidden');
  });

  it('never hides its own scrollbar', () => {
    // `none` leaves overflow with no visible bar at all: the extra tabs were
    // only reachable with a trackpad gesture. The bar is handed back to the
    // app's own 8px `::-webkit-scrollbar` thumb instead (base.css sets `thin` on
    // `html`, which would draw the platform's 16px bar with arrow buttons).
    expect(strip['scrollbar-width']).not.toBe('none');
    expect(cssSource).not.toMatch(/\.tabs::-webkit-scrollbar\s*\{[^}]*display:\s*none/);
  });

  it('keeps a tab label on one line at its content width', () => {
    // `flex: 1 1 0` + `min-width: 0` is what let a crowded strip crush the labels
    // into each other instead of overflowing; `max-content` keeps a tab at its
    // label's width once there is no room left, while `flex-grow: 1` still lets
    // a short strip share the width evenly.
    expect(tab['flex']).toBe('1 0 max-content');
    expect(tab['white-space']).toBe('nowrap');
    // One pathological plugin title must not become a tab wider than the strip.
    expect(tab['max-width']).toBe('100%');
    expect(cssSource).toMatch(/\.tab > span:not\(\.tab-count\):not\(\.tab-icon\) \{[^}]*text-overflow: ellipsis/);
  });

  it('bounds the strip height so an appearing bar cannot shove the panel', () => {
    // The strip is sized by its tab, not by the scrollbar: on a platform whose
    // bar takes space the strip grows by the bar (44 -> 53px) instead of
    // clipping the tabs, so the panel below moves by a bar's height at most.
    expect(strip['min-height']).toBe('44px');
    expect(tab['height']).toBe('44px');
  });
});

describe('changes panel stylesheet', () => {
  it('keeps compact metric tiles and keyboard-visible file selection', () => {
    expect(cssDeclarations('.metric')['min-height']).toBe('64px');
    expect(cssDeclarations('.change-row')['padding']).toBe('12px 4px');
    expect(cssSource).toMatch(/\.change-row:focus-visible\s*\{[^}]*outline: 2px solid var\(--accent\)/);
  });

  it('uses the mock\'s separated, directory-aware file row layout', () => {
    expect(cssDeclarations('.change-row')['border-radius']).toBe('0');
    expect(cssDeclarations('.change-row')['border-bottom']).toBe('1px solid var(--line)');
    expect(cssDeclarations('.file-logo')['width']).toBe('26px');
    expect(cssDeclarations('.file-logo')['height']).toBe('26px');
    expect(cssDeclarations('.change-name')['font-family']).toBe('var(--font-mono)');
    expect(cssDeclarations('.change-dir')['font-family']).toBe('var(--font-mono)');
  });
});

describe('session-info panel stylesheet', () => {
  /* `session-` belongs to the sessions rail.
   *
   * `.session-meta` is the timestamp/message line inside a session row, styled as
   * a one-line flex row in `layout.css`. The Session tab's definition list was
   * also called `.session-meta`, and because `inspector.css` is imported from
   * the same bundle, its `flex-direction: column` + `> div` rules won on source
   * order and restyled every row in the rail: "13m ago" and "832 messages"
   * stacked and centred. A stylesheet test cannot see the cascade, but it can
   * insist the panel stays out of the rail's namespace.
   */
  const htmlSource = readFileSync(
    new URL('../components/SessionInfoTab.tsx', import.meta.url),
    'utf8',
  );

  it('does not use the sessions rail class namespace', () => {
    const railClasses = /\bclass="[^"]*\bsession-[a-z-]+/g;
    const used = [...htmlSource.matchAll(railClasses)].map((match) => match[0]);
    expect(used, `SessionInfoTab must not use session-* classes: ${used.join(', ')}`).toEqual([]);
    // ... and the stylesheet must not define any either.
    expect(cssSource).not.toMatch(/\.session-[a-z-]+\s*[,{:]/);
  });

  it('scopes the panel to si- so a future rename cannot collide', () => {
    expect(htmlSource).toContain('class="inspector-section si-info"');
    expect(cssDeclarations('.si-meta')['flex-direction']).toBe('column');
    // The rail's own rule is untouched and still a row.
    const rail = readFileSync(new URL('./layout.css', import.meta.url), 'utf8');
    const start = rail.indexOf('\n.session-meta {');
    expect(start).toBeGreaterThan(-1);
    expect(rail.slice(start, rail.indexOf('\n}', start))).toContain('display: flex');
  });
});
