//! Shared JSON-RPC plumbing for the MCP transports: request ids, the pending
//! waiter map and error shapes.
//!
//! Both transports speak JSON-RPC 2.0; only the framing differs (newline JSON
//! over a child process stdin/stdout vs. HTTP POST with a JSON or SSE body).

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure of one MCP interaction.
#[derive(Debug)]
pub enum McpError {
    /// The server is unusable: spawn failure, dead process, closed pipe.
    Transport(String),
    /// Non-2xx HTTP response. `body` is an excerpt of the server's own message;
    /// request headers (and therefore credentials) are never included.
    Http { status: u16, body: String },
    /// The server answered with a JSON-RPC error object.
    Remote { code: i64, message: String },
    /// The response could not be understood.
    Protocol(String),
    /// No answer within the configured budget.
    Timeout(Duration),
    /// The caller cancelled.
    Cancelled,
}

impl McpError {
    /// Remove every configured HTTP header value from an error before it can be
    /// logged, returned in server status, or handed to the model.
    ///
    /// Servers can reflect credentials in a non-2xx body, a JSON-RPC error, or even
    /// a transport diagnostic. Redacting at the transport boundary keeps all of
    /// those surfaces covered instead of relying on every message constructor to
    /// remember which strings came from a remote peer.
    pub(crate) fn redacted(self, headers: &BTreeMap<String, String>) -> Self {
        match self {
            Self::Transport(message) => Self::Transport(redact_headers(&message, headers)),
            Self::Http { status, body } => Self::Http {
                status,
                body: redact_headers(&body, headers),
            },
            Self::Remote { code, message } => Self::Remote {
                code,
                message: redact_headers(&message, headers),
            },
            Self::Protocol(message) => Self::Protocol(redact_headers(&message, headers)),
            Self::Timeout(timeout) => Self::Timeout(timeout),
            Self::Cancelled => Self::Cancelled,
        }
    }

    /// One-line description for status reporting and logs.
    pub fn describe(&self) -> String {
        match self {
            McpError::Transport(m) => m.clone(),
            McpError::Http { status, body } => {
                if body.is_empty() {
                    format!("HTTP {status}")
                } else {
                    format!("HTTP {status}: {body}")
                }
            }
            McpError::Remote { code, message } => format!("{message} (code {code})"),
            McpError::Protocol(m) => format!("invalid response: {m}"),
            McpError::Timeout(d) => format!("timed out after {}s", fmt_secs(*d)),
            McpError::Cancelled => "cancelled".to_string(),
        }
    }

    /// Longer, actionable text for a tool result handed back to the model.
    pub fn tool_message(&self, server: &str, tool: &str, budget: Duration) -> String {
        match self {
            McpError::Timeout(d) => format!(
                "MCP tool `{tool}` on server `{server}` timed out after {}s (limit {}s). The \
                 server is still connected; retry with a narrower request or raise the timeout.",
                fmt_secs(*d),
                fmt_secs(budget)
            ),
            McpError::Cancelled => format!(
                "MCP tool `{tool}` on server `{server}` was cancelled; the call was abandoned."
            ),
            McpError::Remote { code, message } => format!(
                "MCP server `{server}` reported an error for tool `{tool}`: {message} (code {code})"
            ),
            McpError::Transport(m) => format!(
                "MCP tool `{tool}` failed: the server `{server}` is unreachable ({m}). Check that \
                 the MCP server is installed and running, then reconnect."
            ),
            McpError::Http { status, body } => format!(
                "MCP tool `{tool}` failed: server `{server}` answered HTTP {status}{}. Check the \
                 MCP server URL and its logs.",
                if body.is_empty() {
                    String::new()
                } else {
                    format!(": {body}")
                }
            ),
            McpError::Protocol(m) => format!(
                "MCP tool `{tool}` failed: server `{server}` sent a response this client cannot \
                 read ({m})."
            ),
        }
    }
}

/// Scrub configured header values, including a Bearer credential echoed without
/// its original scheme or capitalization. Header *names* are not secrets and stay
/// visible so diagnostics remain actionable.
pub(crate) fn redact_headers(text: &str, headers: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for value in headers.values() {
        let secret = value.trim();
        if secret.is_empty() {
            continue;
        }
        // Remove the complete configured value first. The credential-only and
        // case/whitespace-insensitive Bearer passes cover a peer rewriting it.
        out = out.replace(secret, "[redacted]");
        if let Some(credential) = bearer_credential(secret) {
            out = replace_bearer_credentials(&out, credential);
            if !credential.is_empty() {
                out = out.replace(credential, "[redacted]");
            }
        }
    }
    out
}

