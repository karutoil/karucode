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
 *   - the dock stays opaque while reading history or typing, so neither its text
 *     nor the conversation has to be read through another layer;
 *   - the transcript reserves the dock's real height, so nothing hides behind it
 *     at rest or at the phone breakpoints.
 *
 * The measured-height wiring is covered by `ChatView.test.tsx` and
 * `dockHeight.test.ts`; these assertions pin the corresponding visual contract.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const composerCss = readFileSync(new URL('./composer.css', import.meta.url), 'utf8');
const layoutCss = readFileSync(new URL('./layout.css', import.meta.url), 'utf8');
const tokensCss = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');

/** Declarations of one top-level rule, matched by its full selector text. */
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

/** Declarations of one rule inside a media block body (selector lists included). */
function declarationsIn(body: string, selector: string): Record<string, string> {
  const found: Record<string, string> = {};
  for (const rule of body.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = (rule[1] ?? '')
      // Strip the comments first: they carry commas (`16px, 22vh`) and would
      // otherwise split a selector in half.
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .split(',')
      .map((part) => part.trim());
    if (!selectors.includes(selector)) continue;
    // Every matching rule, in source order: the same selector can be extended
    // further down one block (the coarse tier adds a `min-height` to a rule that
    // is already there), and the cascade takes the later declaration.
    for (const line of (rule[2] ?? '').split('\n')) {
      const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
      if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
    }
  }
  expect(Object.keys(found).length, `${selector} is defined in the block`).toBeGreaterThan(0);
  return found;
}

/** The body of the first `@media <query> { … }` block, indentation-aware. */
function mediaBlock(css: string, query: string): string {
  const header = `@media ${query} {`;
  const start = css.indexOf(header);
  expect(start, `${query} block exists`).toBeGreaterThan(-1);
  const end = css.indexOf('\n}', start);
  // Start *inside* the block: the header's own `{` would otherwise pair with the
  // first rule's `}` and shift every selector by one.
  return css.slice(start + header.length, end);
}

/**
 * Evaluate one length declaration to CSS pixels.
 *
 * jsdom has no layout engine, so none of this geometry can be measured by
 * rendering: every dock assertion in the component suite would pass with the
 * popover parked under the keyboard. The sheet ships the arithmetic, so the
 * arithmetic is what gets computed - `var()` resolves against the numbers the
 * audit measured (`--kb-inset: 320px` for an iOS keyboard, `--composer-h: 96px`
 * for the dock), `dvh` against the device height, and `min()`/`max()`/`calc()`
 * are evaluated with `Function`. A value that is not pure arithmetic throws,
 * which is the guard against a selector silently losing its formula.
 */
