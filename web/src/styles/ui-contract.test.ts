// @vitest-environment node
/* The utility layer's static contract.
 *
 * `styles/tailwind.css` maps Tailwind onto karucode's tokens and resets every
 * default scale, so the *only* values a utility can carry are the design
 * system's. Nothing enforced that on the markup side until this file: a
 * `bg-[#1a1a1a]`, a `text-[13px]` or a `bg-zinc-800` in a TSX file compiled,
 * looked plausible, and quietly reintroduced the second palette that
 * `DESIGN-SYSTEM.md` exists to prevent. Those are also the values that survive
 * a Theme Studio change untouched, which is exactly the bug.
 *
 * So this is the guard, not a style opinion:
 *
 *   1. no Tailwind arbitrary value in a class, except pure geometry
 *      (`w-[...]`, `grid-cols-[...]`) where the design system has no step;
 *   2. no `text-faint` — `--faint` is a decorative object colour, not an ink;
 *   3. no default Tailwind palette (`bg-zinc-800`, `text-red-400`, …);
 *   4. every token `tailwind.css` maps must exist in `tokens.css`, so a rename
 *      there cannot silently blank a utility.
 *
 * It reads the source as text, the same way `scale.test.ts` reads the sheets:
 * these are compile-time facts about the tree, not runtime behaviour.
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const STYLES = new URL('.', import.meta.url).pathname;
const SRC = new URL('../', import.meta.url).pathname;

/** Directories whose files are not markup: tests, and the theme engine's data. */
const SKIP_DIRS = new Set(['theme', 'node_modules']);

function isTestFile(name: string): boolean {
  return name.includes('.test.');
}

function sources(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (SKIP_DIRS.has(name)) continue;
    const path = `${dir}${name}`;
    if (statSync(path).isDirectory()) {
      sources(`${path}/`, out);
      continue;
    }
    if (!/\.(ts|tsx)$/.test(name) || isTestFile(name)) continue;
    out.push(path);
  }
  return out.sort();
}

/**
 * Blank `//` and `/* *\/` comments without moving any character.
 *
 * The scanner below looks for class attributes, and a comment is not markup —
 * but the prose in this repository does quote CSS (`class="button"` appears in
 * more than one explanation). Blanking keeps line numbers and offsets intact so
 * a reported line still points at the real one.
 */
function maskComments(source: string): string {
  const out = source.split('');
  let quote: string | null = null;
  let line = false;
  let block = false;

  for (let i = 0; i < source.length; i += 1) {
    const char = source[i]!;
    const next = source[i + 1];
    if (line) {
      if (char === '\n') line = false;
      else out[i] = ' ';
      continue;
    }
    if (block) {
      if (char === '*' && next === '/') {
        out[i] = ' ';
        out[i + 1] = ' ';
        i += 1;
        block = false;
      } else if (char !== '\n') {
        out[i] = ' ';
      }
      continue;
    }
    if (quote) {
      if (char === '\\') i += 1;
      else if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === '`') quote = char;
    else if (char === '/' && next === '/') {
      out[i] = ' ';
      out[i + 1] = ' ';
      i += 1;
      line = true;
    } else if (char === '/' && next === '*') {
      out[i] = ' ';
      out[i + 1] = ' ';
      i += 1;
      block = true;
    }
  }
  return out.join('');
}

/** One class expression found in a source file. */
interface ClassExpression {
  line: number;
  text: string;
}

const QUOTED = String.raw`"([^"\\]*)"|'([^'\\]*)'`;
const CLASS_ATTRIBUTE = new RegExp(
  String.raw`\b(?:class|className)\s*=\s*(?:${QUOTED}|\{\s*(?:${QUOTED}|\x60([^\x60]*)\x60)\s*\})`,
  'g',
);
/** `variants('…')` — the slot helper a component library tends to grow. */
const VARIANTS_CALL = /\bvariants\s*\(([\s\S]*?)\)\s*[,;)]/g;
const STRING_LITERAL = new RegExp(String.raw`${QUOTED}|\x60([^\x60]*)\x60`, 'g');

function lineAt(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index; i += 1) if (source[i] === '\n') line += 1;
  return line;
}

