import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { ContextDrawer } from './ContextDrawer';
import { Inspector } from './Inspector';

vi.mock('./WorkspaceTab', () => ({
  WorkspaceTab: () => <div data-testid="workspace-result">Workspace result</div>,
}));
vi.mock('./git/GitManager', () => ({
  GitManager: ({ variant = 'panel' }: { variant?: string }) => (
    <div data-testid="git-result" data-variant={variant}>Git result</div>
  ),
}));
vi.mock('./SessionInfoTab', () => ({
  SessionInfoTab: () => <div data-testid="usage-result">Usage result</div>,
  sessionInfoTooltip: () => 'Session information',
}));
vi.mock('./AgentsTab', () => ({
  AgentsTab: () => <div data-testid="agents-result">Agents result</div>,
}));
vi.mock('./PluginPanelView', () => ({
  PluginPanelView: ({ panel }: { panel: store.PluginPanelTab }) => (
    <div data-testid="plugin-result" data-panel-id={panel.hosted_id}>Plugin result</div>
  ),
}));

function clearWorkbench(): void {
  workbench.closeContext();
  workbench.contextOpen.value = false;
  workbench.contextSection.value = 'now';
  store.narrowViewport.value = false;
  store.mobileSheet.value = null;
}

describe('Workbench result dock', () => {
  beforeEach(() => {
    resetAppState();
    clearWorkbench();
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => {})));
  });

  afterEach(() => {
    clearWorkbench();
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('keeps the real built-in and dynamic panels with direct, lazy tab switches', async () => {
    store.config.value = {
      ...configFixture,
      plugin_panels: [{
        plugin: 'kc-goals',
        version: '1.2.3',
        panels: [{
          id: 'goals',
          hosted_id: 'plugin__kc-goals__goals',
          title: 'Goals',
          icon: null,
          source: 'tool',
          refresh_ms: 0,
        }],
      }],
    };
    render(<Inspector />);

    expect(screen.getByText('Results')).toBeTruthy();
    expect(screen.queryByText('Project context, always in reach')).toBeNull();
    expect(screen.getByRole('button', { name: 'Context' })).toBeTruthy();
    expect(screen.getByTestId('workspace-result')).toBeTruthy();
    expect(screen.queryByTestId('usage-result')).toBeNull();
    expect(screen.queryByRole('tab', { name: 'Plan' })).toBeNull();

    fireEvent.click(screen.getByRole('tab', { name: 'Session usage' }));
    expect(store.inspectorTab.value).toBe('session');
    expect(screen.getByTestId('usage-result')).toBeTruthy();
    expect(screen.queryByTestId('workspace-result')).toBeNull();

    fireEvent.click(screen.getByRole('tab', { name: 'Git' }));
    expect(store.inspectorTab.value).toBe('changes');
    expect(screen.getByTestId('git-result').getAttribute('data-variant')).toBe('page');

    fireEvent.click(screen.getByRole('tab', { name: 'Agents' }));
    expect(store.inspectorTab.value).toBe('agents');
    expect(screen.getByTestId('agents-result')).toBeTruthy();

    fireEvent.click(screen.getByRole('tab', { name: 'Goals' }));
    expect(store.inspectorTab.value).toBe('plugin__kc-goals__goals');
    expect(screen.getByTestId('plugin-result').getAttribute('data-panel-id')).toBe('plugin__kc-goals__goals');
    expect(screen.queryByTestId('agents-result')).toBeNull();

    // A disabled active plugin falls back to a real result instead of leaving a
    // selected id with no panel attached.
    store.config.value = configFixture;
    await waitFor(() => expect(store.inspectorTab.value).toBe('workspace'));
    expect(screen.getByTestId('workspace-result')).toBeTruthy();
  });

  it('never aliases two legacy panels that emit the same hosted id', async () => {
    store.config.value = {
      ...configFixture,
      plugin_panels: [
        {
          plugin: 'a',
          version: '1',
          panels: [{
            id: 'b__c',
            hosted_id: 'plugin__a__b__c',
            title: 'First panel',
            icon: null,
            source: 'static',
            refresh_ms: 0,
          }],
        },
        {
          plugin: 'a__b',
          version: '2',
          panels: [{
            id: 'c',
            hosted_id: 'plugin__a__b__c',
            title: 'Aliased second panel',
            icon: null,
            source: 'static',
            refresh_ms: 0,
          }],
        },
      ],
    };
    render(<Inspector />);

    expect(screen.getByRole('tab', { name: 'First panel' })).toBeTruthy();
    expect(screen.queryByRole('tab', { name: 'Aliased second panel' })).toBeNull();
    expect(screen.getByRole('alert').textContent).toContain('duplicate hosted id');
    fireEvent.click(screen.getByRole('tab', { name: 'First panel' }));
    await waitFor(() => expect(screen.getAllByTestId('plugin-result')).toHaveLength(1));
    expect(screen.getByTestId('plugin-result').getAttribute('data-panel-id')).toBe('plugin__a__b__c');
  });

  it('keeps a selected result mounted while desktop Context opens and closes', () => {
    store.inspectorTab.value = 'changes';
    render(<><Inspector /><ContextDrawer /></>);
    const result = screen.getByTestId('git-result');

    fireEvent.click(screen.getByRole('button', { name: 'Context' }));
    expect(workbench.contextOpen.value).toBe(true);
    expect(screen.getByRole('dialog', { name: 'Workspace context' })).toBeTruthy();
    expect(screen.getByTestId('git-result')).toBe(result);
    expect(document.querySelector('.inspector')?.hasAttribute('inert')).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Close context' }));
    expect(workbench.contextOpen.value).toBe(false);
    expect(screen.queryByRole('dialog', { name: 'Workspace context' })).toBeNull();
    expect(screen.getByTestId('git-result')).toBe(result);
    expect(store.inspectorTab.value).toBe('changes');
  });

  it('clears transient Context and restores the prior phone interaction on unmount', async () => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = null;
    store.inspectorTab.value = 'agents';
    const view = render(<><Inspector /><ContextDrawer /></>);
    fireEvent.click(screen.getByRole('button', { name: 'Context' }));

    expect(workbench.contextOpen.value).toBe(true);
    expect(store.mobileSheet.value).toBe('inspector');
    await waitFor(() => expect(document.body.style.overflow).toBe('hidden'));

    view.unmount();
    expect(workbench.contextOpen.value).toBe(false);
    expect(store.mobileSheet.value).toBeNull();
    expect(store.inspectorTab.value).toBe('agents');
    expect(document.body.style.overflow).toBe('');
  });

  it('makes phone Context the only active trap without unmounting the underlying result', async () => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = 'inspector';
    store.inspectorTab.value = 'session';
    const view = render(<><Inspector /><ContextDrawer /></>);
    const result = screen.getByTestId('usage-result');
    const inspector = document.querySelector('.inspector');
    if (!inspector) throw new Error('Inspector did not render');

    expect(inspector.getAttribute('aria-modal')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Context' }));
    const drawer = screen.getByRole('dialog', { name: 'Workspace context' });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close context' })));

    expect(drawer.getAttribute('aria-modal')).toBe('true');
    expect(inspector.hasAttribute('inert')).toBe(true);
    expect(inspector.getAttribute('aria-hidden')).toBe('true');
    expect(inspector.getAttribute('aria-modal')).toBeNull();
    expect(screen.getByTestId('usage-result')).toBe(result);
    expect(document.body.style.overflow).toBe('hidden');

    fireEvent.keyDown(drawer, { key: 'Escape' });
    await waitFor(() => expect(workbench.contextOpen.value).toBe(false));
    expect(store.mobileSheet.value).toBe('inspector');
    expect(inspector.hasAttribute('inert')).toBe(false);
    expect(inspector.getAttribute('aria-modal')).toBe('true');
    expect(screen.getByTestId('usage-result')).toBe(result);

    view.unmount();
    expect(document.body.style.overflow).toBe('');
  });

  it('hides the tabs a plugin asked to hide, and moves off a tab that just vanished', async () => {
    store.inspectorTab.value = 'agents';
    store.config.value = {
      ...configFixture,
      plugin_ui: {
        hidden: [{ target: 'inspector.tab.agents', plugins: ['kc-focus'] }],
        slots: [],
        hideable: [],
        contributions: [],
      },
    };
    render(<Inspector />);

    expect(screen.queryByRole('tab', { name: 'Agents' })).toBeNull();
    // The tab that was selected is gone, so the dock lands on a real one rather
    // than showing nothing.
    await waitFor(() => expect(store.inspectorTab.value).toBe('workspace'));
    expect(screen.getByTestId('workspace-result')).toBeTruthy();
  });

  it('never lets a plugin empty the tab strip', async () => {
    store.config.value = {
      ...configFixture,
      plugin_ui: {
        hidden: [
          { target: 'inspector.tab.workspace', plugins: ['kc-focus'] },
          { target: 'inspector.tab.changes', plugins: ['kc-focus'] },
          { target: 'inspector.tab.session', plugins: ['kc-focus'] },
          { target: 'inspector.tab.agents', plugins: ['kc-focus'] },
        ],
      },
    };
    render(<Inspector />);
    // Every request is refused rather than honoured into an empty panel.
    expect(screen.getAllByRole('tab')).toHaveLength(4);
  });

  it('carries plugin-contributed buttons in its header, and lets one hide Context', () => {
    store.config.value = {
      ...configFixture,
      plugin_ui: {
        contributions: [
          {
            plugin: 'kc-focus',
            version: '1.0.0',
            kind: 'button',
            id: 'focus',
            hosted_id: 'plugin__kc-focus__focus',
            slot: 'inspector.header',
            label: 'Focus mode',
            title: 'Focus mode',
            action: 'focus',
          },
        ],
        hidden: [{ target: 'inspector.context', plugins: ['kc-focus'] }],
      },
    };
    render(<Inspector />);

    const control = screen.getByRole('button', { name: 'Focus mode' });
    expect(control.getAttribute('data-plugin')).toBe('kc-focus');
    // The built-in Context button is gone, so the header keeps one way in and one
    // way to close — no layout with a missing control and a duplicated one.
    expect(screen.queryByRole('button', { name: 'Context' })).toBeNull();
  });
});
