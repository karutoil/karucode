// @vitest-environment node
/* Stylesheet fidelity contract for the Theme Studio drawer.
 *
 * The reference is `karucode-v3-themes.html` (Theme Studio block at the end of
 * its stylesheet). Vitest runs component tests with `css: false`, so computed
 * styles cannot be asserted there; this suite reads `theme-studio.css` itself
 * and pins the geometry, colours and breakpoints the drawer must keep.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const cssSource = readFileSync(new URL('./theme-studio.css', import.meta.url), 'utf8');

/** Declarations of a single, non-nested rule in `theme-studio.css`. */
function cssDeclarations(selector: string): Record<string, string> {
  const start = cssSource.indexOf(`\n${selector} {`);
  expect(start, `${selector} is defined in theme-studio.css`).toBeGreaterThan(-1);
  const end = cssSource.indexOf('\n}', start);
  const declarations: Record<string, string> = {};
  for (const line of cssSource.slice(start, end).split('\n')) {
    const match = /^\s*([a-z-]+):\s*(.+);\s*$/.exec(line);
    if (match?.[1] !== undefined && match[2] !== undefined) declarations[match[1]] = match[2];
  }
  return declarations;
}

describe('theme studio stylesheet fidelity', () => {
  it('keeps the drawer on the mock line rhythm and a neutral scrim', () => {
    // `html,body` in the mock is 13px/1.5 and the drawer inherits it.
    expect(cssDeclarations('.theme-studio')['line-height']).toBe('1.5');
    expect(cssDeclarations('.theme-studio')['font-size']).toBe('var(--fs-body)');
    // base.css tightens headings to 1.3; drawer headings stay on 1.5.
    expect(cssSource).toMatch(/\.theme-studio :is\(h1, h2, h3, h4\) \{\s*line-height: 1\.5;/);
    // The scrim dims the app (mock: #0007) instead of tinting it with --theme-bg.
    // One scrim and one blur for the whole overlay family, defined once in
    // overlays.css and mode-derived there (DESIGN-AUDIT overlays B2).
    expect(cssDeclarations('.theme-shade')['background']).toBe('var(--overlay-scrim)');
    expect(cssDeclarations('.theme-shade')['backdrop-filter']).toBe('var(--overlay-blur)');
  });

  it('renders secondary copy with the muted role', () => {
    for (const selector of [
      '.theme-studio-subtitle',
      '.theme-description',
      '.theme-preset-mode',
      '.theme-color-hex',
      '.theme-advanced-help',
      '.theme-code-hint',
      '.theme-autosave',
      '.theme-contrast',
    ]) {
      expect(cssDeclarations(selector)['color'], selector).toBe('var(--muted)');
    }
    // Primary labels keep the brighter secondary-text role (mock `--text2`).
    expect(cssDeclarations('.theme-color-name')['color']).toBe('var(--text-secondary)');
    expect(cssDeclarations('.theme-field-label')['color']).toBe('var(--text-secondary)');
  });

  it('matches the mock header and scroll geometry', () => {
    expect(cssDeclarations('.theme-studio')['width']).toBe('min(580px, calc(100% - 58px))');
    expect(cssDeclarations('.theme-studio-top')['padding']).toBe(
      'var(--space-9) var(--space-10) var(--space-9)',
    );
    expect(cssDeclarations('.theme-studio-top')['gap']).toBe('var(--space-7)');
    expect(cssDeclarations('.theme-studio-title')['font-size']).toBe('var(--fs-hero)');
    expect(cssDeclarations('.theme-studio-title')['margin']).toBe('var(--space-4) 0 var(--space-2)');
    expect(cssDeclarations('.theme-studio-subtitle')['font-size']).toBe('var(--fs-small)');
    expect(cssDeclarations('.theme-close')['width']).toBe('31px');
    expect(cssDeclarations('.theme-close')['height']).toBe('31px');

    expect(cssDeclarations('.theme-studio-scroll')['padding']).toBe(
      '0 var(--space-10) var(--space-9)',
    );
    expect(cssDeclarations('.theme-section')['padding']).toBe(
      'var(--space-9) 0 var(--space-9)',
    );
    expect(cssDeclarations('.theme-section-title')['margin-bottom']).toBe('var(--space-3)');
    expect(cssDeclarations('.theme-description')['margin']).toBe('0 0 var(--space-7)');
    expect(cssDeclarations('.theme-description')['font-size']).toBe('var(--fs-tiny)');
  });

  it('matches the mock gallery geometry', () => {
    expect(cssDeclarations('.theme-gallery')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-gallery')['gap']).toBe('var(--space-5)');
    expect(cssDeclarations('.theme-preset')['padding']).toBe('var(--space-5)');
    expect(cssDeclarations('.theme-preset')['border-radius']).toBe('var(--radius)');
    expect(cssDeclarations('.theme-preset-demo')['height']).toBe('62px');
    expect(cssDeclarations('.theme-mini-rail')['width']).toBe('12%');
    expect(cssDeclarations('.theme-mini-sidebar')['width']).toBe('24%');
    expect(cssDeclarations('.theme-mini-accent')['left']).toBe('43%');
    expect(cssDeclarations('.theme-mini-accent')['height']).toBe('7px');
    expect(cssDeclarations('.theme-preset-meta')['padding']).toBe(
      'var(--space-5) var(--space-1) var(--space-1)',
    );
    expect(cssDeclarations('.theme-preset-name')['font-size']).toBe('var(--fs-small)');
  });

  it('matches the mock colour-cell geometry', () => {
    expect(cssDeclarations('.theme-colors')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-colors')['gap']).toBe('var(--space-5)');
    expect(cssDeclarations('.theme-color-cell')['padding']).toBe(
      'var(--space-5) var(--space-5)',
    );
    expect(cssDeclarations('.theme-color-cell')['gap']).toBe('var(--space-5)');
    expect(cssDeclarations('.theme-color-swatch')['width']).toBe('31px');
    expect(cssDeclarations('.theme-color-swatch')['height']).toBe('31px');
    expect(cssDeclarations('.theme-color-name')['font-size']).toBe('var(--fs-tiny)');
    // The label stays inline (mock), which keeps the two-line 59px cell.
    expect(cssDeclarations('.theme-color-name')['display']).toBeUndefined();
    expect(cssDeclarations('.theme-color-info')['overflow']).toBe('hidden');
    // The mock uses the `font` shorthand for the hex field.
    expect(cssDeclarations('.theme-color-hex')['font']).toBe('var(--fs-tiny) var(--font-mono)');
  });

  it('matches the mock interface (typography controls) geometry', () => {
    expect(cssDeclarations('.theme-setting-grid')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-setting-grid')['gap']).toBe('var(--space-6)');
    expect(cssDeclarations('.theme-field-label')['font-size']).toBe('var(--fs-tiny)');
    expect(cssDeclarations('.theme-field-label')['margin-bottom']).toBe('var(--space-4)');
    expect(cssSource).toMatch(
      /\.theme-studio select,\s*\.theme-name-input \{[^}]*padding: var\(--space-5\);/,
    );
    expect(cssSource).toMatch(
      /\.theme-studio select,\s*\.theme-name-input \{[^}]*font-size: var\(--fs-tiny\);/,
    );
    expect(cssSource).toMatch(
      /\.theme-studio select,\s*\.theme-name-input \{[^}]*border-radius: var\(--radius-md\);/,
    );
    expect(cssDeclarations('.theme-segments')['padding']).toBe('var(--space-2)');
    expect(cssDeclarations('.theme-segment')['padding']).toBe('var(--space-4)');
    expect(cssDeclarations('.theme-range-row')['margin-top']).toBe('var(--space-7)');
    expect(cssDeclarations('.theme-range-row')['gap']).toBe('var(--space-6)');
    expect(cssDeclarations('.theme-range-label')['width']).toBe('90px');
    expect(cssDeclarations('.theme-range-value')['min-width']).toBe('31px');
  });

  it('matches the mock advanced-customization geometry', () => {
    expect(cssDeclarations('.theme-advanced')['border-radius']).toBe('var(--radius)');
    expect(cssDeclarations('.theme-advanced summary')['padding']).toBe('var(--space-6)');
    expect(cssDeclarations('.theme-advanced summary svg')['color']).toBe('var(--muted)');
    expect(cssDeclarations('.theme-advanced-content')['padding']).toBe('var(--space-7)');
    expect(cssDeclarations('.theme-code-editor')['min-height']).toBe('120px');
    expect(cssDeclarations('.theme-code-editor')['max-height']).toBe('250px');
    expect(cssDeclarations('.theme-advanced-actions')['margin-top']).toBe('var(--space-5)');
  });

  it('matches the mock footer geometry and button set', () => {
    expect(cssDeclarations('.theme-studio-foot')['padding']).toBe(
      'var(--space-7) var(--space-10) var(--space-8)',
    );
    expect(cssDeclarations('.theme-studio-foot-top')['margin-bottom']).toBe('var(--space-6)');
    expect(cssDeclarations('.theme-autosave')['font-size']).toBe('var(--fs-tiny)');
    expect(cssDeclarations('.theme-autosave .theme-dot')['width']).toBe('6px');
    expect(cssDeclarations('.theme-foot-buttons')['gap']).toBe('var(--space-4)');
    expect(cssDeclarations('.theme-btn')['padding']).toBe(
      'var(--space-5) var(--space-6)',
    );
    expect(cssDeclarations('.theme-btn')['border-radius']).toBe('var(--radius-md)');
    expect(cssDeclarations('.theme-btn')['font-size']).toBe('var(--fs-tiny)');
    expect(cssDeclarations('.theme-foot-buttons .spacer')['margin-left']).toBe('auto');
    // No square icon-button variant: the mock's delete action is a normal button.
    expect(cssSource).not.toContain('.theme-btn.icon-only');
    expect(cssDeclarations('.theme-studio-import')['display']).toBe('none');
  });

  it('keeps the desktop drawer at tablet widths and only shrinks at 620px', () => {
    expect(cssSource).not.toMatch(/max-width:\s*800px/);
    expect(cssSource).not.toMatch(/max-width:\s*430px/);
    expect(cssSource).toMatch(
      /@media \(max-width: 620px\) \{[\s\S]*?\.theme-studio \{\s*width: calc\(100% - 56px\);/,
    );
  });

  it('keeps the mock button reset from out-specifying component styles', () => {
    // `.theme-studio button` (0,1,1) would beat `.theme-btn`/`.theme-preset` (0,1,0)
    // and strip their surface, border and font-size; `:where()` keeps it at 0,0,1.
    expect(cssSource).toContain('.theme-studio :where(button) {');
    expect(cssSource).not.toMatch(/^\s*\.theme-studio button \{/m);
    expect(cssDeclarations('.theme-studio :where(button)')['border']).toBe('0');
    expect(cssSource).toMatch(
      /\.theme-studio :where\(button\):disabled \{\s*cursor: not-allowed;\s*opacity: 0\.5;/,
    );
  });

  it('keeps the drawer scroll region on the mock 8px scrollbar', () => {
    // base.css sets `thin` + `scrollbar-color`, which supersedes the 8px
    // ::-webkit-scrollbar and would reserve ~15px of the drawer's content width.
    expect(cssDeclarations('.theme-studio-scroll')['scrollbar-gutter']).toBe('stable');
    expect(cssDeclarations('.theme-studio-scroll')['scrollbar-width']).toBe('auto');
    expect(cssDeclarations('.theme-studio-scroll')['scrollbar-color']).toBe('auto');
  });

  it('keeps the mock native height for the interface selects', () => {
    // The mock does not reset `font` on <select>, so selects keep their native
    // 34px line box; the shared base.css reset would make them 39px.
    expect(cssDeclarations('.theme-studio select')['line-height']).toBe('normal');
  });

  it('keeps the mock reduced-motion escape hatch', () => {
    expect(cssSource).toMatch(/@media \(prefers-reduced-motion: reduce\) \{/);
    expect(cssSource).not.toContain('.theme-colors-hint');
  });
});