function pixels(
  value: string,
  context: { height: number; vars?: Record<string, string> },
): number {
  let resolved = value;
  for (let pass = 0; pass < 8; pass += 1) {
    const next = resolved.replace(
      /var\(\s*--([a-z0-9-]+)\s*(?:,\s*([^()]*?))?\s*\)/g,
      (_whole, name: string, fallback: string | undefined) => {
        const mapped = context.vars?.[name] ?? fallback;
        if (mapped === undefined) throw new Error(`unresolved var(--${name}) in ${value}`);
        return mapped;
      },
    );
    if (next === resolved) break;
    resolved = next;
  }
  const arithmetic = resolved
    .replace(/\bcalc\(/g, '(')
    .replace(/\bmin\(/g, 'Math.min(')
    .replace(/\bmax\(/g, 'Math.max(')
    .replace(/(\d+(?:\.\d+)?)dvh\b/g, (_all, n: string) =>
      String((Number(n) / 100) * context.height),
    )
    .replace(/(\d+(?:\.\d+)?)vh\b/g, (_all, n: string) =>
      String((Number(n) / 100) * context.height),
    )
    .replace(/(\d+(?:\.\d+)?)px\b/g, '$1');
  expect(arithmetic.replace(/Math\.(?:min|max)/g, ''), `${value} is arithmetic`).toMatch(
    /^[\d\s()+\-*/.,]+$/,
  );
  const result = Number(new Function(`"use strict"; return (${arithmetic});`)());
  expect(Number.isFinite(result), `${value} resolves to a number`).toBe(true);
  return result;
}

/** The measured numbers the audit's reference phone gives the dock. */
const IOS_KEYBOARD = 320;
const DOCK = 96;
const dockVars = (inset: number, dock = DOCK): Record<string, string> => ({
  'space-2': '4px',
  'space-3': '6px',
  'space-4': '8px',
  'space-6': '12px',
  'space-9': '24px',
  'composer-h': `${dock}px`,
  'kb-inset': `${inset}px`,
});


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
    // The pill must clear the dock, not sit under it. `layout.css` adds the
    // home-indicator band the dock itself sits on (CO-07), so both terms are
    // part of the clearance and the assertion allows either.
    expect(declarations(layoutCss, '.jump-latest')['bottom']).toMatch(
      /^calc\(var\(--composer-h, 0px\) \+ 10px(?: \+ env\(safe-area-inset-bottom, 0px\))?\)$/,
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
    // A modest rest height avoids clipping the 16px phone line; scrollHeight
    // still expands it after the first line, with the same real cap.
    expect(input['min-height']).toBe('44px');
    // The cap is a custom property so the phone tier can shrink it without a
    // second magic number in the component; the default is the mock's 170px.
    expect(input['--compose-input-max']).toBe('170px');
    expect(input['max-height']).toBe('var(--compose-input-max)');
    // Tokens, not literals: 12/12/2 on the spacing scale and the body step on
    // the type scale (§3.2, §3.3).
    expect(input['padding']).toBe('var(--space-6) var(--space-6) var(--space-2)');
    expect(input['font-size']).toBe('var(--fs-body)');
  });

  it('does not force the bottom row onto a second line', () => {
    // The mock breaks this row with a full-width spacer at 800px, which costs
    // ~35px of pure height on a floating dock. The wrap now happens only when the
    // row genuinely cannot fit.
    expect(composerCss).not.toMatch(/\.composer-spacer \{[^}]*flex: 1 0 100%/);
  });

  it('matches the mock bar, attach and send geometry', () => {
    expect(declarations(composerCss, '.compose-bottom')['padding']).toBe(
      'var(--space-2) var(--space-4) var(--space-4) var(--space-4)',
    );
    expect(declarations(composerCss, '.compose-bottom')['min-height']).toBe('39px');
    expect(declarations(composerCss, '.compose-plus')['width']).toBe('24px');
    expect(declarations(composerCss, '.button-send')['width']).toBe('28px');
    expect(declarations(composerCss, '.button-send')['height']).toBe('28px');
    // The dock is the card radius, held locally so a rounding override cannot
    // reshape it (§3.4).
    expect(declarations(composerCss, '.composer')['--radius-composer']).toBe('var(--radius)');
  });

  it('keeps idle controls quiet while preserving phone typing and Settings access', () => {
    const narrow = /@media \(max-width: 800px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    expect(narrow, 'the 800px tier exists').toBeTruthy();
    expect(narrow![1]).toMatch(/\.composer-controls \{[^}]*flex-wrap: nowrap/);
    // Two pills still fit here, so their caps stay - wide enough for the real
    // names, since the 24px floor is what prevents an overlap, not the cap.
    expect(narrow![1]).toContain('max-width: min(220px, 100%)');
    expect(narrow![1]).toContain('max-width: min(190px, 100%)');

    const phone = /@media \(max-width: 640px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    expect(phone, 'the phone tier exists').toBeTruthy();
    const body = phone![1] ?? '';
    // The phone has one pill, not two: the row gives the model the width the
    // reasoning pill used to hold, and the effort lives in the same sheet.
    expect(body).toMatch(/\.compose-bottom \.reasoning-selector \{\s*\n\s*display: none/);
    expect(body).toMatch(/\.composer-session-pins,[\s\S]*?flex: 1 1 auto/);
    // One line, always: the pill shrinks, Send never leaves the row.
    expect(body).toMatch(/\.compose-bottom \{[^}]*flex-wrap: nowrap/);
    expect(body).toMatch(/\.compose-bottom \.model-switcher-button \{[\s\S]*?max-width: none/);
    // The old caps are what turned both labels into "Claud…" and "Def…".
    expect(body).not.toContain('max-width: min(108px, 100%)');
    expect(body).not.toContain('max-width: min(92px, 100%)');
    // Hints can yield, but the status-to-Settings entry may not disappear.
    expect(body).toMatch(/\.composer-hint \{\s*\n\s*display: none/);
    expect(body).not.toMatch(/\.compose-approvals\s*\{[^}]*display:\s*none/);
    const input = /\.composer-input \{([^}]*)\}/.exec(body)?.[1] ?? '';
    expect(input).toContain('font-size: var(--fs-title)');
    expect(input).toContain('--compose-input-max: min(112px, 22vh)');
    expect(input).not.toContain('min-height:');
  });

  it('uses explicit two-row running areas only at the phone tier', () => {
    // Outside the media query these wrappers are transparent, preserving the
    // existing one-line desktop/idle composition and menu positioning.
    expect(composerCss).toMatch(
      /\.compose-configuration,\s*\n\.compose-run-actions \{\s*\n\s*display: contents/,
    );

    const phone = /@media \(max-width: 640px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    const body = phone?.[1] ?? '';
    expect(body).toMatch(/\.compose-bottom\.is-running \{[^}]*display: grid/);
    expect(body).toMatch(/grid-template-rows: auto auto/);
    expect(body).toMatch(/\.is-running > \.button-stop \{[^}]*grid-row: 1/);
    expect(body).toMatch(/\.is-running > \.button-stop \{[^}]*grid-column: 2/);
    expect(body).toMatch(/\.is-running \.compose-run-actions \{[^}]*grid-row: 2/);
    expect(body).toMatch(/\.is-running \.compose-run-actions \{[^}]*grid-column: 1 \/ -1/);
    expect(body).toMatch(/\.is-running \.compose-run-actions \{[^}]*flex-wrap: nowrap/);
    // The layout may add the real second row, but may never hide waiting work.
    expect(composerCss).not.toMatch(/\.queued-(?:panel|list|row)[^{]*\{[^}]*display:\s*none/);
  });

  it('never shrinks composer actions below a 24px target', () => {
    const selectors = [
      '.compose-plus',
      '.queue-mode-option',
      '.queued-recall',
      '.attachment-chip .attachment-chip-remove',
      '.context-pill',
      '.compose-approvals',
    ];
    for (const selector of selectors) {
      const rule = declarations(composerCss, selector);
      const minimum = Number.parseFloat(rule['min-height'] ?? rule['height'] ?? '0');
      expect(minimum, `${selector} target height`).toBeGreaterThanOrEqual(24);
    }
    expect(declarations(composerCss, '.compose-plus')['width']).toBe('24px');
    expect(declarations(composerCss, '.button-send')['min-width']).toBe('28px');
    expect(declarations(composerCss, '.compose-bottom .button-stop')['min-width']).toBe('28px');
    expect(declarations(composerCss, '.compose-bottom .button-queue')['min-width']).toBe('28px');
    expect(declarations(composerCss, '.compose-bottom .model-switcher-button')).toMatchObject({
      'min-width': '24px',
      'min-height': '24px',
    });
  });
});

describe('composer dock: opaque and legible in every reading state', () => {
  const composer = declarations(composerCss, '.composer');

  it('paints the semantic surface, never a raw colour or alpha mix', () => {
    expect(composer['background']).toBe('var(--surface)');
    expect(composer['background']).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(composer['background']).not.toContain('transparent');
    expect(composer['background']).not.toContain('color-mix');
  });

  it('never brings back the 42% history surface or backdrop/filter tricks', () => {
    expect(composerCss).not.toMatch(/--dock-opacity|--dock-blur/);
    expect(composerCss).not.toMatch(/data-translucent/);
    expect(composerCss).not.toMatch(/backdrop-filter\s*:/);
    expect(composerCss).not.toMatch(/^\s*filter\s*:/m);
    // Avoid stacking-context filters around the mention/model popovers too.
    expect(composer['backdrop-filter']).toBeUndefined();
    expect(composer['filter']).toBeUndefined();
  });

  it('keeps draft text, live status and approvals visible while reading history', () => {
    const input = declarations(composerCss, '.composer-input');
    expect(input['color']).toBe('var(--text)');
    expect(composerCss).not.toMatch(/\.compose-foot[^{]*\{[^}]*(?:opacity:\s*0|visibility:\s*hidden)/);
    expect(composerCss).not.toMatch(/\.compose-approvals[^{]*\{[^}]*display:\s*none/);
  });

  it('retains line, focus ring and shadows without fading to focus', () => {
    const focused = declarations(composerCss, '.composer:focus-within');
    expect(composer['border']).toBe('1px solid var(--line-strong)');
    const composerBlock = /\.composer \{([^}]*)\}/.exec(composerCss)?.[1] ?? '';
    expect(composerBlock).toContain('var(--shadow-composer)');
    expect(composerBlock).toContain('var(--shadow-dock)');
    expect(focused['border-color']).toBe('var(--focus-input)');
    expect(focused['outline']).toBe('var(--focus-ring-width) solid var(--focus-ring)');
    expect(focused['--dock-opacity']).toBeUndefined();
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

  it('is a compact, explicit Usage action in the dock', () => {
    expect(pill['height']).toBe('26px');
    expect(pill['min-height']).toBe('26px');
    expect(pill['flex']).toBe('0 0 auto');
    expect(pill['white-space']).toBe('nowrap');
    expect(pill['cursor']).toBe('pointer');
  });

  it('paints from tokens so every theme repaints it', () => {
    expect(fill['background']).toContain('var(--accent)');
    // `--faint` is decoration only (§4); the pill's number is text a reader uses.
    expect(pill['color']).toContain('var(--muted)');
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
    expect(phone).not.toMatch(/\.context-pill-label \{\s*\n\s*display: none/);
  });

  it('gives the spacer up before the buttons on a phone', () => {
    // Measured at 390px: with the pill in the row and the spacer still taking
    // flex, `.compose-bottom` stood 66px tall (two lines) instead of 36px. The
    // spacer must collapse first, and Send/Stop must not shrink below their
    // labels - a squeezed label wraps *inside* the button, which is just as bad.
    const phone = [...composerCss.matchAll(/@media \(max-width: 640px\) \{([\s\S]*?)\n\}/g)]
      .map((match) => match[1] ?? '')
      .join('\n');
    expect(phone).toMatch(/\.compose-bottom \.composer-spacer \{[^}]*flex: 0 1 0/);
    expect(phone).toMatch(/\.compose-bottom \.button-send,\s*\n\s*\.compose-bottom \.button-stop \{[^}]*flex: 0 0 auto/);
  });
});

describe('composer pickers: the menu appears where the thumb is', () => {
  it('anchors the composer menus above the dock instead of to a viewport 100%', () => {
    // The bug: below 800px the menu became `position: fixed` (to escape the
    // clipped pill row) while `bottom: calc(100% + 6px)` stayed. `100%` of a
    // fixed element is the *viewport*, so on an 844px phone the list sat at
    // top: -268px - open, announced by `aria-expanded`, and invisible. Anchoring
    // to the measured dock (`--composer-h`) is the one value that is right at
    // every draft height and every width.
    const narrow = /@media \(max-width: 800px\) \{([\s\S]*?)\n\}/.exec(composerCss);
    const body = narrow?.[1] ?? '';
    expect(body).toMatch(/\.compose-bottom \.model-switcher-menu\.dropup,[\s\S]*?position: fixed/);
    expect(body).not.toContain('bottom: calc(100% + 6px)');
    // The reasoning menu is the right-hand control: it opens under it, and both
    // stay inside the screen width.
    expect(body).toMatch(
      /\.compose-bottom \.reasoning-selector \.model-switcher-menu\.dropup \{[\s\S]*?right: var\(--space-6\)/,
    );
    expect(body).toContain('width: min(300px, calc(100vw - var(--space-9)))');
  });

  it('keeps the phone sheet inside the screen and scrollable', () => {
    // The safe area and the viewport edges are what a sheet has to respect, not
    // a 64px guess above the bottom (which sat under the dock it belongs to).
    expect(layoutCss).toMatch(
      /\.model-switcher-menu \{\s*\n\s*position: fixed;[\s\S]*?right: var\(--space-3\);[\s\S]*?left: var\(--space-3\);/,
    );
    expect(layoutCss).toMatch(/\.model-switcher-menu \{[\s\S]*?max-height: min\(70dvh, 480px\)/);
    expect(layoutCss).toMatch(/\.model-switcher-menu \{[\s\S]*?overscroll-behavior: contain/);
    expect(layoutCss).not.toContain('left: calc(var(--rail-w) + 8px)');
  });
});
describe('composer dock: keyboard-safe geometry (CO-01, CO-04, CO-06, CO-12)', () => {
  // The numbers the audit measured: a 390x844 phone, an iOS keyboard covering
  // 320px of it, and a dock standing 96px tall. Nothing here renders - it
  // computes the shipped formulas, which is the only way CO-01..CO-08 can fail a
  // test instead of merely looking wrong on a phone.
  const PHONE = 844;
  const SHORT = 667; // the 375x667 device CO-04 was measured on
  const LANDSCAPE = 390; // an 844x390 phone, keyboard closed
  const narrow = mediaBlock(composerCss, '(max-width: 800px)');
  const anchorValue = (): string =>
    declarationsIn(narrow, '.compose-bottom .model-switcher-menu.dropup')['bottom'] ?? '';

  it('lifts the model sheet above the keyboard and keeps it under the shell top (CO-01)', () => {
    const menu = declarationsIn(narrow, '.compose-bottom .model-switcher-menu.dropup');
    expect(menu['position']).toBe('fixed');
    const bottom = menu['bottom'] ?? '';
    const height = menu['max-height'] ?? '';
    // The inset has to be part of the anchor: this element is measured against
    // the layout viewport, which iOS never shrinks for the keyboard.
    expect(bottom).toContain('var(--kb-inset, 0px)');
    expect(height).toContain('var(--kb-inset, 0px)');

    for (const [inset, viewport] of [
      [0, PHONE],
      [IOS_KEYBOARD, PHONE],
      [IOS_KEYBOARD, SHORT],
      [0, LANDSCAPE],
    ] as const) {
      const vars = dockVars(inset);
      const anchor = pixels(bottom, { height: viewport, vars });
      const maxHeight = pixels(height, { height: viewport, vars });
      // `layout.css`: the shell is `100dvh - inset` tall and the dock is its
      // bottom `--composer-h`, so the sheet may not hang below that top edge.
      const dockTop = viewport - inset - DOCK;
      const sheetBottom = viewport - anchor;
      expect(sheetBottom, `inset ${inset} at ${viewport}`).toBeLessThanOrEqual(dockTop + 0.01);
      // ...and its top edge stays inside the visible band.
      expect(sheetBottom - maxHeight, `inset ${inset} at ${viewport}`).toBeGreaterThanOrEqual(
        -0.01,
      );
      // A sheet with no room left is a closed sheet: keep a few rows.
      expect(maxHeight).toBeGreaterThan(80);
    }
  });

  it('bounds the searchable model list by what is left above the dock (CO-01)', () => {
    const menu = declarationsIn(narrow, '.compose-bottom .model-switcher-menu-models.dropup');
    const vars = dockVars(IOS_KEYBOARD);
    const maxHeight = pixels(menu['max-height'] ?? '', { height: SHORT, vars });
    const anchor = pixels(anchorValue(), { height: SHORT, vars });
    expect(SHORT - anchor - maxHeight).toBeGreaterThanOrEqual(-0.01);
  });

  it('fits the @ popover into the window the keyboard leaves (CO-04)', () => {
    const maxHeight = declarations(composerCss, '.mention-popover')['max-height'] ?? '';
    expect(maxHeight).toContain('var(--kb-inset, 0px)');
    for (const [inset, viewport] of [
      [0, PHONE],
      [IOS_KEYBOARD, PHONE],
      [IOS_KEYBOARD, SHORT],
      [0, LANDSCAPE],
    ] as const) {
      const resolved = pixels(maxHeight, { height: viewport, vars: dockVars(inset) });
      // The popover is anchored `calc(100% + 8px)` above the dock's own box, and
      // the dock's box starts at the top of the measured region.
      const top = viewport - inset - DOCK - 8 - resolved;
      expect(top, `inset ${inset} at ${viewport}`).toBeGreaterThanOrEqual(-0.01);
      // 88px is the floor: a couple of rows stay readable, and the armed row is
      // scrolled into view (`useComposerPopovers`) rather than clipped.
      expect(resolved).toBeGreaterThanOrEqual(88);
    }
  });

  it('caps the waiting list and the chip strip so the dock cannot leave the pane (CO-06)', () => {
    const list = declarations(composerCss, '.queued-list');
    expect(list['overflow-y']).toBe('auto');
    expect(list['overscroll-behavior']).toBe('contain');

    const chips = declarations(composerCss, '.composer-attachments');
    expect(chips['overflow-y']).toBe('auto');
    expect(chips['overscroll-behavior']).toBe('contain');

    // Eight waiting messages at the 40px coarse row floor are 320px of content
    // that used to be 320px of dock (measured: the tray at y=-46, the transcript
    // at 0px). The list scrolls instead.
    const queueCap = pixels(list['max-height'] ?? '', { height: PHONE, vars: dockVars(0) });
    expect(queueCap).toBe(160);
    expect(8 * 40).toBeGreaterThan(queueCap);

    // The three bands that grow with content are all bounded now, and their
    // bounds still fit inside the shrunken shell with room for the transcript:
    // the queue tray (list + panel padding + head), the chip strip and the draft.
    const draftCap = pixels(
      declarationsIn(mediaBlock(composerCss, '(pointer: coarse)'), '.composer-input')[
        '--compose-input-max'
      ] ?? '',
      { height: PHONE },
    );
    const chipCap = pixels(chips['max-height'] ?? '', { height: PHONE, vars: dockVars(0) });
    const queueTray = queueCap + 16 + 24;
    const shell = PHONE - IOS_KEYBOARD;
    expect(queueTray + chipCap + draftCap).toBeLessThanOrEqual(shell - DOCK);
  });

  it('takes the 16px iOS no-zoom guard onto every coarse pointer (CO-02, CO-03)', () => {
    // `--fs-title` is 16px, and the scale is the only place that may say so.
    expect(tokensCss).toMatch(/--fs-title: 16px/);

    // The portrait phone tier keeps its copy (that is the width the audit
    // measured); the coarse tier is what reaches a landscape phone and an iPad.
    const phone = declarationsIn(mediaBlock(composerCss, '(max-width: 640px)'), '.composer-input');
    const coarse = declarationsIn(mediaBlock(composerCss, '(pointer: coarse)'), '.composer-input');
    for (const body of [phone, coarse]) {
      expect(body['font-size']).toBe('var(--fs-title)');
      expect(body['--compose-input-max']).toBe('min(112px, 22vh)');
    }
    expect(pixels('min(112px, 22vh)', { height: PHONE })).toBe(112);

    // The model filter is a focusable field inside a fixed sheet: 16px in a 32px
    // box, in both tiers, or iOS zooms the page inside the sheet.
    for (const query of ['(max-width: 640px)', '(pointer: coarse)']) {
      const filter = declarationsIn(
        mediaBlock(composerCss, query),
        '.compose-bottom .model-switcher-filter',
      );
      expect(filter['font-size']).toBe('var(--fs-title)');
      expect(filter['height']).toBe('32px');
    }
  });

  it('gives the coarse lists and the title-only controls the thumb floor (CO-10, CO-11)', () => {
    const coarse = mediaBlock(composerCss, '(pointer: coarse)');
    // §5.3: 44 for every control a thumb aims at (44 from this audit on - the
    // 40px house floor it used to allow is now confined to transcript rows).
    expect(declarationsIn(coarse, '.mention-item')['min-height']).toBe('44px');
    expect(declarationsIn(coarse, '.compose-bottom .model-switcher-filter')['min-height']).toBe(
      '32px',
    );
    expect(declarationsIn(coarse, '.queued-recall')['min-height']).toBe('44px');
    expect(declarationsIn(coarse, '.compose-approvals')['min-height']).toBe('44px');
    // A phone has no Escape and no arrows, so the keyboard footer is pure noise.
    expect(declarationsIn(coarse, '.mention-foot')['display']).toBe('none');
  });

  it('owns the attachment Remove target in the sheet, not in a utility (CO-08)', () => {
    // An unlayered `w-[24px] h-[24px]` outranks `layer(components)`, so the
    // coarse rule never applied and a phone got a 24x24 Remove.
    expect(declarations(composerCss, '.attachment-chip .attachment-chip-remove')).toMatchObject({
      width: '24px',
      height: '24px',
    });
    expect(
      declarationsIn(
        mediaBlock(composerCss, '(pointer: coarse)'),
        '.attachment-chip .attachment-chip-remove',
      )['width'],
    ).toBe('32px');
    expect(
      declarationsIn(
        mediaBlock(composerCss, '(pointer: coarse) and (max-width: 640px)'),
        '.attachment-chip .attachment-chip-remove',
      )['width'],
    ).toBe('44px');

    // The component no longer carries the size the sheet has to fight.
    const strip = readFileSync(
      new URL('../features/composer/AttachmentStrip.tsx', import.meta.url),
      'utf8',
    );
    expect(strip).not.toMatch(/\bw-\[|\bh-\[/);
  });

  it('shows the visible line break only where a soft keyboard exists (CO-09)', () => {
    expect(declarations(composerCss, '.compose-foot .compose-newline')['display']).toBe(
      'inline-flex',
    );
    // Reveal inverted: a pointer with the real chords does not need the button.
    expect(
      declarationsIn(
        mediaBlock(composerCss, '(hover: hover) and (pointer: fine)'),
        '.compose-foot .compose-newline',
      )['display'],
    ).toBe('none');
    expect(
      declarationsIn(
        mediaBlock(composerCss, '(pointer: coarse)'),
        '.compose-foot .compose-newline',
      )['min-height'],
    ).toBe('44px');
  });
});
