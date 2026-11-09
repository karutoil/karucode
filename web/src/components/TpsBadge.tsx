import type { JSX } from 'preact';
import { useStreamRate } from '../lib/useStreamRate';
import { formatRate, formatTokenCount, rateTone } from '../lib/tps';

/**
 * Which stream the measured characters came from.
 *
 * The glyph, the unit and the estimate are identical for all three; only the
 * tooltip's noun changes, so a reader hovering a thought is not told they are
 * looking at "output tokens" when they are looking at the model's reasoning.
 */
export type TpsSource = 'text' | 'reasoning' | 'result';

const SOURCE_NOUN: Record<TpsSource, string> = {
  text: 'output tokens',
  reasoning: 'reasoning tokens',
  result: 'tokens of tool output',
};

export interface TpsBadgeProps {
  /** Characters this block has produced so far. */
  chars: number;
  /** True while the block is still producing; false removes the badge. */
  working: boolean;
  /** What the streamed text is, for the tooltip. Defaults to prose output. */
  source?: TpsSource;
}

/**
 * Live throughput for one block of a turn: how many tokens it has produced and
 * how fast they are arriving.
 *
 * It belongs to the block that is producing text - a reasoning block, a
 * streaming message, a running call - and is mounted only while that block
 * works, so it cannot become a stale number sitting under finished prose. The
 * same rule as the "Working" chip, applied per block instead of per turn: a
 * turn-level badge sat above the answer where a long run had scrolled it out of
 * view, and it said nothing about the step the reader was actually watching.
 *
 * Both halves are estimates pending the provider's own `usage` block, which only
 * arrives once the call is over. The character ratio matches the server's
 * (`CHARS_PER_TOKEN`), and the tooltip says "estimated" rather than letting the
 * number imply a tokenizer count.
 */
export function TpsBadge({ chars, working, source = 'text' }: TpsBadgeProps): JSX.Element | null {
  const { tokens, rate } = useStreamRate(chars, working);
  if (!working) return null;

  const noun = SOURCE_NOUN[source];
  const rateText = rate === null ? null : formatRate(rate);
  const title =
    rateText === null
      ? `≈${tokens.toLocaleString()} ${noun} produced so far (estimated from streamed characters). The rate appears while text is arriving.`
      : `≈${tokens.toLocaleString()} ${noun} at ${rateText} tokens per second (estimated from streamed characters; the provider's own count arrives when the call ends).`;

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
