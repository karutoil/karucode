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
    const tier = workbench.indexOf('@container workbench (max-width: 1000px)');
    const nextTier = workbench.indexOf('@container workbench (max-width: 1100px)', tier + 1);
    const compact = workbench.slice(tier, nextTier);
    expect(compact).toMatch(/\.main-head \{[^}]*flex-wrap:\s*wrap;[^}]*height:\s*auto;/s);
    expect(compact).toMatch(/\.workspace-tabs \{[^}]*order:\s*5;[^}]*flex:\s*1 0 100%;/s);
    expect(compact).toMatch(/min-height:\s*calc\(var\(--workbench-header-h\) \+ var\(--workbench-result-bar-h\)\)/);
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
