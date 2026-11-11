//! Pre-content retries for provider streams.
//!
//! The gateway endpoints this client talks to (new-api style aggregators,
//! LiteLLM, a self-hosted proxy) surface a transient upstream death as an
//! arbitrary answer: a dying backend gives 502, a rate-limited channel 429, a
//! broken proxy 200 with an HTML splash, and a stream that simply stops mid-body
//! fails with a *decode* error (`invalid response body … error decoding response
//! body`). None of those are worth failing a run over: the next attempt usually
//! lands on a healthy channel.
//!
//! What is *not* retried is a request the endpoint has already judged: a 400 for
//! an unsupported body field, a 413/422 for a payload it refuses, a 401 for a
//! revoked key, a 404 for a model id that does not exist. Re-sending those 51
//! times with a 30 s backoff ceiling burns ~20 minutes and 51 billed attempts
//! before the user sees the real error, and the answer never changes. See
//! [`retryable`] for the exact table.
//!
//! [`RetryingProvider`] therefore retries the transient classes, up to a
//! configurable attempt budget **and** a wall-clock budget, with an exponential,
//! jittered, capped backoff — but only while **no content has reached the
//! consumer**. Once text, reasoning or a tool-call delta was delivered, replaying
//! the exchange would duplicate it, so the failure is reported honestly instead.
//!
//! Retries live inside the returned stream, not around `stream_chat`, because
//! that is where most of these failures appear: the response headers look fine
//! and the body dies. A failure to even open the stream is handled by the same
//! state machine, so both phases share one policy.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use kc_core::config::ModelConfig;
use kc_core::error::{Error, Result};
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderStream};
use tokio_util::sync::CancellationToken;

use crate::errors;

/// Default number of RETRIES (so `MAX_RETRIES + 1` upstream attempts).
pub const DEFAULT_MAX_RETRIES: u32 = 50;
/// Default first delay; doubles per attempt.
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(500);
/// Default ceiling for the exponential backoff.
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);
/// Default wall-clock budget for one stream's retries.
///
/// The attempt count alone cannot bound a run: 50 retries against a 30 s backoff
/// ceiling is ~20 minutes of a hung turn. This is the hard stop, and it is a
/// constant because `ModelConfig` (in `kc-core`) has no field for it yet — see
/// `retry_max_total_secs` in the report; wiring it up is a one-line
/// `RetryPolicy::with_max_total` call once the field exists.
pub const DEFAULT_MAX_TOTAL_RETRY: Duration = Duration::from_secs(300);

