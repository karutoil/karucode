import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { Config, PluginPanelGroup } from '../api/types';
import {
  MAX_PANEL_PAYLOAD_CHARS,
  findPluginPanelRenderer,
  panelPayloadFits,
  panelPollInterval,
  registerPluginPanelRenderer,
  resetPluginPanelRenderers,
  splitHostedPanelId,
  unregisterPluginPanelRenderer,
} from './pluginPanels';
import { PluginPanelView } from './PluginPanelView';
import { Inspector } from './Inspector';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { configFixture, jsonResponse, resetAppState } from '../test/fixtures';

const TODO_PANEL: PluginPanelGroup = {
  plugin: 'kc-todo',
  version: '0.2.0',
  panels: [
    {
      id: 'todo',
      hosted_id: 'plugin__kc-todo__todo',
      title: 'Todo',
      icon: 'check',
      source: 'static',
      refresh_ms: 0,
    },
    {
      id: 'board',
      hosted_id: 'plugin__kc-todo__board',
      title: 'Board',
      icon: null,
      source: 'static',
      refresh_ms: 0,
    },
  ],
};

function configWith(panels: PluginPanelGroup[] | undefined): Config {
  const config: Config = { ...configFixture };
  if (panels === undefined) delete config.plugin_panels;
  else config.plugin_panels = panels;
  return config;
}

function panelTab(hostedId = 'plugin__kc-todo__todo'): store.PluginPanelTab {
  const groups = configWith([TODO_PANEL]).plugin_panels ?? [];
  const group = groups[0];
  if (!group) throw new Error('no panel group');
  const panel = group.panels.find((candidate) => candidate.hosted_id === hostedId);
  if (!panel) throw new Error(`no panel ${hostedId}`);
  return { ...panel, plugin: group.plugin, version: group.version };
}

/** Route `/api/plugins/:name/panels/:panel` (and anything else) to a stub. */
function stubPanelFetch(handler: (url: string) => Response | Promise<Response>): ReturnType<typeof vi.fn> {
  const mock = vi.fn(async (input: RequestInfo | URL) => handler(String(input)));
  vi.stubGlobal('fetch', mock);
  return mock;
}

beforeEach(() => {
  resetAppState();
  resetPluginPanelRenderers();
  store.config.value = configFixture;
});

describe('the plugin panel registry', () => {
  it('namespaces a hosted id back into its plugin and panel', () => {
    expect(splitHostedPanelId('plugin__kc-todo__todo')).toEqual({
      plugin: 'kc-todo',
      panel: 'todo',
    });
    // A plugin name may itself contain `__`; the last separator wins.
    expect(splitHostedPanelId('plugin__my__plugin__todo')).toEqual({
      plugin: 'my__plugin',
      panel: 'todo',
    });
    // Anything not namespaced belongs to no plugin.
    expect(splitHostedPanelId('workspace')).toBeNull();
    expect(splitHostedPanelId('plugin__nopanel')).toBeNull();
    expect(splitHostedPanelId('plugin____todo')).toBeNull();
  });

  it('prefers a renderer registered for the hosted id over the bare one', () => {
    const bare = { matches: () => true, render: () => null } as never;
    const hosted = { matches: () => true, render: () => null } as never;
    registerPluginPanelRenderer('todo', bare);
    expect(findPluginPanelRenderer(panelTab())).toBe(bare);

    registerPluginPanelRenderer('plugin__kc-todo__todo', hosted);
    expect(findPluginPanelRenderer(panelTab())).toBe(hosted);
    // A different plugin's panel of the same name still gets the bare one.
    expect(
      findPluginPanelRenderer({
        ...panelTab('plugin__kc-todo__board'),
        id: 'todo',
        hosted_id: 'plugin__other__todo',
      }),
    ).toBe(bare);
  });

  it('unregisters, and answers undefined when nothing claims a panel', () => {
    const renderer = { matches: () => true, render: () => null } as never;
    registerPluginPanelRenderer('plugin__kc-todo__todo', renderer);
    expect(unregisterPluginPanelRenderer('plugin__kc-todo__todo')).toBe(true);
    expect(unregisterPluginPanelRenderer('plugin__kc-todo__todo')).toBe(false);
    expect(findPluginPanelRenderer(panelTab())).toBeUndefined();
  });

  it('clamps the poll interval instead of trusting the server', () => {
    expect(panelPollInterval({ ...panelTab(), refresh_ms: 0 })).toBe(0);
    expect(panelPollInterval({ ...panelTab(), refresh_ms: -5 })).toBe(0);
    expect(panelPollInterval({ ...panelTab(), refresh_ms: 5 })).toBe(1_000);
    expect(panelPollInterval({ ...panelTab(), refresh_ms: 5_000 })).toBe(5_000);
    expect(panelPollInterval({ ...panelTab(), refresh_ms: 10_000_000 })).toBe(300_000);
  });

  it('refuses a payload too large to draw', () => {
    expect(panelPayloadFits({ ok: true })).toBe(true);
    expect(panelPayloadFits('x'.repeat(MAX_PANEL_PAYLOAD_CHARS + 1))).toBe(false);
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(panelPayloadFits(cyclic)).toBe(false);
  });
});

