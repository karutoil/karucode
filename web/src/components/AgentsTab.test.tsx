import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { AgentSummary, StoredMessage } from '../api/types';
import { AgentsTab } from './AgentsTab';
import { Inspector } from './Inspector';
import { emptyChatState, type AgentView } from '../state/chat';
import * as actions from '../state/actions';
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
    vi.restoreAllMocks();
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

    fireEvent.click(
      screen.getByRole('button', { name: 'Open details for a_runner' }),
    );

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

  it('does not open the detail when Enter or Space is pressed on Pause or Stop', async () => {
    // The card used to be the button (`role="button"` on the article), so a
    // bubbled Enter/Space from a control inside it ran the card's own handler as
    // well: pausing an agent also drilled into it. The controls are siblings of
    // a separate Details button now, and Enter on them must only pause or stop.
    const control = vi.spyOn(actions, 'controlAgent').mockResolvedValue(undefined);
    store.agentsAll.value = [agent('a_live', { status: 'running', finished_at: null })];
    const { container } = render(<AgentsTab />);

    const pause = screen.getByRole('button', { name: 'Pause a_live' });
    pause.focus();
    fireEvent.keyDown(pause, { key: 'Enter' });
    fireEvent.keyUp(pause, { key: 'Enter' });
    fireEvent.click(pause);
    expect(control).toHaveBeenLastCalledWith('a_live', 'pause');
    expect(container.querySelector('.agent-detail')).toBeNull();

    const stop = screen.getByRole('button', { name: 'Stop a_live' });
    stop.focus();
    fireEvent.keyDown(stop, { key: ' ' });
    fireEvent.click(stop);
    expect(control).toHaveBeenLastCalledWith('a_live', 'cancel');
    expect(container.querySelector('.agent-detail')).toBeNull();
  });

  it('exposes opening the detail as a named, native button on the card', () => {
    // `role="button"` on the article made the whole card a control wrapping the
    // Pause/Stop buttons - nested interactive semantics no screen reader can
    // present. The card is a plain article; one real button owns the action.
    store.agentsAll.value = [agent('a_plain')];
    const { container } = render(<AgentsTab />);

    const card = container.querySelector<HTMLElement>('.agent-card');
    expect(card).not.toBeNull();
    expect(card!.getAttribute('role')).toBeNull();
    expect(card!.getAttribute('tabindex')).toBeNull();
    const open = screen.getByRole('button', { name: 'Open details for a_plain' });
    expect(open.tagName).toBe('BUTTON');
    expect(open.getAttribute('title')).toBe('Open details for a_plain');
  });

  it('gives the progress bar a name and its value', () => {
    // A bar is a picture of a number: without a role and values, the only place
    // the claim appeared was a `title` tooltip.
    store.agentsAll.value = [
      agent('a_progress', {
        status: 'running',
        finished_at: null,
        note: 'working',
        progress: 0.5,
      }),
    ];
    const { container } = render(<AgentsTab />);

    const bar = screen.getByRole('progressbar', { name: 'Progress for a_progress' });
    expect(bar.getAttribute('aria-valuenow')).toBe('50');
    expect(bar.getAttribute('aria-valuemin')).toBe('0');
    expect(bar.getAttribute('aria-valuemax')).toBe('100');
    expect(bar.getAttribute('aria-valuetext')).toBe('50% complete');
    // The fill is still drawn, so the visual claim and the announced one agree.
    expect(container.querySelector('.agent-progress-fill')?.getAttribute('style')).toContain(
      'width: 50%',
    );
  });

  it('explains the empty state', () => {
    render(<AgentsTab />);
    expect(screen.getByText('No subagents yet')).not.toBeNull();
    expect(screen.getByText('0 running · 0 total')).not.toBeNull();
  });

  /**
   * `subagents.max_concurrent = 0` is "no ceiling", so a single fan-out can be
   * hundreds of children. The roster is a plain `map`, so without a page the tab
   * would build every card - each with a live label and a progress bar that
   * re-renders on the clock tick - for agents the user cannot scroll to quickly.
   */
  it('renders a page of a large roster and offers the rest', async () => {
    const roster: AgentSummary[] = [];
    for (let i = 0; i < 150; i += 1) {
      roster.push(agent(`a_${i}`, { status: 'completed', created_at: NOW - i }));
    }
    // Four live agents buried at the bottom of the newest-first order: the sort
    // must lift them into the rendered page, or the pinned list hides exactly the
    // rows the user opened the tab for.
    for (let i = 0; i < 4; i += 1) {
      roster.push(
        agent(`a_live_${i}`, { status: 'running', finished_at: null, created_at: NOW - 1_000 - i }),
      );
    }
    store.agentsAll.value = roster;

    const { container } = render(<AgentsTab />);
    expect(container.querySelectorAll('.agent-card')).toHaveLength(60);
    expect(screen.getByText('4 running · 154 total')).not.toBeNull();

    const rendered = new Set(
      [...container.querySelectorAll('.agent-card .agent-name')].map((node) => node.textContent),
    );
    for (let i = 0; i < 4; i += 1) {
      expect(rendered.has(`a_live_${i}`), `live agent a_live_${i} must be on the first page`).toBe(
        true,
      );
    }

    // The tail says how much is left and how to see it.
    expect(screen.getByText(/Showing 60 of 154 agents/)).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show all 154 agents' }));
    await waitFor(() => expect(container.querySelectorAll('.agent-card')).toHaveLength(154));
    expect(screen.queryByText(/Showing 60 of 154 agents/)).toBeNull();
  });

  it('does not cap a roster that fits on one page', () => {
    store.agentsAll.value = [agent('a_1'), agent('a_2')];
    const { container } = render(<AgentsTab />);
    expect(container.querySelectorAll('.agent-card')).toHaveLength(2);
    expect(container.querySelector('.agent-list-more')).toBeNull();
    expect(screen.queryByRole('button', { name: /Show all/ })).toBeNull();
  });
});


describe('agent history pagination', () => {
  beforeEach(() => resetAppState());
  afterEach(() => { resetAppState(); vi.unstubAllGlobals(); });
  it('loads an older page through the control and removes the incomplete notice', async () => {
    const row = agent('a_runner');
    store.agentsAll.value = [row];
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/agents/')) return jsonResponse({ agent: row, messages: [stored({ id: url.includes('before=') ? 1 : 2, role: 'assistant', content: url.includes('before=') ? 'older output' : 'newer output' })], messages_truncated: !url.includes('before='), messages_before: url.includes('before=') ? null : 2, messages_limit: 2000 });
      return jsonResponse({ agents: [row], running: 0 });
    }));
    render(<AgentsTab />);
    fireEvent.click(await screen.findByRole('button', { name: /Open details for a_runner/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Load older history' }));
    expect(await screen.findByText('older output')).toBeTruthy();
    expect(screen.getByText('newer output')).toBeTruthy();
    expect(screen.queryByText('Earlier history is not loaded.')).toBeNull();
  });
});