fn bearer_credential(secret: &str) -> Option<&str> {
    let scheme = secret.get(..6)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let rest = &secret[6..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim())
}

/// Replace `Bearer <credential>` regardless of scheme case or whitespace. The
/// credential comparison remains case-sensitive: credentials are opaque bytes,
/// and case-folding them could erase an unrelated word from a diagnostic.
fn replace_bearer_credentials(text: &str, credential: &str) -> String {
    if credential.is_empty() || text.is_empty() {
        return text.to_string();
    }
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(relative) = lower[search..].find("bearer") {
        let at = search + relative;
        let scheme_end = at + "bearer".len();
        let after = &text[scheme_end..];
        let whitespace = after.len() - after.trim_start_matches(char::is_whitespace).len();
        if whitespace == 0 {
            search = scheme_end;
            continue;
        }
        let credential_at = scheme_end + whitespace;
        if !text[credential_at..].starts_with(credential) {
            search = scheme_end;
            continue;
        }
        // Do not treat the middle of a longer alphanumeric token as a header.
        if at > 0 {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                search = scheme_end;
                continue;
            }
        }
        out.push_str(&text[copied..at]);
        out.push_str("[redacted]");
        copied = credential_at + credential.len();
        search = copied;
    }
    if copied == 0 {
        return text.to_string();
    }
    out.push_str(&text[copied..]);
    out
}

fn fmt_secs(d: Duration) -> String {
    let ms = d.as_millis();
    if ms.is_multiple_of(1000) {
        format!("{}", ms / 1000)
    } else {
        format!("{:.1}", d.as_secs_f64())
    }
}

/// What a pending request resolved to.
pub enum Reply {
    Ok(Value),
    Remote { code: i64, message: String },
    Transport(String),
}

impl Reply {
    pub(crate) fn into_result(self) -> Result<Value, McpError> {
        match self {
            Reply::Ok(v) => Ok(v),
            Reply::Remote { code, message } => Err(McpError::Remote { code, message }),
            Reply::Transport(m) => Err(McpError::Transport(m)),
        }
    }
}

/// Request ids and their waiters. Shared by the reader task and the callers.
pub(crate) struct Pending {
    next: AtomicU64,
    waiters: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
}