describe('the config payload', () => {
  it('turns plugin_panels into inspector tabs', () => {
    store.config.value = configWith([TODO_PANEL]);
    const tabs = store.pluginPanelTabs.value;
    expect(tabs.map((tab) => tab.hosted_id)).toEqual([
      'plugin__kc-todo__todo',
      'plugin__kc-todo__board',
    ]);
    expect(tabs[0]?.plugin).toBe('kc-todo');
    expect(tabs[0]?.version).toBe('0.2.0');
  });

  it('produces no tabs when the server sends none', () => {
    store.config.value = configWith([]);
    expect(store.pluginPanelTabs.value).toEqual([]);
  });

  it('produces no tabs from an older server that omits the field', () => {
    store.config.value = configWith(undefined);
    expect(store.pluginPanelTabs.value).toEqual([]);
  });

  it('drops a malformed id rather than rendering a broken tab', () => {
    store.config.value = configWith([
      {
        plugin: 'broken',
        version: '1',
        panels: [
          { id: 'ok', hosted_id: 'ok', title: 'Ok', icon: null, source: 'static', refresh_ms: 0 },
          {
            id: 'good',
            hosted_id: 'plugin__broken__good',
            title: 'Good',
            icon: null,
            source: 'static',
            refresh_ms: 0,
          },
        ],
      },
    ]);
    expect(store.pluginPanelTabs.value.map((tab) => tab.id)).toEqual(['good']);
  });
});

