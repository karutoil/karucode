// @vitest-environment node
/* Stylesheet contract for the two-pane settings sheet.
 *
 * Component tests run with `css: false`, so computed style cannot be asserted
 * from jsdom; this suite reads `settings.css` directly and pins the layout model
 * that makes the sheet two panes rather than one scrolling column:
 *
 *   - the shell is a row: a fixed-width rail and a content column beside it;
 *   - the scroll lives on the *panel*, not on the sheet body or the content
 *     column, so the rail, the pane header and the footer stay put while a long
 *     section scrolls under them;
 *   - the pane header is outside the scroller for the same reason: it names the
 *     section, and a title that scrolls away stops doing that;
 *   - a kept-alive panel is inert when hidden (`display: none`), because the
 *     sheet mounts every panel at once to keep typed values alive;
 *   - below 780px the rail turns into a scrolling strip above the content, and
 *     the strip fades at its cut edge instead of pretending it ended there.
 *
 * Each of these is cheap to break by accident — a `padding` moved back to the
 * body, a `hidden` override for a flex layout, a stacking rule that never
 * applied — and each one silently undoes the redesign, which is why they are
 * pinned here rather than left to a screenshot.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('./settings.css', import.meta.url), 'utf8');

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

/** The body of a media query, by its exact header. */
function media(header: string): string {
  const start = css.indexOf(`@media ${header} {`);
  expect(start, `@media ${header} is defined in settings.css`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}\n', start));
}

describe('settings sheet: two panes, and the panel owns the scroll', () => {
  it('lays the sheet out as a rail beside a content column', () => {
    const shell = declarations('.settings-shell');
    expect(shell['display']).toBe('flex');
    expect(shell['min-height']).toBe('0');

    const nav = declarations('.settings-nav');
    // A fixed rail: a percentage would let the content column squeeze it until
    // the labels ellipsised instead of the rail scrolling.
    expect(nav['flex']).toContain('0 0');
    expect(nav['border-right']).toBeDefined();
    // The rail sits on the app's sidebar rung, one step under the sheet's own
    // surface — the same relationship the sessions rail has to the canvas.
    expect(nav['background']).toBe('var(--sidebar)');

    const content = declarations('.settings-content');
    expect(content['display']).toBe('flex');
    expect(content['flex']).toContain('1 1 auto');
    // Without both, a long URL or a wide table pushes the column past the window.
    expect(content['min-width']).toBe('0');
    expect(content['min-height']).toBe('0');
  });

  it('moves the overflow off the sheet body and onto the panel', () => {
    const body = declarations('.settings-body');
    // The body used to scroll, which carried the rail and the footer away with
    // the content. Its only job now is to be a column that can shrink.
    expect(body['overflow-y']).toBeUndefined();
    expect(body['display']).toBe('flex');
    expect(body['flex-direction']).toBe('column');
    expect(body['min-height']).toBe('0');

    const panel = declarations('.settings-panels > .tab-panel');
    expect(panel['overflow-y']).toBe('auto');
    expect(panel['overscroll-behavior']).toBe('contain');
    expect(panel['min-height']).toBe('0');
  });

  it('keeps the pane header out of the scroller', () => {
    // The header names the section on screen. A header inside the panel scrolls
    // away on exactly the long sections where the reader is most lost.
    const head = declarations('.settings-pane-head');
    expect(head['flex']).toContain('0 0 auto');
    expect(css.indexOf('\n.settings-pane-head {')).toBeLessThan(
      css.indexOf('\n.settings-panels {'),
    );
    expect(head['position']).toBeUndefined();
  });

  it('scrolls the section list instead of squashing its labels', () => {
    const list = declarations('.settings-nav-list');
    expect(list['overflow-y']).toBe('auto');
    expect(list['overscroll-behavior']).toBe('contain');

    const item = declarations('.settings-nav-item');
    // A rail row grows to its content and never narrows below it: a squeezed
    // label is an unreadable one.
    expect(item['min-height']).toBeDefined();
    expect(item['width']).toBe('100%');
  });

  it('paints the selected row with the shared accent tint and a focus ring', () => {
    const active = declarations('.settings-nav-item.is-active');
    expect(active['background']).toBe('var(--accent-bg)');
    expect(active['border-color']).toBe('var(--accent-line)');
    // §5.6: every interactive element has a visible ring.
    const focus = declarations('.settings-nav-item:focus-visible');
    expect(focus['outline']).toContain('var(--focus-ring)');
  });

  it('makes a kept-alive panel inert while it is hidden', () => {
    // Panels stay mounted so a section switch cannot discard what was typed.
    // `hidden` only hides them if nothing overrides the attribute's own display,
    // so the rule that does it is explicit rather than inherited from the
    // browser.
    const hidden = declarations('.settings-panels > .tab-panel[hidden]');
    expect(hidden['display']).toBe('none');
  });

  it('keeps the footer out of the scroll', () => {
    const footer = declarations('.settings-footer');
    // `sticky` is what the footer needed when the body scrolled; a footer
    // outside the scroller does not need it and must not claim to be inside one.
    expect(footer['position']).toBeUndefined();
    expect(footer['flex']).toContain('0');
  });

  it('gives the pane header and footer one control height with the fields', () => {
    // The audit's M3: controls in one panel measured 37/32/30/16px tall. Every
    // button in the sheet is the 32px an `.input` is.
    const buttons = declarations('.settings-body .button');
    expect(buttons['height']).toBe('var(--space-10)');
    const close = declarations('.settings-close');
    expect(close['width']).toBeDefined();
    expect(close['height']).toBeDefined();
  });
});

