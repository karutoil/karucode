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
    expect(cssDeclarations('.theme-shade')['background']).toBe('var(--scrim)');
    expect(cssDeclarations('.theme-shade')['backdrop-filter']).toBe('blur(3px)');
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
    expect(cssDeclarations('.theme-studio-top')['padding']).toBe('25px 28px 23px');
    expect(cssDeclarations('.theme-studio-top')['gap']).toBe('14px');
    expect(cssDeclarations('.theme-studio-title')['font-size']).toBe('24px');
    expect(cssDeclarations('.theme-studio-title')['margin']).toBe('8px 0 3px');
    expect(cssDeclarations('.theme-studio-subtitle')['font-size']).toBe('11.5px');
    expect(cssDeclarations('.theme-close')['width']).toBe('31px');
    expect(cssDeclarations('.theme-close')['height']).toBe('31px');

    expect(cssDeclarations('.theme-studio-scroll')['padding']).toBe('0 28px 27px');
    expect(cssDeclarations('.theme-section')['padding']).toBe('23px 0 24px');
    expect(cssDeclarations('.theme-section-title')['margin-bottom']).toBe('6px');
    expect(cssDeclarations('.theme-description')['margin']).toBe('0 0 15px');
    expect(cssDeclarations('.theme-description')['font-size']).toBe('11px');
  });

  it('matches the mock gallery geometry', () => {
    expect(cssDeclarations('.theme-gallery')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-gallery')['gap']).toBe('10px');
    expect(cssDeclarations('.theme-preset')['padding']).toBe('9px');
    expect(cssDeclarations('.theme-preset')['border-radius']).toBe('11px');
    expect(cssDeclarations('.theme-preset-demo')['height']).toBe('62px');
    expect(cssDeclarations('.theme-mini-rail')['width']).toBe('12%');
    expect(cssDeclarations('.theme-mini-sidebar')['width']).toBe('24%');
    expect(cssDeclarations('.theme-mini-accent')['left']).toBe('43%');
    expect(cssDeclarations('.theme-mini-accent')['height']).toBe('7px');
    expect(cssDeclarations('.theme-preset-meta')['padding']).toBe('9px 2px 2px');
    expect(cssDeclarations('.theme-preset-name')['font-size']).toBe('11.5px');
  });

  it('matches the mock colour-cell geometry', () => {
    expect(cssDeclarations('.theme-colors')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-colors')['gap']).toBe('10px');
    expect(cssDeclarations('.theme-color-cell')['padding']).toBe('9px 10px');
    expect(cssDeclarations('.theme-color-cell')['gap']).toBe('10px');
    expect(cssDeclarations('.theme-color-swatch')['width']).toBe('31px');
    expect(cssDeclarations('.theme-color-swatch')['height']).toBe('31px');
    expect(cssDeclarations('.theme-color-name')['font-size']).toBe('10px');
    // The label stays inline (mock), which keeps the two-line 59px cell.
    expect(cssDeclarations('.theme-color-name')['display']).toBeUndefined();
    expect(cssDeclarations('.theme-color-info')['overflow']).toBe('hidden');
    // The mock uses the `font` shorthand for the hex field.
    expect(cssDeclarations('.theme-color-hex')['font']).toBe('10px var(--font-mono)');
  });

  it('matches the mock interface (typography controls) geometry', () => {
    expect(cssDeclarations('.theme-setting-grid')['grid-template-columns']).toBe(
      'repeat(2, minmax(0, 1fr))',
    );
    expect(cssDeclarations('.theme-setting-grid')['gap']).toBe('12px');
    expect(cssDeclarations('.theme-field-label')['font-size']).toBe('10.5px');
    expect(cssDeclarations('.theme-field-label')['margin-bottom']).toBe('8px');
    expect(cssSource).toMatch(/\.theme-studio select,\s*\.theme-name-input \{[^}]*padding: 10px;/);
    expect(cssSource).toMatch(/\.theme-studio select,\s*\.theme-name-input \{[^}]*font-size: 11px;/);
    expect(cssSource).toMatch(/\.theme-studio select,\s*\.theme-name-input \{[^}]*border-radius: 8px;/);
    expect(cssDeclarations('.theme-segments')['padding']).toBe('3px');
    expect(cssDeclarations('.theme-segment')['padding']).toBe('7px');
    expect(cssDeclarations('.theme-range-row')['margin-top']).toBe('15px');
    expect(cssDeclarations('.theme-range-row')['gap']).toBe('13px');
    expect(cssDeclarations('.theme-range-label')['width']).toBe('90px');
    expect(cssDeclarations('.theme-range-value')['min-width']).toBe('31px');
  });

  it('matches the mock advanced-customization geometry', () => {
    expect(cssDeclarations('.theme-advanced')['border-radius']).toBe('9px');
    expect(cssDeclarations('.theme-advanced summary')['padding']).toBe('13px');
    expect(cssDeclarations('.theme-advanced summary svg')['color']).toBe('var(--muted)');
    expect(cssDeclarations('.theme-advanced-content')['padding']).toBe('14px');
    expect(cssDeclarations('.theme-code-editor')['min-height']).toBe('120px');
    expect(cssDeclarations('.theme-code-editor')['max-height']).toBe('250px');
    expect(cssDeclarations('.theme-advanced-actions')['margin-top']).toBe('10px');
  });

  it('matches the mock footer geometry and button set', () => {
    expect(cssDeclarations('.theme-studio-foot')['padding']).toBe('15px 28px 19px');
    expect(cssDeclarations('.theme-studio-foot-top')['margin-bottom']).toBe('11px');
    expect(cssDeclarations('.theme-autosave')['font-size']).toBe('10.5px');
    expect(cssDeclarations('.theme-autosave .theme-dot')['width']).toBe('6px');
    expect(cssDeclarations('.theme-foot-buttons')['gap']).toBe('8px');
    expect(cssDeclarations('.theme-btn')['padding']).toBe('10px 12px');
    expect(cssDeclarations('.theme-btn')['border-radius']).toBe('8px');
    expect(cssDeclarations('.theme-btn')['font-size']).toBe('11px');
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
