// @vitest-environment node
/* The panels this change hardened: source control, the todo list and the
 * control plane (plus the agents card in `inspector.css`).
 *
 * Component tests run with `css: false`, so the contrast and target-size
 * contracts have to be read from the sheets themselves - the same approach as
 * `accent.test.ts` and `inspector.test.ts`. The panel code is themed entirely by
 * tokens, so the matrix at the bottom resolves every built-in theme (dark and
 * light) at every density and at both ends of the rounding knob; a token the
 * sheets reference must exist and mean what the sheet assumes at each of them.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { BUILTIN_THEMES } from '../theme/builtins';
import { contrastRatio, mix } from '../theme/color';
import {
  DENSITY_SCALES,
  MAX_ROUNDING,
  MIN_ROUNDING,
  buildTokenMap,
  resolveTheme,
} from '../theme/tokens';
import type { ThemeDensity } from '../theme/types';

const read = (name: string): string => readFileSync(new URL(name, import.meta.url), 'utf8');

const git = read('./git.css');
const todo = read('./todo.css');
// The control plane's sheet is split by region, with a fourth holding what the
// regions share (the status word, the project glyph) and the shell (the plane's
// root, its banners); `sessionControl` is all of them joined, so a whole-file
// assertion still reads every rule. The phone-width tier below is read from the
// sheet that owns the session header.
const controlShared = read('./control-shared.css');
// The status word's rule lives with the `.session-*` family it belongs to, and is
// shared by the workbench rail and the control plane alike.
const layoutCss = read('./layout.css');
const controlRail = read('./control-rail.css');
const controlOverview = read('./control-overview.css');
const controlSession = read('./control-session.css');
const workbenchNavigation = read('./workbench-navigation.css');
const sessionControl = [controlShared, controlRail, controlOverview, controlSession].join('\n');
// Every sheet that could hide the status word, so re-scoping or re-adding a
// hiding rule anywhere in the family fails here. The original defect lived in a
// sheet no hand-written list had thought to include.
const wordSheets = [layoutCss, workbenchNavigation, controlShared, controlRail, controlOverview, controlSession];
const inspector = read('./inspector.css');
const tokensCss = read('./tokens.css');

/** Declarations of one rule, matched by its full selector text. */
function declarations(css: string, selector: string): Record<string, string> {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  const found: Record<string, string> = {};
  for (const line of css.slice(start, css.indexOf('\n}', start)).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) found[match[1]] = match[2];
  }
  return found;
}

