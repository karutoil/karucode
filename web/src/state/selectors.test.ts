import { describe, expect, it } from 'vitest';
import { emptyChatState } from './chat';
import {
  contextTone,
  contextUsage,
  formatContextPercent,
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
