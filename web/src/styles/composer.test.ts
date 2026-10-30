// @vitest-environment node
/* Stylesheet contract for the floating composer dock.
 *
 * Component tests run with `css: false`, so computed style cannot be asserted
 * from jsdom; this suite reads the two stylesheets that own the dock and pins the
 * geometry that makes it float.
 *
 * The behaviour under test is the layout model itself:
 *
 *   - the composer paints *over* the transcript instead of taking a flex row, so
 *     the pane keeps its full height;
 *   - it is translucent when the reader is scrolled into history, which is what
 *     lets the conversation show through it, and opaque when they are in it;
 *   - the transcript reserves the dock's real height, so nothing hides behind it
 *     at rest.
 *
 * The wiring for the last two points (measuring the dock, flipping the attribute)
 * is covered by `ChatView.test.tsx` and `dockHeight.test.ts`.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const composerCss = readFileSync(new URL('./composer.css', import.meta.url), 'utf8');
const layoutCss = readFileSync(new URL('./layout.css', import.meta.url), 'utf8');

/** Declarations of one rule, matched by its full selector text. */
function declarations(css: string, selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  const found: Record<string, string> = {};
  for (const line of css.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
  }
  return found;
}

describe('composer dock: layout', () => {
  const wrap = declarations(layoutCss, '.composer-wrap');

  it('floats over the transcript instead of taking a band of its own', () => {
    // Absolute against the panel: a flex row here is what made the composer an
    // attached footer and cost the conversation the height of the box.
    expect(wrap['position']).toBe('absolute');
    expect(wrap['bottom']).toBe('0');
    expect(wrap['left']).toBe('0');
    expect(wrap['right']).toBe('0');
    // The old attached band drew a divider and a gradient wash across the pane.
    expect(wrap['border-top']).toBeUndefined();
    expect(wrap['background']).toBeUndefined();
  });

  it('lets the transcript scroll through the gutter beside the box', () => {
    // Without this the dock is a full-width invisible wall over the bottom of
    // the transcript: a wheel there would do nothing.
    expect(wrap['pointer-events']).toBe('none');
    // …and the controls take their events back, or nothing is clickable.
    expect(declarations(composerCss, '.composer')['pointer-events']).toBe('auto');
    expect(declarations(composerCss, '.compose-foot')['pointer-events']).toBe('auto');
  });

  it('reserves exactly its own measured height at the end of the transcript', () => {
    // Measured, not guessed: the box grows with the draft, the chip row and the
    // narrow-viewport wrap. `useHeightVariable` publishes the rounded value.
    expect(declarations(layoutCss, '.chat-inner')['padding-bottom']).toBe(
      'calc(var(--composer-h, 0px) + 18px)',
    );
    expect(declarations(layoutCss, '.chat-scroll')['scroll-padding-bottom']).toBe(
      'var(--composer-h, 24px)',
    );
    // The pill must clear the dock, not sit under it.
    expect(declarations(layoutCss, '.jump-latest')['bottom']).toBe(
      'calc(var(--composer-h, 0px) + 10px)',
    );
  });

  it('keeps the reserved band through the responsive tiers', () => {
    // A `padding` shorthand in a media query silently resets padding-bottom and
    // the last message ends up under the box at that width.
    const tiers = [...layoutCss.matchAll(/@media \(max-width: (\d+)px\) \{([\s\S]*?)\n\}/g)];
    expect(tiers.length).toBeGreaterThan(0);
    for (const [, width, body] of tiers) {
      const inner = /\.chat-inner \{([^}]*)\}/.exec(body ?? '');
      if (!inner) continue;
      const block = inner[1] ?? '';
      if (!/\bpadding\s*:/.test(block)) continue;
      // A shorthand is allowed only if the band is re-declared after it.
      expect(block, `.chat-inner at ${width}px must keep the dock band`).toMatch(
        /padding-bottom:\s*calc\(var\(--composer-h/,
      );
    }
  });
});

describe('composer dock: the box stays compact', () => {
  it('starts on one line and grows with the draft', () => {
    const input = declarations(composerCss, '.composer-input');
    // 47px was two-and-a-bit empty lines at rest; the box auto-grows from here
    // and Composer.tsx still caps it at 150px.
    expect(input['min-height']).toBe('26px');
    expect(input['max-height']).toBe('150px');
  });

  it('does not force the bottom row onto a second line', () => {
    // The mock breaks this row with a full-width spacer at 800px, which costs
    // ~35px of pure height on a floating dock. The wrap now happens only when the
    // row genuinely cannot fit.
    expect(composerCss).not.toMatch(/\.composer-spacer \{[^}]*flex: 1 0 100%/);
  });

  it('is tighter than the attached band it replaced', () => {
    expect(declarations(composerCss, '.compose-top')['padding']).toBe('9px 11px 0 12px');
    expect(declarations(composerCss, '.compose-bottom')['padding']).toBe('0 9px 7px 11px');
    expect(declarations(composerCss, '.compose-bottom')['min-height']).toBe('34px');
  });

  it('keeps one row on a phone', () => {
    // At 390px the pills plus Send did not fit, so the row wrapped and the box
    // stood 129px tall - 15% of an 844px screen. The controls may not wrap and
    // the pills give up width first.
    const narrow = /@media \(max-width: 800px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    expect(narrow, 'the 800px tier exists').toBeTruthy();
    expect(narrow![1]).toMatch(/\.composer-controls \{[^}]*flex-wrap: nowrap/);

    const phone = /@media \(max-width: 640px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    expect(phone, 'the phone tier exists').toBeTruthy();
    const body = phone![1] ?? '';
    // The label caps alone leave the pill too wide: `padding + chevron + label`.
    expect(body).toMatch(/\.model-switcher-button \{[^}]*max-width: 108px/);
    // A wrapped footnote floated two lines of shortcuts over the transcript.
    expect(body).toMatch(/\.composer-hint,\s*\n\s*\.compose-approvals \{\s*\n\s*display: none/);
  });
});

describe('composer dock: see-through while scrolled', () => {
  const composer = declarations(composerCss, '.composer');
  const scrolled = declarations(composerCss, ".chat-panel[data-translucent='true'] .composer");

  it('paints from a token, never a raw colour', () => {
    // Theme Studio repaints every surface; a literal hex here would stay dark in
    // a light theme.
    expect(composer['background']).toContain('var(--surface-input)');
    expect(composer['background']).toContain('--dock-opacity');
    expect(composer['background']).not.toMatch(/#[0-9a-f]{3,8}\b/i);
  });

  it('is fully opaque at rest', () => {
    // The fallback in the `color-mix` is 100%, so an unset knob is opaque.
    expect(composer['background']).toContain('var(--dock-opacity, 100)');
  });

  it('goes translucent and blurs what is behind it when scrolled away', () => {
    expect(scrolled['--dock-opacity']).toBe('42');
    expect(scrolled['backdrop-filter']).toContain('blur(var(--dock-blur))');
    // 42% of the input surface still leaves the draft text legible on top.
    expect(Number(scrolled['--dock-opacity'])).toBeGreaterThanOrEqual(35);
    expect(Number(scrolled['--dock-opacity'])).toBeLessThanOrEqual(60);
    // Low enough to read the conversation through it; a heavy blur turns the
    // text behind into mush, which is the opposite of the point.
    expect(Number(scrolled['--dock-blur']?.replace('px', ''))).toBeLessThanOrEqual(4);
  });

  it('comes back to a near-solid surface while the reader is typing in it', () => {
    const focused = declarations(
      composerCss,
      ".chat-panel[data-translucent='true'] .composer:focus-within",
    );
    expect(Number(focused['--dock-opacity'])).toBeGreaterThan(Number(scrolled['--dock-opacity']));
    expect(Number(focused['--dock-opacity'])).toBeGreaterThanOrEqual(85);
  });

  it('moves the footnote aside only while the box is not in use', () => {
    // `:not(:focus-within)` matters: hiding the status line while the reader is
    // typing hid the very hint the footnote exists to give them.
    expect(composerCss).toMatch(
      /\.chat-panel\[data-translucent='true'\] \.composer-wrap:not\(:focus-within\) \.compose-foot \{/,
    );
    expect(composerCss).not.toMatch(
      /\.chat-panel\[data-translucent='true'\] \.compose-foot \{/,
    );
  });

  it('never makes the box a stacking context at rest', () => {
    // A `backdrop-filter` on the base rule would trap the `@`-mention popover and
    // the dropup model menus, which escape the box by z-index today.
    expect(composer['backdrop-filter']).toBeUndefined();
    expect(composer['filter']).toBeUndefined();
  });

  it('honours reduced motion', () => {
    const reduced = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(composerCss);
    expect(reduced, 'the reduced-motion block exists').toBeTruthy();
    const body = reduced![1] ?? '';
    expect(body).toContain('.compose-foot');
    expect(body).toContain('.composer');
  });
});

describe('context window pill', () => {
  const pill = declarations(composerCss, '.context-pill');
  const fill = declarations(composerCss, '.context-pill-fill');

  it('lives in the bottom row without growing it', () => {
    // The row is 34px tall and `min-height: 34px` on a phone; a taller pill would
    // wrap Send onto a second line at 390px.
    expect(pill['height']).toBe('26px');
    expect(pill['flex']).toBe('0 0 auto');
    expect(pill['white-space']).toBe('nowrap');
  });

  it('paints from tokens so every theme repaints it', () => {
    expect(fill['background']).toContain('var(--accent)');
    expect(pill['color']).toContain('var(--text-body)');
    expect(composerCss).not.toMatch(/\.context-pill[^{]*(?<!var\()[^)]*\{[^}]*#[0-9a-f]{3,8}\b/i);
  });

  it('escalates the fill at the compaction boundary', () => {
    // `warn` starts where a long session compacts often; `hot` where the next
    // tool result may not fit at all.
    expect(composerCss).toMatch(/\.context-pill\[data-tone='warn'\] \.context-pill-fill \{\s*\n\s*background: var\(--warn\)/);
    expect(composerCss).toMatch(/\.context-pill\[data-tone='hot'\] \.context-pill-fill \{\s*\n\s*background: var\(--danger\)/);
  });

  it('drops the meter before the number on a phone', () => {
    const phone = [...composerCss.matchAll(/@media \(max-width: 640px\) \{([\s\S]*?)\n\}/g)]
      .map((match) => match[1] ?? '')
      .join('\n');
    expect(phone).toMatch(/\.context-pill-meter \{\s*\n\s*display: none/);
    expect(phone).not.toMatch(/\.context-pill-text \{\s*\n\s*display: none/);
  });
});