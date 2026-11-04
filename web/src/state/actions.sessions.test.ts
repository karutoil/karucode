import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/endpoints', () => ({
  listSessions: vi.fn(),
  listAgents: vi.fn(async () => []),
  getAgents: vi.fn(async () => []),
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
  created_at: 0,
  updated_at: 2,
  message_count: 3,
  running: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  store.currentProjectId.value = 'p1';
  store.sessions.value = [];
  store.sessionsLoading.value = true;
});

describe('refreshing a project session list', () => {
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
});
