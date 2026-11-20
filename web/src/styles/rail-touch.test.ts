// @vitest-environment node
/* The phone contract of the rail, the panels and the notification centre.
 *
 * These are one or two declarations each, and a declaration is the easiest
 * thing in this repository to lose: a refactor that
 * rewrites a rule, a Tailwind utility added back onto an element, a phone tier
 * that restates what another sheet already owns. Component tests run with CSS
 * off, so the sheets are read as text — the same way `mobile.test.ts` and
 * `scale.test.ts` read them.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const sheet = (name: string): string =>
  readFileSync(new URL(`./${name}`, import.meta.url), 'utf8');

const railCss = sheet('rail.css');
const notificationsCss = sheet('notifications.css');
const goalsCss = sheet('goals.css');
const todoCss = sheet('todo.css');
const skillsCss = sheet('skills.css');
const pluginViewsCss = sheet('plugin-views.css');

/** Every declaration of one rule, matched by its full selector text. */
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

/** The body of the first `@media (<query>)` block, braces balanced. */
function media(css: string, query: string): string {
  const start = css.indexOf(`@media (${query}) {`);
  expect(start, `the sheet has an @media (${query}) block`).toBeGreaterThan(-1);
  let depth = 0;
  for (let i = css.indexOf('{', start); i < css.length; i += 1) {
    if (css[i] === '{') depth += 1;
    if (css[i] === '}') {
      depth -= 1;
      if (depth === 0) return css.slice(start, i + 1);
    }
  }
  throw new Error(`@media (${query}) is not closed`);
}

describe('LS-02/LS-06: the hover-only kebabs have a finger path', () => {
  const coarse = media(railCss, 'pointer: coarse');

  it('reveals both kebabs instead of waiting for a hover', () => {
    // The row kebab is `opacity: 0` until the row is hovered, and the project
    // kebab is 18px on a phone: on a touch screen neither was reachable.
    expect(coarse).toMatch(/\.session-menu-trigger,\s*\n\s*\.project-rail-kebab \{[\s\S]*?opacity: 1;/);
  });

  it('gives them a 44px box, with the coarse floor above it', () => {
    expect(coarse).toMatch(/\.session-menu-trigger,\s*\n\s*\.project-rail-kebab \{[\s\S]*?width: 44px;[\s\S]*?height: 44px;/);
  });

  it('keeps the mouse-size box out of Tailwind, where a media query cannot win', () => {
    // The element used to carry `w-[24px] h-[24px]`; a utility sits above every
    // component layer, so the coarse tier could never have raised it.
    expect(declarations(railCss, '.session-menu-trigger')['width']).toBe('24px');
  });
});

describe('SH-07/SH-08: the sheet has a head, an exit and a safe footer', () => {
  it('names the sheet and sizes its close control to the tap floor', () => {
    expect(declarations(railCss, '.rail-sheet-head')['min-height']).toBe('44px');
    expect(declarations(railCss, '.rail-sheet-close')['min-width']).toBe('44px');
    expect(declarations(railCss, '.rail-sheet-close')['min-height']).toBe('44px');
  });

  it('insets the footer that touches the bottom edge, and not the sheet twice', () => {
    // `layout.css` already pads `.rail` on all four sides at the sheet tier; a
    // second inset there would be dead space above Settings.
    const foot = railCss.slice(railCss.indexOf('.app:not(.is-narrow) .side-footer'));
    expect(foot).toContain('env(safe-area-inset-bottom');
    expect(railCss).not.toMatch(/\n\.rail \{[^}]*safe-area/);
  });
});

describe('LS-03/OV-07: mark-as-read is tappable and the list clears the home bar', () => {
  it('reveals and enlarges the mark button where there is no hover', () => {
    const none = media(notificationsCss, 'hover: none');
    expect(none).toMatch(/\.notification-mark \{[\s\S]*?opacity: 1;[\s\S]*?pointer-events: auto;/);
    expect(none).toMatch(/\.notification-mark \{[\s\S]*?min-width: 44px;[\s\S]*?min-height: 44px;/);
  });

  it('keeps the full-bleed list off the home indicator', () => {
    expect(declarations(notificationsCss, '.notification-list')['padding-bottom']).toContain(
      'env(safe-area-inset-bottom',
    );
  });

  it('no longer restates the width the overlay already owns', () => {
    // `.notification-center`'s phone block was dead CSS: `overlays.css` sets the
    // same box with three classes to this one.
    const phone = media(notificationsCss, 'max-width: 640px');
    expect(phone).not.toContain('.notification-center {');
    expect(phone).not.toContain('100vw - 28px');
  });
});

describe('LS-12/LS-14: the panel controls a thumb has to hit', () => {
  it('gives the goal actions a 44px coarse tier on top of the 24px row', () => {
    expect(declarations(goalsCss, '.goals-btn')['height']).toBe('24px');
    expect(media(goalsCss, 'pointer: coarse')).toMatch(/\.goals-btn \{[\s\S]*?min-height: 44px;/);
  });

  it('raises the todo chevron to 44px, not 32px', () => {
    expect(media(todoCss, 'pointer: coarse')).toMatch(
      /\.todo-item-toggle \{\s*\n\s*width: 44px;\s*\n\s*height: 44px;/,
    );
  });

  it('marks a task the panel just revealed as new', () => {
    expect(declarations(todoCss, '.todo-item.is-new')['background']).toBe('var(--accent-surface)');
    expect(declarations(todoCss, '.todo-new-flag')['text-transform']).toBe('uppercase');
  });

  it('marks the skill the pane just created', () => {
    expect(declarations(skillsCss, '.skill-row.is-new')['border-color']).toBe('var(--accent-line)');
  });

  it('marks the goal the session is working on', () => {
    expect(declarations(goalsCss, '.goals-goal.is-open')['border-color']).toBe('var(--accent-line)');
  });
});

describe('LS-17: the panel keeps its own chrome', () => {
  it('pins the kicker row so refresh does not scroll away with the payload', () => {
    const row = declarations(pluginViewsCss, '.plugin-panel > .inspector-kicker-row');
    expect(row['position']).toBe('sticky');
    expect(row['top']).toBe('0');
    // Opaque, or rows would be legible through the pinned header.
    expect(row['background']).toBeTruthy();
  });

  it('grows the refresh control for a finger', () => {
    expect(media(pluginViewsCss, 'pointer: coarse')).toMatch(
      /\.plugin-panel \.plugin-panel-refresh \{[\s\S]*?width: 44px;[\s\S]*?height: 44px;/,
    );
  });
});
