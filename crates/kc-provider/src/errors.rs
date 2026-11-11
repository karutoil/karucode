//! Error mapping for provider HTTP failures.
//!
//! Rules: be actionable, include the URL and status, include a short excerpt of
//! the upstream body, and never leak the API key or any auth header value.

use crate::Error;
use reqwest::StatusCode;
use std::time::Duration;

/// Bytes of an error response body we are willing to read.
const ERROR_BODY_LIMIT: usize = 4096;
/// Characters of the body excerpt kept in the message.
const ERROR_EXCERPT_CHARS: usize = 480;

/// Machine-readable tag carrying the HTTP status inside a provider error
/// message.
///
/// [`Error::Provider`] is a plain string and the retry layer only ever sees that
/// string, so the status has to travel inside it for [`crate::retry::retryable`]
/// to tell a permanent 400 from a transient 502. The tag is a *suffix* so the
/// human-readable head stays first, and it is bracketed so it cannot be confused
/// with a status code quoted from an upstream body.
pub(crate) const HTTP_STATUS_TAG: &str = kc_core::error::HTTP_STATUS_TAG;

/// Stable tag marking a failure as "the prompt did not fit the context window".
///
/// The definition, and the whole phrase list, live in
/// [`kc_core::error::is_context_overflow`]: `kc-core` has no dependency on an
/// HTTP client, and the runtime (which does the compact-and-retry) depends only
/// on `kc-core`. This is the re-export the provider layer and its callers use.
pub use kc_core::error::CONTEXT_OVERFLOW_TAG;

/// The HTTP status carried by a provider error message, when it has one.
///
/// Delegates to [`kc_core::error::http_status_of`] so the tag is parsed in
/// exactly one place.
pub(crate) fn http_status_of(message: &str) -> Option<u16> {
    kc_core::error::http_status_of(message)
}

/// Does this message describe a prompt that overflowed the model's window?
///
/// A thin delegate to [`kc_core::error::is_context_overflow`], which owns the
/// phrase list and the authoritative-transient-status rule. The runtime calls
/// the `kc-core` function directly (it does not depend on this crate); this
/// re-export is what the provider's own tagging path and its callers use.
pub fn is_context_overflow(message: &str) -> bool {
    kc_core::error::is_context_overflow(message)
}

/// [`is_context_overflow`] for a whole error value.
pub fn is_context_overflow_error(err: &Error) -> bool {
    kc_core::error::is_context_overflow_error(err)
}

/// Does this refusal name a condition that clears by itself?
///
/// A thin delegate to [`kc_core::error::is_transient_refusal`], which owns the
/// phrase list: the runtime's turn-retry layer needs the same answer and
/// `kc-runtime` does not depend on this crate. The shapes that motivated it -
/// a membership RPM limit in a `400`, a bid floor in a `402`, a per-channel
/// cooldown in a `403` - are documented there.
///
/// A caller must still honour an explicit [`is_permanent`] verdict and a
/// [`is_context_overflow`]: those are judgements about *this* request, not about
/// the upstream's mood.
pub fn is_transient_refusal(message: &str) -> bool {
    kc_core::error::is_transient_refusal(message)
}

/// The provider's own "try again in N" hint, when its message carries one.
///
/// A cooldown is not a backoff. Two 429s in this install's database say
/// `"reset_seconds": 1024` and `13259`; a 500 ms→30 s exponential inside a 300 s
/// budget cannot reach either, so the old behaviour was 17–18 billed attempts,
/// five minutes of waiting, and the same failure. With the hint:
///
/// * it fits the remaining budget → wait it out, and the run continues (this is
///   what makes a two-minute bid floor survivable);
/// * it does not → stop at once and say why, instead of pretending to retry.
///
/// Understands `reset_seconds` (the field several gateways emit next to
/// `retry_after`) and prose: `reset after 2m`, `retry after 30s`,
/// `try again in 90`.
pub fn retry_after_hint(message: &str) -> Option<Duration> {
    // ASCII-only lowering keeps every byte offset valid for slicing, and the
    // keywords are ASCII even in a body that is otherwise CJK.
    let haystack = message.to_ascii_lowercase();
    for key in ["reset_seconds", "reset seconds"] {
        if let Some(rest) = value_after(&haystack, key) {
            if let Some(seconds) = leading_number(rest) {
                return Some(clamped_secs(seconds));
            }
        }
    }
    for key in ["reset after", "retry after", "retry in", "try again in"] {
        if let Some(rest) = value_after(&haystack, key) {
            if let Some(duration) = leading_duration(rest) {
                return Some(duration);
            }
        }
    }
    None
}

