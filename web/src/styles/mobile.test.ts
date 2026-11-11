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
 * The tab strip now shares `main-head` with the breadcrumb, so the phone frame is
 * a single header and the transcript is full width.
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
const workbenchCss = readFileSync(new URL('./workbench.css', import.meta.url), 'utf8');
const notificationsCss = readFileSync(new URL('./notifications.css', import.meta.url), 'utf8');

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

  it('compacts the header on a phone', () => {
    const body = tier(layoutCss, 640);
    // 66px is the desktop header; on a 956px screen it was spent on chrome before
    // the first message.
    expect(body).toMatch(/--header-h: 52px/);
  });

  it('keeps the session title while dropping secondary breadcrumb chrome', () => {
    const body = tier(layoutCss, 640);
    expect(body).toMatch(/\.breadcrumb > strong,[\s\S]*?display: none/);
    expect(body).toMatch(/\.breadcrumb \{\s*\n\s*flex: 1 1 auto/);
    expect(body).not.toMatch(/\.breadcrumb \{\s*\n\s*display: none/);
  });
});

describe('phone chrome: every control is a thumb-sized target', () => {
  /** The body of a `@media (pointer: coarse)` block; those blocks are single-block
   *  in this repo, so a brace-counted slice is exact where a regex is not. */
  function coarse(css: string): string {
    const start = css.indexOf('@media (pointer: coarse) {');
    if (start < 0) return '';
    let depth = 0;
    for (let i = css.indexOf('{', start); i < css.length; i++) {
      if (css[i] === '{') depth++;
      else if (css[i] === '}' && --depth === 0) return css.slice(start, i);
    }
    return '';
  }

  it('sizes the workbench header buttons for a finger', () => {
    // Measured on a phone before this rule: every header control was 30x30 with a
    // 4px gap, in the corner a thumb reaches first. The rule lives in
    // workbench.css because that file owns the 30px floor AND loads last: the
    // same declaration in layout.css is silently outranked.
    expect(coarse(workbenchCss)).toMatch(
      /\.chat-main\.is-workbench \.header-buttons > \.head-btn \{[^}]*min-width: 40px;[^}]*min-height: 40px;/,
    );
    // ...and the floor it has to override is still there for pointer-fine.
    expect(workbenchCss).toMatch(/\.header-buttons > \.head-btn \{\s*\n\s*min-height: 30px/);
  });

  it('covers the control-plane header too, which is not `.is-workbench`', () => {
    expect(coarse(layoutCss)).toMatch(/\.main-head \.head-btn \{[^}]*min-height: 40px/);
  });

  it('sizes the dock controls for a finger, Stop included', () => {
    // Stop sat at 28x28 directly beside Queue: the two most consequential
    // neighbours in the product, 4px apart.
    const body = coarse(composerCss);
    expect(body).toMatch(/\.compose-bottom \.button-stop,[\s\S]*?min-height: 40px/);
    expect(body).toMatch(/\.compose-plus \{[^}]*width: 40px;[^}]*height: 40px;/);
    // The capsule around the queue options has to be tall enough to hold them:
    // 32px options in a 30px capsule painted the active one outside its border
    // (measured at 440x956 with touch emulation).
    expect(body).toMatch(/\.queue-mode \{\s*\n\s*height: auto;[\s\S]*?min-height: 34px/);
    // The labelled pills only needed height: they are already 66-107px wide.
    expect(body).toMatch(/\.compose-approvals,[\s\S]*?min-height: 32px/);
  });

  it('keeps the pointer-fine sizes the style contract pins', () => {
    // The coarse tier is additive; the mouse numbers must not move.
    expect(declarations(composerCss, '.compose-plus')['width']).toBe('24px');
    expect(declarations(composerCss, '.compose-bottom .button-stop')['min-width']).toBe('28px');
  });

  it('ellipsizes the notification meta line instead of cutting a word in half', () => {
    // `.notification-where` is `display: flex`, where `text-overflow` does
    // nothing: on a phone the "session · project · source" line was clipped
    // mid-word at the row edge. The children are the flex items, so they own the
    // ellipsis.
    expect(declarations(notificationsCss, '.notification-where > span')).toMatchObject({
      'min-width': '0',
      overflow: 'hidden',
      'text-overflow': 'ellipsis',
    });
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
}

/**
 * Resolve `--header-h` for `<html data-density="…">` at `width`.
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
            const match = /^\s*(--header-h):\s*(\d+)px\s*$/.exec(declaration);
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
  expect(header, '--header-h resolves').toBeDefined();
  return { header: header!.value };
}

describe('the phone frame is resolved per density, not by accident', () => {
  it('keeps the comfortable header at 52px', () => {
    expect(frameAt('comfortable', PHONE_WIDTH)).toEqual({ header: 52 });
  });

  it('makes compact shorter than comfortable, never taller', () => {
    const comfortable = frameAt('comfortable', PHONE_WIDTH);
    const compact = frameAt('compact', PHONE_WIDTH);

    // Before the tier restated the density, `:root[data-density='compact']` won
    // on specificity with a 54px header: a "compact" header 2px taller than the
    // comfortable one it exists to shrink.
    expect(compact.header).toBeLessThan(comfortable.header);
  });

  it('gives spacious no extra chrome on a phone', () => {
    const comfortable = frameAt('comfortable', PHONE_WIDTH);
    const spacious = frameAt('spacious', PHONE_WIDTH);

    // The accidental header was 76px, 8% of a 956px screen. Spacious still opens
    // the transcript up through the spacing scale; the chrome stays in the
    // phone's budget.
    expect(spacious.header).toBeLessThanOrEqual(comfortable.header);
  });

  it('still lets density size the frame on a desktop', () => {
    // The tier fix must not flatten the token layer: the same cascade at 1280px
    // resolves the published 66 / 54 / 76 header scale.
    expect(frameAt('comfortable', 1280)).toEqual({ header: 66 });
    expect(frameAt('compact', 1280)).toEqual({ header: 54 });
    expect(frameAt('spacious', 1280)).toEqual({ header: 76 });
  });
});

describe('phone results dock: the conversation obeys the viewport, not its content', () => {
  it('does not size the conversation pane by its min-content width', () => {
    // The pane is a flex item in `.center-body.has-result`'s row. Left on the
    // default `min-width: auto` it took its min-content width — the widest
    // unbreakable thing in the transcript. Measured in Chromium at a 390px
    // viewport: `.chat-panel` 638px, Stop at right 619, Queue and Approvals
    // outside the screen. The overflow is invisible to a `scrollWidth` check
    // (`.app` is `overflow: hidden`) and unreachable by gesture (`.chat-scroll`
    // was 638 wide, so it never scrolled), so only this rule keeps the dock
    // inside the frame.
    const panel = declarations(workbenchCss, '.chat-main.is-workbench .center-body.has-result > .chat-panel');
    expect(panel['min-width']).toBe('0');
  });

  it('declares the floor before the wide-split container query can raise it', () => {
    // Same selector, so the later `@container workbench (min-width: 1100.01px)`
    // block wins on source order where it applies and only there. If the floor
    // ever moved after that block it would also flatten the desktop split.
    const floor = workbenchCss.indexOf('\n.chat-main.is-workbench .center-body.has-result > .chat-panel {');
    const wide = workbenchCss.indexOf('@container workbench (min-width: 1100.01px)');
    expect(floor).toBeGreaterThan(-1);
    expect(wide).toBeGreaterThan(floor);
  });

  it('keeps the 330px conversation floor for the real desktop split only', () => {
    // `ResultResizer` clamps the Results width to `container - 330 - 24`, so the
    // wide split keeps its floor without the pane ever outgrowing its container.
    const wide = workbenchCss.slice(
      workbenchCss.indexOf('@container workbench (min-width: 1100.01px)'),
      workbenchCss.indexOf('@container workbench (max-width: 1100px)'),
    );
    expect(wide).toMatch(/> \.chat-panel \{[\s\S]*?min-width: 330px;/);
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

describe('phone results sheet: above the veil, not behind it', () => {
  it('stacks the open Results sheet over the backdrop that dims the page', () => {
    // `.sheet-backdrop` is a sibling of `.app` at z-index 35. The inspector's
    // overlay placement (container <= 1100px) is z-index 24, which put an
    // expanded Results panel *under* a dark, blurred scrim: the panel was only
    // legible through the veil, and the whole surface read as a focus bug.
    expect(layoutCss).toMatch(/\.sheet-backdrop \{[\s\S]*?z-index: 35;/);
    const sheet = workbenchCss.slice(workbenchCss.indexOf('@media (max-width: 1023px)'));
    expect(sheet).toMatch(
      /\.inspector\.is-open,\s*\n\s*\.chat-main\.is-workbench \.center-body\.has-result > \.inspector\.is-open \{\s*\n\s*z-index: 41;/,
    );
    // Above the veil (35). The number matters, not the token: a sheet below the
    // backdrop is a sheet nobody can read.
    expect(layoutCss).toMatch(/\.sheet-backdrop \{[\s\S]*?z-index: 35;/);
    const lifted = Number(/z-index: 41;/.exec(sheet)?.[0].replace(/\D/g, ''));
    expect(lifted).toBeGreaterThan(35);
  });

  it('does not lift the docked desktop overlay, which has no veil under it', () => {
    // The same selector outside the phone tier would be a z-index fight with the
    // split layout: the overlay is a sibling of the conversation, not a sheet.
    const wide = workbenchCss.slice(0, workbenchCss.indexOf('@media (max-width: 1023px)'));
    expect(wide).not.toMatch(/z-index: 41/);
  });
});

describe('the phone model sheet is a thumb surface', () => {
  it('gives the reasoning choices the same 40px floor as the rest of the dock', () => {
    // Checked at the iPhone 16 Pro Max viewport (440x956): the effort toggles
    // measured 36px, four of them side by side, between "Low" and "Max" - two
    // neighbours a thumb picks between. The dock's other controls already take
    // `pointer: coarse` to 40px; these live in layout.css, so the rule does too.
    const coarseStart = layoutCss.lastIndexOf('@media (pointer: coarse) {');
    expect(coarseStart).toBeGreaterThan(-1);
    const body = layoutCss.slice(coarseStart);
    expect(body).toMatch(/\.reasoning-choice \{\s*\n\s*min-height: 40px/);
    // ...without moving the pointer-fine row, which stays a compact popover.
    expect(declarations(layoutCss, '.reasoning-choice')['min-height']).toBe('36px');
  });
});