/** Every literal `class`/`className` value and `variants(...)` argument string. */
export function classExpressions(source: string): ClassExpression[] {
  const masked = maskComments(source);
  const found: ClassExpression[] = [];

  for (const match of masked.matchAll(CLASS_ATTRIBUTE)) {
    const text = [match[1], match[2], match[3], match[4], match[5]].find((part) => part !== undefined);
    if (text === undefined) continue;
    found.push({ line: lineAt(masked, match.index ?? 0), text });
  }

  for (const call of masked.matchAll(VARIANTS_CALL)) {
    const body = call[1] ?? '';
    const base = (call.index ?? 0) + call[0].indexOf(body);
    for (const literal of body.matchAll(STRING_LITERAL)) {
      const text = [literal[1], literal[2], literal[3]].find((part) => part !== undefined);
      if (text === undefined) continue;
      found.push({ line: lineAt(masked, base + (literal.index ?? 0)), text });
    }
  }

  return found;
}

/**
 * Utility prefixes whose arbitrary value is a *measurement*, not a colour or a
 * scale step: the design system has no token for "this grid cell is 320px", and
 * inventing one per call site would be worse than the bracket.
 */
const GEOMETRY_PREFIXES = [
  'w-',
  'h-',
  'min-w-',
  'min-h-',
  'max-w-',
  'max-h-',
  'size-',
  'basis-',
  'grid-cols-',
  'grid-rows-',
  'top-',
  'right-',
  'bottom-',
  'left-',
  'inset-',
  'translate-x-',
  'translate-y-',
  'z-',
];

