import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import type {
  AgentSummary,
  Config,
  ControlChatSummary,
  ControlOverview,
  SessionSummary,
} from '../../api/types';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { resetControlPlane } from '../../state/controlPlane';
import { apiRequest } from '../../api/client';
import { getControlOverview, listAgents, listSessions } from '../../api/endpoints';
import { ControlPlane } from './ControlPlane';
import {
  agentBadgeTitle,
  agentStatusWord,
  controlStarter,
  initials,
  sessionStatus,
} from './controlModel';
import { relativeTime } from '../../lib/format';

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

/* ---------------------------------------------------------------- fixtures */

/**
 * The part of `Config` this screen reads (`providers` for both pickers and
 * `active` for the default pin). Everything else is filled by the server and is
 * irrelevant to the control plane, so it is cast rather than copied.
 */
function configWith(
  providers: { id: string; name: string; models: { id: string; name?: string; reasoning?: boolean }[] }[],
): Config {
  return {
    providers: providers.map((profile) => ({
      id: profile.id,
      name: profile.name,
      provider: 'openai_compatible',
      base_url: 'http://127.0.0.1:4000/v1',
      models: profile.models,
      request_timeout_secs: 300,
      api_key_set: true,
    })),
    active: { provider: providers[0]?.id ?? null, model: providers[0]?.models[0]?.id ?? null },
  } as unknown as Config;
}

const NOW = Date.now();

function projectSession(overrides: Partial<SessionSummary> & { id: string }): SessionSummary {
  return {
    title: overrides.id,
    workspace: '/p1',
    project_id: 'p1',
    created_at: 0,
    updated_at: NOW,
    message_count: 1,
    running: false,
    ...overrides,
  } as SessionSummary;
}

const sessions: SessionSummary[] = [
  projectSession({ id: 's1', title: 'First', project_id: 'p1', running: true, updated_at: NOW }),
  projectSession({ id: 's2', title: 'Second', project_id: 'p2', last_run_status: 'completed', updated_at: NOW - 60_000, message_count: 2 }),
];

function controlChat(overrides: Partial<ControlChatSummary> & { id: string }): ControlChatSummary {
  return {
    title: 'Control chat',
    workspace: `/data/control-workspaces/${overrides.id}`,
    project_id: null,
    control: true,
    running: false,
    created_at: 0,
    updated_at: NOW,
    message_count: 4,
    agents: { live: 0, total: 0 },
    model_label: null,
    ...overrides,
  } as ControlChatSummary;
}

const chats: ControlChatSummary[] = [
  // The row the browser script and every rail assertion looks for: a title and a
  // live-plus-finished subagent count. It also carries a `model_label`, which
  // the rail must NOT draw - the model belongs to the composer, not to the name.
  controlChat({ id: 'c1', title: 'Control chat', agents: { live: 1, total: 2 }, model_label: 'gpt-4o-mini' }),
  // Created without a prompt, so the server sends an empty title.
  controlChat({ id: 'c2', title: '', updated_at: NOW - 120_000 }),
];

function agent(id: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    id,
    session_id: 'c1',
    parent_run_id: 'r_1',
    parent_agent_id: null,
    name: id,
    role: 'researcher',
    task: `do ${id}`,
    status: 'running',
    model: 'gpt-4o-mini',
    iterations: 4,
    summary: '',
    error: null,
    owned_paths: [],
    created_at: NOW - 60_000,
    updated_at: NOW - 30_000,
    finished_at: null,
    background: false,
    note: 'reading the control plane',
    session_title: 'Control chat',
    workspace: '/data/control-workspaces/c1',
    project_id: null,
    ...overrides,
  } as AgentSummary;
}

function overviewWith(overrides: Partial<ControlOverview> = {}): ControlOverview {
  return {
    generated_at: NOW,
    sessions: { total: 2, running: 1, failed: 0, control: chats.length },
    agents: { live: 1, paused: 0, failed: 0, total: 2 },
    approvals: { pending: 0 },
    projects: [
      { id: 'p1', name: 'One', path: '/p1', sessions: 1, running: 1 },
      { id: 'p2', name: 'Two', path: '/p2', sessions: 1, running: 0 },
    ],
    control: chats,
    running: [sessions[0] as SessionSummary],
    attention: [],
    waiting: [],
    recent: [sessions[1] as SessionSummary],
    live_agents: [agent('researcher')],
    ...overrides,
  };
}

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

