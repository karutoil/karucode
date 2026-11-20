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

  it('keeps the footer outside the scroller, and draws no band of its own', () => {
    // The band — hairline, inset, pinned height, note on the left, actions on
    // the right — belongs to `.overlay-footer`. The sheet passes its note and
    // buttons into it. A `.settings-footer` box here was a second band: it kept
    // its own flex and hairline while the shell already drew one, which is how
    // the footer could be pushed out of the clipped sheet.
    expect(css).not.toMatch(/\n\.settings-footer \{/);
    expect(css).toMatch(
      /\.overlay-footer-actions \.button \{\n {2}height: var\(--space-10\);/,
    );
    // The panel, not the sheet body, is the scroller that reaches the footer.
    const panel = declarations('.settings-panels > .tab-panel');
    expect(panel['overflow-y']).toBe('auto');
    expect(declarations('.settings-body')['overflow-y']).toBeUndefined();
  });

  it('documents the tree the sheet actually renders', () => {
    // The file header is the map a reader trusts before grepping, and it had
    // drifted: it still showed a `.settings-footer` band and a nav "kicker", both
    // deleted, and it omitted `.settings-body`, which is the real root. Each name
    // the header walks must exist in the markup, and each name it says is gone
    // must be gone. A rename that skips the header fails here.
    const header = css.slice(0, css.indexOf('*/'));
    const source = readFileSync(new URL('../components/SettingsSheet.tsx', import.meta.url), 'utf8');
    const nav = readFileSync(new URL('../components/SettingsNav.tsx', import.meta.url), 'utf8');

    for (const live of [
      '.settings-body',
      '.settings-form',
      '.settings-shell',
      '.settings-nav',
      '.settings-content',
      '.settings-pane-head',
      '.settings-panels',
    ]) {
      // The tree is an indented walk, so a leaf may be followed by a newline and
      // a named one always by a space; either way the name is in the map.
      expect(header, `${live} is in the layout model`).toMatch(
        new RegExp(`${live.replace('.', '\\.')}(?: |$)`, 'm'),
      );
      expect(source + nav, `${live} is rendered`).toContain(`"${live.slice(1)}`);
    }
    expect(header).toMatch(/no `?\.settings-footer`? band/);
    expect(css).not.toContain('.settings-nav-kicker');
    expect(nav).not.toContain('settings-nav-kicker');
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

  it('drops the rail notes to keep the strip one line', () => {
    expect(narrow).toMatch(/\.settings-nav-note \{[^}]*display: none/);
    // The "Settings" kicker is gone at every width, not just here: it named the
    // dialog from inside the dialog. Nothing in the rail head may re-add a
    // heading above the search field.
    expect(css).not.toContain('.settings-nav-kicker');
  });
});

describe('settings sheet: the phone chrome (OV-08, OV-09)', () => {
  /* A 375x667 screen spent ~280px of chrome before the first control: the rail's
   * head, the chip strip, the pane head, and a footer the note pushed to three
   * lines. The band is narrow OR short, because a 740x360 landscape phone is wider
   * than 640 and has less vertical room than any of it. */
  const phone = media('(max-width: 640px), (max-height: 500px)');

  it('drops the pane caption, the second sentence about a chosen section', () => {
    expect(phone).toMatch(/\.settings-pane-caption \{\n {4}display: none;/);
  });

  it('keeps the footer one row of thumb-sized buttons', () => {
    // The note was what wrapped: two 96px buttons and a sentence in a 339px row
    // is a footer three or four lines tall. `flex-basis: 100%` on both slots was
    // the old two-row recipe and it is gone.
    expect(phone).not.toMatch(/flex-basis: 100%/);
    expect(phone).toMatch(
      /\.overlay-tall \.settings-footer-note \{\n {4}display: none;/,
    );
    expect(phone).toMatch(
      /\.overlay-tall \.overlay-footer-actions \.button \{\n {4}min-height: 44px;/,
    );
  });

  it('lands a search jump below the pane header instead of flush against it', () => {
    // `useSettingsSheet` scrolls the matched section to `block: 'start'` inside
    // the panel; with no scroll padding the card's own border arrived glued to the
    // hairline under the pane header.
    expect(declarations('.settings-panels > .tab-panel')['scroll-padding-top']).toBe(
      'var(--space-7)',
    );
    expect(declarations('.settings-section')['scroll-margin-top']).toBe('var(--space-7)');
  });

  it('leaves the desktop sheet on the wider tiers it already had', () => {
    // The height half must not swallow the layout at desk widths: the two-pane
    // strip switch stays a width query.
    expect(css).toContain('@media (max-width: 780px) {');
    expect(phone).not.toMatch(/\.settings-shell \{\n {4}flex-direction: column/);
  });
});

describe('settings sheet: the Advanced disclosure', () => {
  it('is a bordered block whose summary hides the native marker', () => {
    const block = declarations('.settings-advanced');
    expect(block['border']).toBeDefined();

    const summary = declarations('.settings-advanced summary');
    expect(summary['list-style']).toBe('none');
    expect(summary['cursor']).toBe('pointer');
  });

  it('clips its corners without becoming a scroll container', () => {
    // `overflow: hidden` here was the "Advanced cuts off and will not scroll"
    // bug. `hidden` makes this card a scroll container, and a scroll container
    // does not pass its overflowing content's height up to the panel that
    // actually scrolls: the panel's `scrollHeight` equalled its `clientHeight`,
    // so the panel never became scrollable and the last fields of the fold were
    // painted past the bottom edge with no wheel able to reach them.
    //
    // `clip` rounds the corners the same way without creating a scroll box, so
    // the panel sees the real height. The clip margin leaves room for the
    // summary's inset focus ring, which would otherwise be cut off.
    const block = declarations('.settings-advanced');
    expect(block['overflow']).toBe('clip');
    expect(block['overflow']).not.toBe('hidden');
    expect(block['overflow-clip-margin']).toBe('1px');

    // Belt and braces: nothing in this sheet may reintroduce the trap on the
    // fold, wherever it is written.
    const fold = /\.settings-advanced(?![\w-])[^{]*\{([^}]*)\}/g;
    for (const match of css.matchAll(fold)) {
      expect(match[1]).not.toMatch(/overflow:\s*hidden/);
    }
  });

  it('lets a long value inside the fold wrap instead of widening it', () => {
    // The fold holds the values least likely to wrap (base URLs, key ids,
    // `mono` numbers). A flex item's automatic minimum width would let one of
    // them push the fold past the pane and take the second column with it.
    const body = declarations('.settings-advanced-body');
    expect(body['min-width']).toBe('0');
    expect(css).toMatch(/\.settings-advanced-body > \* \{[^}]*min-width: 0/);
  });

  it('is the same card as a section, with the body flattened inside it', () => {
    // The fold used to bleed out of the panel gutter by a negative margin. It is
    // now a card in the panel's own column (no margin, same radius as a section),
    // and the sections inside it are flattened rather than becoming cards inside
    // a card of the same fill.
    const block = declarations('.settings-advanced');
    expect(block['margin']).toBe('0');
    expect(block['border-radius']).toBe('var(--radius)');

    const body = declarations('.settings-advanced-body');
    expect(body['padding']).toBe('var(--space-6) var(--space-7)');
    expect(body['display']).toBe('flex');
    expect(body['flex-direction']).toBe('column');

    const inner = declarations('.settings-advanced-body > .settings-section');
    expect(inner['border']).toBe('0');
    expect(inner['padding']).toBe('0');
    expect(inner['background']).toBe('transparent');
  });

  it('still rules one folded section off the next', () => {
    // Two sections in one fold are transparent now, so the line between them has
    // to be drawn by the fold itself.
    expect(css).toMatch(
      /\.settings-advanced-body > \.settings-section \+ \.settings-section \{[^}]*border-top: 1px solid var\(--line\)/,
    );
  });

  it('rotates the chevron to point at the open body', () => {
    expect(css).toContain('.settings-advanced[open] > summary .settings-advanced-label svg');
  });
});

describe('settings sheet: a section, and the options inside it', () => {
  it('makes a section a card with a ruled title bar', () => {
    // The pane was one flat column: a section was a heading over a field grid on
    // the sheet's own fill, told apart from the next one by a single hairline.
    const section = declarations('.settings-section');
    expect(section['background']).toBe('var(--surface)');
    expect(section['border']).toBe('1px solid var(--line)');
    expect(section['border-radius']).toBe('var(--radius)');
    expect(section['padding']).toBe('var(--space-6) var(--space-7)');

    // …and the head is a title bar, so "what this group is" is separated from
    // the options under it.
    const head = declarations('.settings-section-head');
    expect(head['border-bottom']).toBe('1px solid var(--line)');
    expect(head['padding-bottom']).toBe('var(--space-4)');
  });

  it('boxes every option, whatever shape it takes', () => {
    // One rule for the three shapes an option can have (a labelled field, a
    // switch row, a name-plus-control row), scoped to the sheet body so a field
    // in a dialog is not boxed twice.
    expect(css).toMatch(
      /\.settings-body \.field,\n\.settings-body \.list-editor,\n\.settings-body \.setting-row,\n\.settings-section > \.toggle-field \{/,
    );
    expect(css).toMatch(
      /\.settings-body \.field,[\s\S]*?\{\n  padding: var\(--space-5\) var\(--space-6\);\n  border: 1px solid var\(--line\);\n  border-radius: var\(--radius-sm\);/,
    );
  });

  it('spaces the grid without drawing a second frame around it', () => {
    // The tiles carry the edges; the grid only spaces them, which is what keeps
    // an odd last row from leaving a lit cell.
    const grid = declarations('.field-grid');
    expect(grid['gap']).toBe('var(--space-4)');
    expect(grid['border']).toBeUndefined();
    expect(grid['background']).toBeUndefined();
  });

  it('turns a section subheading into a kicker, not one more field label', () => {
    // "Set a password" and "Which events" sat at the same size and weight as a
    // `.field-label`, so a group heading read as another option.
    const h4 = declarations('.settings-subhead h4');
    expect(h4['text-transform']).toBe('uppercase');
    expect(h4['font-size']).toBe('var(--fs-micro)');
    expect(h4['color']).toBe('var(--text-kicker)');
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
