import { describe, expect, it } from 'vitest';
import type { SessionInfo, SessionInfoAgent, UsageTotals } from '../api/types';
import {
  agentSummary,
  cacheDetail,
  cacheLabel,
  contextFill,
  emptyUsage,
  estimateCost,
  formatCost,
  formatPercent,
  formatSpan,
  formatTokens,
  orderAgents,
  orderedStatuses,
  roleLabel,
  shareOfSession,
  totalTokens,
  turnCounts,
  turnsPerPrompt,
} from './sessionInfo';

function usage(overrides: Partial<UsageTotals> = {}): UsageTotals {
  return { ...emptyUsage(), ...overrides };
}

function agent(overrides: Partial<SessionInfoAgent> = {}): SessionInfoAgent {
  return {
    id: 'a_1',
    name: 'reviewer',
    role: 'reviewer',
    status: 'completed',
    model: 'test-model',
    iterations: 3,
    duration_ms: 1_000,
    created_at: 10,
    finished_at: 20,
    messages: 4,
    usage: usage({ calls: 1, input_tokens: 100, output_tokens: 10 }),
    ...overrides,
  };
}

function info(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session: {
      id: 's_1',
      title: 'Session',
      workspace: '/w',
      project_id: null,
      created_at: 0,
      updated_at: 0,
    },
    running: false,
    stats: {
      messages_total: 0,
      messages_main: 0,
      messages_subagent: 0,
      messages_by_role: {},
      messages_main_by_role: {},
      tool_calls_total: 0,
      tool_calls: {},
      runs_by_status: {},
      runs_total: 0,
      working_ms: 0,
    },
    usage: { main: emptyUsage(), by_agent: {}, total: emptyUsage() },
    context: { model: 'test-model', window: null, used: 0 },
    agents: [],
    messages: { total: 0, main: 0, subagent: 0, by_role: {}, main_by_role: {} },
    tools: { total: 0, by_name: [], truncated: false },
    runs: { total: 0, by_status: {}, working_ms: 0, recent: [] },
    ...overrides,
  };
}

describe('formatTokens', () => {
  it('shows the scale a token count is read at', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(999)).toBe('999');
    expect(formatTokens(1_234)).toBe('1.2k');
    expect(formatTokens(41_023)).toBe('41.0k');
    expect(formatTokens(128_000)).toBe('128k');
    expect(formatTokens(1_500_000)).toBe('1.5M');
    expect(formatTokens(12_000_000)).toBe('12M');
  });

  it('renders a missing number as a dash, never as zero', () => {
    expect(formatTokens(null)).toBe('—');
    expect(formatTokens(undefined)).toBe('—');
    expect(formatTokens(Number.NaN)).toBe('—');
    expect(formatTokens(-5)).toBe('—');
  });
});

describe('formatPercent', () => {
  it('renders a ratio as a percentage', () => {
    expect(formatPercent(0.359)).toBe('35.9%');
    expect(formatPercent(1)).toBe('100.0%');
    expect(formatPercent(0.042)).toBe('4.2%');
  });

  it('keeps a missing ratio missing', () => {
    expect(formatPercent(null)).toBe('—');
    expect(formatPercent(undefined)).toBe('—');
  });
});

describe('formatSpan', () => {
  it('scales a duration', () => {
    expect(formatSpan(250)).toBe('250ms');
    expect(formatSpan(1_000)).toBe('1s');
    expect(formatSpan(42_000)).toBe('42s');
    expect(formatSpan(72_000)).toBe('1m 12s');
    expect(formatSpan(120_000)).toBe('2m');
    expect(formatSpan(3_840_000)).toBe('1h 04m');
    expect(formatSpan(3_600_000)).toBe('1h');
  });

  it('renders an unknown duration as a dash', () => {
    expect(formatSpan(null)).toBe('—');
    expect(formatSpan(-1)).toBe('—');
  });
});

