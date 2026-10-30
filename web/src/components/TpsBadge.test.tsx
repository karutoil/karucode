import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/preact';
import { TpsBadge } from './TpsBadge';
import { RATE_SAMPLE_MS } from '../lib/tps';

/* The badge is a clock plus a character count, so the tests drive the fake timers
 * rather than waiting on real intervals. `Date.now` is stubbed to the same clock
 * so a sampled rate is deterministic. */

function useClock(): { advance: (ms: number) => void; start: number } {
  const start = 1_000_000;
  let now = start;
  vi.useFakeTimers();
  vi.setSystemTime(start);
  const advance = (ms: number): void => {
    now += ms;
    vi.setSystemTime(now);
    vi.advanceTimersByTime(ms);
  };
  return { advance, start };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('TpsBadge', () => {
  it('renders nothing once the turn has settled', () => {
    // The whole point is that it cannot outlive the run: a token count sitting
    // above a finished answer would be a claim about a stream that ended.
    const { container } = render(<TpsBadge chars={4_000} working={false} />);
    expect(container.querySelector('.tps-badge')).toBeNull();
  });

  it('shows the token estimate as soon as the turn is working', () => {
    const { advance } = useClock();
    render(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    // 40 000 characters at the server's four-chars-per-token ratio.
    expect(screen.getByText('10.0k')).toBeTruthy();
  });

  it('adds the rate once two samples span enough time', () => {
    const { advance, start } = useClock();
    // Growing content: one rerender per tick, the way deltas arrive.
    const { rerender } = render(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    rerender(<TpsBadge chars={48_000} working />);
    advance(RATE_SAMPLE_MS);
    rerender(<TpsBadge chars={56_000} working />);
    advance(RATE_SAMPLE_MS);

    const pill = document.querySelector('.tps-badge-rate');
    expect(pill, 'a rate is shown').toBeTruthy();
    expect(pill?.textContent ?? '').toContain('t/s');
    expect(start).toBeGreaterThan(0);
  });

  it('keeps the count but drops the rate while a tool call runs', () => {
    const { advance } = useClock();
    const { rerender } = render(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    rerender(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    // No growth, so only the count is claimed.
    expect(document.querySelector('.tps-badge-rate')).toBeNull();
    expect(document.querySelector('.tps-badge-count')?.textContent).toBe('10.0k');
    // Still working, but the stream went quiet: no rate may be invented.
    advance(5_000);
    expect(document.querySelector('.tps-badge-rate')).toBeNull();
    expect(document.querySelector('.tps-badge')?.getAttribute('data-tone')).toBe('idle');
  });

  it('says both numbers are estimates in the tooltip', () => {
    const { advance } = useClock();
    const { rerender } = render(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    rerender(<TpsBadge chars={60_000} working />);
    advance(RATE_SAMPLE_MS);
    rerender(<TpsBadge chars={80_000} working />);
    advance(RATE_SAMPLE_MS);
    const badge = document.querySelector('.tps-badge') as HTMLElement;
    expect(badge.title).toContain('≈');
    expect(badge.title.toLowerCase()).toContain('estimated');
    expect(document.querySelector('.tps-badge-rate')).toBeTruthy();
    // The count-only tooltip (stream stalled) is explicit too.
    rerender(<TpsBadge chars={80_000} working={false} />);
    rerender(<TpsBadge chars={80_000} working />);
    advance(RATE_SAMPLE_MS);
    expect((document.querySelector('.tps-badge') as HTMLElement).title.toLowerCase()).toContain(
      'estimated',
    );
  });

  it('resets when the turn settles so the next one measures only itself', () => {
    const { advance } = useClock();
    const { rerender } = render(<TpsBadge chars={40_000} working />);
    advance(RATE_SAMPLE_MS);
    expect(screen.getByText('10.0k')).toBeTruthy();

    rerender(<TpsBadge chars={0} working={false} />);
    expect(document.querySelector('.tps-badge')).toBeNull();

    rerender(<TpsBadge chars={400} working />);
    advance(RATE_SAMPLE_MS);
    // The new turn's own count, not the previous turn's total.
    expect(screen.getByText('100')).toBeTruthy();
  });
});
