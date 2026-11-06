/**
 * Shared fixture builders for the control plane's tests.
 *
 * Every control-plane test file imports from here, so the four region suites
 * (rail, overview, session, shell) agree on the same sessions, chats and
 * snapshot. This file holds data only: it deliberately contains **no**
 * `vi.mock` call, because `vi.mock` is hoisted per test file and a mock
 * registered here would not apply to the files that import it. Each test file
 * carries its own verbatim copy of the mocks.
 */
import type {
  AgentSummary,
  Config,
  ControlChatSummary,
  ControlOverview,
  SessionSummary,
} from '../../api/types';

/**
 * The part of `Config` this screen reads (`providers` for both pickers and
 * `active` for the default pin). Everything else is filled by the server and is
 * irrelevant to the control plane, so it is cast rather than copied.
 */
export function configWith(
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

export const NOW = Date.now();

export function projectSession(overrides: Partial<SessionSummary> & { id: string }): SessionSummary {
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

export const sessions: SessionSummary[] = [
  projectSession({ id: 's1', title: 'First', project_id: 'p1', running: true, updated_at: NOW }),
  projectSession({ id: 's2', title: 'Second', project_id: 'p2', last_run_status: 'completed', updated_at: NOW - 60_000, message_count: 2 }),
];

export function controlChat(overrides: Partial<ControlChatSummary> & { id: string }): ControlChatSummary {
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

export const chats: ControlChatSummary[] = [
  // The row the browser script and every rail assertion looks for: a title and a
  // live-plus-finished subagent count. It also carries a `model_label`, which
  // the rail must NOT draw - the model belongs to the composer, not to the name.
  controlChat({ id: 'c1', title: 'Control chat', agents: { live: 1, total: 2 }, model_label: 'gpt-4o-mini' }),
  // Created without a prompt, so the server sends an empty title.
  controlChat({ id: 'c2', title: '', updated_at: NOW - 120_000 }),
];

export function agent(id: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
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

export function overviewWith(overrides: Partial<ControlOverview> = {}): ControlOverview {
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
