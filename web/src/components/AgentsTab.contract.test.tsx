import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/preact';
import type { AgentSummary } from '../api/types';
import { AgentsTab } from './AgentsTab';
import { emptyChatState } from '../state/chat';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { jsonResponse, omitFields, resetAppState, wireAgentSummary } from '../test/fixtures';

/**
 * The payload the Agents endpoints actually send.
 *
 * `AgentRecord` on the server marks `note`, `progress`, `parent_agent_id`,
 * `error` and `finished_at` as `skip_serializing_if`, so an agent that has never
 * called `report_status` arrives with **no `note` key and no `progress` key** —
 * verified against a live server, where all 77 rows of `GET /api/agents`
 * omitted both, running agents included.
 *
 * Every test in this file therefore uses a payload with those keys *absent*.
 * The component tests in `AgentsTab.test.tsx` build rows with `note: ''` and
 * `progress: null`; the whole suite stayed green while the tab was throwing on
 * the real shape, which is exactly what this file exists to prevent.
 */
function wireRunningAgent(id: string, overrides: Record<string, unknown> = {}): AgentSummary {
  return wireAgentSummary(id, { status: 'running', iterations: 3, ...overrides });
}

/** A fetch that never resolves, so a seeded list survives the mount refresh. */
function stubFetchNeverResolving(): void {
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => {})));
}

/** Serve exactly these rows from `GET /api/agents`. */
function stubFetchAgents(rows: unknown[], detail?: unknown): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/agents/')) {
        return jsonResponse(detail ?? { agent: rows[0], messages: [], messages_truncated: false, messages_limit: 2000 });
      }
      if (url.includes('/api/agents')) return jsonResponse({ agents: rows, running: 0 });
      return jsonResponse({});
    }),
  );
}

/** No rendered element may carry a NaN in its inline style (the `width: NaN%` bug). */
function expectNoNaNStyles(container: Element): void {
  const styled = [...container.querySelectorAll('[style]')];
  for (const node of styled) {
    expect(node.getAttribute('style')).not.toContain('NaN');
  }
}

describe('AgentsTab — server-shaped payloads', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = 'p_1';
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('renders a running agent whose row omits note and progress', async () => {
    // The exact cause of the P0: `liveLabel()` ran `agent.note.trim()`, which
    // threw on `undefined` and took the whole workspace down with it.
    const rows = [wireRunningAgent('a_live')];
    stubFetchAgents(rows);

    const { container } = render(<AgentsTab />);

    // The refresh on mount returns the server-shaped rows; the card must appear
    // with a real status label — not a thrown error.
    expect(await screen.findByText('a_live')).not.toBeNull();
    expect(container.querySelectorAll('.agent-card')).toHaveLength(1);
    expect(screen.getByText('thinking')).not.toBeNull();
    expect(screen.getByText('1 running · 1 total')).not.toBeNull();
    expect(screen.queryByText('No subagents yet')).toBeNull();

    // No progress claim means no progress bar, and never a NaN width.
    expect(container.querySelector('.agent-progress')).toBeNull();
    expectNoNaNStyles(container);
  });

  it('renders an unnormalised server row that bypasses the state layer', async () => {
    // Defence in depth: a row can reach the component without going through
    // `refreshAgents`/`seedAgentState` (a stale store, a future code path), so
    // the component itself must tolerate the wire shape.
    const raw = wireRunningAgent('a_raw');
    expect('note' in raw).toBe(false);
    expect('progress' in raw).toBe(false);
    store.agentsAll.value = [raw];
    stubFetchNeverResolving();

    const { container } = render(<AgentsTab />);

    expect(container.querySelectorAll('.agent-card')).toHaveLength(1);
    expect(screen.getByText('thinking')).not.toBeNull();
    expect(container.querySelector('.agent-progress')).toBeNull();
    expectNoNaNStyles(container);
  });

  it('falls back to "starting" for a freshly spawned row with no iterations', () => {
    store.agentsAll.value = [wireRunningAgent('a_new', { iterations: 0 })];
    stubFetchNeverResolving();

    render(<AgentsTab />);

    expect(screen.getByText('starting')).not.toBeNull();
  });

  it('shows the agent note when the server did send one', () => {
    store.agentsAll.value = [
      wireRunningAgent('a_talky', { note: 'checking the fixtures', progress: 0.5 }),
    ];
    stubFetchNeverResolving();

    const { container } = render(<AgentsTab />);

    // The live label wins, and the note is echoed in its own line.
    expect(screen.getAllByText('checking the fixtures').length).toBeGreaterThan(0);
    // A real claim still renders a bar, with a concrete width.
    const fill = container.querySelector<HTMLElement>('.agent-progress-fill');
    expect(fill).not.toBeNull();
    expect(fill!.getAttribute('style')).toContain('width: 50%');
  });

  it('ignores a non-finite progress instead of drawing a NaN bar', () => {
    store.agentsAll.value = [wireRunningAgent('a_bad', { note: 'working', progress: null })];
    stubFetchNeverResolving();

    const { container } = render(<AgentsTab />);
    expect(container.querySelector('.agent-progress')).toBeNull();
    expectNoNaNStyles(container);
  });

  it('renders the session scope when its rows omit note and progress', () => {
    store.chat.value = {
      ...emptyChatState('s_ab12'),
      agents: [wireRunningAgent('a_session')],
    };
    store.currentSessionId.value = 's_ab12';
    store.agentScope.value = 'session';
    stubFetchNeverResolving();

    const { container } = render(<AgentsTab />);

    // Session scope renders `store.chat.value.agents`, which no refresh path
    // normalises — the row here is exactly as the server wrote it.
    expect(container.querySelectorAll('.agent-card')).toHaveLength(1);
    expect(screen.getByText('a_session')).not.toBeNull();
    expect(screen.getByText('thinking')).not.toBeNull();
    expect(container.querySelector('.agent-progress')).toBeNull();
    expectNoNaNStyles(container);
  });

  it('drills into a server-shaped agent without crashing on owned_paths', async () => {
    // `owned_paths` is `serde(default)` too: an agent that owns nothing can send
    // no key, and the detail view read `.length` straight off it.
    const raw = omitFields(wireAgentSummary('a_detail'), 'owned_paths', 'note', 'progress');
    store.agentsAll.value = [raw];
    stubFetchAgents([raw]);

    const { container } = render(<AgentsTab />);
    (container.querySelector('.agent-card-open') as HTMLElement).click();

    await waitFor(() => expect(container.querySelector('.agent-detail')).not.toBeNull());
    expect(container.querySelector('.agent-paths')).toBeNull();
    expectNoNaNStyles(container);
  });
});

describe('refreshAgents — wire normalisation', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = 'p_1';
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('fills in note, progress and owned_paths from a row that omits them', async () => {
    const raw = omitFields(wireRunningAgent('a_wire'), 'note', 'progress', 'owned_paths');
    stubFetchAgents([raw]);

    await actions.refreshAgents();

    const [row] = store.agentsAll.value;
    expect(row?.note).toBe('');
    expect(row?.progress).toBeNull();
    expect(row?.owned_paths).toEqual([]);
  });

  it('keeps values the server did send', async () => {
    stubFetchAgents([wireRunningAgent('a_wire', { note: 'reading src/lib.rs', progress: 0.25 })]);

    await actions.refreshAgents();

    const [row] = store.agentsAll.value;
    expect(row?.note).toBe('reading src/lib.rs');
    expect(row?.progress).toBe(0.25);
  });
});