/// Text after `key`, with the punctuation a JSON field or a sentence puts
/// between a label and its value skipped.
fn value_after<'a>(haystack: &'a str, key: &str) -> Option<&'a str> {
    let start = haystack.find(key)? + key.len();
    let rest = haystack[start..].trim_start_matches(['"', '\'', ':', '=', ' ', '\t']);
    Some(rest)
}

/// A run of ASCII digits at the start of `text`, as seconds.
fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `90`, `30s`, `2m`, `1h` — the unit-less form is seconds, which is what every
/// gateway means by `retry_after: 30`.
fn leading_duration(text: &str) -> Option<Duration> {
    let seconds = leading_number(text)?;
    let unit = text
        .chars()
        .find(|c| !c.is_ascii_digit())
        .map(|c| c.to_ascii_lowercase());
    let seconds = match unit {
        Some('m') => seconds.saturating_mul(60),
        Some('h') => seconds.saturating_mul(3600),
        Some('d') => seconds.saturating_mul(86_400),
        _ => seconds,
    };
    Some(clamped_secs(seconds))
}

/// A hint is a hint, not a scheduler: anything past a day is treated as a day so
/// a nonsense value cannot park a run, and it is compared against a 300 s budget
/// anyway.
fn clamped_secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds.min(86_400))
}

/// Map a transport-level failure. Timeouts become [`Error::Timeout`]; everything
/// else becomes a provider error naming the URL (never the credentials).
pub(crate) fn map_reqwest_error(
    url: &str,
    err: &reqwest::Error,
    timeout: Duration,
    secrets: &[String],
) -> Error {
    let detail = err.to_string();
    let detail = redact(&detail, secrets);
    if err.is_redirect() {
        return permanent(Error::Provider(format!(
            "provider at {url} attempted a redirect: {detail}; set model.base_url to the intended endpoint instead"
        )));
    }
    if err.is_timeout() {
        return Error::Timeout(format!(
            "provider at {url} did not respond within {}s: {detail}",
            timeout.as_secs()
        ));
    }
    if err.is_connect() {
        return Error::Provider(format!(
            "could not connect to provider at {url}: {detail}; check model.base_url"
        ));
    }
    if err.is_body() || err.is_decode() {
        return Error::Provider(format!("invalid response body from {url}: {detail}"));
    }
    Error::Provider(format!("request to {url} failed: {detail}"))
}

/// Tag marking a failure that another attempt cannot fix.
///
/// [`retryable`](crate::retry::retryable) classifies by HTTP status, but some
/// permanent failures arrive with a **200**: an endpoint that answers
/// `stream: true` with a buffered JSON document (or a gateway that returns
/// `{"error":{"message":"insufficient_quota"}}` with a 200), and a 200 whose body
/// is empty. Those carry no status to classify, so the verdict travels in the
/// message like [`HTTP_STATUS_TAG`] does.
pub(crate) const PERMANENT_TAG: &str = "[permanent]";

/// Does this message say another attempt cannot help?
pub(crate) fn is_permanent(message: &str) -> bool {
    message.contains(PERMANENT_TAG)
}

/// Mark an error permanent, so the retry layer surfaces it instead of re-sending
/// the request up to 50 more times - each one a billed generation.
pub(crate) fn permanent(err: Error) -> Error {
    match err {
        Error::Provider(message) if !is_permanent(&message) => {
            Error::Provider(format!("{message} {PERMANENT_TAG}"))
        }
        other => other,
    }
}

