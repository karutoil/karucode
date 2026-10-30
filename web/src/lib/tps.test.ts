import { describe, expect, it } from 'vitest';
import {
  CHARS_PER_TOKEN,
  RATE_IDLE_MS,
  RATE_MIN_SPAN_MS,
  RATE_WINDOW_MS,
  estimateTokens,
  formatRate,
  rateTone,
  tokensPerSecond,
  withSample,
  type TokenRateSample,
} from './tps';

describe('estimateTokens', () => {
  it('divides characters by the same ratio the server budgets with', () => {
    expect(CHARS_PER_TOKEN).toBe(4);
    expect(estimateTokens(400)).toBe(100);
    expect(estimateTokens(401)).toBe(100);
    expect(estimateTokens(402)).toBe(101);
  });

  it('treats nothing, nonsense and negatives as zero', () => {
    for (const value of [0, -20, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(estimateTokens(value)).toBe(0);
    }
  });
});

describe('withSample', () => {
  const sample = (at: number, tokens: number): TokenRateSample => ({ at, tokens });

  it('appends in order', () => {
    const first = withSample([], sample(1_000, 10));
    expect(first).toEqual([{ at: 1_000, tokens: 10 }]);
    const second = withSample(first, sample(1_250, 20));
    expect(second.map((entry) => entry.tokens)).toEqual([10, 20]);
  });

  it('drops samples that fell out of the trailing window', () => {
    const samples = [sample(0, 0), sample(1_000, 100), sample(2_000, 200)];
    const next = withSample(samples, sample(2_500, 250), RATE_WINDOW_MS);
    // 2_500 - 1_000 is 1_500, under the window, so nothing is trimmed yet.
    expect(next).toHaveLength(4);
    const later = withSample(next, sample(4_000, 400), RATE_WINDOW_MS);
    // 4_000 - 1_000 >= 3_000 drops the 0 ms sample, then 4_000 - 2_000 does not.
    expect(later.map((entry) => entry.at)).toEqual([1_000, 2_000, 2_500, 4_000]);
  });

  it('always keeps two samples so a rate stays computable', () => {
    const samples = [sample(0, 0), sample(10, 5)];
    const next = withSample(samples, sample(60_000, 900), RATE_WINDOW_MS);
    expect(next.map((entry) => entry.at)).toEqual([10, 60_000]);
  });
});

describe('tokensPerSecond', () => {
  const from = (...pairs: [number, number][]): TokenRateSample[] =>
    pairs.map(([at, tokens]) => ({ at, tokens }));

  it('is unknown until there is a span to measure over', () => {
    expect(tokensPerSecond([], 1_000)).toBeNull();
    expect(tokensPerSecond(from([1_000, 50]), 1_000)).toBeNull();
  });

  it('refuses a span shorter than the minimum', () => {
    const samples = from([1_000, 10], [1_000 + RATE_MIN_SPAN_MS - 1, 90]);
    expect(tokensPerSecond(samples, 1_500)).toBeNull();
  });

  it('measures growth over the sampled span', () => {
    // 900 tokens in 1800 ms is 500 t/s.
    const samples = from([1_000, 100], [2_000, 700], [2_800, 1_000]);
    expect(tokensPerSecond(samples, 2_800)).toBeCloseTo(500, 5);
  });

  it('is unknown while the answer has not grown', () => {
    expect(tokensPerSecond(from([1_000, 100], [2_000, 100]), 2_000)).toBeNull();
  });

  it('goes stale when generation stops but the turn keeps working', () => {
    const samples = from([1_000, 100], [2_000, 600]);
    expect(tokensPerSecond(samples, 2_000 + RATE_IDLE_MS)).toBeCloseTo(500, 5);
    expect(tokensPerSecond(samples, 2_000 + RATE_IDLE_MS + 1)).toBeNull();
  });
});

describe('formatRate', () => {
  it('keeps one decimal below 100 and rounds above it', () => {
    expect(formatRate(157.12)).toBe('157');
    expect(formatRate(9.44)).toBe('9.4');
    expect(formatRate(99.95)).toBe('100');
  });

  it('never prints a negative or nonsensical rate', () => {
    expect(formatRate(0)).toBe('0.0');
    expect(formatRate(-4)).toBe('0.0');
    expect(formatRate(Number.NaN)).toBe('0.0');
  });
});

describe('rateTone', () => {
  it('bands fast, steady and slow', () => {
    expect(rateTone(120)).toBe('fast');
    expect(rateTone(50)).toBe('fast');
    expect(rateTone(49.9)).toBe('steady');
    expect(rateTone(20)).toBe('steady');
    expect(rateTone(19.9)).toBe('slow');
    expect(rateTone(0)).toBe('slow');
  });
});
