import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/preact';
import {
  findPanelShapeRenderer,
  registerPanelShapeRenderer,
  resetPluginPanelRenderers,
} from './pluginPanels';
import { formatClock, pluginViewRenderer, renderPluginView } from './PluginViews';

/**
 * The `view` vocabulary: a plugin's data drawn as a real view.
 *
 * Two properties matter. It only claims shapes it fully understands — anything else
 * must fall through to the JSON view rather than render half a card — and every
 * field is clamped, because a payload is untrusted input and a bar width is a style.
 */

afterEach(() => {
  cleanup();
  resetPluginPanelRenderers();
});

describe('clock formatting', () => {
  it('is mm:ss below an hour and h:mm:ss above it', () => {
    expect(formatClock(0)).toBe('00:00');
    expect(formatClock(9)).toBe('00:09');
    expect(formatClock(65)).toBe('01:05');
    expect(formatClock(3_599)).toBe('59:59');
    expect(formatClock(3_600)).toBe('1:00:00');
    expect(formatClock(93_261)).toBe('25:54:21');
  });

  it('clamps nonsense instead of printing it', () => {
    expect(formatClock(null)).toBe('—:—');
    expect(formatClock(-5)).toBe('00:00');
    // A day and a bit; the cap keeps a hostile payload from printing 10^9 digits.
    expect(formatClock(1e9)).toBe('99:59:59');
  });
});

describe('claiming a payload', () => {
  it('claims only the views it knows', () => {
    for (const view of ['stat', 'list', 'bars', 'progress', 'clock', 'stack']) {
      expect(pluginViewRenderer.matches({ view }), view).toBe(true);
      expect(pluginViewRenderer.matches({ view: view.toUpperCase() }), view).toBe(true);
    }
    expect(pluginViewRenderer.matches({ view: 'webview' })).toBe(false);
    expect(pluginViewRenderer.matches({ items: [] })).toBe(false);
    expect(pluginViewRenderer.matches([{ view: 'stat' }])).toBe(false);
    expect(pluginViewRenderer.matches(null)).toBe(false);
    expect(pluginViewRenderer.matches('view')).toBe(false);
  });

  it('is the shape fallback only when no id renderer claims the panel', () => {
    registerPanelShapeRenderer(pluginViewRenderer);
    expect(findPanelShapeRenderer({ view: 'stat', value: 1 })).toBe(pluginViewRenderer);
    expect(findPanelShapeRenderer({ items: [] })).toBeUndefined();
  });

  it('skips a shape renderer that throws while deciding', () => {
    registerPanelShapeRenderer({
      matches: () => {
        throw new Error('boom');
      },
      render: () => null as never,
    });
    registerPanelShapeRenderer(pluginViewRenderer);
    // The throwing predicate is a decline, not a broken tab.
    expect(findPanelShapeRenderer({ view: 'stat' })).toBe(pluginViewRenderer);
  });
});

