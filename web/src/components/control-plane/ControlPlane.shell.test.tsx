/**
 * The control plane shell: the navigation between the overview and an open
 * conversation (including the header and its error surface) plus
 * `controlModel`'s pure formatting helpers. These are not one region's markup,
 * so they live in their own file; split out of `ControlPlane.test.tsx` so the
 * three region suites can be edited in parallel.
 *
 * The `vi.mock` blocks below are a per-file copy on purpose: `vi.mock` is
 * hoisted per test file, so a shared fixture module cannot register them.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { controlPlaneOpen, resetControlPlane } from '../../state/controlPlane';
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
import { chats, configWith, overviewWith, sessions } from './controlPlaneFixtures';

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
  refreshSessions: vi.fn(async () => {}),
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
  // The plane's own visibility and the workbench's sheet are module state; a test
  // that left the plane must not hand the next one a closed plane or an open sheet.
  controlPlaneOpen.value = false;
  store.mobileSheet.value = null;
  store.narrowViewport.value = false;
  store.projects.value = [
    { id: 'p1', name: 'One', path: '/p1', created_at: 0, updated_at: 0, session_count: 1 },
    { id: 'p2', name: 'Two', path: '/p2', created_at: 0, updated_at: 0, session_count: 1 },
  ];
  store.currentProjectId.value = 'p1';
  store.currentSessionId.value = null;
  // The plane itself does not read `boot`, but its unmount hook only re-reads
  // the project list once the workspace is ready; default to "not ready" so an
  // automatic cleanup unmount between tests cannot fire that read.
  store.boot.value = 'loading';
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

/* --------------------------------------------------------------- navigation */

describe('leaving the plane', () => {
  /** Open the plane's rail and press its exit. */
  async function pressLeave(): Promise<void> {
    const browser = await screen.findByLabelText('Sessions across projects');
    fireEvent.click(within(browser).getByRole('button', { name: 'Back to sessions' }));
  }

  it('closes the plane and leaves the workbench where it was', async () => {
    controlPlaneOpen.value = true;
    render(<ControlPlane />);
    await pressLeave();
    expect(controlPlaneOpen.value).toBe(false);
    // A wide screen's session list is the workbench rail, already mounted and
    // already on screen: leaving must not also pop a sheet over it.
    expect(store.mobileSheet.value).toBeNull();
    // Leaving is not navigating: no project switch, no session opened.
    expect(actions.switchProject).not.toHaveBeenCalled();
    expect(actions.openSession).not.toHaveBeenCalled();
  });

  it('lands a phone on the sessions sheet, which is where its list lives', async () => {
    // The plane replaces the whole workspace on a phone and `.project-rail` is
    // `display: none` there, so before this control the screen had no way back at
    // all. The destination is the sheet the plane was opened from.
    store.narrowViewport.value = true;
    controlPlaneOpen.value = true;
    render(<ControlPlane />);
    await pressLeave();
    expect(controlPlaneOpen.value).toBe(false);
    expect(store.mobileSheet.value).toBe('sessions');
  });
});

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

  it('re-reads the open project’s session list when the plane closes', async () => {
    // The workbench rail draws `store.sessions` unfiltered, and while the plane
    // is up the reads were cross-project. Leaving it is the moment the ordinary
    // rail comes back, so it must not inherit a stale or foreign row: the same
    // read a project switch makes.
    store.boot.value = 'ready';
    const { unmount } = render(<ControlPlane />);
    await screen.findByLabelText('Sessions across projects');
    expect(actions.refreshSessions).not.toHaveBeenCalled();
    unmount();
    expect(actions.refreshSessions).toHaveBeenCalledTimes(1);
  });
});

/* ------------------------------------------------------------------- misc */

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