/// Read at most [`ERROR_BODY_LIMIT`] bytes of an error body and format a message
/// for the given status.
pub(crate) async fn http_status_error(
    resp: reqwest::Response,
    url: &str,
    model: &str,
    secrets: &[String],
    listing_models: bool,
) -> Error {
    let status = resp.status();
    let excerpt = body_excerpt(resp).await;
    let excerpt = redact(&excerpt, secrets);
    let with_body = |head: String| -> String {
        if excerpt.is_empty() {
            head
        } else {
            format!("{head} (response body: {excerpt})")
        }
    };
    // Transient failures stay transient even when a gateway wraps them in
    // context-overflow wording. Only a non-transient HTTP response whose body
    // actually describes the request's size earns the compact-and-retry tag.
    let transient = status == StatusCode::TOO_MANY_REQUESTS
        || matches!(status.as_u16(), 408 | 409 | 425)
        || status.is_server_error();
    let overflow = !transient && is_context_overflow(&excerpt);
    let text = match status {
        StatusCode::UNAUTHORIZED => with_body(format!(
            "provider authentication failed (401 Unauthorized) at {url}: the configured API key was rejected; \
             set a valid model.api_key (KARUCODE_API_KEY) for this endpoint"
        )),
        StatusCode::FORBIDDEN => with_body(format!(
            "provider access denied (403 Forbidden) at {url}: the configured API key may lack access to model `{model}`"
        )),
        StatusCode::NOT_FOUND if listing_models => with_body(format!(
            "provider model listing unavailable (404 Not Found) at {url}: this endpoint does not expose /models; \
             set model.model explicitly and ignore listing"
        )),
        StatusCode::NOT_FOUND => with_body(format!(
            "provider endpoint not found (404 Not Found) at {url}: check model.base_url (an OpenAI-compatible root such as \
             https://host/v1) and that model `{model}` exists on this endpoint"
        )),
        StatusCode::TOO_MANY_REQUESTS => with_body(format!(
            "provider rate limit hit (429 Too Many Requests) at {url}: retry later, lower request concurrency, \
             or check the plan quota for model `{model}`"
        )),
        // These are HTTP responses, not local clock expiry. Keep them as Provider
        // errors so the status tag survives retry exhaustion and remains visible
        // to the context-overflow classifier (`Error::Timeout` could carry no
        // structured status at all).
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => with_body(format!(
            "provider did not answer in time ({status}) at {url}"
        )),
        s if s.is_server_error() => with_body(format!(
            "upstream provider error ({}) from {url}: the endpoint is failing; retry later",
            status_line(s)
        )),
        s if s.is_client_error() => with_body(format!(
            "provider rejected the request ({}) at {url}: the endpoint refused our payload \
             (model `{model}`, tools or sampling options may be unsupported)",
            status_line(s)
        )),
        s => with_body(format!("unexpected provider response ({}) from {url}", status_line(s))),
    };
    // The status travels inside the message so the retry layer can classify it
    // (see `HTTP_STATUS_TAG`); the overflow tag tells the runtime to compact and
    // retry once instead of failing the turn.
    let mut text = format!("{text} {HTTP_STATUS_TAG}{}]", status.as_u16());
    if overflow {
        text.push(' ');
        text.push_str(CONTEXT_OVERFLOW_TAG);
    }
    let error = Error::Provider(redact(&text, secrets));
    if status.is_redirection() {
        // Provider redirects are explicit configuration errors. Following or
        // retrying them can repeat a billed POST and can carry credentials to a
        // different endpoint; the operator must choose the new base URL.
        permanent(error)
    } else {
        error
    }
}

fn status_line(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_u16().to_string(),
    }
}

/// A `200` whose body is a JSON document rather than an event stream.
///
/// Two shapes reach this: an endpoint that ignored `stream: true` and buffered a
/// whole completion, and a gateway that reports a *rejection* with a 200 and an
/// `error` object in the body. Both are permanent - re-sending the identical
/// request produces the identical answer - and in the second case the body is
/// the only place the real reason appears (`insufficient_quota`, a model that
/// does not exist). The body is read and its reason surfaced, redacted as
/// everywhere else.
///
/// A non-JSON body (an HTML or plain-text splash page) never reaches this: that
/// is a dying channel, which the retry layer is supposed to try again.
pub(crate) async fn non_stream_success_error(
    resp: reqwest::Response,
    url: &str,
    model: &str,
    content_type: &str,
    secrets: &[String],
) -> Error {
    let status = resp.status();
    let excerpt = body_excerpt(resp).await;
    let excerpt = redact(&excerpt, secrets);
    // The reason, when the body names one, belongs at the front of the message:
    // "insufficient_quota" is what the operator has to act on, not the fact that
    // a proxy buffered the stream.
    let reason = json_error_message(&excerpt);
    let head = format!(
        "provider at {url} answered `{content_type}` instead of an event stream: \
         the endpoint ignored `stream: true` (a buffering proxy or an incompatible API?)"
    );
    let mut text = match reason {
        Some(reason) => format!("{head} (upstream said: {reason})"),
        None => format!("{head}, and model `{model}` was not served"),
    };
    if !excerpt.is_empty() && !text.contains(&excerpt) {
        text.push_str(&format!(" (response body: {excerpt})"));
    }
    Error::Provider(format!(
        "{text} {HTTP_STATUS_TAG}{}] {PERMANENT_TAG}",
        status.as_u16()
    ))
}