/** Raw body of one rule, for multi-line values and comments. */
function ruleBody(css: string, selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

const GIT_CONTENT = [
  '.gitm-kicker',
  '.gitm-branch-track',
  '.gitm-commit-btn:disabled',
  '.gitm-commit-hint',
  '.gitm-group-note',
  '.gitm-dir',
  '.gitm-row-label',
  '.gitm-sha,\n.gitm-commit-meta code',
  '.gitm-detail-sub',
  '.gitm-empty-text',
  '.gitm-note',
  '.gitm-commit-file-stat em',
  '.gitm-busy',
  '.gitm-branch-meta',
  '.gitm-hint',
  '.gitm-confirm-note',
] as const;

const GIT_MIXED_CASE_METADATA = [
  '.gitm-branch-track',
  '.gitm-commit-hint',
  '.gitm-group-note',
  '.gitm-row-label',
  '.gitm-detail-sub',
  '.gitm-busy',
  '.gitm-branch-meta',
  '.gitm-hint',
  '.gitm-confirm-note',
] as const;

describe('source control: readable content, decorative glyphs', () => {
  it.each(GIT_CONTENT)('%s takes the readable metadata ink', (selector) => {
    // `--faint` is a 3:1 decorative role (§4). Every one of these carried it in
    // Daybreak, where it measured below 4.5:1 on the panel's own surface.
    expect(declarations(git, selector)['color']).toBe('var(--muted)');
  });

  it('leaves --faint to the two chevron glyphs that are decoration', () => {
    const offenders = [...git.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
      .filter((match) => /(^|[;\s])color:\s*var\(--faint\)/.test(match[2] ?? ''))
      .map((match) => (match[1] ?? '').trim())
      .sort();
    expect(offenders).toEqual(['.gitm-branch-chevron', '.gitm-group-chevron']);
  });

  it('keeps --fs-micro for the uppercase kicker', () => {
    const kicker = declarations(git, '.gitm-kicker');
    expect(kicker['font-size']).toBe('var(--fs-micro)');
    expect(ruleBody(git, '.gitm-kicker')).toContain('text-transform: uppercase');
  });

  it.each(GIT_MIXED_CASE_METADATA)('%s is --fs-tiny, not the 10px kicker step', (selector) => {
    // §3.3: `--fs-micro` is for uppercase kickers. These labels are mixed case
    // ("ahead 2", "src/components", "3 files"), so 10px made them the smallest
    // text on the panel for no reason beyond being quiet.
    expect(declarations(git, selector)['font-size']).toBe('var(--fs-tiny)');
  });

  it('keeps the chips and the narrow-tier description readable too', () => {
    expect(declarations(git, '.gitm-chip')['font-size']).toBe('var(--fs-tiny)');
    expect(declarations(git, '.gitm-commit-file')['font-size']).toBe('var(--fs-tiny)');
    // The ≤560 tier used to drop the description to 10px prose.
    const narrow = git.slice(git.indexOf('@media (max-width: 560px)'));
    expect(narrow).toMatch(/\.gitm-desc \{\s*\n\s*font-size: var\(--fs-tiny\)/);
    expect(narrow).not.toMatch(/\.gitm-desc \{\s*\n\s*font-size: var\(--fs-micro\)/);
  });
});

describe('todo panel: ids, accent ink and disclosure targets', () => {
  it('paints the task id at full opacity', () => {
    // `opacity: 0.75` on top of `--text-subtle` measured below the floor in
    // Daybreak. The id is how the audit and the agent refer to the task.
    expect(declarations(todo, '.todo-id')).not.toHaveProperty('opacity');
  });

  it('inks the active tone with --accent-on-tint, never the canvas accent', () => {
    expect(declarations(todo, '.todo-tone-active')['color']).toBe('var(--accent-on-tint)');
    // The tint the rule paints is the one the role is defined for.
    expect(ruleBody(todo, '.todo-tone-active')).toContain('background: color-mix(in srgb, var(--accent)');
    expect(tokensCss).toMatch(/--accent-on-tint:\s*color-mix\(in srgb, var\(--accent-fill\)/);
  });

  it('gives the disclosure a 24px target', () => {
    const toggle = declarations(todo, '.todo-item-toggle');
    for (const property of ['width', 'height', 'min-width', 'min-height']) {
      expect(toggle[property], property).toBe('24px');
    }
  });

  it('excludes the static leaf marker from disclosure hover', () => {
    const hover = declarations(
      todo,
      '.todo-item-toggle:hover:not(:disabled):not(.is-static)',
    );
    expect(hover['background']).toBe('var(--surface-raised)');
    expect(todo).not.toContain('.todo-item-toggle:hover:not(:disabled) {');
  });

  it('grows the disclosure for a coarse pointer', () => {
    expect(todo).toMatch(
      /@media \(pointer: coarse\) \{\s*\n\s*\.todo-item-toggle \{\s*\n\s*width: 32px;\s*\n\s*height: 32px;/,
    );
  });

  it('derives its shape and rhythm from the tokens the theme moves', () => {
    // Radius comes from the rounding knob; padding from the density scale. A
    // literal here would stop the panel following compact/spacious or 0/22.
    expect(declarations(todo, '.todo-item-toggle')['border-radius']).toBe('var(--radius-xs)');
    expect(declarations(todo, '.todo-item-head')['padding']).toContain('var(--space-');
    expect(declarations(todo, '.todo-status')['padding']).toBe('var(--space-1) var(--space-2)');
  });
});

describe('control plane: status words survive, actions keep names', () => {
  it('keeps a word for every tone outside the phone-width header', () => {
    // The quiet-tone rule used to set `font-size: 0`, so "Finished", "Cancelled"
    // and "No runs" were three identical dots at every width. The word is now the
    // shared `.session-state`, so the floor is pinned on that one rule — which
    // also means the workbench rail can never reintroduce it.
    const word = ruleBody(layoutCss, '.session-state');
    expect(word).not.toContain('font-size: 0');
    expect(word).toContain('color: var(--muted)');
    // A settled state is quiet, not invisible, and a live one is the accent.
    expect(declarations(layoutCss, '.session-state-running')['color']).toBe('var(--accent-fill)');
    expect(declarations(layoutCss, '.session-state-danger')['color']).toBe('var(--danger)');
    expect(declarations(layoutCss, '.session-state-warn')['color']).toBe('var(--warning)');
  });

  it('never hides the word, in any sheet, at any width', () => {
    // `font-size: 0` was the original sin, but it was never the only way to make
    // "Finished", "Cancelled" and "No runs" three identical dots: `display: none`,
    // `visibility: hidden`, `opacity: 0`, `color: transparent`, `text-indent`,
    // `width: 0` or a 1px type step all do it too, in any tier of any sheet. The
    // phone-width header is where the room runs out first, so it is where the
    // temptation lives; the scan is deliberately not scoped to one tier.
    //
    // A redundant tier that merely restated the base values was deleted: it
    // pinned nothing (the base rule already held those values) while reading as
    // a guarantee. This scan is the guarantee.
    const hiding = /(?:^|[;{\s])(?:display\s*:\s*none|visibility\s*:\s*hidden|opacity\s*:\s*0|color\s*:\s*transparent|text-indent\s*:\s*-|width\s*:\s*0(?:px)?\s*(?:!|$|;)|max-width\s*:\s*0|overflow\s*:\s*hidden|font-size\s*:\s*(?:0|1px))/;
    for (const sheet of wordSheets) {
      const rules = sheet.match(/[^{}]*\.session-state[^{}]*\{[^{}]*\}/g) ?? [];
      for (const rule of rules) {
        const selector = rule.slice(0, rule.indexOf('{')).trim();
        const body = rule.slice(rule.indexOf('{') + 1, rule.lastIndexOf('}'));
        // `overflow: hidden` on the *word itself* is how a long label ellipsises
        // inside a narrow header - that is legibility, not hiding - so it is only
        // a defect when paired with a zero width or a negative indent.
        const verdict = body
          .split(';')
          .map((decl) => decl.trim())
          .filter((decl) => hiding.test(decl + ';') || hiding.test(decl));
        expect(verdict, `hiding declaration in \`${selector}\``).toEqual([]);
      }
    }
    // And the word is actually drawn at the type step the family owns.
    expect(declarations(layoutCss, '.session-state')['font-size']).toBe('var(--fs-tiny)');
  });

  it('lets the phone session title shrink and truncate around the status word', () => {
    const wrapper = ruleBody(sessionControl, '.cp-session-id');
    expect(wrapper).toContain('min-width: 0');
    expect(wrapper).toContain('flex: 1');
    const title = declarations(sessionControl, '.cp-session-id h2');
    expect(title['overflow']).toBe('hidden');
    expect(title['text-overflow']).toBe('ellipsis');
    expect(title['white-space']).toBe('nowrap');
  });

  it('sizes the state from the type scale', () => {
    // The table row is a one-line rule; it keeps the same step.
    expect(sessionControl).toMatch(
      /\.cp-table-row \.session-state \{[^}]*font-size: var\(--fs-tiny\)/,
    );
  });
});

describe('agents card: a native details action, not a card-wide button', () => {
  it('gives the details action a 24px target, a token size and a focus ring', () => {
    const open = declarations(inspector, '.agent-card-open');
    expect(open['min-width']).toBe('24px');
    expect(open['min-height']).toBe('24px');
    expect(open['font-size']).toBe('var(--fs-tiny)');
    expect(inspector).toMatch(
      /\.agent-card-open:focus-visible \{\s*\n\s*outline: var\(--focus-ring-width\) solid var\(--focus-ring\)/,
    );
  });

  it('does not paint the card as a pointer target any more', () => {
    // The article was `role="button"` wrapping the Pause/Stop buttons. Its
    // cursor went with the role; the Details button owns that affordance now.
    expect(declarations(inspector, '.agent-card')).not.toHaveProperty('cursor');
  });
});

describe('project dropdown: a floating menu, not an in-flow disclosure', () => {
  it('anchors the panel under the trigger, out of the document flow', () => {
    // In flow it pushed Control plane, New session, the search and the session
    // list down the rail every time the pointer crossed the project title.
    const panel = declarations(workbenchNavigation, '.rail .workbench-projects-disclosure');
    expect(panel['position']).toBe('absolute');
    expect(panel['top']).toBe('calc(100% + var(--space-2))');
    expect(panel['left']).toBe('0');
    expect(panel['right']).toBe('0');
  });

  it('takes the switcher row as its containing block', () => {
    const head = declarations(
      workbenchNavigation,
      '.rail .project-switcher-desktop .project-switcher-head',
    );
    expect(head['position']).toBe('relative');
  });

  it('keeps the panel above the rail and its own menu above the panel', () => {
    // The options menu hangs off the kebab in the same row, directly over the
    // open panel; the keyboard can have both up at once.
    expect(
      declarations(workbenchNavigation, '.rail .workbench-projects-disclosure')['z-index'],
    ).toBe('20');
    expect(
      declarations(workbenchNavigation, '.rail .project-switcher-desktop .project-switcher-menu')[
        'z-index'
      ],
    ).toBe('30');
  });

  it('respects a reduced-motion preference', () => {
    const reduced = workbenchNavigation.slice(
      workbenchNavigation.indexOf('@media (prefers-reduced-motion: reduce)'),
    );
    expect(reduced).toMatch(/\.rail \.workbench-projects-disclosure \{\s*\n\s*animation: none;/);
  });
});

/* ---------------------------------------------------------------- matrix */

const DENSITIES = Object.keys(DENSITY_SCALES) as ThemeDensity[];
const MODES = new Set(BUILTIN_THEMES.map((theme) => theme.mode));

describe('the workflow roles hold in every theme, density and rounding', () => {
  const themes = BUILTIN_THEMES.map((theme) => resolveTheme(theme));

  it('covers dark and light, all three densities and both rounding ends', () => {
    expect(MODES).toEqual(new Set(['dark', 'light']));
    expect(DENSITIES).toEqual(['compact', 'comfortable', 'spacious']);
    expect(MIN_ROUNDING).toBe(0);
    expect(MAX_ROUNDING).toBe(22);
  });

  it('keeps the accent-on-tint ink at 4.5:1 on the accent tint in every theme', () => {
    // `--accent-on-tint` is `--accent-fill` mixed 22% toward primary text. The
    // todo pill and the running-agent badge are both that pair; Daybreak was
    // where a bare `--accent` on the tint measured 3.62:1.
    for (const theme of themes) {
      const onTint = mix(theme.palette.accentText, theme.palette.text, 0.22);
      expect(
        contrastRatio(onTint, theme.palette.accentBg),
        `${theme.id} (${theme.mode}): accent-on-tint on the accent tint`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('keeps the git panel’s metadata ink above 4.5:1 in every theme', () => {
    // `--muted` replaced `--faint` on the git content; the role is floored by
    // `applyInkFloor()` against the nearest surface, and this is the surfaces
    // that panel actually paints on.
    for (const theme of themes) {
      const palette = theme.palette;
      for (const [name, surface] of [
        ['surface', palette.surface],
        ['raised', palette.raised],
        ['inspector', palette.inspector],
        ['sidebar', palette.sidebar],
        ['bg', palette.bg],
        ['hover', palette.hover],
      ] as Array<[string, string]>) {
        expect(
          contrastRatio(palette.muted, surface),
          `${theme.id} (${theme.mode}): muted on ${name}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it.each(DENSITIES)('keeps every referenced token at %s density', (density) => {
    for (const rounding of [MIN_ROUNDING, MAX_ROUNDING]) {
      for (const theme of themes) {
        const values = buildTokenMap({ ...theme, density, rounding });
        const at = `${theme.id}/${density}/${rounding}`;
        // The panel sheets read these; a missing one renders an empty var().
        for (const token of ['--fs-tiny', '--fs-micro', '--theme-muted', '--space-2', '--space-6']) {
          expect(values[token], `${at} ${token}`).toBeTruthy();
        }
        // 11px is the floor at every density (§3.3).
        expect(values['--fs-tiny']).toBe('11px');
        // Shape follows the knob, all the way to square and to round.
        expect(Number.parseFloat(values['--radius-xs'] ?? ''), `${at} --radius-xs`).toBe(
          Math.max(0, rounding - 6),
        );
        // ...and rhythm follows the density scale.
        expect(Number.parseFloat(values['--space-2'] ?? ''), `${at} --space-2`).toBeCloseTo(
          Math.round(4 * DENSITY_SCALES[density].space * 100) / 100,
        );
      }
    }
  });
});