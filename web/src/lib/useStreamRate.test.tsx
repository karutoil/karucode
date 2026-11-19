import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/preact';
import { useStreamRate, type StreamRate } from './useStreamRate';
import { RATE_SAMPLE_MS } from './tps';

/**
 * A clock the test drives, so "how many samples were taken" is countable: the
 * hook reads the clock once per sample and never otherwise.
 */
let now = 0;
let latest: StreamRate = { tokens: 0, rate: null };
const clock = vi.fn((): number => now);

function Probe({ chars, enabled }: { chars: number; enabled?: boolean }): null {
  latest = useStreamRate(chars, enabled ?? true, clock);
  return null;
}

/** Advance the fake clock and the fake timer queue together, flushing effects. */
async function advance(ms: number): Promise<void> {
  await act(async () => {
    now += ms;
    vi.advanceTimersByTime(ms);
  });
}

function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  act(() => {
    document.dispatchEvent(new Event('visibilitychange'));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  now = 0;
  clock.mockClear();
  latest = { tokens: 0, rate: null };
  setVisibility('visible');
});

afterEach(() => {
  vi.useRealTimers();
  // jsdom's own `visibilityState` is a prototype getter; drop the own property
  // this file installs so the next suite sees the real one.
  Reflect.deleteProperty(document, 'visibilityState');
});

describe('useStreamRate', () => {
  it('does not sample while the tab is hidden', async () => {
    const { rerender } = render(<Probe chars={4000} />);
    // One sample on mount, so a reload mid-answer shows a count immediately.
    expect(clock).toHaveBeenCalledTimes(1);
    expect(latest.tokens).toBe(1000);

    await advance(RATE_SAMPLE_MS * 2);
    const whileVisible = clock.mock.calls.length;
    expect(whileVisible).toBeGreaterThan(1);

    setVisibility('hidden');
    rerender(<Probe chars={4400} />);
    // 60 s backgrounded: not one sample, not one re-render of an invisible badge.
    await advance(60_000);
    expect(clock.mock.calls.length).toBe(whileVisible);
    expect(latest.tokens).toBe(1000);
  });

  it('resumes sampling when the tab becomes visible again', async () => {
    render(<Probe chars={4000} />);
    setVisibility('hidden');
    await advance(5_000);
    const hiddenTicks = clock.mock.calls.length;

    setVisibility('visible');
    expect(clock.mock.calls.length).toBe(hiddenTicks + 1);
    await advance(RATE_SAMPLE_MS);
    expect(clock.mock.calls.length).toBeGreaterThan(hiddenTicks + 1);
  });

  it('does not average the hidden gap into the first visible rate', async () => {
    const { rerender } = render(<Probe chars={4000} />);
    await advance(RATE_SAMPLE_MS);
    rerender(<Probe chars={12000} />);
    await advance(RATE_SAMPLE_MS);
    expect(latest.rate).toBe(4000);

    setVisibility('hidden');
    await advance(60_000);
    // The stream kept arriving while the tab was backgrounded.
    rerender(<Probe chars={16000} />);
    setVisibility('visible');

    // One sample over a one-minute gap is not a rate. If the pre-hide window had
    // been carried over, this would report the average of the hidden minute
    // instead of nothing.
    expect(latest.rate).toBeNull();
    expect(latest.tokens).toBe(4000);

    // Two samples 250 ms apart, growing at the same speed as before: the reading
    // is the stream's own throughput again, not a 60-second average of it.
    rerender(<Probe chars={20000} />);
    await advance(RATE_SAMPLE_MS);
    rerender(<Probe chars={24000} />);
    await advance(RATE_SAMPLE_MS);
    expect(latest.rate).toBe(4000);
  });

  it('stops ticking when the turn is no longer working', async () => {
    const { rerender } = render(<Probe chars={4000} />);
    rerender(<Probe chars={4000} enabled={false} />);
    const stopped = clock.mock.calls.length;
    await advance(5_000);
    expect(clock.mock.calls.length).toBe(stopped);
    expect(latest).toEqual({ tokens: 0, rate: null });
  });
});
