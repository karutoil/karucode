import { cleanup, render, screen, waitFor } from '@testing-library/preact';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PluginPanelView } from './PluginPanelView';
import type { PluginPanelTab } from '../state/store';
import {
  findPluginPanelRenderer,
  registerPluginPanelRenderer,
  resetPluginPanelRenderers,
  type PluginPanelSummary,
} from './pluginPanels';
import { TodoPanel, TODO_PANEL_ID, parseTodoPayload, rowOrder, todoPanelRenderer } from './TodoPanel';
import * as actions from '../state/actions';

/**
 * The Todo panel renderer, exercised the way the shell uses it: through the
 * registry's `matches`/`render` contract, not by calling the component directly.
 *
 * The payload fixtures mirror what `panel_todo()` in the plugin's `src/main.rs`
 * actually returns; that contract is the only thing tying the two halves
 * together, so it is worth pinning on both sides.
 */

vi.mock('../state/actions', () => ({
  refreshPluginPanel: vi.fn(async () => undefined),
}));

function item(overrides: Record<string, unknown> = {}) {
  return {
    id: 't1',
    title: 'A task',
    status: 'pending',
    label: 'Not implemented',
    parent: null,
    depth: 0,
    detail: '',
    evidence: '',
    limitation: '',
    reason: '',
    started: false,
    open: true,
    needs: 'nothing extra',
    children: [],
    ...overrides,
  };
}

function payload(overrides: Record<string, unknown> = {}) {
  const items = (overrides.items as unknown[]) ?? [item()];
  return {
    schema: 1,
    version: 4,
    items,
    audit: {
      total: items.length,
      open: [],
      in_progress: [],
      unverified: [],
      missing_evidence: [],
      missing_limitation: [],
      clean: true,
      counts: {},
    },
    carry_over: [],
    open_count: 0,
    workspace: '/w',
    path: '/w/.karucode/todo/list.json',
    ...overrides,
  };
}

/** A summary as `GET /api/config` describes it. */
function summary(): PluginPanelSummary {
  return {
    id: 'todo',
    hosted_id: TODO_PANEL_ID,
    title: 'Todo',
    icon: 'check',
    source: 'static',
    refresh_ms: 0,
  };
}

/** The same panel as the shell's tab type, which widens `icon` to `string | null`. */
function panel(): PluginPanelTab {
  return { ...summary(), icon: 'check', plugin: 'kc-todo', version: '0.1.0' };
}

afterEach(() => {
  cleanup();
  resetPluginPanelRenderers();
});

/**
 * The first parsed item, asserted present.
 *
 * `noUncheckedIndexedAccess` makes `items[0]` possibly-undefined, which is
 * correct for real code and noise in a test that has just asserted the length.
 */
function firstItem(data: unknown) {
  const parsed = parseTodoPayload(data);
  const item = parsed?.items[0];
  if (!item) throw new Error('expected a parsed item');
  return item;
}

describe('parseTodoPayload', () => {
  it('accepts the payload the plugin produces', () => {
    const parsed = parseTodoPayload(payload());
    expect(parsed).toBeDefined();
    expect(parsed?.items).toHaveLength(1);
    expect(firstItem(payload()).label).toBe('Not implemented');
    expect(parsed?.path).toBe('/w/.karucode/todo/list.json');
  });

  it('declines anything that is not a list', () => {
    expect(parseTodoPayload(null)).toBeUndefined();
    expect(parseTodoPayload('hello')).toBeUndefined();
    expect(parseTodoPayload({ words: 12 })).toBeUndefined();
    expect(parseTodoPayload({ items: 'not-an-array' })).toBeUndefined();
  });

  it('accepts an error-only payload, so the reason can be shown', () => {
    const parsed = parseTodoPayload({ error: 'the list is not readable', items: [] });
    expect(parsed?.error).toBe('the list is not readable');
  });

  it('drops items without an id instead of rendering a row nothing can point at', () => {
    const parsed = parseTodoPayload(
      payload({ items: [item(), { title: 'no id at all' }, item({ id: 't2' })] }),
    );
    expect(parsed!.items.map((i) => i.id)).toEqual(['t1', 't2']);
  });

  it('falls back to a label when the plugin omits one', () => {
    expect(firstItem(payload({ items: [item({ label: '', status: 'partial' })] })).label).toBe(
      'Implemented (limited)',
    );
  });

  it('ignores fields of the wrong type rather than throwing', () => {
    const parsed = firstItem(
      payload({ items: [item({ title: 42, detail: null, children: 'nope', evidence: {} })] }),
    );
    expect(parsed.title).toBe('');
    expect(parsed.detail).toBe('');
    expect(parsed.children).toEqual([]);
  });
});

