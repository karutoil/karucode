import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { SessionInfo, SessionInfoAgent, UsageTotals } from '../api/types';
import { SessionInfoTab } from './SessionInfoTab';
import { emptyUsage } from '../lib/sessionInfo';
import { resetAppState, configFixture } from '../test/fixtures';
import * as store from '../state/store';

function usage(overrides: Partial<UsageTotals> = {}): UsageTotals {
  return { ...emptyUsage(), ...overrides };
}

function agent(overrides: Partial<SessionInfoAgent> = {}): SessionInfoAgent {
  return {
    id: 'a_1',
    name: 'reviewer',
    role: 'reviewer',
    status: 'completed',
    model: 'gpt-4o-mini',
    iterations: 4,
    duration_ms: 61_000,
    created_at: 1_000,
    finished_at: 62_000,
    messages: 12,
    usage: usage({ calls: 4, input_tokens: 8_000, output_tokens: 900, cached_tokens: 6_000, cached_known_calls: 4, cache_hit_ratio: 0.75 }),
    ...overrides,
  };
}

/** A session that did some work: two prompts, tools, a subagent, real tokens. */
function fullInfo(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session: {
      id: 's_ab12',
      title: 'Fix the login bug',
      workspace: '/home/me/project',
      project_id: null,
      created_at: 0,
      updated_at: 0,
    },
    running: false,
    stats: {
      messages_total: 24,
      messages_main: 18,
      messages_subagent: 6,
      messages_by_role: { user: 2, assistant: 7, tool: 15 },
      messages_main_by_role: { user: 2, assistant: 5, tool: 11 },
      tool_calls_total: 15,
      tool_calls: { read_file: 9, run_command: 6 },
      runs_by_status: { completed: 1, failed: 1 },
      runs_total: 2,
      working_ms: 3_900_000,
    },
    usage: {
      main: usage({
        calls: 7,
        input_tokens: 41_000,
        output_tokens: 3_200,
        cached_tokens: 30_000,
        cached_known_calls: 7,
        cache_hit_ratio: 30_000 / 41_000,
        last_input_tokens: 41_000,
      }),
      by_agent: {
        a_1: usage({ calls: 4, input_tokens: 8_000, output_tokens: 900 }),
      },
      total: usage({
        calls: 11,
        input_tokens: 49_000,
        output_tokens: 4_100,
        cached_tokens: 36_000,
        cached_known_calls: 11,
        cache_hit_ratio: 36_000 / 49_000,
      }),
    },
    context: { model: 'gpt-4o-mini', window: 128_000, used: 41_000 },
    agents: [agent()],
    messages: {
      total: 24,
      main: 18,
      subagent: 6,
      by_role: { user: 2, assistant: 7, tool: 15 },
      main_by_role: { user: 2, assistant: 5, tool: 11 },
    },
    tools: {
      total: 15,
      by_name: [
        { name: 'read_file', count: 9 },
        { name: 'run_command', count: 6 },
      ],
      truncated: false,
    },
    runs: {
      total: 2,
      by_status: { completed: 1, failed: 1 },
      working_ms: 3_900_000,
      recent: [
        {
          id: 'r_2',
          session_id: 's_ab12',
          status: 'completed',
          model: 'gpt-4o-mini',
          prompt: 'now use a tool',
          error: null,
          started_at: 1_000,
          finished_at: 62_000,
        },
        {
          id: 'r_1',
          session_id: 's_ab12',
          status: 'failed',
          model: 'gpt-4o-mini',
          prompt: 'hello',
          error: 'boom',
          started_at: 0,
          finished_at: 1_000,
        },
      ],
    },
    ...overrides,
  };
}

/** Serve one payload for `GET /api/sessions/:id/info`, and count the calls. */
function stubInfo(payload: SessionInfo | (() => SessionInfo)): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (!url.endsWith('/info')) {
      return new Response(JSON.stringify({ error: { code: 'not_found', message: 'nope' } }), {
        status: 404,
        headers: { 'content-type': 'application/json' },
      });
    }
    const body = typeof payload === 'function' ? payload() : payload;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function seedSession(info?: SessionInfo): void {
  store.config.value = configFixture;
  store.currentSessionId.value = 's_ab12';
  if (info) {
    store.sessionInfo.value = info;
    store.sessionInfoFor.value = 's_ab12';
  }
}

