//! Shared JSON-RPC plumbing for the MCP transports: request ids, the pending
//! waiter map and error shapes.
//!
//! Both transports speak JSON-RPC 2.0; only the framing differs (newline JSON
//! over a child process stdin/stdout vs. HTTP POST with a JSON or SSE body).

use serde_json::Value;
use std::collections::HashMap;
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
