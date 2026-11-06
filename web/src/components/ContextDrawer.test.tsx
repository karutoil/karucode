import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/preact';
import type { ApprovalView, ChatState } from '../state/chat';
import { emptyChatState } from '../state/chat';
import { approvalFixture, configFixture, NOW, resetAppState, summaryFixture } from '../test/fixtures';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { ContextDrawer } from './ContextDrawer';

vi.mock('./SessionInfoTab', () => ({
  SessionInfoTab: () => <div data-testid="delegated-session-info">Delegated SessionInfoTab</div>,
}));

function pendingChat(): ChatState {
  const chat = emptyChatState('s_current');
  chat.running = true;
  chat.runId = 'r_current';
  chat.runStatus = 'running';
  const approval: ApprovalView = {
    id: approvalFixture.id,
    kind: approvalFixture.kind,
    title: approvalFixture.title,
    detail: approvalFixture.detail,
    payload: approvalFixture.payload,
    session_id: 's_current',
    run_id: 'r_current',
    agent_id: null,
    status: 'pending',
    decision: null,
    created_at: NOW,
  };
  chat.items = [{ kind: 'approval', id: approval.id, at: NOW, approval }];
  return chat;
}

function seedTruthfulState(): void {
  store.projects.value = [{
    id: 'p_current',
    name: 'Workbench',
    path: '/srv/project',
    created_at: NOW,
    updated_at: NOW,
    session_count: 1,
  }];
  store.currentProjectId.value = 'p_current';
  store.sessions.value = [{
    ...summaryFixture,
    id: 's_current',
    project_id: 'p_current',
    title: 'Build truthful surfaces',
    workspace: '/srv/session',
    running: true,
    model: { provider: 'pinned-provider', model: 'model-pinned', reasoning_effort: 'high' },
  }];
  store.currentSessionId.value = 's_current';
  store.config.value = {
    ...configFixture,
    active: { provider: 'default', model: 'gpt-4o-mini' },
    approvals: {
      ...configFixture.approvals,
      mode: 'ask_commands',
      allow_commands: ['cargo test', 'npm test'],
      auto_approve_paths: ['src'],
    },
  };
  store.connection.value = { api: 'online', stream: 'open', lastError: null, checkedAt: NOW };
  store.chat.value = pendingChat();
}

function clearWorkbench(): void {
  workbench.closeContext();
  workbench.contextOpen.value = false;
  workbench.contextSection.value = 'now';
  store.narrowViewport.value = false;
  store.mobileSheet.value = null;
}

function factValue(region: HTMLElement, label: string): string | null {
  return within(region).getByText(label).nextElementSibling?.textContent ?? null;
}

