//! Mid-stream restart policy for one agent turn.
//!
//! `kc-provider`'s retry layer re-opens a stream only while **nothing** has been
//! delivered: it hands chunks straight to its consumer, so replaying a started
//! answer would duplicate it, and a layer below the UI cannot take back what it
//! already yielded. A gateway that dies *mid-answer* — `error decoding response
//! body`, a truncated SSE event, a body that simply stops without a
//! `finish_reason` — therefore reaches the runtime as a failed turn.
//!
//! The runtime is the first layer that owns both the request and the event
//! stream, so it can do what the provider cannot: throw the partial attempt
//! away (it was never persisted), tell the client to drop what it rendered
//! ([`kc_core::events::Event::StreamRetry`]), and re-issue the identical
//! request. That is what this policy budgets.
//!
//! Two deliberate choices:
//!
//! * the budget is small (`[model] retry_max_stream_restarts`, default 3)
//!   because every restart is a fresh generation, not a cheap reconnect;
//! * the backoff is the same exponential, equal-jittered shape the provider
//!   layer uses, sharing `retry_base_delay_ms` / `retry_max_delay_ms`, so an
//!   operator tunes one backoff for both layers.

use kc_core::config::ModelConfig;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default number of mid-stream restarts when the config does not say.
pub(crate) const DEFAULT_MAX_STREAM_RESTARTS: u32 = 3;
/// Default first delay before a restart.
pub(crate) const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(500);
/// Default ceiling for the restart backoff.
pub(crate) const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);

/// How many times one turn may replace a stream that died after it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamRestarts {
    max_restarts: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl Default for StreamRestarts {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_STREAM_RESTARTS,
            DEFAULT_BASE_DELAY,
            DEFAULT_MAX_DELAY,
        )
    }
}

impl StreamRestarts {
    pub(crate) fn new(max_restarts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        let base_delay = base_delay.max(Duration::from_millis(1));
        Self {
            max_restarts,
            base_delay,
            max_delay: max_delay.max(base_delay),
        }
    }

    /// The policy the operator configured (`[model] retry_max_stream_restarts`,
    /// overridable with `KARUCODE_RETRY_MAX_STREAM_RESTARTS`). Zero disables
    /// restarts: a mid-stream failure then fails the turn, exactly as before.
    pub(crate) fn from_config(config: &ModelConfig) -> Self {
        Self::new(
            config.retry_max_stream_restarts,
            Duration::from_millis(config.retry_base_delay_ms),
            Duration::from_millis(config.retry_max_delay_ms),
        )
    }

    /// Stream attempts one turn may make: the first try plus its restarts.
    pub(crate) fn max_attempts(&self) -> u32 {
        self.max_restarts.saturating_add(1)
    }

    /// Backoff before restart `attempt` (0 = the first restart): exponential
    /// with equal jitter into `[d/2, d]`, capped at the ceiling.
    pub(crate) fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u64.checked_shl(attempt.min(32)).unwrap_or(u64::MAX);
        let millis = (self.base_delay.as_millis() as u64).saturating_mul(factor);
        let capped = Duration::from_millis(millis).min(self.max_delay);
        let half = capped / 2;
        half + jitter(half)
    }
}

/// Uniform jitter in `[0, span)`: a xorshift over the clock's nanosecond bits.
/// Enough to keep concurrent runs out of lockstep; it can never affect
/// correctness, so it needs no RNG dependency.
fn jitter(span: Duration) -> Duration {
    let span = span.as_nanos() as u64;
    if span == 0 {
        return Duration::ZERO;
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = seed ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    Duration::from_nanos(x % span)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(max_restarts: u32) -> StreamRestarts {
        StreamRestarts::new(
            max_restarts,
            Duration::from_millis(2),
            Duration::from_millis(8),
        )
    }

    #[test]
    fn defaults_allow_three_restarts() {
        let policy = StreamRestarts::default();
        assert_eq!(policy.max_attempts(), 4, "one try plus three restarts");
        let first = policy.delay(0);
        assert!(
            first >= DEFAULT_BASE_DELAY / 2 && first <= DEFAULT_BASE_DELAY,
            "first restart delay {first:?}"
        );
    }

    #[test]
    fn zero_restarts_means_one_attempt() {
        assert_eq!(fast(0).max_attempts(), 1);
    }

    #[test]
    fn config_zero_disables_the_layer() {
        let config = ModelConfig {
            retry_max_stream_restarts: 0,
            ..ModelConfig::default()
        };
        assert_eq!(StreamRestarts::from_config(&config).max_attempts(), 1);
    }

    #[test]
    fn the_policy_follows_the_model_config() {
        let config = ModelConfig {
            retry_max_stream_restarts: 2,
            retry_base_delay_ms: 10,
            retry_max_delay_ms: 40,
            ..ModelConfig::default()
        };
        let policy = StreamRestarts::from_config(&config);
        assert_eq!(policy.max_attempts(), 3);
        assert!(policy.delay(0) <= Duration::from_millis(10));
        // Deep attempts sit on the ceiling instead of exploding.
        for _ in 0..5 {
            assert!(policy.delay(9) <= Duration::from_millis(40));
        }
    }

    #[test]
    fn backoff_grows_then_stays_at_the_cap() {
        let policy = StreamRestarts::new(9, Duration::from_millis(10), Duration::from_millis(40));
        // Equal jitter keeps every delay inside [d/2, d] until the cap.
        for attempt in 0..4u32 {
            let factor = 1u64 << attempt.min(3);
            let expected =
                (Duration::from_millis(10) * factor as u32).min(Duration::from_millis(40));
            let delay = policy.delay(attempt);
            assert!(
                delay >= expected / 2 && delay <= expected,
                "attempt {attempt}: {delay:?} outside [{:?}, {expected:?}]",
                expected / 2
            );
        }
    }
}
