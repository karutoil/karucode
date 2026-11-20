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
const tokensCss = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');
const previewSource = readFileSync(new URL('../components/FilePreview.tsx', import.meta.url), 'utf8');

/** Declarations of one rule, matched by its full selector text. */
function declarations(selector: string): Record<string, string> {
  return rule(css, selector);
}

/** A rule's declarations as a map, from a rule body with its comments removed. */
function readDeclarations(body: string): Record<string, string> {
  const found: Record<string, string> = {};
  let last: string | null = null;
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (line === '') continue;
    const opening = /^([a-z-]+):\s*(.+)$/.exec(line);
    if (opening?.[1] !== undefined && opening[2] !== undefined) {
      last = opening[1];
      found[last] = opening[2].replace(/;$/, '').trim();
    } else if (last !== null) {
      // A value that wrapped (`padding:` and its `calc()` on the next line).
      found[last] = `${found[last] ?? ''} ${line.replace(/;$/, '').trim()}`.trim();
    }
  }
  return found;
}

/**
 * Declarations of one rule inside a sheet or inside a block of one.
 *
 * The selector is written the way a person reads it — `.a, .b` — and the match
 * tolerates the indentation and the newlines a media block adds.
 */
function rule(source: string, selector: string): Record<string, string> {
  const parts = selector
    .split(',')
    .map((part) => part.trim().replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const pattern = new RegExp(
    `(?:^|\\n)[ \\t]*${parts.join(',\\s*')}[ \\t]*\\{([\\s\\S]*?)\\n[ \\t]*\\}`,
  );
  const match = pattern.exec(source);
  expect(match, `${selector} is defined`).toBeTruthy();
  return readDeclarations((match?.[1] ?? '').replace(/\/\*[\s\S]*?\*\//g, ''));
}

/** The body of a media block, by its exact header, braces matched. */
function media(header: string, source = css): string {
  const start = source.indexOf(`@media ${header}`);
  expect(start, `@media ${header} is defined`).toBeGreaterThan(-1);
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1;
    else if (source[index] === '}') {
      depth -= 1;
      if (depth === 0) return source.slice(open, index);
    }
  }
  throw new Error(`@media ${header} is unterminated`);
}

/** One `--token: Npx;` from the token layer, so geometry can be computed. */
function token(name: string): number {
  const match = new RegExp(`--${name}: (\\d+)px;`).exec(tokensCss);
  expect(match?.[1], `--${name} is a px token`).toBeDefined();
  return Number(match?.[1]);
}

/** The phone tier: narrow OR short, which is what a landscape phone is. */
const PHONE = '(max-width: 640px), (max-height: 500px)';

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
    // Two facts: the desktop palette takes the role's measure (not a literal),
    // and the phone tier keeps it a compact floating box rather than promoting
    // it to a full-height sheet the way every other role goes. Its width is the
    // role's measure *capped* by the window, so a 740px-wide landscape phone gets
    // the 560px palette on the phone treatment rather than a 712px band.
    expect(css).toMatch(/\.overlay-palette \{[\s\S]*?width:\s*min\(var\(--overlay-w-palette\)/);
    const phone = media(PHONE);
    expect(rule(phone, '.overlay-palette')['align-self']).toBe('center');
    expect(rule(phone, '.overlay-palette')['width']).toBe(
      'min(var(--overlay-w-palette), calc(100vw - 28px))',
    );
    expect(rule(phone, '.overlay-palette')['border-radius']).toBe('var(--radius)');
  });

  it('insets the phone palette on the field, not on every header row', () => {
    // The regression: the phone tier took the palette's top band from the shared
    // 20px/32px inset down to 4px/2px through a selector list that also matched
    // `.overlay-header` — the row every other role puts its close button in. The
    // palette renders no header (the field *is* the band), so the rule belongs to
    // `.palette-search` alone and no `.overlay-header` may be re-padded there.
    const phone = media(PHONE);
    expect(rule(phone, '.overlay-palette .palette-search')['padding']).toBe(
      'var(--space-2) var(--space-5)',
    );
    expect(phone).not.toMatch(/\.overlay-palette \.overlay-header/);
    expect(phone).not.toMatch(/\.overlay-palette \.dialog-header/);
  });
});

