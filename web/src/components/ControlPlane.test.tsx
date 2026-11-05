import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../state/store';
import * as actions from '../state/actions';
import { apiRequest } from '../api/client';
import { listAgents, listSessions } from '../api/endpoints';
import { ControlPlane, controlStarter, initials, sessionStatus, shortAgo } from './ControlPlane';

vi.mock('../api/client', () => ({ apiRequest: vi.fn() }));
vi.mock('../api/endpoints', () => ({ listSessions: vi.fn(), listAgents: vi.fn() }));
vi.mock('../state/actions', () => ({
  openSession: vi.fn(async (id: string) => { store.currentSessionId.value = id; return true; }),
  switchProject: vi.fn(async (id: string) => { store.currentProjectId.value = id; }),
  fillComposer: vi.fn(),
}));
vi.mock('./ChatView', () => ({
  ChatView: (props: { emptyContent?: unknown; mentionProjects?: { name: string }[] }) =>
    <div>Normal chat{props.emptyContent ? <div>Control intro</div> : null}{props.mentionProjects ? <div>Mentions: {props.mentionProjects.map((p) => p.name).join(', ')}</div> : null}</div>,
}));

const sessions = [
  { id: 's1', project_id: 'p1', title: 'First', workspace: '/p1', running: true, created_at: 0, updated_at: Date.now(), message_count: 1 },
  { id: 's2', project_id: 'p2', title: 'Second', workspace: '/p2', running: false, last_run_status: 'completed' as const, created_at: 0, updated_at: Date.now() - 60_000, message_count: 2 },
];
const controlSessions = [
  { id: 'c1', project_id: null, title: 'Control chat', workspace: '/data/control-workspaces/c1', running: false, created_at: 0, updated_at: Date.now(), message_count: 4 },
];

beforeEach(() => {
  vi.clearAllMocks();
  store.projects.value = [
    { id: 'p1', name: 'One', path: '/p1', created_at: 0, updated_at: 0, session_count: 1 },
    { id: 'p2', name: 'Two', path: '/p2', created_at: 0, updated_at: 0, session_count: 1 },
  ];
  store.currentProjectId.value = 'p1';
  store.currentSessionId.value = null;
  store.chat.value = { sessionId: null, items: [], queuedSteers: [], queuedFollowUps: [] } as never;
  vi.mocked(listSessions).mockResolvedValue([...sessions, ...controlSessions] as never);
  vi.mocked(listAgents).mockResolvedValue([]);
  vi.mocked(apiRequest).mockImplementation((async (path: string, options?: { method?: string }) => {
    if (path !== '/api/control/sessions') throw new Error(`unexpected ${path}`);
    return options?.method === 'POST'
      ? { session_id: 'c-new', project_id: null, workspace: '/data/control-workspaces/c-new', created: true }
      : { sessions: controlSessions };
  }) as never);
});

describe('session control plane', () => {
  it('lists project sessions separately from control chats', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const controlGroup = within(browser).getByLabelText('Control chats');
    expect(within(controlGroup).getByText('Control chat')).toBeTruthy();
    expect(within(browser).getByText('First')).toBeTruthy();
    expect(within(browser).getByText('Second')).toBeTruthy();
    // The control chat is not duplicated as a project row.
    expect(within(browser).getAllByText('Control chat')).toHaveLength(1);
  });

  it('opens a control chat and shows the control intro', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    fireEvent.click(within(controlGroup).getByRole('button', { name: /Control chat/ }));
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
    fireEvent.click(within(browser).getByRole('button', { name: /Second/ }));
    expect(await screen.findByText('Normal chat')).toBeTruthy();
    expect(screen.queryByText('Control intro')).toBeNull();
    expect(actions.switchProject).toHaveBeenCalledWith('p2');
    expect(actions.openSession).toHaveBeenCalledWith('s2');
  });

  it('returns to the overview from an open session', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: /First/ }));
    await screen.findByText('Normal chat');
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect(await screen.findByText(/What are we building/)).toBeTruthy();
  });

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

  it('offers project mentions to the control chat', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    fireEvent.click(within(controlGroup).getByRole('button', { name: /Control chat/ }));
    expect(await screen.findByText('Mentions: One, Two')).toBeTruthy();
  });

  it('filters project sessions to running only', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const filters = within(browser).getByRole('group', { name: 'Filter sessions' });
    fireEvent.click(within(filters).getByRole('button', { name: /Running/ }));
    expect(within(browser).getByText('First')).toBeTruthy();
    expect(within(browser).queryByText('Second')).toBeNull();
  });

  it('keeps a distinct word on every state, not just a coloured dot', async () => {
    // `.cp-state.good, .cp-state.idle` used to be `font-size: 0`, so "Finished",
    // "Cancelled" and "No runs" all rendered as the same grey dot. The word is
    // now always in the row, including the ≤520px session header.
    const quiet = [
      { ...sessions[1]!, id: 's_done', title: 'Done one', last_run_status: 'completed' as const },
      { ...sessions[1]!, id: 's_cancelled', title: 'Cancelled one', last_run_status: 'cancelled' as const },
      { ...sessions[1]!, id: 's_never', title: 'Never ran', last_run_status: undefined },
    ];
    vi.mocked(listSessions).mockResolvedValue([...quiet, ...sessions, ...controlSessions] as never);
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

  it('keeps navigation errors visible after a successful list refresh', async () => {
    store.projects.value = store.projects.value.filter((project) => project.id !== 'p2');
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: /Second/ }));
    expect((await screen.findByRole('alert')).textContent).toContain('project is unavailable');
    vi.useFakeTimers();
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      expect(screen.getByRole('alert').textContent).toContain('project is unavailable');
    } finally { vi.useRealTimers(); }
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
    expect(sessionStatus({ running: false, last_run_status: 'failed' })).toEqual({ label: 'Failed', tone: 'bad' });
    expect(sessionStatus({ running: false }).label).toBe('No runs');
  });

  it('builds a starter that names the target project by id', () => {
    expect(controlStarter({ id: 'p9', name: 'web-platform' })).toBe(
      'Start work in project "web-platform" (project_id: p9).\n\n',
    );
  });

  it('formats recent times', () => {
    const now = 1_000_000_000;
    expect(shortAgo(now - 10_000, now)).toBe('just now');
    expect(shortAgo(now - 120_000, now)).toBe('2m ago');
    expect(shortAgo(now - 7_200_000, now)).toBe('2h ago');
  });
});
