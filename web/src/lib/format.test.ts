import { describe, expect, it } from 'vitest';
import { clockTime, formatDuration, relativeTime, truncate } from './format';

describe('formatDuration', () => {
  it('never renders a suspicious zero', () => {
    expect(formatDuration(0)).toBe('<1ms');
    expect(formatDuration(0.4)).toBe('<1ms');
  });

  it('renders sub-second, second, and minute scales', () => {
    expect(formatDuration(1)).toBe('1ms');
    expect(formatDuration(999)).toBe('999ms');
    expect(formatDuration(1500)).toBe('1.5s');
    expect(formatDuration(45000)).toBe('45s');
    expect(formatDuration(125000)).toBe('2m 5s');
  });

  it('is empty for unknown durations', () => {
    expect(formatDuration(null)).toBe('');
    expect(formatDuration(Number.NaN)).toBe('');
  });
});

describe('truncate', () => {
  it('leaves short text alone and marks cut text', () => {
    expect(truncate('short', 10)).toBe('short');
    expect(truncate('abcdefghij', 4)).toBe('abc…');
  });
});

describe('relativeTime', () => {
  const now = 1_710_000_100_000;

  it('says "just now" for anything under a minute, including a clock ahead of ours', () => {
    expect(relativeTime(now, now)).toBe('just now');
    expect(relativeTime(now - 10_000, now)).toBe('just now');
    // A server timestamp in the future is a clock-skew fact, not a prophecy.
    expect(relativeTime(now + 5_000, now)).toBe('just now');
  });

  it('steps through minutes, hours and days', () => {
    expect(relativeTime(now - 2 * 60_000, now)).toBe('2m ago');
    expect(relativeTime(now - 90 * 60_000, now)).toBe('2h ago');
    expect(relativeTime(now - 3 * 86_400_000, now)).toBe('3d ago');
  });

  it('falls back to a date past a week, with the year only when it is not this one', () => {
    const sameYear = new Date(now).getFullYear();
    const month = new Date(now - 10 * 86_400_000);
    expect(relativeTime(month.getTime(), now)).toContain(
      month.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
    );
    const lastYear = new Date(sameYear - 1, 5, 15).getTime();
    expect(relativeTime(lastYear, now)).toContain(String(sameYear - 1));
  });
});

describe('clockTime', () => {
  it('renders the local time of day, hour and minute', () => {
    // Same construction the transcript's `<time>` renders: assert against the
    // locale the test process runs in rather than against a hard-coded "11:01 AM".
    const at = 1_710_000_100_000;
    expect(clockTime(at)).toBe(
      new Date(at).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }),
    );
    expect(clockTime(at)).toMatch(/\d/);
  });

  it('is empty for a timestamp the server never sent', () => {
    // Rows carry `at: 0` when they could not be timed; an empty string renders no
    // `<time>` element at all, where `Invalid Date` would render a lie.
    expect(clockTime(0)).toBe('');
    expect(clockTime(Number.NaN)).toBe('');
    expect(clockTime(Number.POSITIVE_INFINITY)).toBe('');
    expect(clockTime(-1)).toBe('');
  });
});