describe('overlays.css has no orphaned selector', () => {
  /* Rules for a shell this app does not render are worse than missing rules: they
   * read as "this is handled" while nothing matches, and a test that greps for
   * them pins the dead state. Each case below names the selector, the component
   * fact that makes it unreachable, and the class token that is the live one. */
  const cssWithoutComments = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const overlaySource = readFileSync(new URL('../components/Overlay.tsx', import.meta.url), 'utf8');
  const paletteSource = readFileSync(
    new URL('../components/CommandPalette.tsx', import.meta.url),
    'utf8',
  );

  it('paints no `.dialog-palette .dialog-*` variant', () => {
    // The palette passes `showHeader={false}`, so `Overlay` renders no
    // `.overlay-header` and never a `.dialog-header`; and the surface's own
    // classes are `.overlay .overlay-palette .dialog-palette`. Twenty-one lines
    // of header/title/icon-button rules matched none of that.
    expect(paletteSource).toMatch(/role="palette"/);
    expect(paletteSource).toContain('showHeader={false}');
    expect(cssWithoutComments).not.toMatch(/\.dialog-palette \.dialog-/);
    expect(cssWithoutComments).not.toMatch(/\.overlay-palette \.dialog-/);
  });

  it('paints no `.theme-shade`, which no component renders', () => {
    // The drawer uses the shell's `.dialog-backdrop`; the one-veil rule used to
    // keep naming the private veil it no longer has.
    expect(cssWithoutComments).not.toContain('.theme-shade');
    expect(css).toMatch(/\.dialog-layer:has\(~ \.dialog-layer\)\n {2}\.dialog-backdrop \{/);
  });

  it('gives every vaunted focus cue a rule that can paint', () => {
    // The old cue was `:focus-within .dialog-header { border-bottom-color }`,
    // which needed a header the palette never renders (and a `border-bottom: 0`
    // in the deleted variant above it). The cue is the rail now, and the input
    // paints no second frame.
    expect(cssWithoutComments).not.toContain(':focus-within .dialog-header');
    expect(declarations('.palette-search')['position']).toBe('relative');
    expect(declarations('.palette-search::before')['width']).toBe('2px');
    expect(declarations('.palette-search:focus-within::before')['background']).toBe('var(--accent)');
    expect(declarations('.palette-input:focus-visible')['outline']).toBe('none');
  });

  it('keeps every `flex` in the palette on a row, where it does something', () => {
    // `flex: 1 1 auto` on a child of the palette's *column* is inert: `flex-grow`
    // acts on the main axis, and on a column the main axis is height, which the
    // list bounds with `max-height` anyway. `.palette-search` and `.palette-list`
    // are in that column, so neither may declare one; the field inside the row
    // keeps its share of the row's width.
    const search = declarations('.palette-search');
    expect(search['display']).toBe('flex');
    expect(search['flex']).toBeUndefined();
    expect(declarations('.palette-list')['flex']).toBeUndefined();
    expect(declarations('.palette-input')['flex']).toBe('1 1 auto');
  });

  it('drops `is-headless`, a class no stylesheet defines', () => {
    // `Overlay` emitted it whenever `showHeader` was false and nothing consumed
    // it; the flag's real effect is the `sr-only` title, which is still emitted.
    expect(overlaySource).not.toContain('is-headless');
    expect(cssWithoutComments).not.toContain('is-headless');
    expect(overlaySource).toContain('class="sr-only"');
  });
});

describe('layer ladder', () => {
  /* z-index values are a relationship, and the one that had drifted was toast vs
   * modal: `.dialog-layer` sat at a literal 100 while `.toast-region` sat at an
   * unexplained 130, so a toast always painted over the dialog's own surface.
   * Both read a token now, and the order is asserted here so it cannot drift back
   * into two unrelated literals. */
  const baseCss = readFileSync(new URL('./base.css', import.meta.url), 'utf8');

  /** One `--z-*` rung from the ladder block in overlays.css. */
  function rung(name: string): number {
    const match = new RegExp(`--${name}: (\\d+);`).exec(css);
    expect(match?.[1], `--${name} is declared`).toBeDefined();
    return Number(match?.[1]);
  }

  it('declares the rungs once, in one place', () => {
    for (const name of ['z-sheet', 'z-modal', 'z-toast', 'z-skip-link']) rung(name);
    // A second literal would be exactly the drift this suite exists to catch.
    expect(css).not.toMatch(/\.dialog-layer \{[^}]*z-index: \d/);
    expect(css).not.toMatch(/\.toast-region \{[^}]*z-index: \d/);
  });

  it('puts a sheet under a modal, a modal under a toast, a toast under the skip link', () => {
    expect(rung('z-sheet')).toBeLessThan(rung('z-modal'));
    expect(rung('z-modal')).toBeLessThan(rung('z-toast'));
    expect(rung('z-toast')).toBeLessThan(rung('z-skip-link'));
    // A toast reports on the dialog's own action (a failed save), so one rung
    // above it — not the 30 it had drifted to, and never under it.
    expect(rung('z-toast') - rung('z-modal')).toBeLessThanOrEqual(40);
  });

  it('wires the surfaces to the rungs instead of restating them', () => {
    expect(declarations('.dialog-layer')['z-index']).toBe('var(--z-modal)');
    expect(declarations('.toast-region')['z-index']).toBe('var(--z-toast)');
    // `base.css` owns the skip link, and it is the top of the ladder in both files.
    expect(baseCss).toMatch(/\.skip-link \{[^}]*z-index: 200;/);
    expect(rung('z-skip-link')).toBe(200);
  });

  it('puts a menu above the dialog it was opened from, and under a toast (OV-04)', () => {
    // A long-press or right-click menu is portalled into `<body>`, and it can be
    // opened *inside* a dialog (a session row in the Context drawer). At the sheet
    // rung it painted behind the dialog, so the next tap landed on the backdrop
    // and closed the dialog the menu belonged to.
    expect(rung('z-menu')).toBeGreaterThan(rung('z-modal'));
    expect(rung('z-menu')).toBeLessThan(rung('z-toast'));
    // `layout.css` still carries the old number on `.menu-layer`; the rung is
    // applied where the ladder is published, with the specificity to win.
    expect(rule(css, 'body > .menu-layer')['z-index']).toBe('var(--z-menu)');
  });
});

describe('command palette vocabulary', () => {
  /* The palette is the one overlay that is a *list*, so it is the one most
   * tempted to grow a private row, heading and keycap. It did: an amber-on-amber
   * selected row, a group label that re-authored the kicker, two keycap recipes
   * and an arrow on every row that had no hint. These pins are what keep it on
   * the workbench's vocabulary (DESIGN-SYSTEM §5A). */
  const paletteSource = readFileSync(new URL('../components/CommandPalette.tsx', import.meta.url), 'utf8');

  it('marks the row Enter will run with the rail, not with amber ink', () => {
    const active = declarations('.palette-item.is-active');
    // `--text` on `--accent-bg` is the pair `.session-row.is-active` already uses
    // (`--text` is the label; the tint says which row). An accent-coloured label
    // on an accent tint measured 3.62:1 in Daybreak.
    expect(active['background']).toBe('var(--accent-bg)');
    expect(active['color']).toBe('var(--text)');
    expect(declarations('.palette-item.is-active::before')['background']).toBe('var(--accent)');
    // The rail is a 2px mark inset off the row's own ends, the same shape the
    // session rail draws.
    expect(declarations('.palette-item::before')['width']).toBe('2px');
  });

  it('inks the hint with a floored role, because the tint is not a surface', () => {
    // §4.1's floor is measured against the nearest *surface*; a row with an accent
    // tint under it is not one, so `--muted` (4.5:1 on the surface) is not enough.
    expect(declarations('.palette-item.is-active .palette-hint')['color']).toBe(
      'var(--text-secondary)',
    );
  });

  it('re-uses the shared keycap instead of authoring a third one', () => {
    // base.css styles `kbd` (10px mono, `--line-keycap`, no fill). The palette used
    // to override it twice — an 18px outlined chip in the search row and a 16px
    // filled one in the footer — so the same key looked like two controls. The
    // in-field chip is gone entirely now: the footer's hints already teach Esc,
    // and a keycap inside the input competed with the placeholder.
    expect(css).not.toMatch(/\.palette-esc\s*\{/);
    expect(css).not.toMatch(/\.palette-footer kbd\s*\{/);
    // The footer's keys inherit the shell's footer band; only their layout is
    // authored here.
    expect(declarations('.palette-keys')['display']).toBe('inline-flex');
  });

  it('draws a group heading with the shared label and its mono count', () => {
    expect(paletteSource).toContain('class="session-group-label palette-group-label"');
    // The variant owns padding only: the ink, size, tracking and the count span
    // come from `.session-group-label` in layout.css.
    expect(declarations('.palette-group-label')).toEqual({
      'margin': '0',
      'padding': 'var(--space-5) var(--space-6) var(--space-2)',
    });
  });

  it('gives the arrow to the active row only', () => {
    // A column of `→` down every unhinted row is decoration; on the row Enter will
    // run it is the affordance that says the row goes somewhere.
    expect(declarations('.palette-arrow')['display']).toBe('none');
    expect(declarations('.palette-item.is-active .palette-arrow')['display']).toBe('inline-grid');
  });

  it('states the match count once, not three times', () => {
    // It was announced by the live region, counted on every group heading, and
    // printed again in the footer. The footer copy is gone; the live region is
    // the announcement and the group headings are the reading.
    expect(css).not.toMatch(/\.palette-count\s*\{/);
    expect(paletteSource).toContain('aria-live="polite"');
    expect(paletteSource).not.toContain('palette-count');
    expect(paletteSource).toMatch(/class="session-group-label palette-group-label"/);
  });

  it('highlights the run the filter actually tested', () => {
    // The filter and the highlight must agree, or the palette explains the wrong
    // substring; `lib/paletteMatch.ts` is the single definition of that split.
    expect(paletteSource).toContain("import { matchSegments } from '../lib/paletteMatch';");
    expect(paletteSource).toContain('<mark class="palette-mark">');
    expect(declarations('.palette-mark')['color']).toBe('var(--accent-fill)');
    expect(declarations('.palette-item.is-active .palette-mark')['color']).toBe(
      'var(--accent-on-tint)',
    );
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

  it('declares its measure where the phone tier can override it (OV-09)', () => {
    // The measure used to be written *after* the responsive block, so its 620px
    // came back on top of the tier's `width: 100%` and the rewind was the one
    // dialog the phone rules could not resize.
    expect(declarations('.dialog-rewind')['width']).toBe(
      'min(var(--overlay-w-standard), 100%)',
    );
    expect(css.indexOf('\n.dialog-rewind {')).toBeLessThan(
      css.indexOf(`@media ${PHONE}`),
    );
    // …and the phone tier takes it to the full width like every other role.
    expect(rule(media(PHONE), '.dialog')['width']).toBe('100%');
  });
});

describe('the phone shell knows about the keyboard (OV-02, SH-03)', () => {
  /* `--kb-inset` is the strip of viewport the on-screen keyboard covers, published
   * by `lib/viewportInset.ts`. No overlay surface read it: every full-screen
   * dialog was a bare `100dvh`, which on iOS is the *un*shortened screen, so Save
   * and Continue sat under the keys with nothing able to scroll them out. */

  it('sizes every full-screen surface to the viewport minus the keyboard', () => {
    const phone = media(PHONE);
    const shell = 'calc(100dvh - var(--kb-inset, 0px))';
    for (const selector of [
      '.overlay:not(.overlay-compact):not(.overlay-palette)',
      '.dialog',
      '.dialog-file, .overlay-full',
    ]) {
      const found = rule(phone, selector);
      expect(found['height'], selector).toBe(shell);
      expect(found['max-height'], selector).toBe(shell);
    }
    // The drawer is absolutely positioned inside the layer rather than sized.
    expect(rule(phone, '.overlay-drawer')['bottom']).toBe('var(--kb-inset, 0px)');
    // The palette is the one floating role, and its cap loses the same strip.
    expect(rule(phone, '.overlay-palette')['max-height']).toBe(
      'calc(82dvh - var(--kb-inset, 0px))',
    );
    // No dialogs left on the old arithmetic.
    expect(phone).not.toMatch(/(?:max-)?height: 100dvh;/);
  });

  it('keeps the footer clear of the home indicator', () => {
    // `env()` is 0 wherever there is no safe area, so this is the same padding on
    // a desktop and 34px more on an iPhone's bottom band.
    expect(declarations('.overlay-footer')['padding']).toContain(
      'env(safe-area-inset-bottom, 0px)',
    );
    expect(declarations('.overlay-footer')['padding']).toContain('var(--space-8)');
  });
});

describe('a tall compact dialog on a 375x667 phone (OV-01)', () => {
  /* The defect: `.overlay-compact`/`.dialog-confirm` kept `margin-top:
   * min(17vh, 150px)` (113px at 667) *and* a cap of `100dvh - 48px` (619px) inside
   * a fixed layer that could not scroll, with the body's scroll locked. 113 + 619
   * = 732 in a 667px window, and the confirm row was the part that hung below it. */

  it('centres the dialog instead of anchoring it below the fold', () => {
    const phone = media(PHONE);
    const compact = rule(phone, '.dialog-confirm, .overlay-compact');
    expect(compact['margin-top']).toBe('0');
    expect(compact['align-self']).toBe('center');
    expect(compact['height']).toBe('auto');
  });

  it('leaves its cap inside the 667px band, with the layer able to scroll', () => {
    const height = 667;
    const cap = height - token('space-10') - token('space-7'); // 667 - 32 - 16
    expect(cap).toBe(619);
    // No anchor is added above it on a phone, so the box fits the window …
    expect(cap).toBeLessThan(height);
    // … and if a role ever exceeds it, the layer is the scroller rather than a
    // fixed panel with a row nobody can reach.
    const layer = rule(media(PHONE), '.dialog-layer');
    expect(layer['overflow-y']).toBe('auto');
    expect(layer['overscroll-behavior']).toBe('contain');
    expect(rule(media(PHONE), '.dialog-confirm, .overlay-compact')['max-height']).toBe(
      'calc(100dvh - var(--kb-inset, 0px) - var(--space-10) - var(--space-7))',
    );
  });
});

describe('toasts over an open dialog (OV-06)', () => {
  /* On a phone the toast region moves to the top of the screen — where a dialog
   * draws its header and its close button — and it could cover 46vh of it. */

  it('caps the region to two toasts and a quarter of the screen', () => {
    const phone = media(PHONE);
    expect(phone).toMatch(
      /body:has\(> \.dialog-layer\) \.toast-region \{\n {4}max-height: 25vh;/,
    );
    // `nth-child(n + 3)` drops the oldest: the list renders in arrival order.
    expect(phone).toMatch(
      /body:has\(> \.dialog-layer\) \.toast-region \.toast:nth-child\(n \+ 3\) \{\n {4}display: none;/,
    );
    // The offset clears the header's 16px inset plus its 32px close button.
    expect(rule(phone, '.toast-region')['top']).toBe(
      'calc(var(--space-10) * 2 + env(safe-area-inset-top, 0px))',
    );
    expect(token('space-10') * 2).toBeGreaterThan(token('space-7') + 44);
  });
});

describe('the thumb floor (OV-05)', () => {
  /* No overlay sheet had a coarse-pointer tier: the dialog close button measured
   * 32px, the sheet's close 31px, a rail chip 32px, the toast's dismiss 24px and
   * the sign-in field 38px. 44px is the floor both platform vendors and WCAG
   * 2.5.5 name, and each sheet raises its own controls where its later rules
   * live. */
  const settingsCss = readFileSync(new URL('./settings.css', import.meta.url), 'utf8');
  const authCss = readFileSync(new URL('./auth.css', import.meta.url), 'utf8');

  it('raises every overlay control a thumb hits to 44px', () => {
    const coarse = media('(pointer: coarse)', css);
    const overlayControls = rule(coarse, '.overlay-close, .toast-close, .overlay-footer .button');
    expect(overlayControls['min-height']).toBe('44px');
    expect(overlayControls['min-width']).toBe('44px');

    const settings = media('(pointer: coarse)', settingsCss);
    const sheetControls = rule(settings, '.settings-close, .settings-nav-item');
    expect(sheetControls['min-height']).toBe('44px');
    expect(sheetControls['min-width']).toBe('44px');
    // The rail chip sets 32px in the phone tier at the same specificity, so the
    // pointer tier has to be the later one.
    expect(settingsCss.indexOf('@media (pointer: coarse)')).toBeGreaterThan(
      settingsCss.indexOf('@media (max-width: 780px)'),
    );

    const auth = media('(pointer: coarse)', authCss);
    // The field keeps the 38px it is drawn at on a mouse; the floor is the
    // computed box, which is the larger of the two.
    expect(rule(auth, '.auth-input')['min-height']).toBe('44px');
    expect(rule(auth, '.auth-reveal')['width']).toBe('44px');
    expect(rule(auth, '.auth-reveal')['height']).toBe('44px');
  });
});

describe('the palette on a short viewport (SH-10, OV-11)', () => {
  /* The palette was sized against `vh` with a `12vh` top anchor. On a 740x360
   * landscape phone that is `min(12vh, 110px)` + `min(90vh, 475px)` = 43 + 324 =
   * 367 in a 360px window: seven pixels of the last row, and the footer's key
   * hints, off the screen. `vh` is the *largest* viewport on mobile Safari, so the
   * real gap with the toolbar showing was worse than the arithmetic. */

  it('caps and anchors in `dvh`, not `vh`', () => {
    const palette = declarations('.dialog-palette');
    expect(palette['margin-top']).toBe('min(12dvh, 110px)');
    expect(palette['max-height']).toBe('min(90dvh, 475px)');
    expect(palette['margin-top']).not.toMatch(/(?:^|[^d])vh/);
    expect(palette['max-height']).not.toMatch(/(?:^|[^d])vh/);
  });

  it('fits 740x360, where the anchored box was 7px taller than the window', () => {
    const height = 360;
    const anchored = Math.min(0.12 * height, 110) + Math.min(0.9 * height, 475);
    expect(anchored).toBeGreaterThan(height); // 367 > 360: the defect, in arithmetic
    // The fix: no anchor on a phone, and a cap inside the window minus the keyboard.
    const phone = media(PHONE);
    expect(rule(phone, '.overlay-palette')['margin-top']).toBe('0');
    expect(0.82 * height).toBeLessThanOrEqual(height);
  });

  it('drops the keyboard hints and the band they drew on a phone', () => {
    // "↑ ↓ Enter Esc" teaches a device with no hardware keys, and costs a whole
    // band at the bottom of a 360px screen.
    const phone = media(PHONE);
    expect(rule(phone, '.palette-keys, .overlay-palette .overlay-footer')['display']).toBe(
      'none',
    );
    // The old ≤374px rule hid one group at a time; the whole row is gone now.
    expect(phone).not.toMatch(/palette-keys > span/);
  });
});

describe('the phone band is height-driven too (OV-03, OV-10)', () => {
  /* A 740x360 landscape phone is 740px wide: wider than the 640 tier, so it got
   * the desktop two-pane settings sheet in a 312px box and the onboarding card
   * with its step rail wrapping over the fields. Height is the constraint there,
   * and no height-driven tier existed. */

  const onboardingCss = readFileSync(new URL('./onboarding.css', import.meta.url), 'utf8');
  const settingsCss = readFileSync(new URL('./settings.css', import.meta.url), 'utf8');

  it('gives the short viewport the phone treatment, not the desktop one', () => {
    // Every sheet that had a phone tier now says "narrow OR short" in the same
    // words, so one media query answers "is this a phone" for all of them.
    for (const source of [css, onboardingCss, settingsCss]) {
      expect(source).toContain(`@media ${PHONE} {`);
    }
    expect(rule(media(PHONE), '.dialog')['height']).toBe(
      'calc(100dvh - var(--kb-inset, 0px))',
    );
  });

  it('hides the onboarding step rail and keeps the primary action first', () => {
    const band = media(PHONE, onboardingCss);
    expect(rule(band, '.onboarding-rail')['display']).toBe('none');
    // `column-reverse`: the primary action is the last child and the thumb comes
    // from the bottom of the screen.
    expect(rule(band, '.onboarding-footer')['flex-direction']).toBe('column-reverse');
    expect(onboardingCss).not.toMatch(/@media \(max-width: 640px\) \{/);
  });
});