describe('cacheLabel / cacheDetail', () => {
  it('says "no calls yet" before the session ran', () => {
    expect(cacheLabel(emptyUsage())).toBe('No calls yet');
    expect(cacheDetail(emptyUsage())).toContain('has not called a model');
  });

  it('reports "not reported" rather than a fabricated 0%', () => {
    const totals = usage({ calls: 3, input_tokens: 900, cached_known_calls: 0, cache_hit_ratio: null });
    expect(cacheLabel(totals)).toBe('Not reported');
    expect(cacheDetail(totals)).toContain('not the same as a 0% hit');
  });

  it('shows a real ratio, and names the calls it covers', () => {
    const totals = usage({
      calls: 4,
      input_tokens: 2_000,
      cached_tokens: 1_800,
      cached_known_calls: 2,
      cache_hit_ratio: 0.9,
    });
    expect(cacheLabel(totals)).toBe('90.0%');
    expect(cacheDetail(totals)).toContain('Only 2 of 4 calls');
  });

  it('shows a reported 0 as 0%, because a miss is a fact', () => {
    const totals = usage({
      calls: 2,
      input_tokens: 500,
      cached_known_calls: 2,
      cache_hit_ratio: 0,
    });
    expect(cacheLabel(totals)).toBe('0.0%');
  });
});

describe('contextFill', () => {
  it('renders the fill when the window is declared', () => {
    const fill = contextFill({
      context: { model: 'gpt', window: 128_000, used: 41_023 },
    });
    expect(fill.fraction).toBeCloseTo(41_023 / 128_000, 6);
    expect(fill.label).toBe('41.0k / 128k');
    expect(fill.tone).toBe('calm');
    expect(fill.note).toBeNull();
  });

  it('turns warn at 70% and hot at 90%', () => {
    expect(contextFill({ context: { model: 'm', window: 100, used: 75 } }).tone).toBe('warn');
    expect(contextFill({ context: { model: 'm', window: 100, used: 95 } }).tone).toBe('hot');
  });

  it('refuses a percentage when the window is unknown, and says why', () => {
    const fill = contextFill({ context: { model: 'custom', window: null, used: 5_000 } });
    expect(fill.fraction).toBeNull();
    expect(fill.label).toBe('5.0k');
    expect(fill.note).toContain('declares no context window');
  });

  it('refuses a percentage before any call reported a prompt size', () => {
    const fill = contextFill({ context: { model: 'm', window: 1_000, used: 0 } });
    expect(fill.fraction).toBeNull();
    expect(fill.label).toBe('— / 1.0k');
    expect(fill.note).toContain('reported a prompt size');
  });
});

describe('turnCounts / turnsPerPrompt', () => {
  it('reads prompts, runs, calls and answers from the right places', () => {
    const counts = turnCounts(
      info({
        messages: {
          total: 10,
          main: 8,
          subagent: 2,
          by_role: {},
          main_by_role: { user: 2, assistant: 6 },
        },
        usage: {
          main: usage({ calls: 6 }),
          by_agent: {},
          total: usage({ calls: 8 }),
        },
        runs: { total: 2, by_status: { completed: 2 }, working_ms: 5, recent: [] },
      }),
    );
    expect(counts).toEqual({ prompts: 2, runs: 2, calls: 6, answers: 6 });
  });

  it('computes turns per prompt, and refuses to divide by zero', () => {
    expect(
      turnsPerPrompt({ messages: { total: 0, main: 0, subagent: 0, by_role: {}, main_by_role: { assistant: 4 } } }),
    ).toBeNull();
    expect(
      turnsPerPrompt({ messages: { total: 0, main: 0, subagent: 0, by_role: {}, main_by_role: { user: 2, assistant: 6 } } }),
    ).toBe(3);
  });
});

