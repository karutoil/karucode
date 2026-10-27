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
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            return Error::Timeout(with_body(format!(
                "provider did not answer in time ({status}) at {url}"
            )));
        }
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
    Error::Provider(redact(&text, secrets))
}

fn status_line(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_u16().to_string(),
    }
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

/// Replace every occurrence of a secret with `[redacted]`.
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.is_empty() {
            continue;
        }
        if out.contains(secret) {
            out = out.replace(secret, "[redacted]");
        }
        // `Authorization: Bearer <key>` echoes must not survive either.
        if let Some(token) = secret.strip_prefix("Bearer ").map(str::trim) {
            if !token.is_empty() && out.contains(token) {
                out = out.replace(token, "[redacted]");
            }
        }
        // A bare `Bearer <token>` header value is stored without the prefix by
        // some callers; scrub the token part as well.
        if let Some(token) = secret.strip_prefix("bearer ").map(str::trim) {
            if !token.is_empty() && out.contains(token) {
                out = out.replace(token, "[redacted]");
            }
        }
    }
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
    fn truncation_is_char_safe() {
        assert_eq!(truncate_chars("héllo", 3), "hél…");
        assert_eq!(truncate_chars("ok", 5), "ok");
    }
}
