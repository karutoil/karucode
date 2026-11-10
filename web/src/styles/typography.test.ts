// @vitest-environment node
/* The type contract: a scale with a floor, and components that use it.
 *
 * This file used to assert that `base.css` and `layout.css` reproduced the v3
 * mock's font sizes exactly — 13px body, 11px breadcrumb, 14px main title,
 * `.button-small` at 11.5px, `.badge` at 10px, `.chip` at 10.5px. Individually
 * each looked reasonable. Collectively they were the problem: 25 distinct sizes
 * in the stylesheet, 220 declarations below 11px, and 30 of 85 text nodes below
 * WCAG AA on the default screen — a `Ctrl N` shortcut at 9.5px and 3.14:1, session
 * metadata at 9px and 3.41:1.
 *
 * Individual pixel assertions cannot catch that, because each one passes. So this
 * file asserts the *scale* instead: the steps exist, the floor is real, the
 * kicker has one definition, and the weights come from a small closed set.
 * `scale.test.ts` enforces that components use the tokens.
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

// `import.meta.url`, not `__dirname`: this suite is ESM, and the whole point of
// these files is to read the stylesheet they are next to.
const DIR = new URL('.', import.meta.url).pathname;
const read = (name: string): string => readFileSync(DIR + name, 'utf8');

const tokens = read('tokens.css');
const base = read('base.css');
const layout = read('layout.css');

const rule = (css: string, selector: string): string => {
  const start = css.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined`).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
};

describe('the scale exists and has a floor (§3.3)', () => {
  it.each([
    ['--fs-micro', '10px'],
    ['--fs-tiny', '11px'],
    ['--fs-small', '11.5px'],
    ['--fs-label', '12px'],
    ['--fs-message', '12.5px'],
    ['--fs-body', '13px'],
    ['--fs-lead', '14px'],
    ['--fs-title', '16px'],
    ['--fs-heading', '18px'],
    ['--fs-display', '20px'],
    ['--fs-hero', '24px'],
  ])('%s is %s', (property, value) => {
    expect(tokens).toMatch(new RegExp(`${property}:\\s*${value.replace('.', '\\.')}`));
  });

  it('declares nothing below the 10px floor', () => {
    // The whole point of the scale: no 9px, no 9.5px, anywhere in the token layer.
    const sizes = [...tokens.matchAll(/--fs-[a-z]+:\s*([\d.]+)px/g)].map((m) => Number(m[1]));
    expect(sizes.length).toBeGreaterThan(8);
    for (const size of sizes) expect(size).toBeGreaterThanOrEqual(10);
  });

  it('has one uppercase-kicker definition', () => {
    expect(tokens).toMatch(/--fs-kicker:\s*10px/);
    expect(tokens).toMatch(/--weight-kicker:\s*720/);
    expect(tokens).toMatch(/--tracking-kicker:\s*0\.125em/);
    const eyebrow = rule(base, '.eyebrow');
    expect(eyebrow).toMatch(/font-weight:\s*720/);
    expect(eyebrow).toMatch(/text-transform:\s*uppercase/);
    expect(eyebrow).toMatch(/letter-spacing:\s*0\.125em/);
  });
});

describe('the base layer uses the scale', () => {
  it('body copy takes size, leading and family from tokens', () => {
    const body = base.match(/\nbody\s*{[^}]+}/s)?.[0] ?? '';
    expect(body).toMatch(/font-size:\s*var\(--fs-body\)/);
    expect(body).toMatch(/line-height:\s*var\(--lh-body\)/);
    expect(body).toMatch(/font-family:\s*var\(--font-ui\)/);
  });

  it('ships the Inter stack and a mono stack with real fallbacks', () => {
    expect(tokens).toMatch(/--font-ui:/);
    expect(tokens).toMatch(/Inter/);
    expect(tokens).toMatch(/ui-sans-serif/);
    expect(tokens).toMatch(/--font-mono:/);
    expect(tokens).toMatch(/SFMono-Regular/);
    expect(tokens).toMatch(/Consolas/);
  });

  it('headings are a single closed definition', () => {
    const headings = base.match(/\nh1,\s*h2,\s*h3,\s*h4\s*{[^}]+}/s)?.[0] ?? '';
    expect(headings).toMatch(/font-weight:\s*660/);
    expect(headings).toMatch(/line-height:\s*1\.3/);
    expect(headings).toMatch(/letter-spacing:\s*-0\.02em/);
  });

  it('code, kbd and pre share the mono family', () => {
    const mono = base.match(/\ncode,\s*kbd,\s*pre,\s*samp\s*{[^}]+}/s)?.[0] ?? '';
    expect(mono).toMatch(/font-family:\s*var\(--font-mono\)/);
  });
});

describe('the bundled typefaces are declared honestly', () => {
  /* `/fonts/*.ttf` are static instances: Inter Regular (usWeightClass 400), Inter
   * Bold (700) and JetBrains Mono Regular (400). None of them has an `fvar`
   * table, so a descriptor like `font-weight: 400 600` promises a range the file
   * cannot render — the browser then resolves weight 600 to the 400 face and
   * synthesises it, which is what made headings look smeared rather than bold. */
  const faces = [...base.matchAll(/@font-face\s*\{([^}]+)\}/g)].map((match) => match[1] ?? '');

  it('declares one face per shipped weight, each with a single weight value', () => {
    expect(faces).toHaveLength(3);
    for (const face of faces) {
      const weight = /font-weight:\s*([^;]+);/.exec(face)?.[1]?.trim() ?? '';
      expect(weight, face).toMatch(/^\d{3}$/);
    }
    // The two Inter files are the regular and the bold, and nothing else.
    const weights = faces
      .filter((face) => face.includes("font-family: 'Inter'"))
      .map((face) => /font-weight:\s*(\d+);/.exec(face)?.[1]);
    expect(weights.sort()).toEqual(['400', '700']);
  });

  it('serves every face from the app itself, with no third-party request', () => {
    for (const face of faces) {
      expect(face, face).toMatch(/src:\s*url\('\/fonts\/[A-Za-z-]+\.ttf'\) format\('truetype'\)/);
    }
    // No protocol-relative or absolute URL anywhere in the base layer: the app
    // makes no font request off-origin, and `web/README.md` says so.
    expect(base).not.toMatch(/url\(\s*['"]?(?:https?:)?\/\//);
  });
});

describe('weights are a closed set, not one-offs', () => {
  // DESIGN-SYSTEM.md §3.3: {400, 500, 550, 600, 660, 700, 720, 800}. The shell
  // had drifted into 540, 560, 620, 650, 670, 680, 690, 710 and 740 as well —
  // nine one-offs that read as rounding errors rather than a hierarchy.
  const ALLOWED = new Set(['400', '500', '550', '600', '660', '700', '720', '800', 'bold', 'normal', 'bolder', 'lighter']);

  it.each(['base.css', 'layout.css', 'chat.css', 'composer.css', 'rail.css', 'inspector.css'])(
    '%s uses only scale weights',
    (file) => {
      const css = read(file);
      const weights = [...css.matchAll(/font-weight:\s*([a-z\d]+)/g)].map((m) => m[1]!);
      const offenders = [...new Set(weights)].filter((w) => !ALLOWED.has(w));
      expect(offenders, `${file} off-scale weights`).toEqual([]);
    },
  );
});

describe('the shell titles use the scale', () => {
  it('.main-title is a step, not a literal', () => {
    expect(rule(layout, '.main-title')).toMatch(/font-size:\s*var\(--fs-/);
  });
});
describe('ordinary shell metadata is tiny, not micro', () => {
  // The scale reserves `--fs-micro` for "uppercase kickers and badges only"
  // (tokens.css §3.3). Six shell rules were using it for lowercase metadata a
  // reader scans — the project path, the context meter's label, the provider
  // name and the empty rail — so the quietest text in the chrome sat on the 10px
  // floor instead of the 11px metadata step. Kickers (`.head-activity`,
  // `.session-group-label`), badges (`.badge`, `.session-unread`, `.tab-count`)
  // and keycaps stay micro.
  const rail = read('rail.css');

  it.each([
    ['layout.css', layout, '.path-hint'],
    ['layout.css', layout, '.side-meter-top'],
    ['layout.css', layout, '.settings-row .provider'],
    ['rail.css', rail, '.project-rail-empty'],
  ])('%s %s uses --fs-tiny', (_file, css, selector) => {
    expect(rule(css, selector)).toMatch(/font-size:\s*var\(--fs-tiny\)/);
  });

  it('leaves the uppercase kickers and badges on the micro step', () => {
    expect(rule(layout, '.head-activity')).toMatch(/font-size:\s*var\(--fs-micro\)/);
    expect(rule(layout, '.session-group-label')).toMatch(/font-size:\s*var\(--fs-micro\)/);
    expect(rule(layout, '.session-unread')).toMatch(/font-size:\s*var\(--fs-micro\)/);
    expect(rule(rail, '.project-rail-avatar')).toMatch(/font-size:\s*var\(--fs-micro\)/);
  });
});