describe('rendering a plugin panel', () => {
  it('renders an unregistered panel as read-only JSON', async () => {
    stubPanelFetch(() =>
      jsonResponse({
        title: 'Todo',
        data: { items: [{ id: 't1', status: 'done' }], count: 1 },
        version: 1,
        fetched_ms: 1,
      }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText('t1')).toBeTruthy();
    });
    // Keys and values are visible as text, never as markup.
    expect(screen.getByText('items')).toBeTruthy();
    expect(screen.getByText('count')).toBeTruthy();
    expect(
      screen.getByText(/No renderer is registered for this panel/i),
    ).toBeTruthy();
    expect(document.querySelector('.plugin-panel-json')?.textContent).toContain('done');
  });

  it('hands a registered renderer the payload', async () => {
    let received: unknown = undefined;
    registerPluginPanelRenderer('plugin__kc-todo__todo', {
      matches: () => true,
      render: (props) => {
        received = props.data;
        return <p class="todo-renderer">rendered</p>;
      },
    });
    stubPanelFetch(() =>
      jsonResponse({ title: 'Todo', data: { items: [] }, version: 1, fetched_ms: 1 }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText('rendered')).toBeTruthy();
    });
    expect(received).toEqual({ items: [] });
    // A registered renderer replaces the generic JSON view.
    expect(document.querySelector('.plugin-panel-json')).toBeNull();
  });

  it('passes the panel, plugin, version and data to the renderer', async () => {
    const seen: unknown[] = [];
    registerPluginPanelRenderer('plugin__kc-todo__todo', {
      matches: () => true,
      render: (props) => {
        seen.push(props.data);
        const data = props.data as { items: { id: string }[] };
        return (
          <ul class="todo-renderer">
            <li>
              {props.plugin} v{props.version} · {props.panel.title}
            </li>
            {data.items.map((item) => (
              <li key={item.id}>{item.id}</li>
            ))}
          </ul>
        );
      },
    });
    stubPanelFetch(() =>
      jsonResponse({
        title: 'Todo',
        data: { items: [{ id: 't2' }] },
        version: 1,
        fetched_ms: 1,
      }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText('t2')).toBeTruthy();
    });
    expect(screen.getByText('kc-todo v0.2.0 · Todo')).toBeTruthy();
    expect(seen).toEqual([{ items: [{ id: 't2' }] }]);
    // The renderer replaced the generic view entirely.
    expect(document.querySelector('.plugin-panel-json')).toBeNull();
  });

  it('falls back to the JSON view when a renderer declines the payload', async () => {
    registerPluginPanelRenderer('plugin__kc-todo__todo', {
      matches: () => false,
      render: () => <p>never</p>,
    });
    stubPanelFetch(() =>
      jsonResponse({ title: 'Todo', data: { other: 1 }, version: 1, fetched_ms: 1 }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText('other')).toBeTruthy();
    });
    expect(screen.queryByText('never')).toBeNull();
    expect(
      screen.getByText(/did not claim this payload/i),
    ).toBeTruthy();
  });

  it('shows the plugin error and offers a retry', async () => {
    const mock = stubPanelFetch(() =>
      jsonResponse(
        { error: { code: 'plugin', message: 'the todo list is unreadable' } },
        502,
      ),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText('the todo list is unreadable')).toBeTruthy();
    });
    expect(screen.getByText('Todo is unavailable')).toBeTruthy();

    // The retry button reads the panel again.
    const before = mock.mock.calls.length;
    fireEvent.click(screen.getByText('Try again'));
    await waitFor(() => {
      expect(mock.mock.calls.length).toBeGreaterThan(before);
    });
  });

  it('shows an explicit message for a payload too large to draw', async () => {
    stubPanelFetch(() =>
      jsonResponse({
        title: 'Todo',
        data: 'x'.repeat(MAX_PANEL_PAYLOAD_CHARS + 10),
        version: 1,
        fetched_ms: 1,
      }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText(/too much data to display/i)).toBeTruthy();
    });
  });

  it('never renders a payload as HTML', async () => {
    // A plugin payload is untrusted: a string that looks like markup must be
    // text, not an element.
    stubPanelFetch(() =>
      jsonResponse({
        title: 'Todo',
        data: { note: '<img src=x onerror="alert(1)">' },
        version: 1,
        fetched_ms: 1,
      }),
    );

    render(<PluginPanelView panel={panelTab()} />);

    await waitFor(() => {
      expect(screen.getByText(note())).toBeTruthy();
    });
    expect(document.querySelector('img[src="x"]')).toBeNull();
    function note(): string {
      return '<img src=x onerror="alert(1)">';
    }
  });
});

