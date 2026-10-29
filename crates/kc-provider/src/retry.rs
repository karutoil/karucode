//! Pre-content retries for provider streams.
//!
//! The gateway endpoints this client talks to (new-api style aggregators,
//! LiteLLM, a self-hosted proxy) surface a transient upstream death as an
//! arbitrary answer: a rotated key gives 401, a mid-failover channel 404, a
//! dying backend 502, a broken proxy 200 with an HTML splash, and a stream that
//! simply stops mid-body fails with a *decode* error
//! (`invalid response body … error decoding response body`). None of those are
//! worth failing a run over: the next attempt usually lands on a healthy
//! channel.
//!
//! [`RetryingProvider`] therefore retries **every** failure class, up to a
//! configurable attempt budget with an exponential, jittered, capped backoff —
//! but only while **no content has reached the consumer**. Once text, reasoning
//! or a tool-call delta was delivered, replaying the exchange would duplicate
//! it, so the failure is reported honestly instead.
//!
//! Retries live inside the returned stream, not around `stream_chat`, because
//! that is where most of these failures appear: the response headers look fine
//! and the body dies. A failure to even open the stream is handled by the same
//! state machine, so both phases share one policy.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use kc_core::config::ModelConfig;
use kc_core::error::{Error, Result};
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderStream};
use tokio_util::sync::CancellationToken;

/// Default number of RETRIES (so `MAX_RETRIES + 1` upstream attempts).
pub const DEFAULT_MAX_RETRIES: u32 = 50;
/// Default first delay; doubles per attempt.
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(500);
/// Default ceiling for the exponential backoff.
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);

/// How patient a run is with a failing provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RETRIES, DEFAULT_BASE_DELAY, DEFAULT_MAX_DELAY)
    }
}

impl RetryPolicy {
    pub fn new(max_retries: u32, base_delay: Duration, max_delay: Duration) -> Self {
        let base_delay = base_delay.max(Duration::from_millis(1));
        Self {
            max_retries,
            base_delay,
            max_delay: max_delay.max(base_delay),
        }
    }

    /// A policy that never retries (one attempt, then fail).
    pub fn none() -> Self {
        Self::new(0, DEFAULT_BASE_DELAY, DEFAULT_MAX_DELAY)
    }

    /// The policy the operator configured for this endpoint
    /// (`[model] retry_max_retries / retry_base_delay_ms / retry_max_delay_ms`,
    /// overridable with `KARUCODE_RETRY_*`). Zero retries disables the layer.
    pub fn from_config(config: &ModelConfig) -> Self {
        Self::new(
            config.retry_max_retries,
            Duration::from_millis(config.retry_base_delay_ms),
            Duration::from_millis(config.retry_max_delay_ms),
        )
    }

    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Total upstream attempts allowed: the first try plus its retries.
    pub fn max_attempts(&self) -> u32 {
        self.max_retries.saturating_add(1)
    }

    /// Backoff before retry `attempt` (0 = the first retry): exponential with
    /// equal jitter into `[d/2, d]`, capped at `max_delay` so concurrent runs do
    /// not retry in lockstep and a long episode does not park forever.
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u64.checked_shl(attempt.min(32)).unwrap_or(u64::MAX);
        let millis = (self.base_delay.as_millis() as u64).saturating_mul(factor);
        let capped = Duration::from_millis(millis).min(self.max_delay);
        let half = capped / 2;
        half + jitter(half)
    }
}

/// Uniform jitter in `[0, span)` without pulling in an RNG dependency: a
/// xorshift over the current clock's sub-second bits is plenty to decorrelate
/// retries, and never affects correctness.
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

/// Which failures earn another attempt.
///
/// Every *upstream* failure does, including a 4xx: a 401 from an aggregator
/// means *this channel's* key is dead, and the next attempt is a different
/// channel behind the same endpoint. Two things are not retried: a
/// cancellation (the user asked to stop) and a configuration error (an empty
/// `base_url`, a header value that cannot be sent) — no attempt count fixes
/// those, and the request never left the process.
pub fn retryable(err: &Error) -> bool {
    !matches!(err, Error::Cancelled | Error::Config(_))
}

