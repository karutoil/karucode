/* Tokens per second, measured from text that is still arriving.
 *
 * The provider reports `usage` once per call, at the very end of it, so it can
 * never answer "how fast is this going right now" while the answer is streaming.
 * The only live signal is the text itself, which is why everything here works on
 * a character count and a clock.
 *
 * The estimate is `characters / 4`, the same ratio `kc-core` uses to size the
 * request budget (`CHARS_PER_TOKEN` in `crates/kc-core/src/config.rs`). It is an
 * estimate and is labelled as one: a rate computed from the real `output_tokens`
 * would only exist after generation stopped, which is exactly when nobody needs
 * it.
 */

/**
 * Characters per token, matching `CHARS_PER_TOKEN` in `kc-core`.
 *
 * Keep the two in step: this number decides what the badge claims, and the
 * budget it is compared against on the server is derived from the same ratio.
 */
export const CHARS_PER_TOKEN = 4;

/** One point on the growth curve of a streaming answer. */
export interface TokenRateSample {
  /** Wall-clock time the sample was taken (`Date.now()`). */
  at: number;
  /** Estimated output tokens produced by the turn at that moment. */
  tokens: number;
}

/** How far back the rate is averaged. */
export const RATE_WINDOW_MS = 3_000;

/**
 * The shortest span a rate is computed over.
 *
 * A single 250 ms tick of a fast model looks like 400 t/s, and a partial first
 * chunk understates it just as badly; both are noise rather than speed.
 */
export const RATE_MIN_SPAN_MS = 500;

/**
 * No new tokens for this long means generation stopped.
 *
 * The badge keeps the token count (the turn is still working — a tool call is
 * running) but drops the rate, which would otherwise freeze at whatever the last
 * burst happened to be.
 */
export const RATE_IDLE_MS = 2_000;

/** How often a working turn is sampled while it grows. */
export const RATE_SAMPLE_MS = 250;

/** Estimated tokens for a character count. */
export function estimateTokens(chars: number): number {
  if (!Number.isFinite(chars) || chars <= 0) return 0;
  return Math.round(chars / CHARS_PER_TOKEN);
}

/**
 * Add a sample and drop everything older than the trailing window.
 *
 * At least two samples survive — the newest pair — because the rate needs a
 * span, and an empty window on a long stream would blink the number away.
 */
export function withSample(
  samples: readonly TokenRateSample[],
  next: TokenRateSample,
  windowMs: number = RATE_WINDOW_MS,
): TokenRateSample[] {
  const out = [...samples, next];
  while (out.length > 2) {
    const second = out[1];
    if (second === undefined || next.at - second.at < windowMs) break;
    out.shift();
  }
  return out;
}

/**
 * Tokens per second over the sampled window, or `null` when there is not enough
 * evidence to claim a number: fewer than two samples, a span shorter than
 * [`RATE_MIN_SPAN_MS`], no growth at all, or a stream that has gone quiet for
 * longer than [`RATE_IDLE_MS`].
 */
export function tokensPerSecond(samples: readonly TokenRateSample[], now: number): number | null {
  if (samples.length < 2) return null;
  const last = samples[samples.length - 1];
  const first = samples[0];
  if (!last || !first) return null;
  if (now - last.at > RATE_IDLE_MS) return null;
  const spanMs = last.at - first.at;
  if (spanMs < RATE_MIN_SPAN_MS) return null;
  const delta = last.tokens - first.tokens;
  if (delta <= 0) return null;
  return (delta * 1000) / spanMs;
}

/** `157` — one decimal below 100, whole numbers above it, so the pill is stable. */
export function formatRate(rate: number): string {
  if (!Number.isFinite(rate) || rate <= 0) return '0.0';
  // Round first, then choose the shape: otherwise 99.95 renders as "100.0",
  // which reads as a different magnitude from the "100" it sits next to.
  const rounded = Math.round(rate * 10) / 10;
  return rounded >= 100 ? String(Math.round(rounded)) : rounded.toFixed(1);
}

/**
 * The token counter beside the rate: exact below 10 000, then compact, so the
 * badge keeps a stable width as the count climbs instead of shoving the rate
 * sideways every few seconds.
 */
export function formatTokenCount(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return '0';
  if (tokens < 10_000) return String(Math.round(tokens));
  if (tokens < 1_000_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

/**
 * Rate bands for the badge's colour. Fast is not "better" in any absolute
 * sense — it is a glanceable signal that the model is chewing through output
 * rather than trickling.
 */
export type RateTone = 'fast' | 'steady' | 'slow';

export function rateTone(rate: number): RateTone {
  if (rate >= 50) return 'fast';
  if (rate >= 20) return 'steady';
  return 'slow';
}
