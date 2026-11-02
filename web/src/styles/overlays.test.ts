// @vitest-environment node
/* Stylesheet contract for the file viewer's preview surface.
 *
 * Component tests run with `css: false`, so jsdom cannot measure layout; this
 * suite reads `overlays.css` directly and pins the geometry that decides how
 * much of the modal a rendered file actually uses.
 *
 * The bug it exists for: `.file-preview-markdown` carried `max-width: 78ch` as a
 * reading measure, but that element is *also* the scrolling surface
 * (`FilePreview` puts `file-preview-scroll file-preview-markdown` on one node).
 * The cap therefore shrank the panel instead of the prose column, and the rest of
 * the modal showed as a strip of empty background down the right-hand side.
 * Measured in Chromium at 1440x900 before the fix: `.file-preview-scroll` 564px
 * inside a 1038px `.file-view-pane`, while the HTML, image, PDF and source panes
 * all filled it. That is a visual defect no jsdom assertion can observe, so the
 * measured proportions are written down here instead.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./overlays.css', import.meta.url), 'utf8');
const previewSource = readFileSync(new URL('../components/FilePreview.tsx', import.meta.url), 'utf8');

/** Declarations of one rule, matched by its full selector text. */
function declarations(selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined in overlays.css`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const found: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
  }
  return found;
}

describe('file viewer preview surface', () => {
  it('keeps the markdown preview at the full pane width', () => {
    // `.file-preview-markdown` is the scroll surface, so any `max-width` short of
    // `none` is a blank strip on the right of the modal. A `ch` measure here
    // would also make the column depend on the font size (78ch measured 564px).
    expect(declarations('.file-preview-markdown')['max-width']).toBe('none');
    expect(css).not.toMatch(/\.file-preview-markdown\s*\{[^}]*max-width:\s*\d/);
  });

  it('shares one scrolling surface across every preview kind', () => {
    // The `max-width` bug only happened because the measure and the scroller were
    // the same node. Pinning that pairing makes the reason for the rule above
    // visible to the next person who is tempted to add a reading measure back.
    expect(previewSource).toContain('file-preview-scroll file-preview-markdown');
    expect(declarations('.file-preview-scroll')['overflow']).toBe('auto');
    expect(declarations('.file-preview-scroll')['flex']).toBe('1 1 auto');
  });

  it('stretches the media previews to the pane instead of shrink-wrapping them', () => {
    // HTML and PDF are `width: 100%` children of a flex column; images are
    // `max-width: 100%` so an oversized file scales down rather than scrolling.
    const frame = declarations('.file-preview-html,\n.file-preview-pdf');
    expect(frame['width']).toBe('100%');
    expect(frame['flex']).toBe('1 1 auto');
    expect(declarations('.file-preview-image')['max-width']).toBe('100%');
  });

  it('passes the pane height down so the scroller is the thing that scrolls', () => {
    // `.file-view-pane` is the `role="tabpanel"` pass-through; without `flex: 1`
    // and a zero `min-height` the page grows instead of the preview scrolling.
    const pane = declarations('.file-view-pane');
    expect(pane['flex']).toBe('1 1 auto');
    expect(pane['min-height']).toBe('0');
    expect(declarations('.file-viewer')['min-height']).toBe('0');
  });
});
