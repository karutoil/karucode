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
 * A few sheets used to be imported by the component that owns them instead
 * (`Notifications`, the sessions rail, the control plane). Those imports are gone:
 * every sheet now goes through `index.css`, and `OWNED_BY_A_COMPONENT` below is
 * the empty list of exceptions a reviewer would have to read before that changes
 * back. The test that follows is the strict form of the rule — every stylesheet in
 * `src/styles/` except the three named in `ENTRY_FILES` must appear in `index.css`
 * with `layer(components)`.
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
 * Empty on purpose, and kept rather than deleted: it is the named exception list
 * the "every stylesheet is layered" rule below allows, so the next sheet that
 * wants to skip `index.css` has to add an entry here - a diff a reviewer reads -
 * instead of quietly importing CSS from a component and landing outside every
 * layer. `notifications.css`, `workbench-navigation.css` and the four
 * `control-*.css` were the last six entries; they are in `index.css` now.
 */
const OWNED_BY_A_COMPONENT = new Map<string, string>();

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

/**
 * The three files the "every sheet is layered" rule cannot apply to: the entry
 * that does the importing, and the two sheets it imports unlayered on purpose.
 */
const ENTRY_FILES = new Set(['tokens.css', 'tailwind.css', ENTRY]);

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

  it('imports every stylesheet in src/styles with layer(components)', () => {
    // The rule, in full: no stylesheet may live outside the cascade order. The
    // three entry files are the exceptions the format itself forces — `tokens.css`
    // and `tailwind.css` are imported unlayered on purpose (see the first test in
    // this file) and `index.css` is the entry that does the importing.
    const entries = indexImports(indexCss);
    const imported = new Map(entries.map((entry) => [entry.file, entry.layer]));
    for (const file of sheets()) {
      if (ENTRY_FILES.has(file)) continue;
      expect(imported.has(file), `${file} is imported by index.css`).toBe(true);
      expect(imported.get(file), `${file} is imported with layer(components)`).toBe('components');
    }
  });

  it('has no unlayered exception left to declare', () => {
    // An unlayered sheet outranks the utilities layer, so every entry in the map
    // above is a place a `p-*`/`text-*` utility silently does nothing. The list is
    // empty: a new entry is a decision, not a default.
    expect(OWNED_BY_A_COMPONENT.size).toBe(0);
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
