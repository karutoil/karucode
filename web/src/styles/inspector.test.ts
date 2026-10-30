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
    expect(strip['min-height']).toBe('var(--tabs-h)');
    expect(tab['height']).toBe('var(--tabs-h)');
  });
});
