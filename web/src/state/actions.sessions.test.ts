import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/endpoints', () => ({
  listSessions: vi.fn(),
  listAgents: vi.fn(async () => []),
  getAgents: vi.fn(async () => []),
  getSession: vi.fn(),
}));

import * as api from '../api/endpoints';
import * as actions from './actions';
import * as store from './store';

const projectSession = {
  id: 's-project',
  title: 'Project work',
  workspace: '/p1',
  project_id: 'p1',
  created_at: 0,
  updated_at: 1,
  message_count: 1,
  running: false,
};
const controlSession = {
  id: 's-control',
  title: 'Control chat',
  workspace: '/data/control-workspace',
  project_id: null,
  control: true,
  created_at: 0,
  updated_at: 2,
  message_count: 3,
  running: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  store.currentProjectId.value = 'p1';
  store.currentSessionId.value = null;
  store.sessions.value = [];
  store.sessionsLoading.value = true;
});

describe('refreshing a project session list', () => {
  it('does not re-read the whole agent list when only a run boundary changed', async () => {
    // A busy project re-reads the session list on every `run_started` and
    // `run_finished`. The agent list is the whole project's agents - several
    // megabytes on a project with history - and nothing about a run boundary
    // changes its membership or its labels.
    store.sessions.value = [projectSession] as never;
    vi.mocked(api.listSessions).mockResolvedValue([
      { ...projectSession, running: true, message_count: 9, updated_at: 5 },
    ] as never);

    await actions.refreshSessions();

    expect(api.listAgents).not.toHaveBeenCalled();
  });

  it('re-reads the agent list when a session is added, removed or renamed', async () => {
    store.sessions.value = [projectSession] as never;
    vi.mocked(api.listSessions).mockResolvedValue([
      { ...projectSession, title: 'Project work, renamed' },
    ] as never);
    await actions.refreshSessions();
    expect(api.listAgents).toHaveBeenCalledTimes(1);

    // A project switch clears the list first, so the new project's agents are
    // always read.
    store.sessions.value = [];
    vi.mocked(api.listSessions).mockResolvedValue([projectSession] as never);
    await actions.refreshSessions();
    expect(api.listAgents).toHaveBeenCalledTimes(2);
  });

  it('keeps the open, project-less control chat instead of closing it', async () => {
    store.currentSessionId.value = 's-control';
    vi.mocked(api.listSessions).mockImplementation((async (projectId?: string) =>
      projectId ? [projectSession] : [projectSession, controlSession]) as never);
    await actions.refreshSessions();
    expect(store.currentSessionId.value).toBe('s-control');
  });

  it('still closes a session that no longer exists anywhere', async () => {
    store.currentSessionId.value = 's-gone';
    vi.mocked(api.listSessions).mockImplementation((async (projectId?: string) =>
      projectId ? [projectSession] : [projectSession]) as never);
    await actions.refreshSessions();
    expect(store.currentSessionId.value).toBe('s-project');
  });

  it('never inserts a control chat into the project list', async () => {
    // Opening a chat from the control plane used to merge its summary into
    // `sessions.value`, which the workspace rail draws unfiltered: the chat
    // appeared as a project row and vanished at the next refresh. It belongs to
    // the plane's own list, so opening it must not touch this one.
    vi.mocked(api.getSession).mockResolvedValue({
      session: {
        id: 's-control',
        title: 'Control chat',
        workspace: '/data/control-workspace',
        project_id: null,
        created_at: 0,
        updated_at: 2,
      },
      messages: [],
      runs: [],
      agents: [],
      approvals: [],
      claims: [],
      running: false,
    } as never);
    store.sessions.value = [projectSession as never];
    await actions.openSession('s-control');
    expect(store.sessions.value.map((session) => session.id)).toEqual([projectSession.id]);
    // …and it is open, which is what the pills and the composer read.
    expect(store.currentSessionId.value).toBe('s-control');
  });

  it('moves the project onto a session opened from another project', async () => {
    // `openSession` is also reached from a notification whose record has lost its
    // project and from the palette. The rail, the file tree and the terminals are
    // project-scoped, so the conversation must bring its project with it instead
    // of being merged into the list of the project that happened to be open - the
    // foreign row a project switch and back used to clear.
    const other = { ...projectSession, id: 's-other', title: 'Other project', project_id: 'p2', workspace: '/p2' };
    vi.mocked(api.getSession).mockResolvedValue({
      session: {
        id: 's-other',
        title: 'Other project',
        workspace: '/p2',
        project_id: 'p2',
        created_at: 0,
        updated_at: 3,
      },
      messages: [],
      runs: [],
      agents: [],
      approvals: [],
      claims: [],
      running: false,
    } as never);
    vi.mocked(api.listSessions).mockResolvedValue([other] as never);
    store.sessions.value = [projectSession as never];
    await actions.openSession('s-other');
    expect(store.currentProjectId.value).toBe('p2');
    // The session is open and its row is the only one: p1's row is gone with the
    // project it belonged to.
    expect(store.currentSessionId.value).toBe('s-other');
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s-other']);
    await vi.waitFor(() => expect(api.listSessions).toHaveBeenCalledWith('p2'));
    expect(store.sessions.value.map((session) => session.id)).toEqual(['s-other']);
  });

  it('keeps control chats out of the list even when the read is unscoped', async () => {
    // With no project open the API answers with every session on the server,
    // control chats included. The workbench rail draws whatever lands here, so
    // the write - not the reader - has to hold the invariant.
    store.currentProjectId.value = null;
    const projectless = { ...projectSession, id: 's-projectless', project_id: null };
    vi.mocked(api.listSessions).mockResolvedValue(
      [projectSession, controlSession, projectless] as never,
    );
    await actions.refreshSessions();
    expect(store.sessions.value.map((session) => session.id)).toEqual([
      's-project',
      's-projectless',
    ]);
  });
});
