// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

const read = (name: string): string => readFileSync(new URL(name, import.meta.url), 'utf8');
const workbench = read('workbench.css');
const tokens = read('tokens.css');

describe('Workbench result resizing', () => {
  it('uses a fixed separator and right dock around a bounded flexible conversation', () => {
    expect(tokens).toMatch(/--workbench-resizer-w:\s*24px/);
    expect(workbench).toMatch(/\.center-body\.has-result \{[^}]*display:\s*flex;[^}]*flex-direction:\s*row;/s);
    expect(workbench).toMatch(/> \.chat-panel \{\s*flex:\s*0\.72 1 0;[^}]*min-width:\s*330px;/s);
    expect(workbench).toMatch(/style\*='--workbench-result-w'\] > \.chat-panel \{[^}]*flex:\s*1 1 0;/s);
    expect(workbench).toMatch(/\.result-resizer \{[^}]*flex:\s*0 0 var\(--workbench-resizer-w\);[^}]*margin-inline-start:\s*auto;/s);
    expect(workbench).toMatch(/style\*='--workbench-result-w'\] > \.inspector \{\s*flex:\s*0 0 var\(--workbench-result-w\);/);
    expect(workbench).toMatch(/\.result-resizer \{[^}]*cursor:\s*col-resize;[^}]*touch-action:\s*none;/s);
  });

  it('matches the resized transcript to the composer while keeping speakers distinct', () => {
    const wide = workbench.slice(
      workbench.indexOf('@container workbench (min-width: 1100.01px)'),
      workbench.indexOf('@container workbench (max-width: 1100px)'),
    );
    expect(wide).toMatch(/\.chat-inner,\s*[^\{]*\.composer-inner \{[^}]*max-width:\s*var\(--content-max\);/s);
  });

  it('never narrows the conversation by collapsing the results dock', () => {
    // The 70ch reading measure is a *split* rule: side by side, the conversation
    // is the narrow half. Applying it to the whole centre pane as well made
    // collapsing the dock shrink the transcript - measured in Chromium at a
    // 1920px viewport, `.chat-inner` and `.composer-inner` went 820px -> 638px
    // when the reader collapsed the dock to get more room. So the measure may
    // only appear inside the wide container query, and every other tier keeps
    // the content measure that `.composer-inner` already carries.
    const wide = workbench.indexOf('@container workbench (min-width: 1100.01px)');
    const reading = workbench.indexOf('--workbench-reading-w');
    expect(reading).toBeGreaterThan(wide);

    const split = workbench.slice(wide, workbench.indexOf('@container workbench (max-width: 1100px)'));
    expect(split).toMatch(
      /\.center-body\.has-result \.chat-inner,\s*\.chat-main\.is-workbench \.center-body\.has-result \.composer-inner \{[^}]*max-width:\s*calc\(var\(--workbench-reading-w\)/s,
    );

    const base = workbench.slice(0, wide);
    expect(base).toMatch(
      /\.chat-main\.is-workbench \.chat-inner,\s*\.chat-main\.is-workbench \.composer-inner \{\s*max-width:\s*var\(--content-max\);/s,
    );
  });

  it('removes the separator from intermediate overlay and phone layouts', () => {
    const containerTier = workbench.indexOf('@container workbench (max-width: 1100px)');
    const phoneTier = workbench.indexOf('@media (max-width: 1023px)');
    expect(containerTier).toBeGreaterThan(-1);
    expect(phoneTier).toBeGreaterThan(-1);
    expect(workbench.slice(containerTier, phoneTier)).toMatch(/> \.result-resizer \{\s*display: none;/);
    expect(workbench.slice(phoneTier)).toMatch(/> \.result-resizer \{\s*display: none;/);
  });

  it('keeps Results flush with the right edge instead of animating inside its grid slot', () => {
    const tier = workbench.indexOf('@container workbench (min-width: 1100.01px)');
    const nextTier = workbench.indexOf('@container workbench (max-width: 1100px)', tier + 1);
    expect(tier).toBeGreaterThan(-1);
    expect(workbench.slice(tier, nextTier)).toMatch(
      /> \.inspector \{[^}]*transition-property:\s*border-color, visibility;/s,
    );
  });

  it('keeps views in the header and wraps only that tablist when space is constrained', () => {
    expect(workbench).toContain('.chat-main.is-workbench .main-head > .workspace-tabs');
    expect(workbench).not.toContain('.chat-main.is-workbench > .workspace-tabbar');
    // Two rows only where the container can afford them: the wrap is scoped to
    // 641-1000px, so a phone (<=640) keeps one row (SH-02).
    const tier = workbench.indexOf('@container workbench (min-width: 641px) and (max-width: 1000px)');
    expect(tier).toBeGreaterThan(-1);
    const nextTier = workbench.indexOf('@container workbench (max-width: 1100px)', tier + 1);
    const compact = workbench.slice(tier, nextTier);
    expect(compact).toMatch(/\.main-head \{[^}]*flex-wrap:\s*wrap;[^}]*height:\s*auto;/s);
    expect(compact).toMatch(/\.workspace-tabs \{[^}]*order:\s*5;[^}]*flex:\s*1 0 100%;/s);
    expect(compact).toMatch(
      /min-height:[\s\S]*?var\(--workbench-header-h\) \+ var\(--workbench-result-bar-h\)/,
    );
  });

  it('gives the phone one 52px header row instead of a wrapped 104px one (SH-02)', () => {
    // Measured before this rule: a 58px breadcrumb row plus a 45px tab row = 104px
    // at 360/375/390, while `--header-h` and `mobile.test.ts` promise 52. The
    // number chosen here is the token plus the status-bar inset, i.e. 52px on a
    // plain browser and 52 + inset on an installed app - the same shape
    // `layout.css` gives the non-workbench `.main-head`.
    const start = workbench.indexOf('@container workbench (max-width: 640px) {');
    expect(start).toBeGreaterThan(-1);
    const phone = workbench.slice(start, workbench.indexOf('@container workbench (min-width: 1100.01px)', start));
    expect(phone).toMatch(
      /\.main-head \{[^}]*height:\s*calc\(var\(--header-h\) \+ env\(safe-area-inset-top, 0px\)\);/s,
    );
    expect(phone).toMatch(/\.main-head \{[^}]*flex-wrap:\s*nowrap;/s);
    // The strip shares the row: no full-width second line, and the label gives way
    // to the icon (the accessible name is unchanged).
    expect(phone).not.toMatch(/\.workspace-tabs \{[^}]*flex:\s*1 0 100%;/s);
    expect(phone).toMatch(/\.workspace-tab > span:not\(\.tab-icon\) \{\s*\n\s*display: none;/);
    // ...and the two-row block is not reachable at phone widths.
    const wrap = workbench.indexOf('@container workbench (min-width: 641px) and (max-width: 1000px)');
    expect(wrap).toBeLessThan(start);
  });

  it('keeps the status-bar inset in every tier (SH-05)', () => {
    // The base rule and the phone tier both used a `padding` shorthand, which
    // re-zeroed the `env(safe-area-inset-top)` the <=1023 tier adds, and put the
    // phone header under the clock.
    expect(workbench).toMatch(
      /\.chat-main\.is-workbench \.main-head,\s*\n\.main-head\.cp-session-head \{[\s\S]*?padding-top: env\(safe-area-inset-top, 0px\);/,
    );
    const start = workbench.indexOf('@media (max-width: 640px) {');
    const phone = workbench.slice(start);
    expect(phone).toMatch(/\.main-head \{[^}]*padding-top: env\(safe-area-inset-top, 0px\);/s);
    expect(workbench).not.toMatch(/\.main-head \{[^}]*padding: 0 var\(--space-5\);/s);
  });

  it('bleeds the phone results sheet across the viewport (WB-01)', () => {
    // `min(380px, 92vw)` is 359px on a 390px screen: a 31px sliver of dimmed
    // transcript down one edge, which reads as a misaligned panel.
    const start = workbench.indexOf('@media (max-width: 640px) {');
    const phone = workbench.slice(start);
    expect(phone).toMatch(/\.chat-main\.is-workbench \.center-body\.has-result > \.inspector,\s*\n\s*\.chat-main\.is-workbench \.inspector-inner \{\s*\n\s*width: 100%;/);
  });

  it('lets a plugin chip shrink instead of pushing the bell off the header (SH-09)', () => {
    expect(workbench).toMatch(/\.header-buttons \{[^}]*min-width: 0;/s);
    const start = workbench.indexOf('@container workbench (max-width: 640px) {');
    const phone = workbench.slice(start);
    expect(phone).toMatch(/> \.plugin-slot \{[^}]*overflow: hidden;/s);
  });

  it('gives the tab close and the file actions a thumb-sized box (WB-05, WB-10)', () => {
    // The mask is what says the strip overflows: the fade is 8px, inside the tab's
    // own 10px padding, so a strip that fits shows no dimmed label.
    expect(workbench).toMatch(/\.main-head > \.workspace-tabs \{[\s\S]*?mask-image: linear-gradient\(/);
    const coarse = workbench.slice(workbench.indexOf('@media (pointer: coarse) {'));
    expect(coarse).toMatch(/\.chat-main\.is-workbench \.tab-close \{[^}]*min-width: 44px;[^}]*min-height: 44px;/);
    expect(coarse).toMatch(/\.file-action \{\s*\n\s*min-height: 44px;/);
  });


  it('gives a popped-out panel the same width variable and a card on the same geometry', () => {
    // The pop-out is the only place the dock's width variable is *required*: its
    // card and its handle both read it, so they cannot drift apart while dragging.
    const popout = workbench.slice(workbench.indexOf('/* ---------- popped-out results'));
    expect(popout.length).toBeGreaterThan(0);

    expect(popout).toMatch(/width:\s*min\(var\(--workbench-result-w, 560px\), calc\(100vw - var\(--space-10\)\)\);/);
    expect(popout).toMatch(
      /\.center-body > \.result-resizer \{[^}]*position:\s*fixed;[^}]*right:\s*calc\(\s*var\(--space-6\) \+ min\(var\(--workbench-result-w, 560px\)/s,
    );
    expect(popout).toMatch(/\.center-body > \.result-resizer \{[^}]*width:\s*var\(--workbench-resizer-w\);/s);
    // Closed means display:none, not the docked "0 width, hidden" column.
    expect(popout).toMatch(/collapsed='true'[^}]*> \.inspector \{\s*display:\s*none;/s);
    // The card's inner box follows the card instead of the rail width.
    expect(popout).toMatch(/> \.inspector\s*> \.inspector-inner \{\s*width:\s*100%;/s);
  });

  it('scopes the pop-out to the desktop tier so the phone sheet keeps its contract', () => {
    const start = workbench.indexOf('/* ---------- popped-out results');
    // Everything the pop-out adds sits inside one `min-width: 1024px` query: below
    // it the inspector is a slide-over sheet with its own trap and width.
    const block = workbench.slice(start, workbench.indexOf('@container workbench (max-width: 780px)', start));
    expect(block.startsWith('/*')).toBe(true);
    expect(block).toContain('@media (min-width: 1024px) {');
    const rules = block.slice(block.indexOf('@media (min-width: 1024px) {'));
    // The block has to end where the media query does: one closing brace for it.
    expect(rules.trimEnd().endsWith('}')).toBe(true);
    expect((rules.match(/data-inspector-mode='popout'/g) ?? []).length).toBeGreaterThanOrEqual(4);
    expect(rules).not.toContain('@media (max-width: 1023px)');
  });
});