/* --------------------------------------------------------------- navigation */

describe('opening conversations', () => {
  it('opens a control chat and shows the control intro', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    // Anchored at the start: the row's kebab is named "Actions for Control chat",
    // so a bare /Control chat/ now matches two buttons.
    fireEvent.click(within(controlGroup).getByRole('button', { name: /^Control chat/ }));
    expect(await screen.findByText('Normal chat')).toBeTruthy();
    expect(screen.getByText('Control intro')).toBeTruthy();
    expect(actions.openSession).toHaveBeenCalledWith('c1');
    // No project switch for a project-less session.
    expect(actions.switchProject).not.toHaveBeenCalled();
  });

  it('creates another control chat without disturbing the first', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    fireEvent.click(within(controlGroup).getByRole('button', { name: 'New control chat' }));
    await waitFor(() => expect(apiRequest).toHaveBeenCalledWith('/api/control/sessions', { method: 'POST', body: {} }));
    await waitFor(() => expect(actions.openSession).toHaveBeenCalledWith('c-new'));
  });

  it('opens a session in its own project using the normal chat', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: /^Second/ }));
    expect(await screen.findByText('Normal chat')).toBeTruthy();
    expect(screen.queryByText('Control intro')).toBeNull();
    expect(actions.switchProject).toHaveBeenCalledWith('p2');
    expect(actions.openSession).toHaveBeenCalledWith('s2');
  });

  it('returns to the overview from an open session', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: /^First/ }));
    await screen.findByText('Normal chat');
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect(await screen.findByText(/What are we building/)).toBeTruthy();
  });

  it('headers a control chat as the control plane, and leaves the model to the composer', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');

    const head = screen.getByLabelText('Open session');
    expect(within(head).getByText('Control plane · across projects')).toBeTruthy();
    // The composer owns the model and the reasoning effort, and it is the one
    // place they stay shown. The header used to mount a second copy of the same
    // two selectors, and that copy was what came and went as the snapshot
    // refreshed - so the header must not carry a trigger at all.
    expect(within(head).queryByRole('button', { name: /^Model for this session:/ })).toBeNull();
    expect(within(head).queryByRole('button', { name: /^Reasoning effort:/ })).toBeNull();
    // A chat is not inside a project, so there is nothing to open.
    expect(within(head).queryByRole('button', { name: 'Open in project' })).toBeNull();
  });

  it('keeps navigation errors visible after a successful list refresh', async () => {
    store.projects.value = store.projects.value.filter((project) => project.id !== 'p2');
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: /^Second/ }));
    expect((await screen.findByRole('alert')).textContent).toContain('project is unavailable');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh sessions' }));
    await waitFor(() => expect(getControlOverview).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('alert').textContent).toContain('project is unavailable');
  });
});

/* ------------------------------------------------------------------ launch */

