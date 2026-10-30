import type { JSX } from 'preact';
import { useStreamRate } from '../lib/useStreamRate';
import { formatRate, formatTokenCount, rateTone } from '../lib/tps';

export interface TpsBadgeProps {
  /** Estimated output characters the turn has produced so far. */
  chars: number;
  /** True while the turn is still doing something; false removes the badge. */
  working: boolean;
}

/**
 * Live throughput for a working turn: how many tokens it has produced and how
 * fast they are arriving.
 *
 * It is mounted only while the turn works, so it cannot become a stale number
 * sitting above a finished answer — the same rule as the "Working" chip, which
 * this sits next to.
 *
 * Both halves are estimates pending the provider's own `usage` block, which only
 * arrives once the call is over. The character ratio matches the server's
 * (`CHARS_PER_TOKEN`), and the tooltip says "estimated" rather than letting the
 * number imply a tokenizer count.
 */
export function TpsBadge({ chars, working }: TpsBadgeProps): JSX.Element | null {
  const { tokens, rate } = useStreamRate(chars, working);
  if (!working) return null;

  const rateText = rate === null ? null : formatRate(rate);
  const title =
    rateText === null
      ? `≈${tokens.toLocaleString()} output tokens produced so far (estimated from streamed characters). The rate appears while text is arriving.`
      : `≈${tokens.toLocaleString()} output tokens at ${rateText} tokens per second (estimated from streamed characters; the provider's own count arrives when the call ends).`;

  return (
    <span class="tps-badge" title={title} data-tone={rateText === null ? 'idle' : rateTone(rate as number)}>
      <span class="tps-badge-count">{formatTokenCount(tokens)}</span>
      {rateText === null ? null : (
        <span class="tps-badge-rate">
          {rateText}
          <span class="tps-badge-unit">t/s</span>
        </span>
      )}
    </span>
  );
}
