import { cleanup, render, screen, waitFor } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PluginPanelView } from './PluginPanelView';
import { registerPluginPanelRenderer, resetPluginPanelRenderers } from './pluginPanels';
import { TODO_PANEL_ID, todoPanelRenderer } from './TodoPanel';
import type { PluginPanelTab } from '../state/store';

/**
 * The panel is scoped by session: TODO lists are per session now, so the tab has
 * to re-read when the selected session changes. Without that dependency the tab
 * would keep showing the previous conversation's list, which is exactly the bug
 * the per-session layout exists to prevent.
 */

const getPluginPanel = vi.fn();

vi.mock('../api/endpoints', async () => {
  const actual = await vi.importActual<typeof import('../api/endpoints')>('../api/endpoints');
  return {
    ...actual,
    getPluginPanel: (...args: unknown[]) => getPluginPanel(...args),
  };
});

function panel(): PluginPanelTab {
  return {
    id: 'todo',
    hosted_id: TODO_PANEL_ID,
    title: 'Todo',
    icon: 'check',
    source: 'static',
    refresh_ms: 0,
    plugin: 'kc-todo',
    version: '0.1.0',
  };
}

function reply(title: string) {
  return {
    title: 'Todo',
    version: 1,
    fetched_ms: Date.now(),
    data: {
      workspace: '/w',
      session: 'current',
      items: [
        {
          id: 't1',
          title,
          status: 'in_progress',
          label: 'In progress',
          parent: null,
          depth: 0,
          detail: '',
          evidence: '',
          limitation: '',
          reason: '',
          started: true,
          open: true,
          needs: 'nothing extra',
          children: [],
        },
      ],
      audit: {
        total: 1,
        open: ['t1'],
        in_progress: ['t1'],
        unverified: [],
        missing_evidence: [],
        missing_limitation: [],
        clean: false,
        counts: { in_progress: 1 },
      },
      carry_over: [],
      open_count: 1,
      path: '/w/.karucode/todo/sessions/current.json',
    },
  };
}

beforeEach(async () => {
  getPluginPanel.mockReset();
  registerPluginPanelRenderer(TODO_PANEL_ID, todoPanelRenderer);
  const store = await import('../state/store');
  store.currentProjectId.value = 'p_one';
  store.currentSessionId.value = 's_alpha';
  store.pluginPanelData.value = {};
});

afterEach(() => {
  cleanup();
  resetPluginPanelRenderers();
});

describe('PluginPanelView session scoping', () => {
  it('asks for the currently selected session', async () => {
    getPluginPanel.mockResolvedValue(reply('Alpha work'));
    render(<PluginPanelView panel={panel()} />);

    await waitFor(() => expect(getPluginPanel).toHaveBeenCalled());
    const params = getPluginPanel.mock.calls[0]?.[2] as { session?: string };
    expect(params.session).toBe('s_alpha');
  });

  it('re-reads and redraws when the session changes', async () => {
    getPluginPanel.mockResolvedValueOnce(reply('Alpha work'));
    const { rerender } = render(<PluginPanelView panel={panel()} />);
    expect(await screen.findByText('Alpha work')).toBeTruthy();

    // Switch conversation: the same tab must now show the other session's list.
    const store = await import('../state/store');
    getPluginPanel.mockResolvedValueOnce(reply('Beta work'));
    store.currentSessionId.value = 's_beta';
    rerender(<PluginPanelView panel={panel()} />);

    expect(await screen.findByText('Beta work')).toBeTruthy();
    expect(screen.queryByText('Alpha work')).toBeNull();

    const sessions = getPluginPanel.mock.calls.map(
      (call) => (call[2] as { session?: string }).session,
    );
    expect(sessions).toContain('s_alpha');
    expect(sessions).toContain('s_beta');
  });
});