impl Pending {
    pub(crate) fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            waiters: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, oneshot::Sender<Reply>>> {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Allocate the next id and register a waiter for it.
    pub(crate) fn register(&self) -> (u64, oneshot::Receiver<Reply>) {
        let (tx, rx) = oneshot::channel();
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.lock().insert(id, tx);
        (id, rx)
    }

    /// Deliver a reply to the waiter for `id`. Returns false when nobody waits.
    pub(crate) fn resolve(&self, id: u64, reply: Reply) -> bool {
        match self.lock().remove(&id) {
            Some(tx) => {
                let _ = tx.send(reply);
                true
            }
            None => false,
        }
    }

    pub(crate) fn forget(&self, id: u64) {
        self.lock().remove(&id);
    }

    /// Fail every in-flight request (used when the transport dies).
    pub(crate) fn fail_all(&self, message: &str) {
        let waiters: Vec<_> = self.lock().drain().map(|(_, tx)| tx).collect();
        for tx in waiters {
            let _ = tx.send(Reply::Transport(message.to_string()));
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Build a JSON-RPC request line (no newline).
pub(crate) fn request_body(id: u64, method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// Build a JSON-RPC notification (no id, no response expected).
pub(crate) fn notification_body(method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// Interpret one inbound JSON-RPC message.
///
/// Returns `None` for messages that are not a response to `id` (notifications,
/// responses to other requests), and the result/error otherwise.
pub(crate) fn take_response(value: &Value, id: u64) -> Option<Result<Value, McpError>> {
    if value.get("method").is_some() {
        return None;
    }
    let is_response = value.get("result").is_some() || value.get("error").is_some();
    if !is_response {
        return None;
    }
    let matches = value
        .get("id")
        .and_then(|v| v.as_u64())
        .map(|v| v == id)
        .unwrap_or(false);
    if !matches {
        return None;
    }
    Some(response_payload(value))
}

/// Extract result/error from an object already known to be a response.
pub(crate) fn response_payload(value: &Value) -> Result<Value, McpError> {
    match value.get("error") {
        Some(err) if !err.is_null() => Err(McpError::Remote {
            code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-32000),
            message: err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unspecified MCP error")
                .to_string(),
        }),
        _ => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// Pick the response for `id` out of a JSON body that may be a single object or
/// a batch. Servers that omit ids on single responses are tolerated.
pub(crate) fn pick_response(body: Value, id: u64) -> Result<Value, McpError> {
    let items = match body {
        Value::Array(items) => items,
        other => vec![other],
    };
    for item in &items {
        if let Some(result) = take_response(item, id) {
            return result;
        }
    }
    // Tolerate a single id-less response (some lightweight servers do this).
    let responses: Vec<&Value> = items
        .iter()
        .filter(|v| {
            v.get("method").is_none() && (v.get("result").is_some() || v.get("error").is_some())
        })
        .collect();
    if responses.len() == 1 {
        return response_payload(responses[0]);
    }
    Err(McpError::Protocol(format!(
        "no response for request {id} (got {} message(s))",
        items.len()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configured_credentials_are_redacted_in_every_bearer_form() {
        let headers = BTreeMap::from([
            (
                "Authorization".to_string(),
                "Bearer sk-live-Secret".to_string(),
            ),
            ("X-Api-Key".to_string(), "second-secret".to_string()),
        ]);
        let reflected = concat!(
            "raw sk-live-Secret; configured=Bearer sk-live-Secret; ",
            "mixed=bEaReR   sk-live-Secret; token=second-secret; ",
            "decoy=BEARER sk-live-other"
        );
        let out = redact_headers(reflected, &headers);
        for secret in ["sk-live-Secret", "second-secret"] {
            assert!(!out.contains(secret), "{secret} survived in {out}");
        }
        assert!(!out.contains("bEaReR   "), "{out}");
        // Credentials are opaque and case-sensitive; redaction must not eat a
        // different token that merely shares a prefix.
        assert!(out.contains("BEARER sk-live-other"), "{out}");
        assert!(out.matches("[redacted]").count() >= 4, "{out}");
    }

    #[test]
    fn every_remote_error_variant_is_scrubbed() {
        let headers =
            BTreeMap::from([("Authorization".to_string(), "bearer top-secret".to_string())]);
        let errors = vec![
            McpError::Transport("echo TOP-SECRET top-secret".into()),
            McpError::Http {
                status: 500,
                body: "BEARER   top-secret".into(),
            },
            McpError::Remote {
                code: -1,
                message: "top-secret".into(),
            },
            McpError::Protocol("bearer top-secret".into()),
        ];
        for error in errors {
            let error = error.redacted(&headers);
            let text = format!(
                "{} / {}",
                error.describe(),
                error.tool_message("s", "t", Duration::from_secs(1))
            );
            assert!(!text.contains("top-secret"), "{text}");
        }
    }

    #[test]
    fn responses_match_their_id() {
        let msg = json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}});
        let got = take_response(&msg, 4).unwrap().unwrap();
        assert_eq!(got, json!({"ok": true}));
        assert!(take_response(&msg, 5).is_none());
        // Notifications and server→client requests are never responses.
        let note = json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {}});
        assert!(take_response(&note, 1).is_none());
    }

    #[test]
    fn jsonrpc_errors_keep_code_and_message() {
        let msg = json!({"id": 1, "error": {"code": -32601, "message": "no such tool"}});
        let err = take_response(&msg, 1).unwrap().unwrap_err();
        match err {
            McpError::Remote { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "no such tool");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn pick_response_handles_batches_and_id_less_bodies() {
        let batch = json!([{"id": 2, "result": "x"}, {"id": 1, "result": "y"}]);
        assert_eq!(pick_response(batch, 1).unwrap(), json!("y"));
        // A single response without an id is tolerated.
        assert_eq!(pick_response(json!({"result": 7}), 9).unwrap(), json!(7));
        let nothing = json!([{"method": "notifications/progress"}]);
        assert!(pick_response(nothing, 3).is_err());
    }

    #[test]
    fn pending_routes_replies_and_fails_all() {
        let pending = Pending::new();
        let (id, rx) = pending.register();
        assert!(pending.resolve(id, Reply::Ok(json!(1))));
        assert!(!pending.resolve(id, Reply::Ok(json!(2))));
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(rt.block_on(rx).unwrap().into_result().unwrap(), json!(1));

        let (_id2, rx2) = pending.register();
        pending.fail_all("dead");
        let err = rt.block_on(rx2).unwrap().into_result().unwrap_err();
        assert!(err.describe().contains("dead"));
    }
}