describe('SessionInfoTab', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('asks for nothing when no session is open', () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    render(<SessionInfoTab />);
    expect(screen.getByText('No session open')).not.toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('reads the session it was opened for', async () => {
    const fetchMock = stubInfo(fullInfo());
    seedSession();
    render(<SessionInfoTab />);

    await waitFor(() => expect(store.sessionInfo.value).not.toBeNull());
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(String(fetchMock.mock.calls[0]?.[0])).toContain('/api/sessions/s_ab12/info');
    // The summary lives in the refresh button's tooltip, not in the body: the
    // panel already shows every one of those numbers separately.
    const refresh = await screen.findByRole('button', { name: /refresh session information/i });
    // The whole session's input tokens (main + subagent), not the main agent's.
    expect(refresh.getAttribute('title')).toBe(
      'Fix the login bug — 24 messages, 2 runs, 49.0k input tokens',
    );
  });

  it('shows the context fill, usage, work, agents and runs', async () => {
    stubInfo(fullInfo());
    seedSession();
    const { container } = render(<SessionInfoTab />);
    await screen.findByText('Main agent context');

    // Context: the newest main-agent prompt over the model's own window.
    expect(screen.getByText('32.0%')).not.toBeNull();
    expect(screen.getByText('41.0k / 128k')).not.toBeNull();
    const meter = document.querySelector('.si-window-meter');
    expect(meter?.getAttribute('role')).toBe('meter');
    expect(meter?.getAttribute('aria-valuenow')).toBe('32');
    expect(meter?.getAttribute('data-tone')).toBe('calm');

    // Usage: input/output/cached/cache-hit for the main agent.
    expect(screen.getByText('41.0k')).not.toBeNull();
    expect(screen.getByText('3.2k')).not.toBeNull();
    expect(screen.getByText('30.0k')).not.toBeNull();
    expect(screen.getByText('73.2%')).not.toBeNull();

    // Work: prompts, turns, model calls, tool calls.
    expect(screen.getByText('Your prompts')).not.toBeNull();
    expect(screen.getByText('Model calls')).not.toBeNull();
    expect(screen.getByText('Tool calls')).not.toBeNull();
    // 3.9M ms reads as an hour-plus, not a raw millisecond count.
    expect(screen.getByText('1h 05m')).not.toBeNull();
    // The tool breakdown is listed by name.
    expect(screen.getByText('read_file')).not.toBeNull();
    expect(screen.getByText('run_command')).not.toBeNull();

    // Agents: one row, with its own tokens, calls, cache, duration. The name and
    // the role are both "reviewer" here, so scope to the name element.
    const agentCard = container.querySelector('.si-agent');
    expect(agentCard?.querySelector('.si-agent-name')?.textContent).toBe('reviewer');
    // The agent's own tokens (8k in + 0.9k out), its cache rate and its duration.
    const agentValues = [...(agentCard?.querySelectorAll('.si-agent-meta dd') ?? [])].map(
      (node) => node.textContent ?? '',
    );
    expect(agentValues[0]).toContain('8.9k');
    expect(agentValues[1]).toBe('4');
    expect(agentValues[2]).toBe('75.0%');
    expect(agentValues[3]).toBe('12');
    expect(agentValues[4]).toBe('1m 1s');

    // Runs: newest first, with a readable status.
    expect(screen.getByText('Recent runs')).not.toBeNull();
    expect(screen.getByText('now use a tool')).not.toBeNull();
    expect(screen.getByText('hello')).not.toBeNull();
  });

  it('reports the delegated share, not 100% of the session', async () => {
    // main 44 200 of 53 100 total -> 83.2% main, so 16.8% delegated. Passing the
    // *total* to `shareOfSession` and inverting it always reads 0%, which looks
    // like "nothing was delegated" even when a subagent spent a sixth of it.
    stubInfo(fullInfo());
    seedSession();
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');
    expect(screen.getByText('17% of tokens')).not.toBeNull();
    expect(screen.queryByText('0.0% of tokens')).toBeNull();
  });

  it('shows no duration for a run that is still in flight', async () => {
    // `RunRecord.finished_at` is `skip_serializing_if = "Option::is_none"` in Rust
    // (`kc-core/src/store.rs`), so a live run sends **no key at all** rather than
    // `null`. The guard used to be `run.finished_at !== null`, which is true for
    // `undefined`, so the row computed `undefined - started_at` -> `NaN` and only
    // escaped a "NaNms" by luck: `formatDuration` rejects non-finite input as a
    // second line of defence. Both the guard (`!= null`) and the type
    // (`finished_at?: number | null`) now say what the wire actually sends, and
    // this pins that the row renders the start time and no invented duration.
    const live = fullInfo();
    live.runs.recent = [
      {
        id: 'r_live',
        session_id: 's_ab12',
        status: 'running',
        model: 'gpt-4o-mini',
        prompt: 'still going',
        // No `error` and no `finished_at`: exactly what the server sends.
        started_at: 1_000,
      },
    ];
    stubInfo(live);
    seedSession(live);
    const { container } = render(<SessionInfoTab />);
    await screen.findByText('Recent runs');

    expect(screen.getByText('still going')).not.toBeNull();
    // No duration, and nothing that looks like one.
    expect(container.textContent).not.toContain('NaN');
    expect(container.textContent).not.toContain('<1ms');
    // The row still says *when* it started, which is all a live run can report.
    const meta = container.querySelector('.si-run-meta')?.textContent ?? '';
    expect(meta).not.toBe('');
    expect(meta).not.toContain('·');
  });

  it('says "not reported" instead of inventing a 0% cache hit', async () => {
    stubInfo(
      fullInfo({
        usage: {
          main: usage({ calls: 3, input_tokens: 4_000, output_tokens: 200, cached_known_calls: 0, cache_hit_ratio: null }),
          by_agent: {},
          total: usage({ calls: 3, input_tokens: 4_000, output_tokens: 200 }),
        },
        agents: [],
      }),
    );
    seedSession();
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');

    // The tiles read "Not reported"; nothing anywhere claims a percentage.
    expect(screen.getAllByText('Not reported').length).toBeGreaterThan(0);
    expect(screen.queryByText('0.0%')).toBeNull();
  });

  it('refuses a percentage when the model declares no context window', async () => {
    stubInfo(fullInfo({ context: { model: 'hand-added', window: null, used: 12_000 } }));
    seedSession();
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');

    expect(document.querySelector('.si-window-meter')?.getAttribute('role')).toBeNull();
    expect(screen.getByText(/declares no context window/)).not.toBeNull();
    // The raw count is still shown: the number is real, only the ratio is not.
    expect(screen.getByText('12.0k')).not.toBeNull();
  });

  it('turns the meter red near the ceiling', async () => {
    stubInfo(fullInfo({ context: { model: 'gpt-4o-mini', window: 100_000, used: 95_000 } }));
    seedSession();
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');
    expect(document.querySelector('.si-window-meter')?.getAttribute('data-tone')).toBe('hot');
    expect(screen.getByText('95.0%')).not.toBeNull();
  });

  it('shows no subagent section for a session that delegated nothing', async () => {
    stubInfo(fullInfo({ agents: [] }));
    seedSession();
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');
    expect(screen.queryByText('Subagents')).toBeNull();
    expect(screen.queryByText(/done/)).toBeNull();
  });

  it('surfaces a read failure with a retry', async () => {
    const fetchMock = vi.fn(async () => {
      return new Response(
        JSON.stringify({ error: { code: 'internal', message: 'database is busy' } }),
        { status: 500, headers: { 'content-type': 'application/json' } },
      );
    });
    vi.stubGlobal('fetch', fetchMock);
    seedSession();
    render(<SessionInfoTab />);

    expect(await screen.findByText(/database is busy/)).not.toBeNull();
    const retry = screen.getByRole('button', { name: /try again/i });
    // A retry issues another read rather than leaving the stale error on screen.
    fetchMock.mockImplementation(async () => {
      return new Response(JSON.stringify(fullInfo()), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    });
    fireEvent.click(retry);
    expect(await screen.findByText('Main agent context')).not.toBeNull();
  });

  it('refetches when a run reports a usage frame', async () => {
    // The tab's numbers are server-side totals, so a live usage frame has to
    // trigger a read rather than being folded in locally.
    const fetchMock = stubInfo(fullInfo());
    seedSession();
    render(<SessionInfoTab />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    vi.useFakeTimers();
    try {
      store.sessionInfoTick.value += 1;
      await vi.advanceTimersByTimeAsync(500);
      expect(fetchMock.mock.calls.length).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('prices the session only for a model that declares rates', async () => {
    stubInfo(fullInfo());
    seedSession();
    const { container } = render(<SessionInfoTab />);
    await screen.findByText('Main agent context');

    // configFixture's model declares no per-million prices, so a cost would be a
    // fabricated $0.00.
    expect(container.textContent).toContain('Est. cost');
    expect(container.textContent).not.toContain('$0.00');
  });

  it('shows an estimate when the model declares rates', async () => {
    const priced = structuredClone(configFixture);
    if (priced.providers[0]) {
      const model = priced.providers[0].models[0];
      if (model) {
        model.cost_input = 3;
        model.cost_output = 15;
      }
    }
    store.config.value = priced;
    stubInfo(fullInfo());
    store.currentSessionId.value = 's_ab12';
    render(<SessionInfoTab />);
    await screen.findByText('Main agent context');

    // 49k input at $3/M plus 4.1k output at $15/M.
    const expected = ((49_000 / 1_000_000) * 3 + (4_100 / 1_000_000) * 15).toFixed(3);
    expect(await screen.findByText(`$${expected}`)).not.toBeNull();
  });
});
