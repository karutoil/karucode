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

/// Would another attempt at the *identical* request plausibly succeed?
///
/// Only the transient half of `kc_provider::retry::status_retryable` matters
/// here, and it is duplicated rather than shared because `kc-core` cannot
/// depend on `kc-provider`. The two are kept in step by the provider's own
/// `transient_status_is_not_reclassified_by_context_wording` test and by
/// `a_transient_status_is_never_an_overflow` here.
fn status_is_transient(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || (500..=599).contains(&status)
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
}
