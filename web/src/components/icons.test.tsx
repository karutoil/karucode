import { describe, expect, it } from 'vitest';
import type { JSX } from 'preact';
import { render } from '@testing-library/preact';
import * as icons from './icons';
import { ExternalIcon, PLUGIN_PANEL_ICON_NAMES, type IconProps } from './icons';

/**
 * `icons.tsx` is the app's whole glyph vocabulary, and every member is one
 * function returning an `<svg>` built from the same `svgProps` — that is what
 * keeps the set visually consistent. What is worth pinning is that a *new* member
 * keeps those metrics instead of arriving with its own viewBox or stroke weight,
 * and that no glyph is empty.
 */

/** Every exported `…Icon` component, minus the `pluginPanelIcon` resolver. */
function iconComponents(): Array<[string, (props: IconProps) => JSX.Element]> {
  return Object.entries(icons).filter(
    (entry): entry is [string, (props: IconProps) => JSX.Element] =>
      // `pluginPanelIcon` also ends in `Icon` but resolves a *name*, not a glyph.
      entry[0] !== 'pluginPanelIcon' &&
      entry[0].endsWith('Icon') &&
      typeof entry[1] === 'function',
  );
}

function draw(Icon: (props: IconProps) => JSX.Element, props: IconProps = {}): Element {
  const { container } = render(<Icon {...props} />);
  const svg = container.querySelector('svg');
  expect(svg).not.toBeNull();
  return svg as Element;
}

describe('the icon set', () => {
  it('has more than fifty members, all functions', () => {
    const members = iconComponents();
    expect(members.length).toBeGreaterThan(50);
    for (const [name, Icon] of members) {
      expect(typeof Icon, name).toBe('function');
    }
  });

  it('draws every icon on the same 24px grid, at the same weight, hidden from a reader', () => {
    for (const [name, Icon] of iconComponents()) {
      const svg = draw(Icon, { size: 15 });
      expect(svg.getAttribute('viewBox'), name).toBe('0 0 24 24');
      expect(svg.getAttribute('stroke-width'), name).toBe('1.6');
      expect(svg.getAttribute('fill'), name).toBe('none');
      expect(svg.getAttribute('stroke'), name).toBe('currentColor');
      // Decorative by default: the button around an icon carries its name.
      expect(svg.getAttribute('aria-hidden'), name).toBe('true');
      expect(svg.getAttribute('focusable'), name).toBe('false');
    }
  });

  it('honours `size` and `class`, and defaults to 16px', () => {
    for (const [name, Icon] of iconComponents()) {
      const svg = draw(Icon, { size: 11, class: 'test-glyph' });
      expect(svg.getAttribute('width'), name).toBe('11');
      expect(svg.getAttribute('height'), name).toBe('11');
      expect(svg.getAttribute('class'), name).toBe('test-glyph');
      expect(draw(Icon).getAttribute('width'), name).toBe('16');
    }
  });

  it('draws something in every glyph — no empty member can slip in', () => {
    for (const [name, Icon] of iconComponents()) {
      const svg = draw(Icon);
      const strokes = svg.querySelectorAll('path, circle, rect, line, polyline, polygon');
      expect(strokes.length, name).toBeGreaterThan(0);
      expect(svg.textContent ?? '', name).toBe('');
    }
  });
});

describe('ExternalIcon', () => {
  it('is the "open elsewhere" arrow: a corner, the diagonal leaving it, and the frame', () => {
    const svg = draw(ExternalIcon, { size: 13 });
    const paths = svg.querySelectorAll('path');
    expect(paths).toHaveLength(3);
    for (const path of paths) expect(path.getAttribute('d')).toMatch(/[Mm]/);
    // The point of the icon: the role is drawn, not typed as a `↗` character.
    expect(svg.textContent ?? '').not.toContain('↗');
  });

  it('is deliberately not a plugin panel icon name', () => {
    // `PLUGIN_PANEL_ICONS` is the file's list of icons a *plugin* may name in its
    // manifest, and its names are documented in `docs/PLUGINS.md` and the
    // `@kc-doc: plugins` block. Adding one there widens what a plugin may ask the
    // host to draw, which is a documented capability change — it needs the doc
    // block updated in the same change (AGENTS.md §1). "Open elsewhere" is an app
    // navigation role, not a panel glyph, so it stays out until someone wants it
    // and updates the doc.
    expect(PLUGIN_PANEL_ICON_NAMES).not.toContain('external');
  });
});
