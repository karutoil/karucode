// @vitest-environment node
/* Stylesheet contract for the phone layout.
 *
 * Component tests run with `css: false`, so computed style cannot be asserted
 * from jsdom; this suite reads the stylesheets that own the shell and pins the
 * geometry that decides how much conversation a phone actually shows.
 *
 * The numbers are measurements, not preferences. At 440x956 (iPhone 16 Pro Max,
 * CSS viewport) the frame used to spend 66px on the header, 44px on the tab strip
 * and 57px on a bottom toolbar - 167px, 17.5% of the screen, before a single
 * message - and the project rail held 56 of 440px of width to show two glyphs.
 * `main-head + workspace-tabbar` is now 90px and the transcript is full width.
 *
 * These are cheap to break by accident: a `padding` shorthand in a media query, a
 * re-added toolbar, a `--rail-w` that goes back to 56px. Each one silently costs
 * the reader a chunk of a screen, which is exactly what this file exists to catch.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const layoutCss = readFileSync(new URL('./layout.css', import.meta.url), 'utf8');
const tokensCss = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');
const railCss = readFileSync(new URL('./rail.css', import.meta.url), 'utf8');
const chatCss = readFileSync(new URL('./chat.css', import.meta.url), 'utf8');
const composerCss = readFileSync(new URL('./composer.css', import.meta.url), 'utf8');

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

/** The body of every `@media (max-width: Npx)` block, joined. */
function tier(css: string, width: number): string {
  return [...css.matchAll(/@media \(max-width: (\d+)px\) \{([\s\S]*?)\n\}/g)]
    .filter((match) => Number(match[1]) === width)
    .map((match) => match[2] ?? '')
    .join('\n');
}

