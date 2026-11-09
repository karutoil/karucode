// @vitest-environment node
/* Stylesheet contract for the tabbed settings sheet.
 *
 * Component tests run with `css: false`, so computed style cannot be asserted
 * from jsdom; this suite reads `settings.css` directly and pins the layout model
 * that makes the sheet tabbed rather than one long scroll.
 *
 * The model is:
 *
 *   - the scroll lives on the *panel*, not on the sheet body, so the tab strip
 *     and the footer stay put while the settings between them scroll;
 *   - a kept-alive panel is inert when hidden (`display: none`), because the
 *     sheet mounts every tab at once to keep typed values alive;
 *   - the Advanced disclosure is collapsed by default and its body is a plain
 *     stack of sections, not a padded box.
 *
 * Each of these is cheap to break by accident — a `padding` moved back to the
 * body, a `hidden` override for a flex layout, a disclosure that grows its own
 * padding — and each one silently undoes the redesign, which is why they are
 * pinned here rather than left to a screenshot.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./settings.css', import.meta.url), 'utf8');
const settingsCss = css;

/** Declarations of one rule, matched by its full selector text. */
function declarations(selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined in settings.css`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const found: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
  }
  return found;
}

describe('settings sheet: the panel owns the scroll', () => {
  it('moves the overflow off the sheet body and onto the panel', () => {
    const body = declarations('.settings-body');
    // The body used to scroll, which carried the tab strip and the footer away
    // with the content. Its only job now is to be a column that can shrink.
    expect(body['overflow-y']).toBeUndefined();
    expect(body['display']).toBe('flex');
    expect(body['flex-direction']).toBe('column');
    expect(body['min-height']).toBe('0');

    const panel = declarations('.settings-panels > .tab-panel');
    expect(panel['overflow-y']).toBe('auto');
    expect(panel['overscroll-behavior']).toBe('contain');
    expect(panel['min-height']).toBe('0');
  });

  it('scrolls the tab strip instead of squashing its labels', () => {
    const strip = declarations('.settings-tabs');
    expect(strip['overflow-x']).toBe('auto');
    // A vertical scrollbar inside the strip would eat the active underline.
    expect(strip['overflow-y']).toBe('hidden');

    // Grow to share the row, never shrink below the label: the inspector learned
    // this the hard way (a shrunken tab overlaps its neighbour instead of
    // overflowing), and the same rule applies here.
    const tab = declarations('.settings-tab');
    expect(tab['flex']).toContain('0');
    expect(tab['flex']).toContain('max-content');
  });

  it('shows the phone strip it can scroll, with a faded cut edge', () => {
    // Five tabs measure 369px against a 319px strip at a 320px viewport, so
    // "Plugins & MCP" sits past the edge. Scrolling is the deliberate contract
    // above; the fade is what tells the reader there is more, because a hard cut
    // reads as a truncated row. It is a mask (alpha only), so it is
    // theme-independent and invisible where the strip already fits.
    const start = settingsCss.indexOf('@media (max-width: 600px) {');
    const body = settingsCss.slice(start, settingsCss.indexOf('@media (prefers-reduced-motion', start));
    expect(body).toMatch(/\.settings-tabs \{[^}]*mask-image: linear-gradient\(to right, black/);
    expect(body).toMatch(/\.settings-tabs \{[^}]*scroll-padding-inline: var\(--space-5\)/);
    // A mask that fades must not also stop the scroll.
    expect(settingsCss).not.toMatch(/\.settings-tabs \{[^}]*overflow-x: hidden/);
  });

  it('makes a kept-alive panel inert while it is hidden', () => {
    // Panels stay mounted so a tab switch cannot discard what was typed. `hidden`
    // only hides them if nothing overrides the attribute's own display, so the
    // rule that does it is explicit rather than inherited from the browser.
    const hidden = declarations('.settings-panels > .tab-panel[hidden]');
    expect(hidden['display']).toBe('none');
  });

  it('keeps the footer out of the scroll', () => {
    const footer = declarations('.settings-footer');
    // `sticky` is what the footer needed when the body scrolled; a footer outside
    // the scroller does not need it and must not claim to be inside one.
    expect(footer['position']).toBeUndefined();
    expect(footer['flex']).toContain('0');
  });
});

describe('settings sheet: the Advanced disclosure', () => {
  it('is a bordered block whose summary hides the native marker', () => {
    const block = declarations('.settings-advanced');
    expect(block['border']).toBeDefined();
    expect(block['overflow']).toBe('hidden');

    const summary = declarations('.settings-advanced summary');
    expect(summary['list-style']).toBe('none');
    expect(summary['cursor']).toBe('pointer');
  });

  it('insets the body to the summary gutter, and leaves the sections unpadded', () => {
    // The sections inside bring their own vertical rhythm but no horizontal
    // padding, so without an inset here their labels sit flush against the
    // disclosure's border — visible in a browser, invisible to jsdom, hence the
    // measured check in scripts/verify_settings_tabs.py as well.
    const body = declarations('.settings-advanced-body');
    // The block bleeds out by exactly this inset, so controls inside the fold
    // share the panel's gutter with the controls outside it (DESIGN-AUDIT M3).
    expect(body['padding']).toMatch(/^0 var\(--space-6\) /);
    expect(declarations('.settings-advanced')['margin']).toMatch(
      /calc\(-1 \* var\(--space-6\)\)/,
    );
    expect(body['display']).toBe('flex');
    expect(body['flex-direction']).toBe('column');
  });

  it('draws no divider under the last section inside the fold', () => {
    // The disclosure is the end of the panel, so the section divider that keeps
    // unrelated sections apart has nothing to separate there.
    expect(css).toContain('.settings-advanced-body .settings-section:last-of-type');
  });

  it('rotates the chevron to point at the open body', () => {
    expect(css).toContain('.settings-advanced[open] > summary .settings-advanced-label svg');
  });
});
