/**
 * The open control chat's session region: the subagent inspector under the
 * transcript. Split out of `ControlPlane.test.tsx` so a session-view change
 * never has to touch the rail's or the overview's tests.
 *
 * The `vi.mock` blocks below are a per-file copy on purpose: `vi.mock` is
 * hoisted per test file, so a shared fixture module cannot register them.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { resetControlPlane } from '../../state/controlPlane';
import { apiRequest } from '../../api/client';
import { getControlOverview, listAgents, listSessions } from '../../api/endpoints';
import { ControlPlane } from './ControlPlane';
import {
  NOW,
  agent,
  chats,
  configWith,
  controlChat,
  overviewWith,
  sessions,
} from './controlPlaneFixtures';

vi.mock('../../api/client', () => ({
  apiRequest: vi.fn(),
  toErrorMessage: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));
vi.mock('../../api/endpoints', () => ({
  listSessions: vi.fn(),
  listAgents: vi.fn(),
  getControlOverview: vi.fn(),
}));
vi.mock('../../state/actions', () => ({
  openSession: vi.fn(async (id: string) => { store.currentSessionId.value = id; return true; }),
  switchProject: vi.fn(async (id: string) => { store.currentProjectId.value = id; }),
  fillComposer: vi.fn(),
  setSessionModelSelection: vi.fn(async () => true),
  clearSessionModelSelection: vi.fn(async () => true),
  openSettings: vi.fn(),
  openAgentDetail: vi.fn(async () => {}),
  closeAgentDetail: vi.fn(),
  stopRun: vi.fn(async () => {}),
  renameSessionById: vi.fn(async () => true),
  // The plane's delete path closes the open session through this action; the
  // real one clears the id, the transcript and the remembered pin.
  closeSession: vi.fn(() => { store.currentSessionId.value = null; }),
}));
vi.mock('../ChatView', () => ({
  ChatView: (props: { emptyContent?: unknown; mentionProjects?: { name: string }[] }) =>
    <div>Normal chat{props.emptyContent ? <div>Control intro</div> : null}{props.mentionProjects ? <div>Mentions: {props.mentionProjects.map((p) => p.name).join(', ')}</div> : null}</div>,
}));

beforeEach(() => {
  vi.clearAllMocks();
  // The snapshot lives in module signals; without this a previous test's
  // overview is still on screen when the next one mounts.
  resetControlPlane();
  store.projects.value = [
    { id: 'p1', name: 'One', path: '/p1', created_at: 0, updated_at: 0, session_count: 1 },
    { id: 'p2', name: 'Two', path: '/p2', created_at: 0, updated_at: 0, session_count: 1 },
  ];
  store.currentProjectId.value = 'p1';
  store.currentSessionId.value = null;
  store.chat.value = { sessionId: null, items: [], queuedSteers: [], queuedFollowUps: [] } as never;
  store.config.value = configWith([
    { id: 'default', name: 'Default', models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini' }] },
  ]);
  store.selectedAgentId.value = null;
  store.agentHistory.value = null;
  vi.mocked(actions.openSession).mockImplementation(async (id: string) => {
    store.currentSessionId.value = id;
    return true;
  });
  vi.mocked(actions.switchProject).mockImplementation(async (id: string) => {
    store.currentProjectId.value = id;
  });
  // `/api/sessions` lists control chats too (it is the same table); the rail has
  // to keep them out of the project group by their flag.
  vi.mocked(listSessions).mockResolvedValue([...sessions, ...chats] as never);
  // The open conversation reads its own session's subagents; the default is "a
  // session that delegated nothing", which each subagent test overrides.
  vi.mocked(listAgents).mockResolvedValue([]);
  vi.mocked(getControlOverview).mockResolvedValue(overviewWith());
  vi.mocked(apiRequest).mockImplementation((async (path: string, options?: { method?: string; body?: { title?: string } }) => {
    if (path === '/api/control/sessions') {
      return options?.method === 'POST'
        ? { session_id: 'c-new', project_id: null, workspace: '/data/control-workspaces/c-new', created: true }
        : { sessions: chats };
    }
    if (options?.method === 'PATCH') return { id: path.split('/').pop(), title: options.body?.title };
    if (options?.method === 'DELETE') return { deleted: true };
    throw new Error(`unexpected ${path}`);
  }) as never);
});

/* ---------------------------------------------------------------- subagents */