describe('orderedStatuses', () => {
  it('shows statuses in a stable order and hides zeros', () => {
    expect(
      orderedStatuses({ failed: 2, running: 1, completed: 5, cancelled: 0 }).map(([status]) => status),
    ).toEqual(['completed', 'running', 'failed']);
  });

  it('keeps a status this build does not know, last', () => {
    const ordered = orderedStatuses({ completed: 1, something_new: 3 });
    expect(ordered).toEqual([
      ['completed', 1],
      ['something_new', 3],
    ]);
  });
});

describe('orderAgents', () => {
  it('puts running agents first, then newest', () => {
    const list = [
      agent({ id: 'old', created_at: 1 }),
      agent({ id: 'new', created_at: 5 }),
      agent({ id: 'live', created_at: 2, status: 'running', duration_ms: null }),
    ];
    expect(orderAgents(list).map((a) => a.id)).toEqual(['live', 'new', 'old']);
  });
});

describe('shareOfSession', () => {
  it('is the fraction of the session total', () => {
    const session = info({
      usage: {
        main: usage({ input_tokens: 300, output_tokens: 0 }),
        by_agent: {},
        total: usage({ input_tokens: 400, output_tokens: 0 }),
      },
    });
    expect(shareOfSession(usage({ input_tokens: 100 }), session)).toBeCloseTo(0.25, 6);
  });

  it('is null when nothing has been billed', () => {
    expect(shareOfSession(usage({ input_tokens: 5 }), info())).toBeNull();
  });
});

describe('agentSummary', () => {
  it('reads as a sentence for every case', () => {
    expect(agentSummary([])).toBe('No subagents');
    expect(agentSummary([agent({ status: 'completed' })])).toBe('1/1 done');
    expect(
      agentSummary([
        agent({ id: 'a', status: 'completed' }),
        agent({ id: 'b', status: 'running' }),
        agent({ id: 'c', status: 'failed' }),
      ]),
    ).toBe('1/3 done · 1 running · 1 failed');
  });
});

describe('roleLabel', () => {
  it('names the roles the server reports', () => {
    expect(roleLabel('user')).toBe('your prompts');
    expect(roleLabel('tool')).toBe('tool results');
    expect(roleLabel('mystery')).toBe('mystery');
  });
});

describe('estimateCost', () => {
  it('prices input and output per million', () => {
    const cost = estimateCost(usage({ input_tokens: 1_000_000, output_tokens: 500_000, cached_tokens: 400_000 }), {
      input: 3,
      output: 15,
    });
    expect(cost).not.toBeNull();
    expect(cost?.input).toBeCloseTo(3, 6);
    expect(cost?.output).toBeCloseTo(7.5, 6);
    // Cached tokens are a subset of the prompt: shown, never added again.
    expect(cost?.cached).toBeCloseTo(1.2, 6);
    expect(cost?.total).toBeCloseTo(10.5, 6);
    expect(cost?.partial).toBe(false);
  });

  it('refuses to price a model with no declared rates', () => {
    expect(estimateCost(usage({ input_tokens: 100 }), {})).toBeNull();
    expect(estimateCost(usage({ input_tokens: 100 }), { input: null, output: null })).toBeNull();
  });

  it('marks a total built from only one rate as partial', () => {
    const cost = estimateCost(usage({ input_tokens: 1_000_000 }), { input: 2 });
    expect(cost?.partial).toBe(true);
    expect(cost?.total).toBeCloseTo(2, 6);
  });
});

describe('formatCost', () => {
  it('keeps significant digits on small amounts', () => {
    expect(formatCost(0)).toBe('$0');
    expect(formatCost(0.00042)).toBe('$0.0004');
    expect(formatCost(0.0421)).toBe('$0.042');
    expect(formatCost(4.2)).toBe('$4.20');
    expect(formatCost(null)).toBe('—');
  });
});

describe('totalTokens', () => {
  it('is input plus output', () => {
    expect(totalTokens(usage({ input_tokens: 10, output_tokens: 5 }))).toBe(15);
  });
});
