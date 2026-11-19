// @vitest-environment node
/* The cascade-layer contract.
 *
 * Before this file existed, nothing checked *where* a stylesheet's rules land in
 * the cascade. `tailwind.css` declared `theme, base, components, utilities` and
 * `base.css` put its element resets in `base`, but the other 21 sheets were
 * unlayered — and an unlayered rule beats every layer whatever its specificity.
 * So `class="button p-8"` kept `.button`'s padding: the utility compiled, shipped,
 * and lost. That is invisible in a unit test and invisible in a diff, which is
 * exactly why it needs one.
 *
 * `index.css` is the single place the decision is written down:
 *
 *   - `tokens.css` and `tailwind.css` stay unlayered (the first declares the
 *     variables, the second declares the layer order itself);
 *   - every component sheet is imported with `@import './x.css' layer(components)`,
 *     which puts its rules *inside* `components` without touching the file, so
 *     the rules keep their source order for ties and the static sheet tests
 *     (`scale.test.ts` and friends) still read rule text at column 0;
 *   - `base.css` nests its resets in `@layer base`, which becomes
 *     `components.base` — above Tailwind's Preflight, below the rest of the
 *     components layer, below `utilities`.
 *
 * A handful of sheets are imported by the component that owns them instead
 * (`Notifications`, the control plane). They are listed in
 * `OWNED_BY_A_COMPONENT` with the reason and a size assertion, so the unlayered
 * set cannot grow by accident: the next sheet either joins `index.css` or gets
 * an entry a reviewer has to read.
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const DIR = new URL('.', import.meta.url).pathname;
const SRC = new URL('../', import.meta.url).pathname;

const read = (name: string): string => readFileSync(DIR + name, 'utf8');
const indexCss = read('index.css');

/**
 * Sheets a component imports itself, and why they are not in `index.css`.
 *
 * They are unlayered today, so a utility still cannot override them — that is
 * the remaining gap of the layer migration, recorded here rather than hidden.
 * Moving them means deciding that the control plane's rules may lose a tie to
 * `layout.css` (they are emitted *before* `index.css` today), which is its own
 * change with its own visual verification.
 */
const OWNED_BY_A_COMPONENT = new Map<string, string>([
  ['notifications.css', 'imported by components/NotificationCenter.tsx'],
  ['workbench-navigation.css', 'imported by components/SessionsRail.tsx'],
  ['control-shared.css', 'imported by components/control-plane/ControlPlane.tsx'],
  ['control-rail.css', 'imported by components/control-plane/ControlRail.tsx'],
  ['control-overview.css', 'imported by components/control-plane/ControlOverview.tsx'],
  ['control-session.css', 'imported by components/control-plane/ControlSession.tsx'],
]);

interface ImportStatement {
  file: string;
  layer: string | null;
}

/** Every `@import './x.css'` in `index.css`, in order, with its layer if any. */
function indexImports(css: string): ImportStatement[] {
  return [...css.matchAll(/@import\s+'\.\/([\w.-]+\.css)'(?:\s+layer\(([\w.-]+)\))?/g)].map(
    (match) => ({ file: match[1]!, layer: match[2] ?? null }),
  );
}

/** Every stylesheet in `src/styles`. */
function sheets(): string[] {
  return readdirSync(DIR)
    .filter((name) => name.endsWith('.css'))
    .sort();
}

/** The entry sheet itself: `main.tsx` imports it, and it imports the rest. */
const ENTRY = 'index.css';

/** Stylesheets imported from a module (`import '../styles/x.css'`). */
function componentImports(): Set<string> {
  const found = new Set<string>();
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      if (name === 'node_modules' || name.startsWith('.')) continue;
      const path = `${dir}${name}`;
      if (statSync(path).isDirectory()) {
        walk(`${path}/`);
        continue;
      }
      if (!/\.(ts|tsx)$/.test(name) || name.includes('.test.')) continue;
      for (const match of readFileSync(path, 'utf8').matchAll(/styles\/([\w.-]+\.css)/g)) {
        // The entry is imported by `main.tsx` by design and is not a owner-import.
        if (match[1] === ENTRY) continue;
        found.add(match[1]!);
      }
    }
  };
  walk(SRC);
  return found;
}

