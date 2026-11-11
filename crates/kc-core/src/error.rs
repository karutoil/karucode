use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("operation cancelled")]
    Cancelled,

    #[error("timed out: {0}")]
    Timeout(String),

    #[error("path {path} escapes workspace root {root}")]
    PathEscape { path: PathBuf, root: PathBuf },

    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("storage error: {0}")]
    Store(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Stable machine-readable code used by the HTTP layer and the UI.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::BadRequest(_) => "bad_request",
            Error::NotFound(_) => "not_found",
            Error::PermissionDenied(_) => "permission_denied",
            Error::Unauthorized => "unauthorized",
            Error::Cancelled => "cancelled",
            Error::Timeout(_) => "timeout",
            Error::PathEscape { .. } => "path_escape",
            Error::Provider(_) => "provider",
            Error::Tool(_) => "tool",
            Error::Store(_) => "store",
            Error::Plugin(_) => "plugin",
            Error::Io { .. } => "io",
            Error::Internal(_) => "internal",
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(format!("json: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Context-overflow classification
// ---------------------------------------------------------------------------
//
// One definition of "the prompt did not fit", shared by the provider layer
// (which tags the failure) and the runtime (which recovers from it).
//
// It lives in `kc-core` because the runtime deliberately does not depend on
// `kc-provider`: the runtime is the only layer that owns both the request and
// the transcript, so it is the only layer that can compact and retry, and it
// must be able to read the classification without pulling in an HTTP client.
//
// `kc_provider::errors::is_context_overflow` is a thin delegate to
// [`is_context_overflow`] here, so there is exactly one copy of the phrases.

/// Stable tag marking a failure as "the prompt did not fit the context window".
///
/// [`Error`] has no dedicated variant for this, so the classification travels in
/// the message: a provider appends the tag when it knows the status was not
/// transient, and the runtime matches it to compact and retry once instead of
/// failing the turn.
pub const CONTEXT_OVERFLOW_TAG: &str = "[context-overflow]";

/// Prefix of the machine-readable tag carrying the HTTP status inside a provider
/// error message (`[http 400]`).
///
/// A provider error is a plain string and the retry/overflow classifiers only
/// ever see that string, so the status has to travel inside it. The tag is a
/// bracketed suffix so it cannot be confused with a status quoted from an
/// upstream body.
pub const HTTP_STATUS_TAG: &str = "[http ";

/// Phrases providers use for a prompt that overflows the context window.
///
/// Covers OpenAI (`maximum context length`, `context_length_exceeded`),
/// Anthropic (`prompt is too long`), vLLM/TGI (`too many tokens`), Gemini
/// (`reduce the length`), the generic gateway wording, and the item-count
/// variant a gateway in front of the model uses when it caps the number of
/// *items* rather than tokens (`conversation history exceeds the limit on input
/// items (1002 > 1000 items)`).
const CONTEXT_OVERFLOW_MARKERS: [&str; 9] = [
    "context length",
    "maximum context",
    "too many tokens",
    "context_length_exceeded",
    "reduce the length",
    "prompt is too long",
    // A gateway that caps the request by *item count* says nothing about
    // tokens: the shortest session can carry 1,000 messages while staying far
    // under any character budget. Observed verbatim from this machine's own
    // gateway as `Input is too long: conversation history exceeds the limit on
    // input items (1002 > 1000 items).`
    "input items",
    "exceeds the limit on input items",
    "conversation history exceeds",
];

/// The HTTP status carried by a provider error message, when it has one.
pub fn http_status_of(message: &str) -> Option<u16> {
    let start = message.find(HTTP_STATUS_TAG)? + HTTP_STATUS_TAG.len();
    let rest = &message[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
}

/// Does this message describe a prompt that overflowed the model's window?
///
/// Deliberately a substring test over the lowercased text: providers word this
/// differently and a gateway may wrap the upstream message in its own prose.
///
/// A **transient HTTP status is authoritative**: a 429 or a 5xx that happens to
/// quote `context length` means "this endpoint is busy", not "this request is
/// permanently too large", and compacting a conversation because of a rate limit
/// would throw away context for no reason. Only a status-less message or a
/// non-transient one (400/413/422) can be classified by its wording.
pub fn is_context_overflow(message: &str) -> bool {
    if http_status_of(message).is_some_and(status_is_transient) {
        return false;
    }
    if message.contains(CONTEXT_OVERFLOW_TAG) {
        return true;
    }
    let lower = message.to_ascii_lowercase();
    CONTEXT_OVERFLOW_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// [`is_context_overflow`] for a whole error value.
pub fn is_context_overflow_error(err: &Error) -> bool {
    match err {
        Error::Provider(message) => is_context_overflow(message),
        _ => false,
    }
}

/// Is this HTTP status one where the endpoint, not the request, is the problem?
///
/// The transient half of the provider's retry table: the endpoint is busy, the
/// request raced, or the upstream is failing. Kept here so the provider retry
/// layer and the runtime's turn-retry layer classify identically.
pub fn status_is_transient(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || (500..=599).contains(&status)
}

/// Words a gateway uses for a condition that clears by itself.
///
/// Deliberately narrow, and deliberately **not** containing quota or balance
/// wording (`insufficient_quota`, `exceeded your current quota`, `余额`): those
/// need the operator to recharge, and every retry is a billed refusal. What *is*
/// here is the opposite: rate limits, cooldowns and "the upstream is busy",
/// which another attempt (after the right wait) can satisfy.
const TRANSIENT_MARKERS: &[&str] = &[
    "rate limit",
    "rate_limit",
    "rate-limit",
    "ratelimit",
    "too many requests",
    "tier_limit_exceeded",
    "rpm limit",
    "concurrency limit",
    "cooling down",
    "cooldown",
    "temporarily unavailable",
    "temporarily busy",
    "service unavailable",
    "upstream is busy",
    "overloaded",
    "try again later",
    "please retry",
    "retry after",
    "reset after",
    "reset_seconds",
    "will reset",
];

/// Does this refusal name a condition that clears by itself?
///
/// The retry table classifies by HTTP status, which is right almost always and
/// wrong exactly where gateways put a rate limit inside a status that means
/// "your request is wrong". Three shapes from one install's own database:
///
/// ```text
/// [400] {"code":"bad_request",
///        "message":"[code=tier_limit_exceeded] membership rpm limit exceeded:
///                   101/100 resets 2026-11-09T22:00:00Z"}
/// [402] no provider's ask matches your max-per-mtok bid (reset after 2m)
/// [403] the API key has no subscription quota or balance available
///       for this request ... (reset after 3s)
/// ```
///
/// [`kc_provider::is_transient_refusal`] is a delegate to this, so the provider
/// retry layer and the runtime agree about what "transient" means.
pub fn is_transient_refusal(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    TRANSIENT_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
}

/// Could another attempt at this request, *later*, plausibly succeed?
///
/// This is the question the runtime's turn-retry layer asks: the provider layer
/// has already spent its own attempt budget on the identical request, so the
/// only thing left that can change is time. Three ways to be "yes":
///
/// * the message carries a transient HTTP status (408/409/425/429/5xx);
/// * the body names a rate limit, cooldown or reset window, whatever the status
///   ([`is_transient_refusal`]);
/// * there is no status at all — a connection reset, a body that died mid-decode,
///   a splash page — which is transport-shaped by construction.
///
/// Two verdicts override all of that: an explicit `[permanent]` tag and a context
/// overflow. Both are statements about *this request*, which another attempt
/// repeats unchanged.
///
/// `[http 401]`, `[http 403]` and `[http 404]` are **not** transient here even on
/// a multi-channel gateway: that opt-in (`ProviderProfile::aggregator`) belongs
/// to the provider layer, which sees the profile. A run that ends on a dead key
/// should tell the operator, not spend three more generations finding out.
pub fn is_transient_provider_failure(message: &str) -> bool {
    if is_permanent_tagged(message) || is_context_overflow(message) {
        return false;
    }
    match http_status_of(message) {
        Some(status) => status_is_transient(status) || is_transient_refusal(message),
        None => true,
    }
}

/// Does the message carry the provider's "another attempt cannot help" tag?
/// Mirrors `kc_provider::errors::is_permanent`, which owns the constant.
fn is_permanent_tagged(message: &str) -> bool {
    message.contains("[permanent]")
}

/// [`is_transient_provider_failure`] for a whole error value.
///
/// The variant matters as much as the wording, and more than the absence of an
/// HTTP status: only a provider or transport failure is something a wait can fix.
/// A [`Error::Store`] refusal (a mission's attempt budget is spent), a bad request,
/// a configuration error and a cancellation are local verdicts on *this* run, and
/// retrying them would spend a generation to be told the same thing.
pub fn is_transient_provider_failure_error(err: &Error) -> bool {
    match err {
        Error::Provider(message) => is_transient_provider_failure(message),
        // A timeout is transport-shaped by definition, even though its message
        // carries no status tag.
        Error::Timeout(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure that started this: a real gateway refusing a real session.
    /// The message is reproduced verbatim, including the `[400]` the gateway
    /// writes (which is *not* the `[http 400]` tag this crate parses, so the
    /// wording is what has to carry the classification).
    const OBSERVED: &str = "provider stream error: provider error: [400]: [code=context_length_exceeded]\n\
        Input is too long: conversation history exceeds the limit on input items (1002 > 1000 items).\n\
        Compact the conversation and retry. If compaction also exceeds the limit, shorten the history or\n\
        start a new session.";

    #[test]
    fn the_observed_gateway_failure_is_an_overflow() {
        assert!(is_context_overflow(OBSERVED));
        assert!(is_context_overflow_error(&Error::Provider(OBSERVED.into())));
    }

    /// An item-count ceiling with no token wording at all. This is the shape
    /// that must be recognised by the phrases alone.
    #[test]
    fn item_count_wording_is_recognised_without_any_token_phrase() {
        for message in [
            "Input is too long: conversation history exceeds the limit on input items (1002 > 1000 items)",
            "the request has too many input items",
            "conversation history exceeds the limit",
        ] {
            assert!(is_context_overflow(message), "not detected: {message}");
        }
    }

    /// Every provider's own wording, because the runtime's recovery hangs off
    /// this classification.
    #[test]
    fn real_provider_overflows_are_recognised() {
        for body in [
            // OpenAI
            "This model's maximum context length is 128000 tokens. However, your \
             messages resulted in 200000 tokens. Please reduce the length of the messages.",
            // OpenAI error code form
            "{\"error\":{\"message\":\"...\",\"type\":\"invalid_request_error\",\
             \"code\":\"context_length_exceeded\"}}",
            // Anthropic
            "prompt is too long: 210000 tokens > 200000 maximum",
            // vLLM / TGI
            "This model's maximum context length is 32768 tokens. \
             However, you requested 40000 tokens (36000 in the messages, 4000 in the completion). \
             Please reduce the length of the messages or completion.",
            // Gemini
            "The input token count (1048577) exceeds the maximum number of tokens allowed (1048576). \
             Please reduce the length of the messages.",
            // Generic gateway wording
            "too many tokens in request",
            "maximum context window exceeded",
            // The explicit tag, on its own
            CONTEXT_OVERFLOW_TAG,
        ] {
            assert!(is_context_overflow(body), "not detected: {body}");
        }
    }

    #[test]
    fn ordinary_errors_are_not_overflows() {
        for body in [
            "provider authentication failed (401 Unauthorized)",
            "provider rate limit hit (429 Too Many Requests)",
            "upstream provider error (502 Bad Gateway)",
            "invalid response body from http://h/v1: error decoding response body",
            "model `gpt-nope` does not exist",
            "unsupported parameter: max_tokens",
        ] {
            assert!(!is_context_overflow(body), "false positive: {body}");
        }
        assert!(!is_context_overflow_error(&Error::Timeout("slow".into())));
        assert!(!is_context_overflow_error(&Error::Cancelled));
    }

    /// A busy endpoint that quotes a length message stays a transient failure:
    /// the status wins, so the runtime never compacts for a rate limit.
    #[test]
    fn a_transient_status_is_never_an_overflow() {
        for status in [408, 409, 425, 429, 500, 503, 599] {
            let message = format!("upstream busy: too many tokens, context length [http {status}]");
            assert!(!is_context_overflow(&message), "misclassified {status}");
        }
        // A non-transient status leaves the wording in charge.
        assert!(is_context_overflow(
            "provider rejected: too many tokens [http 400]"
        ));
        assert!(is_context_overflow(
            "provider rejected: context_length_exceeded [http 413]"
        ));
    }

    #[test]
    fn status_tag_is_parsed_out_of_a_message() {
        assert_eq!(http_status_of("boom [http 400]"), Some(400));
        assert_eq!(http_status_of("boom [http 503] tail"), Some(503));
        assert_eq!(http_status_of("boom"), None);
        assert_eq!(http_status_of("[http not-a-number]"), None);
        // The gateway's own `[400]:` is not this tag: it carries no `http`.
        assert_eq!(http_status_of("provider error: [400]: too long"), None);
    }

    /// The question the runtime's slow turn-retry layer asks: is this a failure
    /// another attempt, later, could survive?
    #[test]
    fn a_failure_a_wait_can_fix_is_recoverable() {
        for message in [
            // Transport-shaped: no status, nothing said about the request.
            "provider request failed: error sending request for url (https://gw/v1)",
            "provider stream failed: invalid response body: error decoding response body",
            // A transient status.
            "upstream provider error (503 Service Unavailable) [http 503] \
             (gave up after 50 attempts)",
            "provider rate limit hit (429 Too Many Requests) [http 429]",
            // A rate limit wearing a permanent status.
            "provider rejected the request (400): [code=tier_limit_exceeded] rpm limit exceeded \
             [http 400]",
            "provider rejected the request (402): no provider's ask matches your max-per-mtok \
             bid (reset after 2m) [http 402]",
        ] {
            assert!(
                is_transient_provider_failure(message),
                "must be recoverable: {message}"
            );
        }
    }

    #[test]
    fn a_judgement_on_the_request_is_not_recoverable() {
        for message in [
            // A dead key is the operator's job, not a wait: retrying it spends
            // generations to learn nothing.
            "provider request failed: authentication failed (401 Unauthorized) [http 401]",
            "provider access denied (403 Forbidden) [http 403]",
            "provider access denied (403 Forbidden): content safety policy [http 403]",
            // A per-key spend limit that does not reset.
            "provider access denied (403): Key limit exceeded (total limit) $10.51 of $10.00 \
             used; does not reset [http 403]",
            // The provider's own verdict, and a request that is too large.
            "provider rejected the request [http 400] [permanent]",
            "provider rejected the request: context_length_exceeded [http 400]",
            // Ordinary 4xx from a direct API.
            "provider rejected the request (422 Unprocessable Entity) [http 422]",
        ] {
            assert!(
                !is_transient_provider_failure(message),
                "must not be recoverable: {message}"
            );
        }
    }
}