describe('the inspector tab strip', () => {
  it('adds one tab per plugin panel and no more', () => {
    store.config.value = configWith([TODO_PANEL]);
    stubPanelFetch(() =>
      jsonResponse({ title: 'Todo', data: {}, version: 1, fetched_ms: 1 }),
    );

    render(<Inspector />);

    expect(screen.getByRole('tab', { name: /Todo/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Board/ })).toBeTruthy();
    // The built-in tabs are untouched.
    expect(screen.getByRole('tab', { name: /Files/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Changes/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Commands/ })).toBeTruthy();
    expect(screen.getByRole('tab', { name: /Agents/ })).toBeTruthy();
  });

  it('adds no tab when no plugin declares a panel', () => {
    store.config.value = configWith([]);
    stubPanelFetch(() => jsonResponse({}));

    render(<Inspector />);

    const tabs = screen.getAllByRole('tab').map((tab) => tab.textContent ?? '');
    expect(tabs.some((label) => label.includes('Todo'))).toBe(false);
    expect(tabs).toHaveLength(4);
  });

  it('opens a plugin tab and reads its payload', async () => {
    store.config.value = configWith([TODO_PANEL]);
    const mock = stubPanelFetch((url) =>
      url.includes('/panels/todo')
        ? jsonResponse({
            title: 'Todo',
            data: { items: [{ id: 't9' }] },
            version: 1,
            fetched_ms: 1,
          })
        : jsonResponse({}),
    );

    render(<Inspector />);
    fireEvent.click(screen.getByRole('tab', { name: /Todo/ }));

    await waitFor(() => {
      expect(screen.getByText('t9')).toBeTruthy();
    });
    // The request names the plugin and the bare panel id, and is scoped.
    const urls = mock.mock.calls.map((call) => String(call[0]));
    expect(urls.some((url) => url.includes('/api/plugins/kc-todo/panels/todo'))).toBe(true);
  });

  it('falls back to the Files tab when the open plugin panel disappears', async () => {
    store.config.value = configWith([TODO_PANEL]);
    stubPanelFetch(() =>
      jsonResponse({ title: 'Todo', data: {}, version: 1, fetched_ms: 1 }),
    );

    render(<Inspector />);
    fireEvent.click(screen.getByRole('tab', { name: /Todo/ }));
    expect(store.inspectorTab.value).toBe('plugin__kc-todo__todo');

    // The plugin was disabled: the tab list no longer carries it.
    store.config.value = configWith([]);
    await waitFor(() => {
      expect(store.inspectorTab.value).toBe('workspace');
    });
  });
});

describe('refreshing a panel', () => {
  it('coalesces concurrent reads of the same panel', async () => {
    // Each call gets its own deferred, so the test controls exactly when a
    // response arrives.
    const pending: ((response: Response) => void)[] = [];
    const mock = vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          pending.push(resolve);
        }),
    );
    vi.stubGlobal('fetch', mock);

    const panel = panelTab();
    const first = actions.refreshPluginPanel(panel);
    const second = actions.refreshPluginPanel(panel);
    // Concurrent reads of one panel share a request: a poll and a manual refresh
    // must not ask the plugin twice.
    expect(mock).toHaveBeenCalledTimes(1);

    pending[0]?.(jsonResponse({ title: 'Todo', data: { a: 1 }, version: 1, fetched_ms: 7 }));
    await Promise.all([first, second]);
    expect(store.pluginPanelData.value[panel.hosted_id]?.data).toEqual({ a: 1 });
    expect(store.pluginPanelData.value[panel.hosted_id]?.error).toBeNull();

    // A later read is a new request.
    const third = actions.refreshPluginPanel(panel);
    expect(mock).toHaveBeenCalledTimes(2);
    pending[1]?.(jsonResponse({ title: 'Todo', data: { a: 2 }, version: 2, fetched_ms: 8 }));
    await third;
    expect(store.pluginPanelData.value[panel.hosted_id]?.data).toEqual({ a: 2 });
  });

  it('keeps the last payload visible when a refresh fails', async () => {
    const panel = panelTab();
    stubPanelFetch(() =>
      jsonResponse({ title: 'Todo', data: { a: 1 }, version: 1, fetched_ms: 7 }),
    );
    await actions.refreshPluginPanel(panel);

    stubPanelFetch(() =>
      jsonResponse({ error: { code: 'plugin', message: 'plugin is down' } }, 502),
    );
    await actions.refreshPluginPanel(panel);

    const state = store.pluginPanelData.value[panel.hosted_id];
    expect(state?.error).toBe('plugin is down');
    expect(state?.data).toEqual({ a: 1 });
  });

  it('drops every cached payload when the project changes', async () => {
    store.config.value = configWith([TODO_PANEL]);
    store.currentSessionId.value = 's_ab12';
    stubPanelFetch((url) =>
      url.includes('/api/sessions')
        ? jsonResponse({ sessions: [] })
        : jsonResponse({ title: 'Todo', data: { a: 1 }, version: 1, fetched_ms: 1 }),
    );

    await actions.refreshPluginPanel(panelTab());
    expect(Object.keys(store.pluginPanelData.value)).toHaveLength(1);

    actions.resetPluginPanelData();
    expect(store.pluginPanelData.value).toEqual({});
  });
});
