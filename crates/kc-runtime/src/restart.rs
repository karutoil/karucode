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

use kc_core::config::{Limits, ModelConfig};
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

/// How many times, and how patiently, a *turn* may be asked again after the
/// provider layer has already given up on it.
///
/// This is the third and slowest retry layer, and it answers a different question
/// from [`StreamRestarts`]. Both provider budgets are measured in attempts and
/// seconds, which is right for a channel that dies mid-request; they are hopeless
/// for a condition that clears in minutes - a model in cooldown, a gateway whose
/// whole upstream is briefly out. When they are spent the run simply ends, and the
/// only recovery left is the operator sending the prompt again.
///
/// So this layer waits: `[limits] turn_retry_delay_ms` before the first retry,
/// doubling per retry up to eight times that, for at most
/// `[limits] max_turn_retries` passes (`2` by default, `0` disables the layer).
/// Every retry is announced (`stream_retry` plus a warning notice) and nothing of
/// a failed attempt is persisted, so a retry can never duplicate an answer.
///
/// Only a failure a wait can fix reaches it: the caller asks
/// [`kc_core::error::is_transient_provider_failure`] first, so a dead key, a
/// content refusal, a judgement on the request and a context overflow still end the
/// run at once with their real reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnRetries {
    max_retries: u32,
    base_delay: Duration,
}

impl TurnRetries {
    pub(crate) fn new(max_retries: u32, base_delay: Duration) -> Self {
        Self {
            max_retries,
            base_delay,
        }
    }

    /// The policy the operator configured (`[limits] max_turn_retries` /
    /// `turn_retry_delay_ms`).
    pub(crate) fn from_limits(limits: &Limits) -> Self {
        Self::new(
            limits.max_turn_retries,
            Duration::from_millis(limits.turn_retry_delay_ms),
        )
    }

    pub(crate) fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// The wait before retry `used + 1`, or `None` once the budget is spent.
    pub(crate) fn remaining(&self, used: u32) -> Option<Duration> {
        if used >= self.max_retries {
            return None;
        }
        Some(self.delay(used))
    }

    /// The base wait, doubling per retry and capped at eight times the base.
    ///
    /// `max_turn_retries` is at most 10, so this never shifts far; the cap exists
    /// so a large configured base cannot turn one retry into an all-day park.
    pub(crate) fn delay(&self, used: u32) -> Duration {
        let factor = 1u32 << used.min(3);
        self.base_delay.saturating_mul(factor)
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

    #[test]
    fn the_slow_layer_waits_longer_each_time_and_then_stops() {
        let policy = TurnRetries::new(3, Duration::from_secs(30));
        assert_eq!(policy.max_retries(), 3);
        // 30 s, 60 s, 120 s - minutes, not milliseconds, because the condition it
        // waits for is a channel coming back.
        assert_eq!(policy.remaining(0), Some(Duration::from_secs(30)));
        assert_eq!(policy.remaining(1), Some(Duration::from_secs(60)));
        assert_eq!(policy.remaining(2), Some(Duration::from_secs(120)));
        assert_eq!(policy.remaining(3), None, "the budget is spent");
        assert_eq!(policy.remaining(99), None);
    }

    #[test]
    fn the_wait_is_capped_at_eight_times_the_base() {
        let policy = TurnRetries::new(10, Duration::from_secs(30));
        // 30, 60, 120, 240, then the 8x ceiling.
        assert_eq!(policy.delay(3), Duration::from_secs(240));
        for used in 4..10 {
            assert_eq!(policy.delay(used), Duration::from_secs(240));
        }
    }

    #[test]
    fn zero_turn_retries_disable_the_layer() {
        let policy = TurnRetries::new(0, Duration::from_secs(30));
        assert_eq!(policy.remaining(0), None);
        assert_eq!(policy.max_retries(), 0);
    }

    #[test]
    fn the_policy_follows_the_limits_and_keeps_the_documented_default() {
        // The default that an operator gets: two retries, a 30 s first wait.
        let defaults = TurnRetries::from_limits(&Limits::default());
        assert_eq!(defaults.max_retries(), 2);
        assert_eq!(defaults.delay(0), Duration::from_secs(30));
        assert_eq!(Limits::default().max_turn_retries, 2);

        let configured = TurnRetries::from_limits(&Limits {
            max_turn_retries: 1,
            turn_retry_delay_ms: 1_000,
            ..Limits::default()
        });
        assert_eq!(configured.max_retries(), 1);
        assert_eq!(configured.delay(0), Duration::from_secs(1));
    }
}
