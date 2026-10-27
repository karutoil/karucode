import { describe, expect, it } from 'vitest';
import { formatDuration, truncate } from './format';

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
