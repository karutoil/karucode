// @vitest-environment node
/* Regenerate the first-paint token block in `src/styles/tokens.css`.
 *
 * The stylesheet paints one frame before `initTheme()` runs, so it carries a copy
 * of the default theme's resolved values. A copy drifts, so it is not maintained
 * by hand: `roles.test.ts` fails the moment it disagrees with `resolveRoles()`,
 * and this generator is how you fix that failure.
 *
 *     EMIT_FIRST_PAINT=1 npx vitest run src/theme/first-paint.emit.test.ts
 *
 * It is env-gated rather than always-on because a test that rewrites a source file
 * on every run would hide exactly the drift it exists to catch.
 */

import { writeFileSync, readFileSync } from 'node:fs';
import { it } from 'vitest';
import { DEFAULT_THEME } from './builtins';
import { resolveRoles } from './roles';
import { resolveTheme } from './tokens';

const TARGET = 'src/styles/tokens.css';
const START = '  /* ---- surfaces';
const END = '  /* ===================================================================== */\n  /* Public aliases';

function block(): string {
  const theme = resolveTheme(DEFAULT_THEME);
  const p = theme.palette;
  const out: string[] = [];

  out.push('  /* ---- surfaces -------------------------------------------------------- */');
  const surfaces: Array<[string, string, string]> = [
    ['bg', p.bg, 'app canvas'],
    ['rail', p.rail, 'project rail'],
    ['sidebar', p.sidebar, 'session sidebar'],
    ['inspector', p.inspector, 'right inspector'],
    ['surface', p.surface, 'cards, panels, inputs'],
    ['raised', p.raised, 'elevated surface'],
    ['hover', p.hover, 'hover surface'],
  ];
  for (const [key, value, note] of surfaces) out.push(`  --theme-${key}: ${value}; /* ${note} */`);

  out.push('', '  /* ---- lines ----------------------------------------------------------- */');
  out.push(`  --theme-line: ${p.line};`);
  out.push(`  --theme-line-soft: ${p.lineSoft};`);
  out.push(`  --theme-line-strong: ${p.lineStrong};`);

  out.push('', '  /* ---- text ------------------------------------------------------------ */');
  out.push(`  --theme-text: ${p.text};`);
  out.push(`  --theme-text-secondary: ${p.textSecondary};`);
  out.push(`  --theme-muted: ${p.muted};`);
  out.push(`  --theme-faint: ${p.faint};`);

  out.push('', '  /* ---- accents & status ------------------------------------------------ */');
  out.push(`  --theme-accent: ${p.accent};`);
  out.push(`  --theme-accent-hi: ${p.accentHi};`);
  out.push(`  --theme-accent-bg: ${p.accentBg};`);
  out.push(`  --theme-accent-line: ${p.accentLine};`);
  out.push(`  --theme-accent-text: ${p.accentText}; /* accent as ink */`);
  out.push(`  --theme-on-accent: ${p.onAccent}; /* ink on an accent fill */`);
  out.push(`  --theme-success: ${p.success};`);
  out.push(`  --theme-success-bg: ${p.successBg};`);
  out.push(`  --theme-warn: ${p.warn};`);
  out.push(`  --theme-danger: ${p.danger};`);
  out.push(`  --theme-danger-bg: ${p.dangerBg};`);
  out.push(`  --theme-info: ${p.info};`);

  out.push('', '  /* ---- code ------------------------------------------------------------ */');
  out.push(`  --theme-syntax-keyword: ${p.syntaxKeyword};`);
  out.push(`  --theme-syntax-string: ${p.syntaxString};`);
  out.push(`  --theme-syntax-comment: ${p.syntaxComment};`);
  out.push(`  --theme-shadow: ${p.shadow};`);

  out.push(
    '',
    '  /* ---- fidelity roles --------------------------------------------------',
    '   * Generated — do not hand-edit. Run:',
    '   *   EMIT_FIRST_PAINT=1 npx vitest run src/theme/first-paint.emit.test.ts',
    '   * `roles.test.ts` fails if this block drifts from the resolver. */',
  );
  for (const [property, value] of Object.entries(resolveRoles(theme))) {
    out.push(`  ${property}: ${value};`);
  }
  out.push('');
  return out.join('\n');
}

it('regenerates the first-paint block when asked', () => {
  if (!process.env.EMIT_FIRST_PAINT) return;
  const css = readFileSync(TARGET, 'utf8');
  const start = css.indexOf(START);
  const end = css.indexOf(END);
  if (start < 0 || end < 0) throw new Error('cannot find the first-paint block in tokens.css');
  writeFileSync(TARGET, css.slice(0, start) + block() + '\n' + css.slice(end));
});