describe('the open chat’s subagents', () => {
  it('lists the session’s own agents, whatever their status', async () => {
    // The rail badge comes from the snapshot (live only); the inspector reads the
    // session itself, so a finished child is still listed - which is the whole
    // point of "what did this chat delegate".
    vi.mocked(listAgents).mockResolvedValue([
      agent('researcher', { note: 'reading the control plane', iterations: 4 }),
      agent('reviewer', { status: 'completed', finished_at: NOW - 10_000, note: '' }),
    ]);
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');

    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.querySelectorAll('.cp-agent')).toHaveLength(2));
    expect(vi.mocked(listAgents)).toHaveBeenCalledWith({ session: 'c1' });
    const rows = section.querySelectorAll('.cp-agent');
    expect(rows[0]?.textContent).toContain('researcher');
    expect(rows[0]?.textContent).toContain('running');
    expect(rows[0]?.textContent).toContain('reading the control plane');
    expect(rows[0]?.textContent).toContain('4 iterations');
    expect(rows[1]?.textContent).toContain('reviewer');
    expect(rows[1]?.textContent).toContain('done');

    // "Details" reads the agent's own transcript through the existing store.
    fireEvent.click(within(section).getAllByRole('button', { name: 'Details' })[0]!);
    expect(actions.openAgentDetail).toHaveBeenCalledWith('researcher');
  });

  it('falls back to the snapshot’s live agents until its own read lands', async () => {
    // A read that never answers must not blank the list: the snapshot already
    // knows who is working.
    vi.mocked(listAgents).mockImplementation(() => new Promise(() => {}));
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [agent('researcher')],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    expect(section.textContent).toContain('researcher');
    expect(section.textContent).toContain('1 live');
  });

  it('says so when the chat has delegated nothing', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [controlChat({ id: 'c1', title: 'Control chat' })],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    expect(within(section).getByText(/No subagents yet/)).toBeTruthy();
  });

  it('never claims a chat with finished children delegated nothing', async () => {
    // The session's read is what answers this, so two completed children are two
    // rows - not "no subagents yet" and not "none loaded yet".
    vi.mocked(listAgents).mockResolvedValue([
      agent('first', { status: 'completed', finished_at: NOW - 20_000 }),
      agent('second', { status: 'cancelled', finished_at: NOW - 10_000 }),
    ]);
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [controlChat({ id: 'c1', title: 'Control chat', agents: { live: 0, total: 2 } })],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.querySelectorAll('.cp-agent')).toHaveLength(2));
    expect(section.textContent).toContain('done');
    expect(section.textContent).toContain('cancelled');
    expect(within(section).queryByText(/No subagents yet/)).toBeNull();
    expect(within(section).queryByText(/none of them loaded yet/)).toBeNull();
  });

  it('drops the previous chat’s subagents when another chat opens', async () => {
    // The rows are read per session, so switching chats must clear them first:
    // showing chat A's children under chat B's heading (until B's read lands) is
    // the same class of lie as a stale title.
    vi.mocked(listAgents).mockImplementation((async (options?: { session?: string }) => {
      if (options?.session === 'c1') return [agent('first', { session_id: 'c1' })];
      return new Promise(() => {}); // the second chat's read never answers
    }) as never);
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [
        controlChat({ id: 'c1', title: 'First chat' }),
        controlChat({ id: 'c2', title: 'Second chat' }),
      ],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^First chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.textContent).toContain('first'));

    fireEvent.click(within(group).getByRole('button', { name: /^Second chat/ }));
    // Re-query: the inspector is re-rendered for the new session, so the element
    // captured above may be detached by now.
    await waitFor(() =>
      expect(screen.getByLabelText('Subagents of this chat').textContent).not.toContain('first'),
    );
    expect(within(screen.getByLabelText('Subagents of this chat')).getByText(/No subagents yet/)).toBeTruthy();
  });
});
