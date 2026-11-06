import { describe, expect, it } from 'vitest';
import type { AgentStatus, RunStatus, SessionSummary } from '../api/types';
import { emptyChatState } from './chat';
import {
  AGENT_STATUS_LABEL,
  agentStatusLabel,
  agentStatusTone,
  contextTone,
  contextUsage,
  formatContextPercent,
  runStatusLabel,
  sessionStatusView,
  totalUsage,
  type ContextUsage,
} from './selectors';

function usage(overrides: Partial<ReturnType<typeof emptyChatState>> = {}) {
  return { ...emptyChatState('s_ab12'), ...overrides };
}

describe('contextUsage', () => {
  it('is empty until a model call reports a prompt size', () => {
    // No fabricated number: the pill stays hidden rather than showing 0% or a
    // client-side tokenizer guess.
    expect(contextUsage(usage(), 128_000)).toBeNull();
  });

  it('is empty when the model declares no window', () => {
    const state = usage({ lastPromptTokens: 12_000 });
    expect(contextUsage(state, null)).toBeNull();
    expect(contextUsage(state, 0)).toBeNull();
    expect(contextUsage(state, Number.NaN)).toBeNull();
  });

  it('divides the newest call by the window', () => {
    const state = usage({ lastPromptTokens: 32_000 });
    const result = contextUsage(state, 128_000);
    expect(result).toEqual({ window: 128_000, tokens: 32_000, fraction: 0.25 });
  });

  it('uses the newest call, not the session total', () => {
    // `usage` is a running cost across the whole run; a long run sums well past
    // the window while each individual request stays small.
    const state = usage({
      lastPromptTokens: 64_000,
      usage: { input_tokens: 900_000, output_tokens: 4_000 },
    });
    expect(contextUsage(state, 128_000)?.fraction).toBeCloseTo(0.5, 5);
  });

  it('clamps an over-window count instead of reporting 180%', () => {
    const state = usage({ lastPromptTokens: 230_000 });
    const result = contextUsage(state, 128_000) as ContextUsage;
    expect(result.fraction).toBe(1);
    expect(result.tokens).toBe(230_000);
  });
});

describe('formatContextPercent', () => {
  it('keeps a decimal where it carries information', () => {
    expect(formatContextPercent(0.574)).toBe('57.4%');
    expect(formatContextPercent(0.1)).toBe('10.0%');
  });

  it('drops the decimal at small fills, where it is noise', () => {
    expect(formatContextPercent(0.004)).toBe('0%');
    expect(formatContextPercent(0.043)).toBe('4%');
    expect(formatContextPercent(0.099)).toBe('10%');
  });

  it('never exceeds 100%', () => {
    expect(formatContextPercent(1)).toBe('100.0%');
    expect(formatContextPercent(2)).toBe('100.0%');
  });
});

describe('contextTone', () => {
  it('warns before the window is exhausted', () => {
    expect(contextTone(0)).toBe('calm');
    expect(contextTone(0.69)).toBe('calm');
    expect(contextTone(0.7)).toBe('warn');
    expect(contextTone(0.89)).toBe('warn');
    expect(contextTone(0.9)).toBe('hot');
    expect(contextTone(1)).toBe('hot');
  });
});

describe('totalUsage', () => {
  it('sums the session and reports the cache hit rate', () => {
    expect(totalUsage(usage())).toBe('');
    expect(
      totalUsage(
        usage({ usage: { input_tokens: 1_500, output_tokens: 400, cached_tokens: 1_200 } }),
      ),
    ).toBe('1,500 in · 400 out · 80% cached');
  });
});

