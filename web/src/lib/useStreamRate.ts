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

    // One sample now, so a turn that is already mid-answer when this mounts (a
    // reload during a run) shows its count immediately rather than after a tick.
    take();
    const timer = setInterval(take, RATE_SAMPLE_MS);
    return () => clearInterval(timer);
  }, [enabled, clock]);

  return state;
}