describe('rowOrder', () => {
  it('puts each subtask directly after its parent', () => {
    const parsed = parseTodoPayload(
      payload({
        items: [
          item({ id: 't1', title: 'Parent one' }),
          item({ id: 't2', title: 'Child of one', parent: 't1', depth: 1 }),
          item({ id: 't3', title: 'Parent two' }),
        ],
      }),
    );
    expect(rowOrder(parsed!.items).map((i) => i.title)).toEqual([
      'Parent one',
      'Child of one',
      'Parent two',
    ]);
  });

  it('promotes an orphan whose parent is missing', () => {
    const parsed = parseTodoPayload(payload({ items: [item({ id: 't9', parent: 't404' })] }));
    expect(rowOrder(parsed!.items).map((i) => i.id)).toEqual(['t9']);
  });

  it('keeps an orphaned cycle visible', () => {
    const parsed = parseTodoPayload(
      payload({
        items: [
          item({ id: 't1', parent: 't2' }),
          item({ id: 't2', parent: 't1' }),
          item({ id: 't3', title: 'Normal' }),
        ],
      }),
    );
    expect(rowOrder(parsed!.items).map((i) => i.id)).toEqual(['t3', 't1', 't2']);
  });
});

describe('the registered renderer', () => {
  it('claims a todo payload and declines anything else', () => {
    expect(todoPanelRenderer.matches(payload())).toBe(true);
    expect(todoPanelRenderer.matches({ unrelated: true })).toBe(false);
  });

  it('is found under the hosted id', () => {
    registerPluginPanelRenderer(TODO_PANEL_ID, todoPanelRenderer);
    expect(findPluginPanelRenderer(panel())).toBe(todoPanelRenderer);
  });

  it('is not borrowed by another plugin panel of the same bare name', () => {
    registerPluginPanelRenderer(TODO_PANEL_ID, todoPanelRenderer);
    const other = { ...panel(), hosted_id: 'plugin__someone-else__todo' };
    expect(findPluginPanelRenderer(other)).toBeUndefined();
  });
});