describe('the launch box', () => {
  it('carries a launch draft into the control chat composer', async () => {
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat');
    fireEvent.input(input, { target: { value: 'Add a health endpoint to the API' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.fillComposer).toHaveBeenCalledWith(
      expect.stringContaining('Add a health endpoint to the API'),
    ));
    expect(vi.mocked(actions.fillComposer).mock.calls[0]?.[0]).toContain('(project_id: p1)');
  });

  it('pins the chosen model before handing over the draft', async () => {
    store.config.value = configWith([
      { id: 'default', name: 'Default', models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini', reasoning: true }] },
      { id: 'fast', name: 'Fast', models: [{ id: 'gpt-5-mini', name: 'GPT-5 mini' }] },
    ]);
    const order: string[] = [];
    vi.mocked(actions.setSessionModelSelection).mockImplementation(async () => { order.push('pin'); return true; });
    vi.mocked(actions.fillComposer).mockImplementation(() => { order.push('fill'); });

    render(<ControlPlane />);
    const model = await screen.findByLabelText('Model for the new chat') as HTMLSelectElement;
    fireEvent.change(model, { target: { value: 'fast::gpt-5-mini' } });
    fireEvent.change(screen.getByLabelText('Reasoning for the new chat'), { target: { value: 'high' } });
    fireEvent.input(screen.getByLabelText('Start something with control chat'), { target: { value: 'Ship it' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));

    await waitFor(() => expect(order).toEqual(['pin', 'fill']));
    // `default` effort is sent as `null`: the endpoint decides, which is what the
    // composer's own selector does when it shows "Default".
    expect(actions.setSessionModelSelection).toHaveBeenCalledWith('fast', 'gpt-5-mini', 'high');
    expect(actions.fillComposer).toHaveBeenCalledWith(expect.stringContaining('Ship it'));
  });

  it('does not offer reasoning for a model that cannot reason', async () => {
    store.config.value = configWith([
      { id: 'plain', name: 'Plain', models: [{ id: 'no-reasoning', name: 'No reasoning', reasoning: false }] },
    ]);
    render(<ControlPlane />);
    const model = await screen.findByLabelText('Model for the new chat');
    expect(model).toBeTruthy();
    // `reasoning: false` is a positive "cannot reason" — the same rule the
    // composer's ReasoningSelector follows.
    expect(screen.queryByLabelText('Reasoning for the new chat')).toBeNull();
  });

  it('retains the launch prompt when opening the control chat fails', async () => {
    let finish: ((ok: boolean) => void) | undefined;
    vi.mocked(actions.openSession).mockImplementation(
      () => new Promise<boolean>((resolve) => { finish = resolve; }),
    );
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat') as HTMLInputElement;
    fireEvent.input(input, { target: { value: 'Retry this prompt' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.openSession).toHaveBeenCalled());
    await act(async () => {
      finish?.(false);
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Could not open session.'));
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect((await screen.findByLabelText('Start something with control chat') as HTMLInputElement).value).toBe(
      'Retry this prompt',
    );
    expect(actions.fillComposer).not.toHaveBeenCalled();
  });

  it('does not overwrite edits made while a successful launch is pending', async () => {
    let finish: ((ok: boolean) => void) | undefined;
    vi.mocked(actions.openSession).mockImplementation(
      () => new Promise<boolean>((resolve) => { finish = resolve; }),
    );
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat') as HTMLInputElement;
    fireEvent.input(input, { target: { value: 'submitted prompt' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.openSession).toHaveBeenCalled());
    fireEvent.input(input, { target: { value: 'edited while pending' } });
    const opened = vi.mocked(actions.openSession).mock.calls[0]?.[0];
    await act(async () => {
      store.currentSessionId.value = opened ?? null;
      finish?.(true);
      await Promise.resolve();
    });

    await waitFor(() => expect(actions.fillComposer).toHaveBeenCalledWith(
      expect.stringContaining('submitted prompt'),
    ));
    // The input is unmounted as soon as c1 opens, so the prompt was accepted.
    expect(screen.queryByLabelText('Start something with control chat')).toBeNull();
    // Re-open the overview to inspect the preserved value without remounting state.
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect((await screen.findByLabelText('Start something with control chat') as HTMLInputElement).value).toBe(
      'edited while pending',
    );
  });
});

/* ------------------------------------------------------------------ rename */

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

/* ---------------------------------------------------------------- overview */

describe('the overview snapshot', () => {
  it('drives every counter from the one read', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      sessions: { total: 9, running: 4, failed: 2, control: 3 },
      agents: { live: 5, paused: 1, failed: 0, total: 8 },
      projects: [
        { id: 'p1', name: 'One', path: '/p1', sessions: 4, running: 3 },
        { id: 'p2', name: 'Two', path: '/p2', sessions: 5, running: 1 },
        { id: 'p3', name: 'Three', path: '/p3', sessions: 0, running: 0 },
      ],
    }));
    render(<ControlPlane />);
    const summary = await screen.findByLabelText('Workspace summary');
    const counters = [...summary.querySelectorAll('div')].map((cell) => [
      cell.querySelector('strong')?.textContent,
      cell.querySelector('span')?.textContent,
    ]);
    expect(counters).toEqual([
      ['4', 'running'],
      ['2', 'chats'],
      ['9', 'sessions'],
      ['5', 'agents'],
      ['3', 'projects'],
    ]);
    // The per-project counts are the snapshot's, not a recount of the rail.
    const projects = screen.getByLabelText('Projects');
    expect(within(projects).getByText('4 sessions · 3 running')).toBeTruthy();
    expect(within(projects).getByText('0 sessions')).toBeTruthy();
  });

  it('lists the snapshot’s attention, running, recent and live agents', async () => {
    const failed = projectSession({ id: 's_failed', title: 'Broken build', project_id: 'p2', last_run_status: 'failed' });
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      attention: [failed],
      live_agents: [agent('reviewer', { note: 'reading the diff' })],
    }));
    render(<ControlPlane />);
    expect(await screen.findByText('Needs attention')).toBeTruthy();
    expect(screen.getByText('Broken build')).toBeTruthy();
    expect(screen.getByText('reading the diff')).toBeTruthy();
    expect(screen.getByRole('button', { name: /Control chat →/ })).toBeTruthy();
  });

  it('calls out a run parked on an approval, rather than calling it running', async () => {
    const stalled = projectSession({ id: 's_wait', title: 'Awaiting approval', project_id: 'p2', running: true });
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      approvals: { pending: 1 },
      waiting: [stalled],
      control: [controlChat({ id: 'c1', title: 'Control chat', running: true, pending_approvals: 2 })],
    }));
    render(<ControlPlane />);
    // Above the dashboard: this is the one thing that cannot progress alone.
    const banner = await screen.findByText(/waiting on you — open the session/);
    expect(banner.textContent).toContain('1');

    const waiting = screen.getByLabelText('Waiting on you');
    expect(within(waiting).getByText('Awaiting approval')).toBeTruthy();
    expect(within(waiting).getByText('Needs you')).toBeTruthy();

    // The chat's own row says the same thing instead of the misleading "Running".
    const row = within(screen.getByLabelText('Control chats')).getByRole('button', { name: /^Control chat/ });
    expect(within(row).getByText('Needs you')).toBeTruthy();
  });

  it('marks a failed read stale without erasing the last snapshot', async () => {
    render(<ControlPlane />);
    const summary = await screen.findByLabelText('Workspace summary');
    expect(summary.querySelector('strong')?.textContent).toBe('1');

    vi.mocked(getControlOverview).mockRejectedValue(new Error('HTTP 503'));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh sessions' }));
    await waitFor(() => expect(screen.getByText(/stale · HTTP 503/)).toBeTruthy());
    // The numbers are still the last good read: "nothing is running" is a
    // different answer, and a wrong one.
    expect(screen.getByLabelText('Workspace summary').querySelector('strong')?.textContent).toBe('1');
  });
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

