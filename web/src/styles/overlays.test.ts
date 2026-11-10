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

  it('keeps palette overlays compact on narrow viewports', () => {
    expect(css).toContain('.dialog.dialog-palette {');
    expect(css).toMatch(/\.dialog\.dialog-palette\s*\{[\s\S]*?width:\s*calc\(100vw - 28px\)/);
    expect(css).toMatch(/\.dialog\.dialog-palette\s*\{[\s\S]*?border-radius:\s*var\(--radius\)/);
  });
});

describe('file viewer chrome', () => {
  /* The file panel used to stack three grey rows above the document - a path
   * header (`.file-view-head`), a metadata toolbar and a Preview/Source band -
   * over a tab whose own label and the session breadcrumb already named the file.
   * One bar is the whole contract now, so it is pinned here: the rules that drew
   * the other two must not come back, and the one bar must stay transparent
   * rather than painting another `--surface-menu` band across the page. */
  it('draws no path header and no second bar for the view tabs', () => {
    expect(css).not.toContain('.file-view-head');
    expect(css).not.toContain('.file-view-path');
    expect(css).not.toContain('.file-tabbar');
  });

  it('keeps the one bar a rule without a fill of its own', () => {
    const bar = declarations('.file-toolbar');
    expect(bar['background']).toBeUndefined();
    expect(bar['border-bottom']).toContain('var(--line)');
    // The actions own the right-hand end, so the toggle stays on the left.
    expect(declarations('.file-toolbar-actions')['margin-left']).toBe('auto');
  });

  it('paints the bar and status bar text in readable ink, not decoration', () => {
    // §4: `--faint` is decoration only (3:1 floor); this bar's words - the
    // secondary actions, the path, the language, the line count - are read.
    for (const selector of ['.file-action', '.file-statusbar', '.file-status-path']) {
      const rule = declarations(selector);
      expect(rule['color'] ?? '', `${selector} does not use --faint`).not.toContain('var(--faint)');
    }
    expect(declarations('.file-statusbar')['color']).toBe('var(--muted)');
  });
});

describe('rewind dialog scroll bounds', () => {
  /* `.dialog` is `max-height: calc(100dvh - 48px)` (the whole `100dvh` on a
   * phone) with `overflow: hidden`, and `.rewind-body` carries a quote, a count,
   * three mode cards, an optional note field and a file list. Nothing scrolled:
   * the body grew past the dialog, the footer was clipped, and a rewind could not
   * be confirmed on a short screen. The body is the scroller and the actions stay
   * out of it, which is the shape `.dialog-model-edit` already uses. */
  it('bounds the rewind body so the footer is always reachable', () => {
    const body = declarations('.rewind-body');
    expect(body['flex']).toBe('1 1 auto');
    // Without this the flex item refuses to shrink below its content height and
    // still overflows the dialog.
    expect(body['min-height']).toBe('0');
    expect(body['overflow-y']).toBe('auto');
    expect(body['overscroll-behavior']).toBe('contain');
  });

  it('keeps the rewind actions out of the scrolling body', () => {
    expect(declarations('.dialog-rewind > .dialog-actions')['flex']).toBe('0 0 auto');
    const source = readFileSync(new URL('../components/RewindDialog.tsx', import.meta.url), 'utf8');
    // The fieldsets and the copy live inside `.rewind-body`; the footer is a
    // sibling of it, so a scrolling body can never carry the buttons away.
    expect(source).toMatch(/<\/div>\s*<footer class="dialog-actions">/);
  });

  it('still caps the file list inside the scroller', () => {
    // A nested cap keeps a 200-file revert from becoming one long strip even on a
    // tall screen (the outer scroller only matters on short ones).
    expect(declarations('.rewind-files')['max-height']).toBe('220px');
    expect(declarations('.rewind-files')['overflow-y']).toBe('auto');
  });
});