describe('TodoPanel', () => {
  it('renders the vocabulary the plugin and the prompt share', () => {
    const data = payload({
      items: [
        item({ id: 't1', title: 'Not started', status: 'pending', label: 'Not implemented' }),
        item({ id: 't2', title: 'Half done', status: 'partial', label: 'Implemented (limited)', started: true }),
        item({ id: 't3', title: 'Finished', status: 'done', label: 'Implemented', evidence: 'tests pass', started: true, open: false }),
      ],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);

    for (const label of ['Not implemented', 'Implemented (limited)', 'Implemented']) {
      // Once as a summary pill, once on the row.
      expect(screen.getAllByText(label).length, label).toBe(2);
    }
  });

  it('indents a subtask', () => {
    const data = payload({
      items: [item({ id: 't1' }), item({ id: 't2', parent: 't1', depth: 1, title: 'Child' })],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(document.querySelectorAll('.todo-item.is-subtask')).toHaveLength(1);
  });

  it('flags an unverified completion', () => {
    const data = payload({
      items: [item({ status: 'done', label: 'Implemented', evidence: 'trust me', started: false })],
      audit: {
        total: 1,
        open: [],
        in_progress: [],
        unverified: ['t1'],
        missing_evidence: [],
        missing_limitation: [],
        clean: false,
        counts: { done: 1 },
      },
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText('Needs a second look')).toBeTruthy();
    expect(screen.getByText(/marked implemented without ever being in progress \(t1\)/)).toBeTruthy();
  });

  it('reports a clean list without a warning', () => {
    const data = payload({
      items: [item({ status: 'done', label: 'Implemented', evidence: 'ok', started: true })],
      audit: {
        total: 1, open: [], in_progress: [], unverified: [], missing_evidence: [],
        missing_limitation: [], clean: true, counts: { done: 1 },
      },
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText('No unverified claims.')).toBeTruthy();
    expect(screen.queryByText('Needs a second look')).toBeNull();
  });

  it('shows the empty state for a list with no items', () => {
    render(
      <TodoPanel
        {...{
          panel: panel(), plugin: 'kc-todo', version: '0.1.0',
          data: payload({ items: [] }), refresh: () => {},
        }}
      />,
    );
    expect(screen.getByText('No plan yet')).toBeTruthy();
  });

  it('shows the reason when the plugin could not read the list', () => {
    const data = { error: 'the TODO list at `/w/list.json` is not readable', items: [] };
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText('Could not read the list')).toBeTruthy();
    expect(screen.getByRole('alert')).toBeTruthy();
  });

  it('surfaces a carry-over that is still open', () => {
    const data = payload({
      items: [item({ status: 'in_progress', label: 'In progress', started: true })],
      carry_over: [
        { run_id: 'r1', at_ms: 1, in_progress: ['t1'], pending: [], still_open: ['t1'] },
      ],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText(/Still open from the last run/)).toBeTruthy();
    expect(screen.getByText(/t1 still in progress/)).toBeTruthy();
  });

  /**
   * Regression, from a real screenshot: a run-end audit records what was open at
   * that moment. Once those items are closed, the tab kept claiming "Left open by
   * the last run" over a fully-finished list, which reads as a bug to the user.
   */
  it('does not claim a carry-over once those items are closed', () => {
    const data = payload({
      items: [
        item({ id: 't1', status: 'done', label: 'Implemented', evidence: 'ok', started: true, open: false }),
        item({ id: 't2', status: 'done', label: 'Implemented', evidence: 'ok', started: true, open: false }),
      ],
      audit: {
        total: 2, open: [], in_progress: [], unverified: [], missing_evidence: [],
        missing_limitation: [], clean: true, counts: { done: 2 },
      },
      carry_over: [
        { run_id: 'r1', at_ms: 1, in_progress: [], pending: ['t1', 't2'], still_open: [] },
      ],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.queryByText(/Still open from the last run/)).toBeNull();
  });

  it('reports only the still-open subset of a mixed carry-over', () => {
    const data = payload({
      items: [
        item({ id: 't1', status: 'done', label: 'Implemented', evidence: 'ok', started: true, open: false }),
        item({ id: 't2', status: 'pending', label: 'Not implemented' }),
      ],
      carry_over: [
        { run_id: 'r1', at_ms: 1, in_progress: [], pending: ['t1', 't2'], still_open: ['t2'] },
      ],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText(/Still open from the last run/)).toBeTruthy();
    // t1 is finished, so it must not be named.
    expect(screen.queryByText(/t1 never started/)).toBeNull();
    expect(screen.getByText(/t2 never started/)).toBeTruthy();
  });

  it('falls back to the raw ids when an older payload omits still_open', () => {
    const data = payload({
      items: [item({ status: 'in_progress', label: 'In progress', started: true })],
      carry_over: [{ run_id: 'r1', at_ms: 1, in_progress: ['t1'], pending: [] }],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    expect(screen.getByText(/Still open from the last run/)).toBeTruthy();
  });

  it('expands an item to show its limitation', async () => {
    const data = payload({
      items: [item({ status: 'partial', label: 'Implemented (limited)', limitation: 'no retry on 429', started: true })],
    });
    render(<TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />);
    const toggle = screen.getByRole('button', { name: 'Show details' });
    toggle.click();
    await waitFor(() => expect(screen.getByText('no retry on 429')).toBeTruthy());
  });

  it('renders markup in a title as text, never as HTML', () => {
    const data = payload({
      items: [item({ title: '<img src=x onerror="alert(1)">' })],
    });
    const { container } = render(
      <TodoPanel {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data, refresh: () => {} }} />,
    );
    // The literal string is visible…
    expect(screen.getByText('<img src=x onerror="alert(1)">')).toBeTruthy();
    // …and it did not become an element.
    expect(container.querySelector('img')).toBeNull();
  });

  it('degrades to a readable message instead of blanking on an unreadable payload', () => {
    render(
      <TodoPanel
        {...{ panel: panel(), plugin: 'kc-todo', version: '0.1.0', data: { nope: true }, refresh: () => {} }}
      />,
    );
    expect(screen.getByText('Unrecognised task list')).toBeTruthy();
  });
});

describe('through the shell', () => {
  it('draws the plugin payload inside PluginPanelView when registered', async () => {
    registerPluginPanelRenderer(TODO_PANEL_ID, todoPanelRenderer);
    // The shell reads the payload from the store; fake a finished read.
    const store = await import('../state/store');
    store.pluginPanelData.value = {
      [TODO_PANEL_ID]: {
        loading: false,
        data: payload({ items: [item({ title: 'From the plugin', status: 'in_progress', label: 'In progress' })] }),
        title: 'Todo',
        error: null,
        fetchedAt: Date.now(),
      },
    };

    render(<PluginPanelView panel={panel()} />);
    expect(await screen.findByText('From the plugin')).toBeTruthy();
    // The shell's own chrome is still there.
    expect(screen.getByText('kc-todo v0.1.0')).toBeTruthy();
    expect(actions.refreshPluginPanel).toHaveBeenCalled();
  });

  it('falls back to the generic JSON view when no renderer is registered', async () => {
    const store = await import('../state/store');
    store.pluginPanelData.value = {
      [TODO_PANEL_ID]: {
        loading: false,
        data: payload({ items: [item({ title: 'Raw payload row' })] }),
        title: 'Todo',
        error: null,
        fetchedAt: null,
      },
    };

    render(<PluginPanelView panel={panel()} />);
    expect(await screen.findByText(/No renderer is registered for this panel/)).toBeTruthy();
  });
});