describe('settings sheet: the phone layout', () => {
  const narrow = media('(max-width: 780px)');

  it('stacks the two panes rather than shrinking them', () => {
    // Two panes at 390px is two unusable columns; the rail becomes the header
    // band of the content.
    expect(narrow).toMatch(/\.settings-shell \{[^}]*flex-direction: column/);
    expect(narrow).toMatch(/\.settings-nav \{[^}]*border-bottom: 1px solid var\(--line\)/);
  });

  it('turns the rail into a scrolling strip that fades at its cut edge', () => {
    // Seven labels do not fit 320-430px. The strip scrolls rather than squashing
    // them (the contract above), and a hard cut reads as a truncated row — so
    // the edge is faded. The gradient is a mask, not a paint: only its alpha is
    // used, so it is theme-independent and invisible where the strip fits.
    expect(narrow).toMatch(/\.settings-nav-list \{[^}]*overflow-x: auto/);
    expect(narrow).toMatch(/\.settings-nav-list \{[^}]*overflow-y: hidden/);
    expect(narrow).toMatch(/mask-image: linear-gradient\(to right, black/);
    expect(narrow).toMatch(/scroll-padding-inline: var\(--space-5\)/);
    // A mask that fades must not also stop the scroll.
    expect(css).not.toMatch(/\.settings-nav-list \{[^}]*overflow-x: hidden/);
  });

  it('drops the rail notes and the kicker to keep the strip one line', () => {
    expect(narrow).toMatch(/\.settings-nav-note \{[^}]*display: none/);
    expect(narrow).toMatch(/\.settings-nav-kicker \{[^}]*display: none/);
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

describe('settings sheet: a section a search landed on', () => {
  it('flashes the section, and stops flashing when motion is reduced', () => {
    expect(declarations('.settings-section.is-found')['animation']).toContain('settings-found');
    const reduced = media('(prefers-reduced-motion: reduce)');
    expect(reduced).toMatch(/\.settings-section\.is-found \{[^}]*animation: none/);
  });

  it('draws the focus ring inside the panel gutter, not outside the sheet', () => {
    // The panel runs flush to the sheet's right edge inside an `overflow:
    // hidden` shell, so a ring with a positive offset is clipped there.
    expect(css).toMatch(
      /\.settings-section\.is-found:focus-visible \{[^}]*outline-offset: calc\(-1 \* var\(--focus-ring-offset\)\)/,
    );
  });
});