/* ------------------------------------------------------------------- misc */

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

describe('control plane formatting', () => {
  it('derives compact project initials', () => {
    expect(initials('web-platform')).toBe('WP');
    expect(initials('karucode')).toBe('K');
    expect(initials('---')).toBe('?');
  });

  it('reports honest status labels', () => {
    expect(sessionStatus({ running: true }).label).toBe('Running');
    expect(sessionStatus({ running: false, last_run_status: 'failed' })).toEqual({ label: 'Failed', tone: 'danger' });
    expect(sessionStatus({ running: false }).label).toBe('No runs');
  });

  it('spells out a subagent badge', () => {
    expect(agentBadgeTitle({ live: 1, total: 2 })).toBe('1 subagent running · 2 total');
    expect(agentBadgeTitle({ live: 0, total: 3 })).toBe('0 subagents running · 3 total');
  });

  it('names every subagent status as a word', () => {
    expect(agentStatusWord('completed')).toBe('done');
    expect(agentStatusWord('paused')).toBe('paused');
    expect(agentStatusWord('interrupted')).toBe('interrupted');
  });

  it('builds a starter that names the target project by id', () => {
    expect(controlStarter({ id: 'p9', name: 'web-platform' })).toBe(
      'Start work in project "web-platform" (project_id: p9).\n\n',
    );
  });

  it('formats recent times', () => {
    const now = 1_000_000_000;
    expect(relativeTime(now - 10_000, now)).toBe('just now');
    expect(relativeTime(now - 120_000, now)).toBe('2m ago');
    expect(relativeTime(now - 7_200_000, now)).toBe('2h ago');
  });
});
