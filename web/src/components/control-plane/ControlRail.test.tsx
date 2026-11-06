/**
 * The control rail: the two lists (control chats vs project sessions), the
 * per-row actions and the session filters. Split out of `ControlPlane.test.tsx`
 * so a rail change never has to touch the overview's or the session's tests.
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
import { chats, configWith, overviewWith, projectSession, sessions } from './controlPlaneFixtures';

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

/* ------------------------------------------------------------- the two lists */

describe('the rail separates control chats from project sessions', () => {
  it('lists both groups, each from its own source', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const controlGroup = within(browser).getByLabelText('Control chats');
    expect(within(controlGroup).getByText('Control chat')).toBeTruthy();
    expect(within(controlGroup).getByText('Untitled chat')).toBeTruthy();
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).getByText('First')).toBeTruthy();
    expect(within(projectGroup).getByText('Second')).toBeTruthy();
  });

  it('never draws a control chat as a project row — by the control flag, not by id', async () => {
    // `c1` is in both reads (the id guard would have caught it). `c_flag` is only
    // in `/api/sessions`, flagged as a chat: the id set does not know it, so only
    // the flag can keep it out of the Sessions group.
    vi.mocked(listSessions).mockResolvedValue([
      ...sessions,
      ...chats,
      projectSession({ id: 'c_flag', title: 'Flag only chat', project_id: null, control: true }),
    ] as never);
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).queryByText('Control chat')).toBeNull();
    expect(within(projectGroup).queryByText('Flag only chat')).toBeNull();
    // ...and the real chat is listed exactly once, in its own group.
    expect(within(browser).getAllByText('Control chat')).toHaveLength(1);
  });

  it('keeps the model off the row: the chat name is not a model label', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    const row = within(group).getByRole('button', { name: /^Control chat/ });

    // The snapshot still carries a `model_label` for this chat, but the rail must
    // not draw it: a chat's model is not part of its identity, and the composer
    // is the one place the model is named.
    expect(row.textContent).not.toContain('gpt-4o-mini');
    expect(row.querySelector('.cp-badge-model')).toBeNull();
    expect(row.querySelector('[title^="Model:"]')).toBeNull();

    const agents = row.querySelector('.cp-badge.cp-badge-agents');
    expect(agents?.textContent).toBe('1/2 agents');
    expect(agents?.getAttribute('title')).toBe('1 subagent running · 2 total');
  });

  it('hides the badge a chat has nothing to say for', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    const untitled = within(group).getByRole('button', { name: /^Untitled chat/ });
    // No subagents: a "0/0" chip would be noise, not information.
    expect(untitled.querySelector('.cp-badge-agents')).toBeNull();
  });
});

/* ---------------------------------------------------------------- row actions */

describe('row actions', () => {
  it('renames a chat from its kebab and re-reads the snapshot', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: 'Actions for Control chat' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));

    const dialog = await screen.findByRole('dialog');
    fireEvent.input(within(dialog).getByLabelText('Session title'), { target: { value: 'Release planning' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rename' }));

    // The shared action owns the PATCH (and the optimistic row patch the
    // workspace rail needs); the plane only asks it and re-reads its snapshot.
    await waitFor(() => expect(actions.renameSessionById).toHaveBeenCalledWith('c1', 'Release planning'));
    // A rename is only real once the rail's own source says so.
    await waitFor(() => expect(getControlOverview).toHaveBeenCalledTimes(2));
  });

  it('deletes the open chat after a confirmation and returns to the overview', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');

    // A control chat is listed on the left while it is open, so its kebab is here.
    const rail = screen.getByLabelText('Sessions across projects');
    fireEvent.click(within(rail).getByRole('button', { name: 'Actions for Control chat' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete…' }));

    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/cannot be undone/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete session' }));

    await waitFor(() => expect(apiRequest).toHaveBeenCalledWith('/api/sessions/c1', { method: 'DELETE' }));
    await waitFor(() => expect(getControlOverview).toHaveBeenCalledTimes(2));
    // Deleting what was open leaves nothing to show, so the overview comes back.
    expect(await screen.findByText(/What are we building/)).toBeTruthy();
    // ...and the workspace store does not keep pointing at the deleted row.
    expect(store.currentSessionId.value).toBeNull();
  });
});

/* ------------------------------------------------------------ session filters */

describe('session filters', () => {
  it('keeps the chats when the project filter narrows to running only', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const filters = within(browser).getByRole('group', { name: 'Filter sessions' });
    fireEvent.click(within(filters).getByRole('button', { name: /Running/ }));
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).getByText('First')).toBeTruthy();
    expect(within(projectGroup).queryByText('Second')).toBeNull();
    // Filters are about project sessions; a chat is not one.
    expect(within(browser).getByLabelText('Control chats')).toBeTruthy();
  });

  it('keeps a distinct word on every state, not just a coloured dot', async () => {
    // `.cp-state.good, .cp-state.idle` used to be `font-size: 0`, so "Finished",
    // "Cancelled" and "No runs" all rendered as the same grey dot. The word is
    // now always in the row, including the ≤520px session header.
    const quiet = [
      projectSession({ id: 's_done', title: 'Done one', last_run_status: 'completed' }),
      projectSession({ id: 's_cancelled', title: 'Cancelled one', last_run_status: 'cancelled' }),
      projectSession({ id: 's_never', title: 'Never ran' }),
    ];
    vi.mocked(listSessions).mockResolvedValue([...quiet, ...sessions, ...chats] as never);
    render(<ControlPlane />);

    const browser = await screen.findByLabelText('Sessions across projects');
    const row = (title: string): HTMLElement =>
      within(browser).getByRole('button', { name: new RegExp(`^${title}`) });

    expect(within(row('Done one')).getByText('Finished')).toBeTruthy();
    expect(within(row('Cancelled one')).getByText('Cancelled')).toBeTruthy();
    expect(within(row('Never ran')).getByText('No runs')).toBeTruthy();
    // The tooltip repeats the state for a reader who sees only the colour cue.
    expect(within(row('Done one')).getByTitle('Latest state: Finished')).toBeTruthy();
  });

  it('offers project mentions to the control chat', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    fireEvent.click(within(controlGroup).getByRole('button', { name: /^Control chat/ }));
    expect(await screen.findByText('Mentions: One, Two')).toBeTruthy();
  });
});
