import { useEffect, useRef, useState } from 'preact/hooks';
import {
  RATE_SAMPLE_MS,
  estimateTokens,
  tokensPerSecond,
  withSample,
  type TokenRateSample,
} from './tps';

/** What a working turn's badge needs to render. */
export interface StreamRate {
  /** Estimated output tokens produced so far. */
  tokens: number;
  /** Tokens per second, or `null` when there is not enough evidence to say. */
  rate: number | null;
}

/**
 * Follow a streaming turn's throughput.
 *
 * The clock runs only while the turn is working (`enabled`). Sampling on a fixed
 * tick rather than on each delta is what makes a stalled stream visible: the
 * count stops growing, [`tokensPerSecond`] stops finding growth in the window and
 * returns `null`, and the badge drops the number while keeping the count. A
 * run that spends 40 s inside `cargo test` therefore shows "8.4k" and no rate,
 * instead of freezing at whatever the last burst happened to be.
 *
 * Clearing on `!enabled` means the next turn measures only itself instead of
 * averaging across the gap between two turns.
 *
 * `clock` exists for tests: passing a fake keeps this deterministic instead of
 * racing a real interval.
 */
export function useStreamRate(
  contentChars: number,
  enabled: boolean,
  clock: () => number = Date.now,
): StreamRate {
  const samples = useRef<TokenRateSample[]>([]);
  const latestChars = useRef(contentChars);
  latestChars.current = contentChars;
  const [state, setState] = useState<StreamRate>({ tokens: 0, rate: null });

  useEffect(() => {
    if (!enabled) {
      samples.current = [];
      setState({ tokens: 0, rate: null });
      return;
    }

    const take = (): void => {
      // A hidden tab's badge is not on screen, so nothing there is worth a wake
      // up: browsers throttle a background interval to ~1/s rather than to zero,
      // and every tick still estimates, samples and can re-render. Several
      // streaming blocks each held one of these timers.
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      const now = clock();
      const tokens = estimateTokens(latestChars.current);
      const next = withSample(samples.current, { at: now, tokens });
      samples.current = next;
      const rate = tokensPerSecond(next, now);
      // A long `cargo test` ticks with nothing to report; re-rendering the badge
      // 160 times for an unchanged pair is pure waste.
      setState((previous) =>
        previous.tokens === tokens && previous.rate === rate ? previous : { tokens, rate },
      );
    };

    /**
     * Coming back into view starts a new window rather than continuing the old
     * one.
     *
     * The samples from before the tab was hidden are a minute old, and dividing
     * however much arrived since by that gap reports the average of a stretch
     * nobody was watching: 100 tokens that trickled in while backgrounded for a
     * minute would read "1.7 t/s" while the stream is really producing thousands.
     * The count is taken again immediately; the rate follows one tick later, when
     * there are two samples 250 ms apart to compare.
     */
    const resume = (): void => {
      if (typeof document === 'undefined' || document.visibilityState === 'hidden') return;
      samples.current = [];
      take();
    };

    // One sample now, so a turn that is already mid-answer when this mounts (a
    // reload during a run) shows its count immediately rather than after a tick.
    take();
    const timer = setInterval(take, RATE_SAMPLE_MS);
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', resume);
    return () => {
      clearInterval(timer);
      if (typeof document !== 'undefined') {
        document.removeEventListener('visibilitychange', resume);
      }
    };
  }, [enabled, clock]);

  return state;
}