/// Wraps a provider so its streams survive transient upstream failures.
pub struct RetryingProvider {
    inner: Arc<dyn Provider>,
    policy: RetryPolicy,
}

impl RetryingProvider {
    pub fn new(inner: Arc<dyn Provider>, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }

    pub fn policy(&self) -> RetryPolicy {
        self.policy
    }
}

#[async_trait]
impl Provider for RetryingProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ProviderStream> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let state = RetryState {
            inner: Arc::clone(&self.inner),
            request,
            cancel,
            policy: self.policy,
            attempt: 0,
            content_seen: false,
            current: None,
            done: false,
        };
        Ok(Box::pin(stream::unfold(state, |mut state| async move {
            let item = state.next_item().await?;
            Some((item, state))
        })))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // A probe, not a run: the caller decides whether to try again.
        self.inner.list_models().await
    }
}

struct RetryState {
    inner: Arc<dyn Provider>,
    request: ChatRequest,
    cancel: CancellationToken,
    policy: RetryPolicy,
    /// Attempts started so far.
    attempt: u32,
    /// Real content reached the consumer; retrying is over for this stream.
    content_seen: bool,
    current: Option<ProviderStream>,
    done: bool,
}

impl RetryState {
    async fn next_item(&mut self) -> Option<Result<ProviderChunk>> {
        loop {
            if self.done {
                return None;
            }
            if self.cancel.is_cancelled() {
                self.done = true;
                return Some(Err(Error::Cancelled));
            }

            if self.current.is_none() {
                self.attempt = self.attempt.saturating_add(1);
                match self
                    .inner
                    .stream_chat(self.request.clone(), self.cancel.clone())
                    .await
                {
                    Ok(stream) => {
                        self.current = Some(stream);
                        continue;
                    }
                    Err(err) => {
                        if let Some(item) = self.on_failure(err).await {
                            return Some(item);
                        }
                        continue;
                    }
                }
            }

            let cancel = self.cancel.clone();
            let mut current = self.current.take().expect("stream checked above");
            let item = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.done = true;
                    return Some(Err(Error::Cancelled));
                }
                item = current.next() => item,
            };

            let Some(item) = item else {
                // The upstream stream ended. Whatever it delivered already
                // reached the consumer; there is nothing left to retry.
                self.done = true;
                return None;
            };
            self.current = Some(current);

            match item {
                Ok(ProviderChunk::Error { message, .. }) if !self.content_seen => {
                    // Any stream-level failure before content is worth another
                    // attempt: the channel died mid-negotiation, or the
                    // endpoint answered with a splash/JSON error body instead
                    // of an event stream. (After content, the chunk is
                    // forwarded unchanged below.)
                    if let Some(item) = self.on_failure(Error::Provider(message)).await {
                        return Some(item);
                    }
                    continue;
                }
                Ok(chunk) => {
                    if is_content(&chunk) {
                        self.content_seen = true;
                    }
                    return Some(Ok(chunk));
                }
                Err(err) => {
                    if let Some(item) = self.on_failure(err).await {
                        return Some(item);
                    }
                    continue;
                }
            }
        }
    }

    /// Decide what a pre-content failure means. Returns `Some(item)` when the
    /// failure must be surfaced; `None` after a retry was scheduled.
    async fn on_failure(&mut self, err: Error) -> Option<Result<ProviderChunk>> {
        self.current = None; // drop the dead stream before sleeping

        if !retryable(&err) || self.content_seen || self.attempt >= self.policy.max_attempts() {
            self.done = true;
            return Some(Err(self.exhausted(err)));
        }

        let delay = self.policy.delay(self.attempt.saturating_sub(1));
        tracing::warn!(
            provider = self.inner.id(),
            attempt = self.attempt,
            max_attempts = self.policy.max_attempts(),
            delay_ms = delay.as_millis() as u64,
            error = %err,
            "provider attempt failed before any content; retrying"
        );

        let cancel = self.cancel.clone();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.done = true;
                return Some(Err(Error::Cancelled));
            }
            _ = tokio::time::sleep(delay) => {}
        }
        None
    }

    /// Name the attempt count on the terminal error so the user can tell a
    /// single failed call from a retry budget that was exhausted.
    fn exhausted(&self, err: Error) -> Error {
        let attempts = self.attempt.max(1);
        if attempts <= 1 {
            return err;
        }
        let suffix = format!(" (gave up after {attempts} attempts)");
        match err {
            Error::Provider(message) => Error::Provider(message + &suffix),
            Error::Timeout(message) => Error::Timeout(message + &suffix),
            other => Error::Provider(format!("{other}{suffix}")),
        }
    }
}

