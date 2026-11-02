import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { AgentSummary, StoredMessage } from '../api/types';
import { AgentsTab } from './AgentsTab';
import { Inspector } from './Inspector';
import { emptyChatState, type AgentView } from '../state/chat';
import * as store from '../state/store';
import { jsonResponse, resetAppState } from '../test/fixtures';

const NOW = 1_700_000_000_000;

function agent(id: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    id,
    session_id: 's_ab12',
    parent_run_id: 'r_x',
    parent_agent_id: null,
    name: id,
    role: 'reviewer',
    task: `review ${id}`,
    status: 'completed',
    model: 'gpt-5',
    iterations: 3,
    summary: '',
    error: null,
    owned_paths: [],
    created_at: NOW,
    updated_at: NOW,
    finished_at: NOW + 1_000,
    background: false,
    note: '',
    progress: null,
    session_title: 'Review the harness',
    workspace: '/work/karucode',
    project_id: 'p_1',
    ...overrides,
  };
}

function stored(partial: Partial<StoredMessage> & Pick<StoredMessage, 'id' | 'role'>): StoredMessage {
  return {
    session_id: 's_ab12',
    run_id: 'r_x',
    agent_id: 'a_runner',
    content: '',
    tool_calls: null,
    tool_call_id: null,
    name: null,
    created_at: NOW,
    ...partial,
  };
}

/** The tab refreshes on mount; keep the mock consistent with the seeded list. */
function stubFetch(options: { detail?: unknown } = {}): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/agents/')) {
        return jsonResponse(
          options.detail ?? {
            agent: agent('a_runner', { status: 'running', finished_at: null }),
            messages: [],
            messages_truncated: false,
            messages_limit: 2000,
          },
        );
      }
      if (url.includes('/api/agents')) {
        return jsonResponse({ agents: store.agentsAll.value, running: 0 });
      }
      return jsonResponse({});
    }),
  );
}

describe('AgentsTab', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = 'p_1';
    stubFetch();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('lists every agent in the project with status counts', async () => {
    store.agentsAll.value = [
      agent('a_done', { status: 'completed' }),
      agent('a_failed', { status: 'failed', created_at: NOW - 10 }),
      agent('a_live', { status: 'running', finished_at: null, created_at: NOW - 20 }),
    ];
    const { container } = render(<AgentsTab />);

    expect(container.querySelectorAll('.agent-card')).toHaveLength(3);
    // Running agents come first, then newest first.
    expect(
      [...container.querySelectorAll('.agent-card .agent-name')].map((node) => node.textContent),
    ).toEqual(['a_live', 'a_done', 'a_failed']);
    expect(screen.getByText('1 running · 3 total')).not.toBeNull();
    // Every status has a filter chip with its count, including zeros.
    expect(screen.getByRole('button', { name: /All/ })).not.toBeNull();
    expect(screen.getByRole('button', { name: /Failed/ })).not.toBeNull();
    expect(screen.getByRole('button', { name: /Cancelled/ }).hasAttribute('disabled')).toBe(true);

    await waitFor(() => expect(fetch).toHaveBeenCalled());
  });

  it('counts a paused agent as live in the inspector badge', () => {
    // The badge is `store.runningAgents`, rendered on the inspector's Agents tab
    // (`Inspector.tsx:89`). It counted `status === 'running'` only, while the
    // server's own tally is running|paused ("a paused agent still holds a subagent
    // slot", `routes/agents.rs:80-89`) and every row in the tab already sorts and
    // labels a paused agent as live. A project whose agents were all paused
    // therefore showed no badge at all while still holding their slots.
    store.agentsAll.value = [
      agent('a_run', { status: 'running', finished_at: null }),
      agent('a_paused', { status: 'paused', finished_at: null }),
      agent('a_done', { status: 'completed' }),
      agent('a_failed', { status: 'failed' }),
    ];

    expect(store.runningAgents.value).toBe(2);

    // The tab pill shows that number, so the two surfaces cannot drift.
    render(<Inspector />);
    const tab = screen.getByRole('tab', { name: /Agents/ });
    expect(tab.querySelector('.tab-count')?.textContent).toBe('2');
  });

  it('filters the list to one status', () => {
    store.agentsAll.value = [
      agent('a_done', { status: 'completed' }),
      agent('a_failed', { status: 'failed' }),
    ];
    const { container } = render(<AgentsTab />);

    fireEvent.click(screen.getByRole('button', { name: /Failed/ }));
    expect(
      [...container.querySelectorAll('.agent-card .agent-name')].map((node) => node.textContent),
    ).toEqual(['a_failed']);

    // An empty filtered list explains itself instead of looking like no agents.
    fireEvent.click(screen.getByRole('button', { name: /Cancelled/ }));
    expect(screen.getByText('No agents match this filter')).not.toBeNull();
  });

  it('shows the project and the current session as two scopes', () => {
    const sessionAgent: AgentView = {
      ...agent('a_session', { status: 'completed' }),
      owned_paths: [],
    };
    store.chat.value = { ...emptyChatState('s_ab12'), agents: [sessionAgent] };
    store.currentSessionId.value = 's_ab12';
    store.agentsAll.value = [agent('a_project')];
    const { container } = render(<AgentsTab />);

    expect(container.querySelectorAll('.agent-card')).toHaveLength(1);
    expect(screen.getByText('a_project')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'This session' }));
    expect(screen.getByText('a_session')).not.toBeNull();
    expect(screen.queryByText('a_project')).toBeNull();
  });

  it('drills into an agent and shows its stored transcript', async () => {
    store.agentsAll.value = [agent('a_runner', { status: 'running', finished_at: null })];
    stubFetch({
      detail: {
        agent: agent('a_runner', { status: 'running', finished_at: null }),
        messages: [
          stored({ id: 1, role: 'user', content: 'review the diff' }),
          stored({
            id: 2,
            role: 'assistant',
            tool_calls: [{ id: 'call_1', name: 'read_file', arguments: '{"path":"src/lib.rs"}' }],
          }),
          stored({ id: 3, role: 'tool', tool_call_id: 'call_1', name: 'read_file', content: 'fn main() {}' }),
          stored({ id: 4, role: 'assistant', content: 'looks fine' }),
        ],
        messages_truncated: false,
        messages_limit: 2000,
      },
    });
    const { container } = render(<AgentsTab />);

    fireEvent.click(container.querySelector('.agent-card')!);

    await waitFor(() => expect(container.querySelector('.agent-detail')).not.toBeNull());
    await waitFor(() => expect(screen.getByText('looks fine')).not.toBeNull());
    // The task is shown once at the top; the activity list is what the agent did.
    expect(screen.getByText('review a_runner')).not.toBeNull();
    expect(screen.queryByText('review the diff')).toBeNull();
    expect(screen.getByText('read_file')).not.toBeNull();
    expect(screen.getByText('fn main() {}')).not.toBeNull();

    // Back returns to the list without losing the agents.
    fireEvent.click(screen.getByRole('button', { name: /Back to all agents/ }));
    expect(container.querySelectorAll('.agent-card')).toHaveLength(1);
  });

  it('explains the empty state', () => {
    render(<AgentsTab />);
    expect(screen.getByText('No subagents yet')).not.toBeNull();
    expect(screen.getByText('0 running · 0 total')).not.toBeNull();
  });
});