describe('drawing a view', () => {
  function draw(data: unknown) {
    // The renderer is the contract the panel view uses; render through it.
    const rendered = pluginViewRenderer.render({
      panel: { id: 'x', hosted_id: 'plugin__p__x', title: 'X', source: 'static', refresh_ms: 0 },
      plugin: 'p',
      version: '1.0.0',
      data,
      refresh: vi.fn(),
    });
    return render(rendered);
  }

  it('draws a stat with its label, value and tone', () => {
    draw({ view: 'stat', label: 'Words', value: 12_408, detail: 'across 31 files', tone: 'ok' });
    expect(screen.getByText('Words')).toBeTruthy();
    // A number is printed as text, never as markup.
    expect(screen.getByText('12408')).toBeTruthy();
    expect(screen.getByText('across 31 files')).toBeTruthy();
    expect(document.querySelector('.plugin-view-stat.tone-ok')).toBeTruthy();
  });

  it('draws rows with their details, and nothing when the list is empty', () => {
    draw({ view: 'list', title: 'Files', items: [{ text: 'src/lib.rs', detail: '1.2 MB', status: 'warn' }] });
    expect(screen.getByText('src/lib.rs')).toBeTruthy();
    expect(screen.getByText('1.2 MB')).toBeTruthy();
    expect(document.querySelector('.plugin-view-row.tone-warn')).toBeTruthy();

    cleanup();
    draw({ view: 'list', items: [] });
    expect(screen.getByText('Nothing to show.')).toBeTruthy();
  });

  it('scales bars against the tallest one and ignores unusable values', () => {
    draw({
      view: 'bars',
      label: 'Weight',
      bars: [
        { label: 'crates', value: 300, display: '300 kB' },
        { label: 'web', value: 100 },
        { label: 'zero', value: 0 },
        { label: 'nope', value: 'lots' },
      ],
    });
    const fills = [...document.querySelectorAll<HTMLElement>('.plugin-view-bar-fill')];
    expect(fills).toHaveLength(2);
    expect(fills[0]?.style.width).toBe('100%');
    expect(fills[1]?.style.width).toBe('33%');
    expect(screen.queryByText('zero')).toBeNull();
    // A display string is what is printed, and what a screen reader reads; the bar
    // still scales from the number.
    expect(screen.getByText('300 kB')).toBeTruthy();
    expect(screen.getByText('100')).toBeTruthy();
    expect(screen.getByRole('img', { name: 'crates: 300 kB' })).toBeTruthy();
  });

  it('draws a progress bar clamped to 0..1', () => {
    draw({ view: 'progress', label: 'Context', fraction: 1.7, detail: '170%' });
    const bar = screen.getByRole('progressbar', { name: 'Context' });
    expect(bar.getAttribute('aria-valuenow')).toBe('100');
    expect(document.querySelector<HTMLElement>('.plugin-view-progress-fill')?.style.width).toBe(
      '100%',
    );

    cleanup();
    draw({ view: 'progress', label: 'Context', fraction: -3 });
    expect(
      screen.getByRole('progressbar', { name: 'Context' }).getAttribute('aria-valuenow'),
    ).toBe('0');
  });

  it('draws a live clock, and marks it running unless told otherwise', () => {
    draw({ view: 'clock', label: 'Writing', remaining_secs: 932, detail: '15 min' });
    expect(screen.getByText('15:32')).toBeTruthy();
    expect(document.querySelector('.plugin-view-clock.is-running')).toBeTruthy();

    cleanup();
    draw({ view: 'clock', label: 'Paused', remaining_secs: 5, running: false });
    expect(screen.getByText('00:05')).toBeTruthy();
    expect(document.querySelector('.plugin-view-clock.is-running')).toBeNull();
  });

  it('stacks views in order and bounds the recursion', () => {
    // A hostile payload: 50 nested stacks. Depth is capped, so the tab draws a
    // couple of levels instead of building ten thousand nodes.
    let payload: Record<string, unknown> = { view: 'stat', value: 1 };
    for (let index = 0; index < 50; index += 1) {
      payload = { view: 'stack', views: [payload, payload] };
    }
    draw(payload);
    expect(document.querySelectorAll('.plugin-view-stack').length).toBeGreaterThan(0);
    expect(document.querySelectorAll('.plugin-view').length).toBeLessThan(30);

    cleanup();
    draw({ view: 'stack', views: [{ view: 'stat', value: 'first' }, { view: 'stat', value: 'second' }] });
    // Siblings keep the payload's order.
    const values = [...document.querySelectorAll('.plugin-view-value')].map((node) => node.textContent);
    expect(values).toEqual(['first', 'second']);
  });

  it('truncates a long text field instead of letting it run', () => {
    draw({ view: 'stat', label: 'x'.repeat(200), value: 'v'.repeat(200) });
    const label = document.querySelector('.plugin-view-label')?.textContent ?? '';
    const value = document.querySelector('.plugin-view-value')?.textContent ?? '';
    expect(label.length).toBeLessThanOrEqual(41);
    expect(label.endsWith('…')).toBe(true);
    expect(value.length).toBeLessThanOrEqual(41);
  });

  it('returns null for a view it cannot draw', () => {
    expect(renderPluginView({ view: 'nope' })).toBeNull();
    expect(renderPluginView(null)).toBeNull();
  });
});