/// Chunks that mean the answer has really started. Once one of these reached
/// the consumer, a retry would duplicate text/reasoning/tool-call fragments, so
/// the policy stops retrying and reports the failure instead.
fn is_content(chunk: &ProviderChunk) -> bool {
    matches!(
        chunk,
        ProviderChunk::Text(_) | ProviderChunk::Reasoning(_) | ProviderChunk::ToolCallDelta { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_relentless_but_capped() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries(), DEFAULT_MAX_RETRIES);
        assert_eq!(policy.max_attempts(), 51);
        // The first retry waits half-to-full the base delay, not nothing.
        let first = policy.delay(0);
        assert!(
            first >= DEFAULT_BASE_DELAY / 2 && first <= DEFAULT_BASE_DELAY,
            "first retry delay {first:?}"
        );
        assert!(policy.delay(50) <= DEFAULT_MAX_DELAY);
    }

    #[test]
    fn backoff_grows_then_stays_at_the_cap() {
        let policy = RetryPolicy::new(50, Duration::from_millis(100), Duration::from_secs(10));
        // Equal jitter keeps every delay inside [d/2, d].
        for attempt in 0..12u32 {
            let factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
            let expected =
                (Duration::from_millis(100) * factor as u32).min(Duration::from_secs(10));
            let delay = policy.delay(attempt);
            assert!(
                delay >= expected / 2 && delay <= expected,
                "attempt {attempt}: {delay:?} outside [{:?}, {expected:?}]",
                expected / 2
            );
        }
        // Deep attempts sit on the ceiling instead of exploding.
        for _ in 0..5 {
            assert!(policy.delay(50) <= Duration::from_secs(10));
        }
    }

    #[test]
    fn zero_retries_means_one_attempt() {
        let policy = RetryPolicy::none();
        assert_eq!(policy.max_attempts(), 1);
    }

    #[test]
    fn policy_follows_the_model_config() {
        let config = ModelConfig {
            retry_max_retries: 7,
            retry_base_delay_ms: 10,
            retry_max_delay_ms: 40,
            ..ModelConfig::default()
        };
        let policy = RetryPolicy::from_config(&config);
        assert_eq!(policy.max_retries(), 7);
        assert_eq!(policy.max_attempts(), 8);
        // Equal jitter keeps the first retry inside [5ms, 10ms] ...
        let first = policy.delay(0);
        assert!(
            first >= Duration::from_millis(5) && first <= Duration::from_millis(10),
            "first retry delay {first:?}"
        );
        // ... and the ceiling inside [20ms, 40ms].
        for _ in 0..5 {
            assert!(policy.delay(40) <= Duration::from_millis(40));
        }
    }

    #[test]
    fn zero_config_retries_disable_the_layer() {
        let config = ModelConfig {
            retry_max_retries: 0,
            ..ModelConfig::default()
        };
        assert_eq!(RetryPolicy::from_config(&config).max_attempts(), 1);
    }

    #[test]
    fn cancellation_and_config_errors_are_never_retried() {
        assert!(!retryable(&Error::Cancelled));
        assert!(!retryable(&Error::Config("base_url is empty".into())));
        assert!(retryable(&Error::Provider("500".into())));
        assert!(retryable(&Error::Timeout("slow".into())));
    }

    #[test]
    fn content_classes_stop_retrying() {
        assert!(is_content(&ProviderChunk::Text("hi".into())));
        assert!(is_content(&ProviderChunk::Reasoning("hm".into())));
        assert!(is_content(&ProviderChunk::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: Some("{".into()),
        }));
        assert!(!is_content(&ProviderChunk::Usage {
            input_tokens: 1,
            output_tokens: 2,
        }));
        assert!(!is_content(&ProviderChunk::Finish {
            reason: "stop".into()
        }));
    }
}