describe('sessionStatusView', () => {
  /** One session row, with only the two fields the view reads. */
  const session = (
    running: boolean,
    lastRunStatus: RunStatus | null | undefined = null,
  ): Pick<SessionSummary, 'running' | 'last_run_status'> => ({
    running,
    // The server skips the key when it is null, so an absent key must behave
    // exactly like an explicit null.
    ...(lastRunStatus === undefined ? {} : { last_run_status: lastRunStatus }),
  });

  it('announces a run in flight, whatever the last settled run did', () => {
    expect(sessionStatusView(session(true))).toEqual({ label: 'Running', tone: 'running' });
    // A stored `failed` is the *previous* run; the row is running now.
    expect(sessionStatusView(session(true, 'failed'))).toEqual({
      label: 'Running',
      tone: 'running',
    });
  });

  it('announces a failed run', () => {
    expect(sessionStatusView(session(false, 'failed'))).toEqual({
      label: 'Failed',
      tone: 'danger',
    });
  });

  it('announces an interrupted run', () => {
    expect(sessionStatusView(session(false, 'interrupted'))).toEqual({
      label: 'Interrupted',
      tone: 'warn',
    });
  });

  it('stays quiet for a settled or never-run session', () => {
    // A list where every row wears a badge is a list where no badge means
    // anything: completed, cancelled and never-run rows render no indicator.
    for (const status of ['completed', 'cancelled', 'limit_reached', null, undefined] as const) {
      expect(sessionStatusView(session(false, status))).toEqual({ label: null, tone: null });
    }
  });

  it('ignores a stale `running` status on a session that is not running', () => {
    // The `running` flag is the authority; a status left mid-run by a crash must
    // not paint a live dot on a dead row.
    expect(sessionStatusView(session(false, 'running'))).toEqual({ label: null, tone: null });
  });

  it('spells the word the way the rest of the app spells it', () => {
    // Derived, not copied: the labels come from `runStatusLabel` and the tones
    // from `agentStatusTone`, so the rails and the announcer cannot drift.
    expect(sessionStatusView(session(true)).label).toBe(runStatusLabel('running'));
    expect(sessionStatusView(session(false, 'failed')).label).toBe(runStatusLabel('failed'));
    expect(sessionStatusView(session(false, 'failed')).tone).toBe(agentStatusTone('failed'));
    expect(sessionStatusView(session(false, 'interrupted')).tone).toBe(
      agentStatusTone('interrupted'),
    );
  });

  it('never returns a label without a tone, or a tone without a label', () => {
    for (const status of [
      'running',
      'completed',
      'failed',
      'cancelled',
      'interrupted',
      'limit_reached',
      null,
    ] as const) {
      for (const running of [true, false]) {
        const view = sessionStatusView(session(running, status));
        expect(view.label === null).toBe(view.tone === null);
      }
    }
  });
});

describe('AGENT_STATUS_LABEL / agentStatusLabel', () => {
  it('says the one word each status has everywhere in the UI', () => {
    expect(AGENT_STATUS_LABEL).toEqual({
      running: 'running',
      paused: 'paused',
      completed: 'done',
      failed: 'failed',
      cancelled: 'cancelled',
      interrupted: 'interrupted',
    });
  });

  it('covers every status the wire type declares', () => {
    const statuses: AgentStatus[] = [
      'running',
      'paused',
      'completed',
      'failed',
      'cancelled',
      'interrupted',
    ];
    for (const status of statuses) {
      expect(agentStatusLabel(status)).toBe(AGENT_STATUS_LABEL[status]);
    }
    // A new status added to the type fails here until it gets a word.
    expect(Object.keys(AGENT_STATUS_LABEL).sort()).toEqual([...statuses].sort());
  });

  it('echoes a status this build has never seen instead of dropping it', () => {
    // The server can be newer than the bundle; a blank cell would hide an agent.
    expect(agentStatusLabel('paused_by_policy')).toBe('paused_by_policy');
    expect(agentStatusLabel('')).toBe('');
  });
});

describe('agentStatusTone', () => {
  it('maps live, settled and unknown statuses onto the shared tone scale', () => {
    expect(agentStatusTone('running')).toBe('running');
    expect(agentStatusTone('completed')).toBe('ok');
    expect(agentStatusTone('failed')).toBe('danger');
    expect(agentStatusTone('cancelled')).toBe('warn');
    expect(agentStatusTone('interrupted')).toBe('warn');
    expect(agentStatusTone('paused')).toBe('muted');
    expect(agentStatusTone('something_new')).toBe('muted');
  });
});