/// How patient a run is with a failing provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    base_delay: Duration,
    max_delay: Duration,
    max_total: Duration,
    /// The endpoint fronts several upstream channels, so a 401/403/404 may mean
    /// "this channel's key is dead" rather than "your request is wrong".
    aggregator: bool,
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
            max_total: DEFAULT_MAX_TOTAL_RETRY,
            aggregator: false,
        }
    }

    /// Mark the endpoint as a multi-channel aggregator, which makes a
    /// 401/403/404 retryable (see [`retryable_with`]).
    pub fn with_aggregator(mut self, aggregator: bool) -> Self {
        self.aggregator = aggregator;
        self
    }

    /// Whether this endpoint is treated as an aggregator.
    pub fn aggregator(&self) -> bool {
        self.aggregator
    }

    /// Override the wall-clock retry budget (default [`DEFAULT_MAX_TOTAL_RETRY`]).
    pub fn with_max_total(mut self, max_total: Duration) -> Self {
        self.max_total = max_total;
        self
    }

    /// A policy that never retries (one attempt, then fail).
    pub fn none() -> Self {
        Self::new(0, DEFAULT_BASE_DELAY, DEFAULT_MAX_DELAY)
    }

    /// The policy the operator configured for this endpoint
    /// (`[model] retry_max_retries / retry_base_delay_ms / retry_max_delay_ms /
    /// retry_max_total_secs`, overridable with `KARUCODE_RETRY_*`). Zero retries
    /// disables the layer; zero `retry_max_total_secs` leaves only the attempt
    /// count as a bound.
    pub fn from_config(config: &ModelConfig) -> Self {
        let policy = Self::new(
            config.retry_max_retries,
            Duration::from_millis(config.retry_base_delay_ms),
            Duration::from_millis(config.retry_max_delay_ms),
        );
        if config.retry_max_total_secs == 0 {
            // `Duration::MAX` is the "no wall-clock bound" sentinel; the attempt
            // count still applies.
            policy
                .with_max_total(Duration::MAX)
                .with_aggregator(config.aggregator)
        } else {
            policy
                .with_max_total(Duration::from_secs(config.retry_max_total_secs))
                .with_aggregator(config.aggregator)
        }
    }

    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Wall-clock budget for the retries of one stream.
    pub fn max_total(&self) -> Duration {
        self.max_total
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
/// The table, by HTTP status (the status is carried in the message by
/// `errors::http_status_error`, see `errors::HTTP_STATUS_TAG`):
///
/// | status | retried | why |
/// |---|---|---|
/// | 408, 409, 425, 429 | yes | the endpoint is busy or the request raced; the next attempt can succeed |
/// | 5xx | yes | the upstream is failing, not the request |
/// | 400, 413, 422 | **no** | a malformed or oversized request stays malformed; re-sending it 51 times only delays the real error |
/// | 401, 403, 404 | **no** (default) | a revoked key or a missing model id is permanent for this endpoint |
/// | other 4xx | **no** | the endpoint judged the request |
///
/// 401/403/404 are the interesting case: behind a multi-channel aggregator they
/// *are* transient (a rotated upstream key, a channel in failover), and retrying
/// them is what makes such a gateway usable. That behaviour needs an explicit
/// opt-in — `ProviderProfile::aggregator` in `kc-core` — which does not exist
/// yet, so the conservative default is used: do not retry them. See
/// [`retryable_with`] for the opt-in hook.
///
/// Transport failures ([`Error::Timeout`], a connection reset, a body that dies
/// mid-decode) are always retried: nothing about them says the request was bad.
/// Two things are never retried: a cancellation (the user asked to stop) and a
/// configuration error (an empty `base_url`, a missing API key, a header value
/// that cannot be sent) — no attempt count fixes those, and the request never
/// left the process.
pub fn retryable(err: &Error) -> bool {
    retryable_with(err, false)
}

/// [`retryable`] with the aggregator opt-in made explicit.
///
/// `aggregator = true` additionally retries 401/403/404, which is correct only
/// for a multi-channel gateway where the next attempt lands on a different
/// upstream channel. It is not reachable from config yet: `ProviderProfile` has
/// no `aggregator` field, and inventing one here would be a guess about the
/// operator's topology.
pub fn retryable_with(err: &Error, aggregator: bool) -> bool {
    match err {
        Error::Cancelled | Error::Config(_) => false,
        // Status takes precedence over free-text wording: a 429/5xx proxy or
        // overloaded upstream can mention context limits without saying that
        // this request is permanently too large.
        Error::Provider(message)
            if errors::http_status_of(message)
                .is_some_and(|status| status_retryable(status, aggregator)) =>
        {
            true
        }
        // The provider tagged this as "another attempt cannot help" (a 200
        // carrying a buffered error body or nothing at all, a rejected request).
        // The status table cannot see those, because the status is 200.
        Error::Provider(message) if errors::is_permanent(message) => false,
        // With no transient status to override it, context-overflow wording is
        // terminal until the runtime compacts the conversation. This guard also
        // covers remote errors that carry no HTTP status tag.
        Error::Provider(message) if errors::is_context_overflow(message) => false,
        // A refusal that names a condition clearing by itself is transient
        // whatever status carried it. Gateways put rate limits inside 400/402
        // ("tier_limit_exceeded", "no provider's ask matches your max-per-mtok
        // bid (reset after 2m)"), where the status table below reads "the
        // request is wrong" and ends the run. The two guards above still win:
        // an explicit permanent verdict, and a request that is too large for the
        // window, are judgements about *this* request.
        Error::Provider(message) if errors::is_transient_refusal(message) => true,
        Error::Provider(message) => match errors::http_status_of(message) {
            Some(status) => status_retryable(status, aggregator),
            // No status: a transport/decode failure, a splash page, a stream
            // that died mid-frame. Always worth another attempt.
            None => true,
        },
        // Timeouts, and every other error class that reaches this layer, are
        // transport-shaped.
        _ => true,
    }
}

/// The status half of [`retryable`], split out so the table is testable without
/// building error messages.
pub fn status_retryable(status: u16, aggregator: bool) -> bool {
    if kc_core::error::status_is_transient(status) {
        return true;
    }
    match status {
        // Permanent for this endpoint unless it is a multi-channel gateway.
        401 | 403 | 404 => aggregator,
        // 400/413/422 and every other 4xx: the request itself was refused.
        _ => false,
    }
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
            started: Instant::now(),
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
    /// When the first attempt started; the wall-clock retry budget is measured
    /// from here, so a run cannot spend 20 minutes retrying even when the
    /// attempt count would still allow it.
    started: Instant,
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
                if let Some(admission) = &self.request.admission {
                    if let Err(err) = admission.admit().await {
                        self.done = true;
                        return Some(Err(err));
                    }
                }
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
                Ok(ProviderChunk::Error {
                    message,
                    retryable: stream_retryable,
                }) if !self.content_seen => {
                    // The stream layer classified this failure itself: a cut
                    // body or a splash page is worth another attempt, a rejected
                    // request (`retryable: false`, e.g. an in-stream
                    // `invalid_request_error`) is not. Honour that flag instead
                    // of re-deriving it from the message. (After content, the
                    // chunk is forwarded unchanged below.)
                    if !stream_retryable {
                        self.done = true;
                        // Forwarded as a chunk rather than raised as an `Err`:
                        // `Error::Provider` is transport-shaped to the runtime,
                        // which would restart the whole stream
                        // `retry_max_stream_restarts` times on a failure the
                        // provider has already judged permanent. The chunk
                        // carries the verdict (`retryable: false`) through to
                        // the runtime, which then fails the turn with the real
                        // reason - and without re-billing a generation per
                        // restart.
                        return Some(Ok(ProviderChunk::Error {
                            message,
                            retryable: false,
                        }));
                    }
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

        if !retryable_with(&err, self.policy.aggregator())
            || self.content_seen
            || self.attempt >= self.policy.max_attempts()
        {
            self.done = true;
            return Some(Err(self.exhausted(err)));
        }

        let delay = self.policy.delay(self.attempt.saturating_sub(1));
        // The provider may have named its own window ("reset after 2m",
        // "reset_seconds": 1024). A cooldown is not a backoff: honouring the hint
        // is the difference between a run that continues after the window and one
        // that spends the whole budget crawling a 30 s exponential towards a
        // reset it can never reach.
        let hint = errors::retry_after_hint(&err.to_string());
        let delay = match hint {
            Some(hint) if hint > delay => hint,
            _ => delay,
        };
        // Wall-clock budget: the attempt count alone would allow ~20 minutes of
        // retrying against a 30 s backoff ceiling. Stop before sleeping past the
        // budget rather than after, so the user sees the real error promptly.
        let elapsed = self.started.elapsed();
        let remaining = self.policy.max_total().saturating_sub(elapsed);
        if delay > remaining {
            self.done = true;
            // Name the window when the *provider* is what makes the wait
            // impossible: "rate limited for another 17 minutes" is actionable
            // (switch model, lower concurrency), "gave up after 18 attempts" is
            // not.
            let note = hint.filter(|hint| *hint > remaining).map(|hint| {
                format!(
                    " (the provider asked for a {}s wait, longer than the {}s retry budget left; \
                     a different model, or fewer parallel requests, would not be waiting)",
                    hint.as_secs(),
                    remaining.as_secs()
                )
            });
            tracing::warn!(
                provider = self.inner.id(),
                attempt = self.attempt,
                elapsed_secs = elapsed.as_secs(),
                budget_secs = self.policy.max_total().as_secs(),
                delay_secs = delay.as_secs(),
                error = %err,
                "provider retry budget exhausted; surfacing the failure"
            );
            return Some(Err(self.exhausted_with(err, note.as_deref())));
        }

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
    ///
    /// An error the provider already classified as permanent is returned as it
    /// is: it is surfaced after one attempt, so there is no count to report, and
    /// a suffix would only bury the reason the operator has to act on.
    fn exhausted(&self, err: Error) -> Error {
        self.exhausted_with(err, None)
    }

    /// [`Self::exhausted`] plus an operator-facing note explaining *why* the
    /// retries stopped (today: the provider's own reset window did not fit the
    /// budget).
    fn exhausted_with(&self, err: Error, note: Option<&str>) -> Error {
        let attempts = self.attempt.max(1);
        let mut suffix = String::new();
        if attempts > 1 && !errors::is_permanent(&err.to_string()) {
            suffix.push_str(&format!(" (gave up after {attempts} attempts)"));
        }
        if let Some(note) = note {
            suffix.push_str(note);
        }
        if suffix.is_empty() {
            return err;
        }
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

    /// The status table, asserted directly so a future edit cannot quietly make
    /// a permanent failure retryable again.
    #[test]
    fn status_classification_table() {
        // Transient: the endpoint is busy or the upstream is failing.
        for status in [408, 409, 425, 429, 500, 502, 503, 504, 599] {
            assert!(status_retryable(status, false), "{status} must retry");
        }
        // Permanent: the request itself was refused. Re-sending it 51 times
        // only delays the real error and bills 51 attempts.
        for status in [400, 413, 422, 405, 415, 451] {
            assert!(
                !status_retryable(status, false),
                "{status} must NOT retry by default"
            );
        }
        // 401/403/404 are permanent for one endpoint, transient behind a
        // multi-channel aggregator (opt-in only).
        for status in [401, 403, 404] {
            assert!(
                !status_retryable(status, false),
                "{status} must NOT retry by default"
            );
            assert!(
                status_retryable(status, true),
                "{status} must retry for an aggregator"
            );
        }
    }

    /// A 400 must not be retried: the payload is malformed and stays malformed.
    #[test]
    fn bad_request_is_not_retried() {
        let err = Error::Provider(
            "provider rejected the request (400 Bad Request) at http://h/v1: \
             unsupported field `foo` [http 400]"
                .to_string(),
        );
        assert!(!retryable(&err), "{err}");
    }

    /// A 401 must not be retried by default: a revoked key is not fixed by
    /// re-sending the request 51 times.
    #[test]
    fn unauthorized_is_not_retried_by_default() {
        let err = Error::Provider(
            "provider authentication failed (401 Unauthorized) at http://h/v1 [http 401]".into(),
        );
        assert!(!retryable(&err), "{err}");
        // ... but an aggregator may opt in, where the next attempt is a
        // different upstream channel.
        assert!(retryable_with(&err, true));
    }

    #[test]
    fn rate_limit_and_server_errors_are_retried() {
        let limited = Error::Provider("provider rate limit hit (429) [http 429]".into());
        assert!(retryable(&limited), "{limited}");
        let down = Error::Provider("upstream provider error (502) [http 502]".into());
        assert!(retryable(&down), "{down}");
    }

    /// A transport failure carries no status and is always worth another try.
    #[test]
    fn transport_failures_are_retried() {
        assert!(retryable(&Error::Provider(
            "invalid response body from http://h/v1: error decoding response body".into()
        )));
        assert!(retryable(&Error::Timeout("no headers in 300s".into())));
    }

    /// A context overflow is not fixed by re-sending: the runtime has to
    /// compact first, so the retry layer must surface it immediately.
    #[test]
    fn context_overflow_is_not_retried() {
        let err = Error::Provider(format!(
            "provider rejected the request (400) at http://h/v1: \
             maximum context length is 32768 tokens [http 400] {}",
            errors::CONTEXT_OVERFLOW_TAG
        ));
        assert!(!retryable(&err), "{err}");
    }

    /// The status tag must survive the message formatting and be readable back.
    #[test]
    fn status_tag_round_trips() {
        let message = "provider rejected the request (400) at http://h/v1 [http 400]";
        assert_eq!(errors::http_status_of(message), Some(400));
        assert_eq!(errors::http_status_of("no status here"), None);
        // A status quoted from an upstream body is not a tag.
        assert_eq!(
            errors::http_status_of("upstream said 400 Bad Request"),
            None
        );
    }

    #[test]
    fn wall_clock_budget_defaults_to_five_minutes() {
        assert_eq!(RetryPolicy::default().max_total(), Duration::from_secs(300));
        assert_eq!(
            RetryPolicy::from_config(&ModelConfig::default()).max_total(),
            Duration::from_secs(300)
        );
        assert_eq!(
            RetryPolicy::none()
                .with_max_total(Duration::from_secs(1))
                .max_total(),
            Duration::from_secs(1)
        );
    }

    /// A provider that fails every attempt, counting them.
    ///
    /// The point of the test below is the *sleep* between attempts, so the
    /// failure has to be the cheap, always-retryable kind.
    #[derive(Default)]
    struct AlwaysFails {
        attempts: std::sync::atomic::AtomicU32,
    }

    #[async_trait]
    impl Provider for AlwaysFails {
        fn id(&self) -> &str {
            "always-fails"
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
            _cancel: CancellationToken,
        ) -> Result<ProviderStream> {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(Error::Provider("upstream broke".into()))
        }
    }

    /// A stop that lands inside the retry backoff must not wait the backoff out.
    ///
    /// The default policy's first delay is half to full of `retry_base_delay_ms`,
    /// which an operator can set to thirty seconds. Without the cancel arm on the
    /// sleep, pressing stop during a retry would leave the run (and the session)
    /// waiting for a timer nobody is interested in any more.
    #[tokio::test]
    async fn cancelling_during_a_retry_sleep_returns_at_once() {
        let inner = Arc::new(AlwaysFails::default());
        let provider = RetryingProvider::new(
            Arc::clone(&inner) as Arc<dyn Provider>,
            RetryPolicy::new(50, Duration::from_secs(30), Duration::from_secs(30)),
        );
        let cancel = CancellationToken::new();
        let request = ChatRequest {
            admission: None,
            model: "test-model".into(),
            messages: Vec::new(),
            tools: Vec::new(),
            temperature: 0.0,
            send_temperature: true,
            max_tokens: None,
            top_p: None,
            parallel_tool_calls: false,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
        };
        let mut stream = provider
            .stream_chat(request, cancel.clone())
            .await
            .expect("the retry layer opens a stream immediately");

        let started = tokio::time::Instant::now();
        let read = tokio::spawn(async move { stream.next().await });
        // Let the first attempt fail and the backoff start before stopping.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let item = tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .expect("the stop must not wait out a 15-30s backoff")
            .expect("read task");
        let elapsed = started.elapsed();

        assert!(matches!(item, Some(Err(Error::Cancelled))), "{item:?}");
        assert!(
            elapsed < Duration::from_secs(2),
            "a cancelled retry returned after {elapsed:?}"
        );
        assert_eq!(
            inner.attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a cancelled retry must not start another attempt"
        );
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
            cached_tokens: None,
        }));
        assert!(!is_content(&ProviderChunk::Finish {
            reason: "stop".into()
        }));
    }
}