/** `bg-[#fff]`, `text-[rgb(0_0_0/50%)]`, `border-[hsl(0_0%_0%)]`. */
const ARBITRARY_COLOUR = /\[(?:#[0-9a-fA-F]{3,8}|rgba?\(|hsla?\()/;

/** The whole Tailwind palette: `bg-zinc-800`, `text-gray-400`, `border-red-500/40`. */
const PALETTE =
  /\b(?:bg|text|border|border-[xytrbl]|divide|ring|ring-offset|outline|fill|stroke|from|via|to|decoration|shadow|accent|caret)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(?:50|[1-9]00|950)(?:\/\d+)?\b/;

/** One reason a class expression is refused. */
interface Violation {
  line: number;
  token: string;
  why: string;
}

/** Split a class expression into utility tokens, ignoring interpolations. */
function tokens(text: string): string[] {
  return text
    .split(/\s+/)
    .map((token) => token.trim())
    .filter(Boolean);
}

export function violationsIn({ line, text }: ClassExpression): Violation[] {
  const out: Violation[] = [];
  for (const token of tokens(text)) {
    if (token.includes('${')) continue;
    const bare = token.replace(/^[a-z-]+:/g, '');

    if (ARBITRARY_COLOUR.test(bare)) {
      out.push({ line, token, why: 'hand-picked colour — use a token utility (bg-surface, text-muted, …)' });
      continue;
    }
    if (bare.includes('[') && !GEOMETRY_PREFIXES.some((prefix) => bare.startsWith(prefix))) {
      out.push({ line, token, why: 'arbitrary value — use the token scale, or a geometry prefix if it is a measurement' });
      continue;
    }
    if (bare === 'text-faint' || bare.startsWith('text-faint/')) {
      out.push({ line, token, why: '`--faint` is a decorative object colour, not readable ink — use text-muted' });
      continue;
    }
    if (PALETTE.test(bare)) {
      out.push({ line, token, why: 'default Tailwind palette — the token scales are reset, so this is a typo the build cannot see' });
    }
  }
  return out;
}

function scanTree(): { file: string; violations: Violation[] }[] {
  const report: { file: string; violations: Violation[] }[] = [];
  for (const file of sources(SRC)) {
    const source = readFileSync(file, 'utf8');
    const violations = classExpressions(source).flatMap(violationsIn);
    if (violations.length > 0) report.push({ file: file.replace(SRC, 'src/'), violations });
  }
  return report;
}

const describe_ = (report: { file: string; violations: Violation[] }[]): string =>
  report
    .flatMap(({ file, violations }) => violations.map((v) => `${file}:${v.line}  ${v.token}  — ${v.why}`))
    .join('\n');

describe('the scanner reads what it claims to', () => {
  it('finds literal, braced and template class attributes with their line', () => {
    const source = [
      '<div class="a b" />', // 1
      '<div class="c" />', // 2
      '<div class={`d ${e}`} />', // 3
      '<div class={"f"} />', // 4
    ].join('\n');
    expect(classExpressions(source).map((found) => found.text)).toEqual(['a b', 'c', 'd ${e}', 'f']);
    expect(classExpressions(source)[2]?.line).toBe(3);
  });

  it('ignores a class attribute quoted in a comment', () => {
    const source = ['/* the mock had class="bg-zinc-800" here */', '<div class="bg-surface" />'].join('\n');
    expect(classExpressions(source)).toEqual([{ line: 2, text: 'bg-surface' }]);
  });

  it('reads variants(...) string arguments too', () => {
    const source = ["const v = variants('bg-surface', { tone: 'text-red-400' });"].join('\n');
    expect(classExpressions(source).map((found) => found.text)).toEqual(['bg-surface', 'text-red-400']);
  });
});

describe('no arbitrary colour or type size in markup', () => {
  it('refuses a hand-picked colour and an arbitrary length', () => {
    const found = [
      ...violationsIn({ line: 1, text: 'bg-[#1a1a1a]' }),
      ...violationsIn({ line: 1, text: 'text-[13px]' }),
      ...violationsIn({ line: 1, text: 'p-[6px] rounded-[3px] gap-[5px]' }),
      ...violationsIn({ line: 1, text: 'border-[rgb(0_0_0)]' }),
    ];
    expect(found).toHaveLength(6);
  });

  it('leaves a geometry value alone, because the scale has no step for it', () => {
    expect(violationsIn({ line: 1, text: 'w-[320px] h-[calc(100%-2rem)] max-w-[720px] grid-cols-[auto_1fr]' })).toEqual([]);
  });

  it('accepts the token utilities it is supposed to accept', () => {
    expect(
      violationsIn({
        line: 1,
        text: 'bg-surface text-muted p-4 gap-2 rounded-sm border border-line text-body font-mono md:p-6 hover:bg-hover',
      }),
    ).toEqual([]);
  });
});

describe('no unreadable or off-palette utility', () => {
  it('refuses text-faint', () => {
    expect(violationsIn({ line: 1, text: 'text-faint' })).toHaveLength(1);
    expect(violationsIn({ line: 1, text: 'text-faint/70' })).toHaveLength(1);
  });

  it('refuses the default Tailwind palette in every shape it is written', () => {
    for (const text of ['bg-zinc-800', 'text-gray-300', 'bg-red-500', 'text-amber-400', 'border-neutral-700', 'ring-blue-500', 'bg-sky-400/40', 'dark:bg-slate-900']) {
      expect(violationsIn({ line: 1, text }), text).toHaveLength(1);
    }
  });

  it('does not mistake a karucode class for a palette class', () => {
    expect(violationsIn({ line: 1, text: 'status-danger badge-warn text-accent-text bg-accent-bg border-line-strong text-on-accent' })).toEqual([]);
  });
});

describe('the tree holds the contract', () => {
  it('has no arbitrary colour, type size, unreadable ink or palette class in src', () => {
    const report = scanTree();
    expect(report, `forbidden utilities:\n${describe_(report)}`).toEqual([]);
  });

  it('scans the app, not an empty set', () => {
    // A silent walk (bad path, renamed directory) would pass every rule above.
    expect(sources(SRC).length).toBeGreaterThan(50);
    const expressions = sources(SRC).reduce((total, file) => total + classExpressions(readFileSync(file, 'utf8')).length, 0);
    expect(expressions).toBeGreaterThan(200);
  });
});

describe('the theme mapping names real tokens', () => {
  const tailwind = readFileSync(`${STYLES}tailwind.css`, 'utf8');
  const tokens = readFileSync(`${STYLES}tokens.css`, 'utf8');

  it('maps a colour, spacing, radius, type and font scale', () => {
    for (const name of ['--color-bg', '--color-surface', '--color-accent-fill', '--color-on-accent']) {
      expect(tailwind, name).toContain(`${name}:`);
    }
    for (const name of ['--spacing-1', '--spacing-10', '--radius-sm', '--radius-pill', '--text-body', '--text-hero', '--font-ui', '--font-mono']) {
      expect(tailwind, name).toContain(`${name}:`);
    }
  });

  it('resets the default scales, so a default utility cannot resolve', () => {
    for (const reset of ['--color-*: initial', '--spacing-*: initial', '--radius-*: initial', '--text-*: initial', '--font-*: initial', '--shadow-*: initial']) {
      expect(tailwind, reset).toContain(reset);
    }
  });

  it('exposes no text colour for the decorative faint ink', () => {
    expect(tailwind).not.toMatch(/--color-faint:/);
  });

  it('points every mapped token at a variable tokens.css defines', () => {
    const missing: string[] = [];
    for (const match of tailwind.matchAll(/var\(--([a-z0-9-]+)\)/g)) {
      const name = match[1]!;
      if (!new RegExp(`--${name}\\s*:`).test(tokens)) missing.push(name);
    }
    expect([...new Set(missing)], 'tokens mapped by tailwind.css but not defined in tokens.css').toEqual([]);
  });
});
