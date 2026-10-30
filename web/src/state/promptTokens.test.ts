import { beforeEach, describe, expect, it } from 'vitest';
import {
  forgetPromptTokens,
  recallPromptTokens,
  rememberPromptTokens,
  resetPromptTokens,
} from './promptTokens';

/* The store is a browser cache of one provider number per session. The cases
 * that matter are the ones that would otherwise show a *wrong* pill rather than
 * no pill: a stale session's number leaking into another conversation, or a
 * corrupt payload throwing on the first render. */

describe('promptTokens', () => {
  beforeEach(() => {
    resetPromptTokens();
  });

  it('round-trips a number per session', () => {
    rememberPromptTokens('s_a', 46_000);
    rememberPromptTokens('s_b', 12_000);
    expect(recallPromptTokens('s_a')).toBe(46_000);
    expect(recallPromptTokens('s_b')).toBe(12_000);
  });

  it('answers null for a session it has never seen', () => {
    expect(recallPromptTokens('s_never')).toBeNull();
    expect(recallPromptTokens(null)).toBeNull();
  });

  it('keeps sessions apart instead of sharing one figure', () => {
    rememberPromptTokens('s_a', 100_000);
    rememberPromptTokens('s_b', 5_000);
    expect(recallPromptTokens('s_a')).toBe(100_000);
  });

  it('overwrites with the newest call rather than accumulating', () => {
    rememberPromptTokens('s_a', 40_000);
    rememberPromptTokens('s_a', 55_000);
    expect(recallPromptTokens('s_a')).toBe(55_000);
  });

  it('ignores nonsense instead of storing it', () => {
    for (const value of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      rememberPromptTokens('s_a', value);
      expect(recallPromptTokens('s_a')).toBeNull();
    }
    rememberPromptTokens(null, 5_000);
    expect(recallPromptTokens(null)).toBeNull();
  });

  it('rounds to a whole token', () => {
    rememberPromptTokens('s_a', 1_234.6);
    expect(recallPromptTokens('s_a')).toBe(1_235);
  });

  it('caps how many sessions it remembers, dropping the least recent', () => {
    for (let index = 0; index < 60; index += 1) rememberPromptTokens(`s_${index}`, 1_000 + index);
    // The last 50 survive; the first ten are gone.
    expect(recallPromptTokens('s_0')).toBeNull();
    expect(recallPromptTokens('s_9')).toBeNull();
    expect(recallPromptTokens('s_10')).toBe(1_010);
    expect(recallPromptTokens('s_59')).toBe(1_059);
  });

  it('moves a re-used session to the front of the eviction order', () => {
    for (let index = 0; index < 50; index += 1) rememberPromptTokens(`s_${index}`, 1_000);
    // Touch the oldest, then add one more: the next-oldest should go instead.
    rememberPromptTokens('s_0', 2_000);
    rememberPromptTokens('s_new', 3_000);
    expect(recallPromptTokens('s_0')).toBe(2_000);
    expect(recallPromptTokens('s_1')).toBeNull();
  });

  it('forgets one session without touching the rest', () => {
    rememberPromptTokens('s_a', 1_000);
    rememberPromptTokens('s_b', 2_000);
    forgetPromptTokens('s_a');
    expect(recallPromptTokens('s_a')).toBeNull();
    expect(recallPromptTokens('s_b')).toBe(2_000);
  });

  it('survives a corrupt payload instead of throwing on first render', () => {
    localStorage.setItem('karucode.lastPromptTokens', '{not json');
    expect(recallPromptTokens('s_a')).toBeNull();
    // …and still works afterwards.
    rememberPromptTokens('s_a', 4_000);
    expect(recallPromptTokens('s_a')).toBe(4_000);
  });

  it('drops junk entries inside an otherwise valid payload', () => {
    localStorage.setItem(
      'karucode.lastPromptTokens',
      JSON.stringify({ good: 9_000, zero: 0, text: 'lots', negative: -5 }),
    );
    expect(recallPromptTokens('good')).toBe(9_000);
    expect(recallPromptTokens('zero')).toBeNull();
    expect(recallPromptTokens('text')).toBeNull();
    expect(recallPromptTokens('negative')).toBeNull();
  });
});