describe('ContextDrawer', () => {
  beforeEach(() => {
    resetAppState();
    clearWorkbench();
    seedTruthfulState();
  });

  afterEach(() => {
    clearWorkbench();
    resetAppState();
    store.projects.value = [];
    store.currentProjectId.value = null;
    vi.restoreAllMocks();
  });

  it('Now reports the actual workspace, session, connection, pending count and model pin', () => {
    render(<ContextDrawer />);
    act(() => workbench.openContext());

    const workspace = screen.getByRole('region', { name: 'Workspace and session' });
    expect(factValue(workspace, 'Project')).toBe('Workbench');
    expect(factValue(workspace, 'Project path')).toBe('/srv/project');
    expect(factValue(workspace, 'Session')).toBe('Build truthful surfaces');
    expect(factValue(workspace, 'Session workspace')).toBe('/srv/session');
    expect(factValue(workspace, 'Activity')).toBe('Waiting for approval');
    expect(factValue(workspace, 'Connection')).toBe('Live');
    expect(factValue(workspace, 'Pending approvals')).toBe('1');

    const model = screen.getByRole('region', { name: 'Model for the next run' });
    expect(within(model).getByText('Session pin')).toBeTruthy();
    expect(factValue(model, 'Model')).toBe('model-pinned');
    expect(factValue(model, 'Provider')).toBe('pinned-provider');
    expect(factValue(model, 'Reasoning')).toBe('high');

    const inputs = screen.getByRole('region', { name: 'What the agent can use' });
    expect(factValue(inputs, 'Tool-reachable workspace')).toBe('/srv/session');
    expect(within(inputs).getByText(/explicit @ paths added in Composer/)).toBeTruthy();
    expect(within(inputs).getByText(/does not attach its contents or a UI snapshot/)).toBeTruthy();
    expect(screen.getByText(/Terminal tabs are server-global/)).toBeTruthy();
    expect(screen.queryByText(/\$0/)).toBeNull();
    expect(screen.getByRole('link', { name: 'Open this session in the conversation' }).getAttribute('href')).toBe('#chat-main');
  });

  it('shows install defaults and endpoint reasoning when the session has no pin', () => {
    store.sessions.value = [{ ...store.sessions.value[0]!, model: null }];
    render(<ContextDrawer />);
    act(() => workbench.openContext());

    const model = screen.getByRole('region', { name: 'Model for the next run' });
    expect(within(model).getByText('Install default')).toBeTruthy();
    expect(factValue(model, 'Model')).toBe('gpt-4o-mini');
    expect(factValue(model, 'Provider')).toBe('default');
    expect(factValue(model, 'Reasoning')).toBe('Endpoint default');
  });

  it('direct Context switches do not change the selected or collapsed result', () => {
    store.inspectorTab.value = 'changes';
    store.inspectorCollapsed.value = true;
    render(<ContextDrawer />);
    act(() => workbench.openContext());

    fireEvent.click(screen.getByRole('tab', { name: 'Usage' }));
    expect(screen.getByTestId('delegated-session-info')).toBeTruthy();
    expect(screen.getByText(/newest provider-reported main-agent prompt/)).toBeTruthy();
    expect(screen.getByText(/Session token totals are cumulative across calls/)).toBeTruthy();
    expect(store.inspectorTab.value).toBe('changes');
    expect(store.inspectorCollapsed.value).toBe(true);

    fireEvent.click(screen.getByRole('tab', { name: 'Access' }));
    expect(screen.queryByTestId('delegated-session-info')).toBeNull();
    expect(store.inspectorTab.value).toBe('changes');
    expect(store.inspectorCollapsed.value).toBe(true);

    fireEvent.click(screen.getByRole('tab', { name: 'Usage' }));
    expect(screen.getByTestId('delegated-session-info')).toBeTruthy();
  });

  it('reveals collapsed results with the existing selection action and leaves chat untouched', () => {
    const select = vi.spyOn(actions, 'selectInspectorTab');
    store.inspectorCollapsed.value = true;
    const chat = store.chat.value;
    render(<ContextDrawer />);
    act(() => workbench.openContext());

    fireEvent.click(screen.getByRole('button', { name: 'Source control' }));
    expect(select).toHaveBeenCalledExactlyOnceWith('changes');
    expect(store.inspectorTab.value).toBe('changes');
    expect(store.inspectorCollapsed.value).toBe(false);
    expect(localStorage.getItem('karucode.inspectorCollapsed')).toBe('false');
    expect(workbench.contextOpen.value).toBe(false);
    expect(store.chat.value).toBe(chat);
  });

  it('routes built-in and plugin results only through an injected workspace reveal', () => {
    store.config.value = {
      ...store.config.value!,
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
    const onRevealResult = vi.fn();
    const select = vi.spyOn(actions, 'selectInspectorTab');
    store.inspectorTab.value = 'workspace';
    store.inspectorCollapsed.value = true;
    render(<ContextDrawer onRevealResult={onRevealResult} />);
    act(() => workbench.openContext());

    fireEvent.click(screen.getByRole('button', { name: 'Source control' }));
    expect(onRevealResult).toHaveBeenCalledExactlyOnceWith('changes');
    fireEvent.click(screen.getByRole('button', { name: 'Goals' }));
    expect(onRevealResult).toHaveBeenNthCalledWith(2, 'plugin__kc-goals__goals');

    // ChatView owns closing Context and revealing the result. A bare callback
    // must not also whistle through the standalone global-store fallback.
    expect(select).not.toHaveBeenCalled();
    expect(store.inspectorTab.value).toBe('workspace');
    expect(store.inspectorCollapsed.value).toBe(true);
    expect(workbench.contextOpen.value).toBe(true);
  });

  it('routes existing conversation links through the injected workspace navigation', () => {
    const onShowConversation = vi.fn();
    render(<ContextDrawer onShowConversation={onShowConversation} />);
    act(() => workbench.openContext());

    fireEvent.click(screen.getByRole('link', { name: '1 pending approval' }));
    fireEvent.click(screen.getByRole('link', { name: 'Conversation' }));
    fireEvent.click(screen.getByRole('link', { name: 'Open this session in the conversation' }));

    expect(onShowConversation).toHaveBeenCalledTimes(3);
    expect(workbench.contextOpen.value).toBe(true);
  });

  it('does not rewrite an already expanded desktop result or collapse state on a phone', () => {
    const persist = vi.spyOn(store, 'persistBool');
    store.inspectorCollapsed.value = false;
    const view = render(<ContextDrawer />);
    act(() => workbench.openContext());
    fireEvent.click(screen.getByRole('button', { name: 'Agents' }));
    expect(store.inspectorCollapsed.value).toBe(false);
    expect(persist).not.toHaveBeenCalled();

    view.unmount();
    store.narrowViewport.value = true;
    store.mobileSheet.value = null;
    store.inspectorCollapsed.value = true;
    render(<ContextDrawer />);
    act(() => workbench.openContext());
    fireEvent.click(screen.getByRole('button', { name: 'Files' }));
    expect(store.mobileSheet.value).toBe('inspector');
    expect(store.inspectorCollapsed.value).toBe(true);
    expect(workbench.contextOpen.value).toBe(false);
  });

  it('Access uses exact approval copy and invokes the existing Settings action', () => {
    const openSettings = vi.spyOn(actions, 'openSettings');
    render(<ContextDrawer />);
    act(() => workbench.openContext('access'));

    expect(screen.getByText('Approvals: commands only')).toBeTruthy();
    const policy = screen.getByText(/^File writes inside the workspace are allowed/);
    expect(policy.textContent).toBe(
      'File writes inside the workspace are allowed; shell commands still ask. 2 allowlisted commands, 1 auto-approved path. Change this in Settings → Approvals.',
    );
    expect(policy.getAttribute('title')).toBe(policy.textContent);
    expect(screen.getByText(/install-scoped, not a per-message switch/)).toBeTruthy();
    expect(screen.queryByRole('textbox')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Open approval settings' }));
    expect(openSettings).toHaveBeenCalledExactlyOnceWith();
    expect(store.settingsOpen.value).toBe(true);
    expect(workbench.contextOpen.value).toBe(false);
  });

  it('keeps pending approvals, current run state and errors discoverable outside Now', () => {
    render(<ContextDrawer />);
    act(() => workbench.openContext('access'));
    expect(screen.getByText('Run active')).toBeTruthy();
    expect(screen.getByRole('link', { name: '1 pending approval' }).getAttribute('href')).toBe('#chat-main');

    act(() => {
      const chat = store.chat.value;
      store.chat.value = {
        ...chat,
        running: false,
        runStatus: 'failed',
        runError: { runId: 'r_current', message: 'Provider returned a real failure', kind: 'provider' },
      };
    });
    expect(screen.getByText('Last run: Failed')).toBeTruthy();
    const error = screen.getByRole('link', { name: 'Run error' });
    expect(error.getAttribute('title')).toBe('Provider returned a real failure');
    expect(error.getAttribute('href')).toBe('#chat-main');

    fireEvent.click(screen.getByRole('tab', { name: 'Usage' }));
    expect(screen.getByRole('link', { name: 'Run error' })).toBeTruthy();
    expect(screen.getByRole('link', { name: '1 pending approval' })).toBeTruthy();
  });
});