describe('phone shell: the frame leaves the conversation alone', () => {
  it('has no bottom toolbar', () => {
    // Four tabs at 57px were a permanent tax on every screen, and every one of
    // those actions already lives in the header or the sessions sheet: sessions
    // and inspector are the two header icons, new session and settings are in
    // the sheet. A re-introduced bar has to delete this test first.
    expect(layoutCss).not.toMatch(/\.mobile-toolbar/);
    expect(layoutCss).not.toMatch(/\.mobile-tab\b/);
    expect(layoutCss).not.toMatch(/57px \+ env\(safe-area-inset-bottom/);
  });

  it('lets the chat column use the full width on a phone', () => {
    // The rail is hidden there, so reserving its column left a 56px band of dead
    // background beside the conversation.
    const phone = layoutCss.slice(layoutCss.indexOf('@media (max-width: 1023px)'));
    const head = phone.slice(0, phone.indexOf('\n}\n'));
    expect(head).not.toMatch(/padding-left: var\(--rail-w\)/);
  });

  it('reserves no rail column below 1023px', () => {
    // `--rail-w` at 56px is what offset the sheets, the backdrop and the
    // composer's model menu - all against a rail that is `display: none`.
    const body = tier(tokensCss, 1023);
    expect(body).toMatch(/--rail-w: 0px/);
    expect(body).toMatch(/--sidebar-w: 0px/);
  });

  it('gives the sheets the whole screen height', () => {
    // They used to stop 57px short to clear the toolbar. With the bar gone, a
    // sheet that still stopped there would float above nothing.
    const body = tier(layoutCss, 1023);
    expect(body).toMatch(/\.rail \{[\s\S]*?bottom: 0;/);
    expect(body).toMatch(/\.inspector \{[\s\S]*?bottom: 0;/);
    expect(body).toMatch(/\.sheet-backdrop \{[\s\S]*?inset: 0;/);
  });

  it('insets the header for the status bar', () => {
    // `index.html` sets `viewport-fit=cover`, so on an installed phone app the
    // top of the viewport is under the clock. Ignoring the inset put the session
    // buttons behind the status bar.
    const body = tier(layoutCss, 1023);
    expect(body).toMatch(/\.main-head \{[\s\S]*?env\(safe-area-inset-top/);
  });

  it('reserves the home-indicator band at the bottom', () => {
    const body = tier(layoutCss, 1023);
    expect(body).toMatch(/\.chat-main \{[\s\S]*?padding-bottom: env\(safe-area-inset-bottom/);
  });

  it('compacts the header and the tab strip on a phone', () => {
    const body = tier(layoutCss, 640);
    // 66 + 44 is the desktop frame; on a 956px screen it was 11.5% of the height
    // spent on chrome before the first message.
    expect(body).toMatch(/--header-h: 52px/);
    expect(body).toMatch(/--tabs-h: 38px/);
  });

  it('keeps the session title while dropping secondary breadcrumb chrome', () => {
    const body = tier(layoutCss, 640);
    expect(body).toMatch(/\.breadcrumb > strong,[\s\S]*?display: none/);
    expect(body).toMatch(/\.breadcrumb \{\s*\n\s*flex: 1 1 auto/);
    expect(body).not.toMatch(/\.breadcrumb \{\s*\n\s*display: none/);
  });
});

describe('phone shell: the project rail moves into the sheet', () => {
  it('hides the rail column and shows the switcher instead', () => {
    const body = tier(railCss, 1023);
    expect(body).toMatch(/\.project-rail \{\s*\n\s*display: none/);
    expect(body).toMatch(/\.rail \.project-switcher \{\s*\n\s*display: flex/);
  });

  it('keeps the switcher hidden above the phone tier', () => {
    // Desktop keeps the rail; two project switchers at once would be a bug the
    // CSS alone cannot catch, so the base rule is `display: none` and only the
    // narrow tier turns it on.
    expect(declarations(railCss, '.project-switcher')['display']).toBe('none');
  });

  it('drops the duplicated sheet head on a phone', () => {
    // The switcher names the project, lists the rest and owns the project menu,
    // so `Your workspace` + the same title + the same path was a duplicate that
    // cost ~135px at the top of the sheet. The component decides this too (see
    // `SessionsRail.test.tsx`), so the DOM holds one `Project options` button.
    const source = readFileSync(
      new URL('../components/SessionsRail.tsx', import.meta.url),
      'utf8',
    );
    expect(source).toMatch(/\{isNarrow \? null : \(\s*\n\s*<div class="side-head">/);
  });

  it('hides keyboard hints in the sheet', () => {
    // `Ctrl N` and `/` mean nothing on a touch screen.
    const body = tier(layoutCss, 640);
    expect(body).toMatch(/\.rail \.shortcut,[\s\S]*?display: none/);
  });

  it('keeps the draft from eating the screen when the keyboard is up', () => {
    // The cap is CSS, not a JS constant, so it can shrink with the viewport.
    // `max-height` beats an inline `height`, which is what lets the phone tier
    // lower the cap while the on-screen keyboard is up - a 150px draft there left
    // the conversation a sliver.
    // The rule lives in composer.css, which loads after layout.css and owns the
    // input; a copy in the layout tier would be silently overridden.
    const composerPhone = tier(composerCss, 640);
    expect(composerPhone).toMatch(/--compose-input-max: min\(112px, 22vh\)/);
    // The component grows to fit and leaves the clamping to CSS.
    const source = readFileSync(
      new URL('../components/Composer.tsx', import.meta.url),
      'utf8',
    );
    expect(source).toContain('element.style.height = `${element.scrollHeight}px`');
    expect(source).not.toContain('MAX_TEXTAREA_HEIGHT');
  });

  it('keeps the switcher menu inside the sheet', () => {
    // `.rail-menu` is `position: fixed` for the pointer-anchored menus elsewhere.
    // Left fixed with `right: 0` it anchors to the viewport and spills ~140px
    // outside a 300px sheet, so this rule has to override the position too.
    const menu = declarations(railCss, '.project-switcher-menu');
    expect(menu['position']).toBe('absolute');
    expect(menu['right']).toBe('0');
    // The gutter is the 24px spacing step, not a literal (§3.2).
    expect(menu['max-width']).toContain('calc(100vw - var(--space-9))');
  });
});

describe('phone transcript: a card does not fill the screen', () => {
  const body = tier(chatCss, 640);

  it('drops the avatar column', () => {
    // 26px of avatar plus a 9px gap is a fifth of a 390px column spent on a badge
    // that repeats the author line under it.
    expect(body).toMatch(/\.msg-avatar \{\s*\n\s*display: none/);
    expect(body).toMatch(/\.msg \{[^}]*gap: 0/);
  });

  it('stops indenting cards to line up under the avatar', () => {
    // With the avatar gone the old 35px indent points at nothing.
    expect(body).toMatch(
      /\.transcript > :is\(\.tool-card[^{]*\{\s*\n\s*margin-left: 0/,
    );
  });

  it('cuts the tool-card output preview and its padding', () => {
    // Measured: 44px head + a 112px preview + 33px footer = 191px a card, so two
    // finished calls filled a 956px screen. The preview is one line of context
    // here; the full output is one tap away in "Show output".
    expect(body).toMatch(/\.tool-body \{[^}]*max-height: 74px/);
    expect(body).toMatch(/\.tool-head,\s*\.command-summary \{[^}]*min-height: 38px/);
    expect(body).toMatch(/\.tool-foot \{[^}]*min-height: 28px/);
  });

  it('lets an opened card show more than the collapsed one', () => {
    // The clamp has to stay a clamp: expanded output must be readable.
    expect(body).toMatch(/\.tool-card\[data-open='true'\] \.tool-body \{[^}]*max-height: 320px/);
  });

  it('keeps the transcripts one scrolling column', () => {
    // A regression here is a horizontal scrollbar on the whole page, which on a
    // phone makes the header slide sideways under the thumb.
    expect(chatCss).not.toMatch(/body \{[^}]*overflow-x: scroll/);
  });
});

describe('transcript row cost: off-screen rows skip layout and paint', () => {
  const row = declarations(chatCss, '.msg');

  it('opts the message row into content-visibility', () => {
    // A session can hold thousands of rows and a run streams a token at a time;
    // without this the browser lays out and paints the whole conversation on
    // every one of them. `auto` (not `hidden`) keeps the row focusable, findable
    // by find-in-page, and reachable by `scrollIntoView`.
    expect(row['content-visibility']).toBe('auto');
  });

  it('reserves a plausible height for a row it has not measured yet', () => {
    // `auto 160px` remembers each row's last measured height and falls back to
    // 160px the first time. Without the fallback the scrollbar jumps as rows are
    // skipped and re-measured; `contain-intrinsic-size: none` would collapse
    // every off-screen row to zero and make the scrollbar meaningless.
    expect(row['contain-intrinsic-size']).toBe('auto 160px');
  });

  it('leaves the date marker and the cards outside the containment', () => {
    // Only message rows are contained. Applying it to the tool cards too would
    // break `position: sticky` headers inside them and the diff cards' height
    // measurement, which `content-visibility: auto` freezes while off-screen.
    expect(chatCss).not.toMatch(/\.tool-card \{[^}]*content-visibility/);
    expect(chatCss).not.toMatch(/\.date-marker \{[^}]*content-visibility/);
  });
});

/* ------------------------------------------------------------------ *
 * Density and the phone frame
 *
 * The frame used to be asserted by grepping the phone media block for
 * `--header-h: 52px`. That passes while the browser computes something else:
 * the density rules in `tokens.css` are `:root[data-density='…']` and outranked
 * the bare `:root` in that block, so "compact" actually produced 54px (taller
 * than the 52 it exists to shrink) and "spacious" produced 76 + 51 = 127px. A
 * grep cannot see that, so this resolves the properties with the cascade the
 * browser uses: selector specificity across `tokens.css` and `layout.css`, in
 * `index.css` import order, counting only the media queries that apply.
 * ------------------------------------------------------------------ */

/** The breakpoint the phone numbers are measured at (iPhone 16 Pro Max). */
const PHONE_WIDTH = 440;

/** Index of the `}` that closes the `{` at `open`. */
function matchingBrace(css: string, open: number): number {
  let depth = 0;
  for (let index = open; index < css.length; index += 1) {
    if (css[index] === '{') depth += 1;
    else if (css[index] === '}') {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return css.length;
}

interface Frame {
  header: number;
  tabs: number;
}

/**
 * Resolve `--header-h` / `--tabs-h` for `<html data-density="…">` at `width`.
 *
 * Comments are stripped first (they contain examples like
 * `.main-head{height:66px}`), then rules are read with their media context. A
 * selector only participates if it can match the root element at the requested
 * density; specificity is `:root` (0,1,0) plus one class-level unit per
 * `[data-density]`, and source order breaks ties. Files are visited in the order
 * `index.css` imports them.
 */
function frameAt(density: 'compact' | 'comfortable' | 'spacious', width: number): Frame {
  const winner = new Map<string, { value: number; specificity: number; order: number }>();
  let order = 0;

  const visit = (css: string, applies: boolean): void => {
    const source = css.replace(/\/\*[\s\S]*?\*\//g, '');
    let index = 0;
    while (index < source.length) {
      const open = source.indexOf('{', index);
      if (open === -1) return;
      const selectorText = source.slice(index, open).trim();
      if (selectorText.startsWith('@')) {
        const close = matchingBrace(source, open);
        const max = /max-width:\s*(\d+)px/.exec(selectorText);
        const min = /min-width:\s*(\d+)px/.exec(selectorText);
        const nested =
          applies &&
          (max === null || width <= Number(max[1])) &&
          (min === null || width >= Number(min[1]));
        visit(source.slice(open + 1, close), nested);
        index = close + 1;
        continue;
      }
      const close = source.indexOf('}', open);
      if (close === -1) return;
      const body = source.slice(open + 1, close);
      index = close + 1;
      if (applies) {
        for (const part of selectorText.split(',')) {
          const candidate = part.trim();
          const densities = [...candidate.matchAll(/\[data-density=['"]?([a-z]+)['"]?\]/g)].map(
            (match) => match[1],
          );
          const rest = candidate
            .replace(/:root/g, '')
            .replace(/\[data-density=['"]?[a-z]+['"]?\]/g, '')
            .trim();
          if (rest !== '') continue;
          if (densities.some((value) => value !== density)) continue;
          const specificity = 1 + densities.length;
          for (const declaration of body.split(';')) {
            const match = /^\s*(--header-h|--tabs-h):\s*(\d+)px\s*$/.exec(declaration);
            if (!match?.[1] || !match[2]) continue;
            const current = winner.get(match[1]);
            if (
              current === undefined ||
              specificity > current.specificity ||
              (specificity === current.specificity && order > current.order)
            ) {
              winner.set(match[1], { value: Number(match[2]), specificity, order });
            }
          }
        }
      }
      order += 1;
    }
  };

  // `index.css` imports tokens.css, base.css, layout.css … in that order.
  visit(tokensCss, true);
  visit(layoutCss, true);
  const header = winner.get('--header-h');
  const tabs = winner.get('--tabs-h');
  expect(header, '--header-h resolves').toBeDefined();
  expect(tabs, '--tabs-h resolves').toBeDefined();
  return { header: header!.value, tabs: tabs!.value };
}

describe('the phone frame is resolved per density, not by accident', () => {
  it('keeps the comfortable frame at 52 + 38', () => {
    expect(frameAt('comfortable', PHONE_WIDTH)).toEqual({ header: 52, tabs: 38 });
  });

  it('makes compact shorter than comfortable, never taller', () => {
    const comfortable = frameAt('comfortable', PHONE_WIDTH);
    const compact = frameAt('compact', PHONE_WIDTH);

    // Before the tier restated the density, `:root[data-density='compact']` won
    // on specificity with 54 + 36: a "compact" header 2px taller than the
    // comfortable one it exists to shrink.
    expect(compact.header).toBeLessThan(comfortable.header);
    expect(compact.tabs).toBeLessThan(comfortable.tabs);
    expect(compact.header + compact.tabs).toBeLessThan(comfortable.header + comfortable.tabs);
  });

  it('gives spacious no extra chrome on a phone', () => {
    const comfortable = frameAt('comfortable', PHONE_WIDTH);
    const spacious = frameAt('spacious', PHONE_WIDTH);

    // The accidental frame was 76 + 51 = 127px, 13% of a 956px screen. Spacious
    // still opens the transcript up through the spacing scale; the chrome stays
    // in the phone's budget.
    expect(spacious.header + spacious.tabs).toBeLessThanOrEqual(
      comfortable.header + comfortable.tabs,
    );
    expect(spacious.header + spacious.tabs).not.toBe(127);
  });

  it('still lets density size the frame on a desktop', () => {
    // The tier fix must not flatten the token layer: the same cascade at 1280px
    // resolves the published 66 / 54 / 76 header scale.
    expect(frameAt('comfortable', 1280)).toEqual({ header: 66, tabs: 44 });
    expect(frameAt('compact', 1280)).toEqual({ header: 54, tabs: 36 });
    expect(frameAt('spacious', 1280)).toEqual({ header: 76, tabs: 51 });
  });
});

describe('the branch chip keeps a long repository name in its box', () => {
  it('truncates the name instead of letting it push the header', () => {
    const chip = declarations(layoutCss, '.head-btn.branch-chip');
    expect(chip['min-width']).toBe('0');

    const name = declarations(layoutCss, '.branch-chip-name');
    expect(name['min-width']).toBe('0');
    expect(name['overflow']).toBe('hidden');
    expect(name['text-overflow']).toBe('ellipsis');
    expect(name['white-space']).toBe('nowrap');
  });

  it('becomes a 30px icon-only control at 1100px and keeps its accessible name', () => {
    // A 1024px desktop still shows the rail and inspector, so it has less header
    // room than a phone. Reuse the phone chip there instead of letting a long
    // branch shrink the session title to zero. Read the whole tier: `tier()` stops
    // at the first nested rule's line-anchored close.
    const start = layoutCss.indexOf('@media (max-width: 1100px) {');
    const end = layoutCss.indexOf('\n}\n', start);
    const body = layoutCss.slice(start, end);

    expect(body).toMatch(/\.head-btn\.branch-chip \{[^}]*flex: 0 0 30px/);
    expect(body).toMatch(/\.head-btn\.branch-chip \{[^}]*width: 30px/);
    expect(body).toMatch(/\.head-btn\.branch-chip \{[^}]*padding: 0/);
    expect(body).toMatch(/\.branch-chip-name \{\s*\n\s*display: none/);

    // Hiding the text is only safe because the button carries the full branch in
    // its accessible name and tooltip at every width.
    const source = readFileSync(new URL('../components/ChatView.tsx', import.meta.url), 'utf8');
    expect(source).toContain('aria-label={`Branch ${branch} — open Changes`}');
    expect(source).toContain('title={`On branch ${branch} — open Changes`}');
  });
});