/// The `error.message` (or bare `message`) of a JSON body, if it has one.
fn json_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .or_else(|| value.get("message"))
        .and_then(|message| message.as_str())
        .map(str::trim)
        .filter(|message| !message.is_empty())?;
    Some(truncate_chars(message, ERROR_EXCERPT_CHARS))
}

async fn body_excerpt(mut resp: reqwest::Response) -> String {
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < ERROR_BODY_LIMIT {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = ERROR_BODY_LIMIT - buf.len();
                let take = room.min(chunk.len());
                buf.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) => break,
            // A broken error body must not mask the status mapping.
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&single_line, ERROR_EXCERPT_CHARS)
}

/// Replace every occurrence of a secret with `[redacted]`. Full header values
/// and the credential component of a Bearer value are both removed. HTTP auth
/// schemes are case-insensitive, so an upstream echo of `bEaReR token` must not
/// evade redaction merely because configuration stored `Bearer token`.
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.is_empty() {
            continue;
        }
        out = replace_ascii_case_insensitive(&out, secret);
        if secret
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer "))
        {
            let token = secret[7..].trim();
            if !token.is_empty() {
                out = replace_ascii_case_insensitive(&out, token);
            }
        }
    }
    out
}

fn replace_ascii_case_insensitive(text: &str, needle: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    // ASCII case folding does not change byte lengths or UTF-8 boundaries, so
    // offsets in `lowered` are also valid slices of the original string.
    let lowered = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(relative) = lowered[from..].find(&needle) {
        let at = from + relative;
        out.push_str(&text[from..at]);
        out.push_str("[redacted]");
        from = at + needle.len();
    }
    out.push_str(&text[from..]);
    out
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_covers_raw_key_and_bearer_form() {
        let secrets = vec!["Bearer sk-secret".to_string()];
        let out = redact("bad key sk-secret in header Bearer sk-secret", &secrets);
        assert!(!out.contains("sk-secret"), "{out}");
        assert_eq!(out.matches("[redacted]").count(), 2);
    }

    #[test]
    fn bearer_redaction_is_case_insensitive_for_scheme_and_component() {
        let secrets = vec!["Bearer Sk-SeNsItIvE".to_string()];
        let out = redact(
            "echoed bEaReR Sk-SeNsItIvE then bare sK-sEnSiTiVe",
            &secrets,
        );
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains("sk-sensitive"), "{out}");
        assert!(!lower.contains("bearer"), "{out}");
        assert_eq!(out.matches("[redacted]").count(), 2, "{out}");

        let lower_scheme = vec!["bEaReR other-secret".to_string()];
        let out = redact("BEARER other-secret / OTHER-SECRET", &lower_scheme);
        assert!(!out.to_ascii_lowercase().contains("other-secret"), "{out}");
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate_chars("héllo", 3), "hél…");
        assert_eq!(truncate_chars("ok", 5), "ok");
    }

    /// Real provider bodies for a prompt that does not fit the window. Every one
    /// of these must be recognised, because the runtime's recovery (compact and
    /// retry once) hangs off this classification.
    #[test]
    fn context_overflow_is_detected_in_real_provider_bodies() {
        let bodies = [
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
        ];
        for body in bodies {
            assert!(is_context_overflow(body), "not detected: {body}");
        }
    }

    /// Ordinary failures must not be mistaken for an overflow, or the runtime
    /// would compact a conversation that was never too long.
    #[test]
    fn ordinary_errors_are_not_context_overflows() {
        let bodies = [
            "provider authentication failed (401 Unauthorized)",
            "provider rate limit hit (429 Too Many Requests)",
            "upstream provider error (502 Bad Gateway)",
            "invalid response body from http://h/v1: error decoding response body",
            "model `gpt-nope` does not exist",
            "unsupported parameter: max_tokens",
        ];
        for body in bodies {
            assert!(!is_context_overflow(body), "false positive: {body}");
        }
    }

    #[test]
    fn transient_status_is_not_reclassified_by_context_wording() {
        for status in [408, 409, 425, 429, 500, 503, 599] {
            let message = format!("upstream busy: too many tokens, context length [http {status}]");
            assert!(!is_context_overflow(&message), "misclassified {status}");
        }
        assert!(is_context_overflow(
            "provider rejected: too many tokens [http 400]"
        ));
        assert!(is_context_overflow(
            "provider rejected: context_length_exceeded [http 413]"
        ));
    }

    #[test]
    fn overflow_tag_is_recognised_on_its_own() {
        assert!(is_context_overflow(CONTEXT_OVERFLOW_TAG));
        assert!(is_context_overflow_error(&Error::Provider(format!(
            "provider rejected the request (400) {CONTEXT_OVERFLOW_TAG}"
        ))));
        assert!(!is_context_overflow_error(&Error::Timeout("slow".into())));
        assert!(!is_context_overflow_error(&Error::Cancelled));
    }

    #[test]
    fn status_tag_is_parsed_out_of_a_message() {
        assert_eq!(http_status_of("boom [http 400]"), Some(400));
        assert_eq!(http_status_of("boom [http 503] tail"), Some(503));
        assert_eq!(http_status_of("boom"), None);
        assert_eq!(http_status_of("[http not-a-number]"), None);
    }

    // The three bodies below are verbatim from this install's database: each one
    // killed a run or a subagent because its status said "your request is wrong".
    #[test]
    fn a_rate_limit_inside_a_refusal_status_is_transient() {
        let bodies = [
            "provider rejected the request (400 Bad Request): the endpoint refused our payload \
             (response body: {\"error\":{\"message\":\"[400]: [code=tier_limit_exceeded] membership \
             rpm limit exceeded: 101/100 resets 2026-11-09T22:00:00Z\",\"type\":\"invalid_request_error\"}}) \
             [http 400]",
            "provider rejected the request (402 Payment Required): (response body: {\"error\":\
             {\"message\":\"[402]: no provider's ask matches your max-per-mtok bid (reset after 2m)\"}}) \
             [http 402]",
            "provider error: the API key has no subscription quota or balance available for this \
             request; ... (reset after 3s)",
            "provider error: All credentials for model hy4-codefast/hy4 are cooling down",
            "provider rate limit hit (429 Too Many Requests) [http 429]",
        ];
        for body in bodies {
            assert!(is_transient_refusal(body), "not transient: {body}");
        }
    }

    #[test]
    fn recharging_and_judgements_are_not_transient() {
        for body in [
            // Needs an operator, not another attempt.
            "provider rejected the request: insufficient_quota [http 400]",
            "provider error: You exceeded your current quota, please check your plan and billing",
            // A per-key spend limit that does not reset.
            "provider access denied (403): Key limit exceeded (total limit) $10.51 of $10.00 used; \
             does not reset.",
            // A real request problem.
            "provider rejected the request (400 Bad Request): model `sonnet` is not supported [http 400]",
            "provider rejected the request: context_length_exceeded [http 400]",
            "",
        ] {
            assert!(!is_transient_refusal(body), "wrongly transient: {body}");
        }
    }

    #[test]
    fn the_reset_window_is_read_out_of_the_body() {
        assert_eq!(
            retry_after_hint("{\"reset_seconds\": 1024, \"retry_after\":\"2026-11-10T02:15:31Z\"}"),
            Some(Duration::from_secs(1024))
        );
        assert_eq!(
            retry_after_hint("membership rpm limit exceeded (reset after 2m)"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            retry_after_hint("no provider's ask matches your max-per-mtok bid (reset after 2m)"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            retry_after_hint("slow down (retry after 30)"),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_after_hint("try again in 90s"),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            retry_after_hint("reset_seconds: 3600"),
            Some(Duration::from_secs(3600))
        );
        // Absurd values cannot park a run for a year.
        assert_eq!(
            retry_after_hint("\"reset_seconds\": 999999999"),
            Some(Duration::from_secs(86_400))
        );
        // No hint is not a zero-length hint: the caller falls back to backoff.
        assert_eq!(retry_after_hint("provider rejected the request (400)"), None);
        assert_eq!(retry_after_hint("reset_seconds: none"), None);
    }
}