describe('index.css decides the layer of every sheet it imports', () => {
  const imports = indexImports(indexCss);

  it('imports the token layer and Tailwind unlayered, first', () => {
    // `tokens.css` is the variable surface, not a component; `tailwind.css`
    // declares `theme, base, components, utilities` and puts Tailwind's own
    // output in the first three. Wrapping either in `layer(components)` would
    // either bury the variables under the component sheets or re-order the
    // layer list Tailwind publishes.
    expect(imports[0]).toEqual({ file: 'tokens.css', layer: null });
    expect(imports[1]).toEqual({ file: 'tailwind.css', layer: null });
  });

  it('puts every other sheet in the components layer', () => {
    const unlayered = imports.filter((entry) => entry.layer !== 'components').slice(2);
    expect(
      unlayered.map((entry) => entry.file),
      'a sheet imported without layer(components) is stronger than every utility',
    ).toEqual([]);
  });

  it('imports every sheet it names and nothing that does not exist', () => {
    for (const entry of imports) expect(sheets(), entry.file).toContain(entry.file);
    // An import that no file backs is a build error, not a style choice.
    expect(new Set(imports.map((entry) => entry.file)).size).toBe(imports.length);
  });
});

describe('the sheets outside index.css are named, not forgotten', () => {
  it('accounts for every stylesheet in src/styles', () => {
    const imported = new Set([...indexImports(indexCss).map((entry) => entry.file), ENTRY]);
    const unaccounted = sheets().filter((name) => !imported.has(name) && !OWNED_BY_A_COMPONENT.has(name));
    expect(
      unaccounted,
      'a sheet that is neither imported by index.css nor listed in OWNED_BY_A_COMPONENT',
    ).toEqual([]);
  });

  it('lists exactly the sheets a component imports itself', () => {
    const imported = new Set(indexImports(indexCss).map((entry) => entry.file));
    const fromComponents = [...componentImports()].filter((name) => !imported.has(name)).sort();
    expect([...OWNED_BY_A_COMPONENT.keys()].sort()).toEqual(fromComponents);
  });

  it('keeps the unlayered set small and gives each entry a reason', () => {
    // An unlayered sheet outranks the utilities layer, so every entry is a place
    // a `p-*`/`text-*` utility silently does nothing. It is debt with a name.
    expect(OWNED_BY_A_COMPONENT.size).toBeLessThanOrEqual(6);
    for (const [file, reason] of OWNED_BY_A_COMPONENT) {
      expect(reason.length, `${file} has a reason`).toBeGreaterThan(20);
      expect(read(file).length, `${file} exists`).toBeGreaterThan(0);
    }
  });
});

describe('the layers themselves are declared where they are documented', () => {
  it('keeps the token layer out of every layer', () => {
    // Custom properties are not subject to the layer order in a useful way here,
    // and the theme engine rewrites `--theme-*` on `:root`; a layer would only
    // make the one file that must always win depend on where it sits.
    expect(read('tokens.css')).not.toMatch(/@layer/);
  });

  it('keeps the element resets in a nested base layer inside base.css', () => {
    // `@import './base.css' layer(components)` wraps the file, so this block
    // becomes `components.base`: above Tailwind's Preflight, below the rest of
    // the components layer (a class rule still beats a reset), below utilities.
    expect(read('base.css')).toMatch(/^@layer base \{/m);
  });

  it('documents the order Tailwind declares', () => {
    // The order comes from Tailwind's own output; what this asserts is that the
    // sheets are imported *after* the file that declares it, so `components`
    // already exists when the first `layer(components)` import lands.
    const imports = indexImports(indexCss);
    expect(imports.findIndex((entry) => entry.file === 'tailwind.css')).toBeLessThan(
      imports.findIndex((entry) => entry.layer === 'components'),
    );
  });
});
