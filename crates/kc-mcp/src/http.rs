//! Streamable-HTTP transport.
//!
//! Each JSON-RPC message is an HTTP `POST` of a JSON body. The server may answer
//! with `application/json` or with `text/event-stream` (SSE); both are parsed.
//! When the server returns an `Mcp-Session-Id` header the client carries it on
//! every later request, as the MCP streamable-HTTP transport requires.
//!
//! Request headers (which may hold API keys) are never logged or included in
//! error messages; only the URL's host and the HTTP status are.

use crate::rpc::{notification_body, pick_response, request_body, take_response, McpError};
use crate::transport::{bounded, Transport};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::debug;

const SESSION_HEADER: &str = "Mcp-Session-Id";
const ACCEPT: &str = "application/json, text/event-stream";

/// Hard cap for one HTTP response body (JSON or SSE). A hostile or broken
/// server must not be able to make the client buffer an unbounded body.
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Hard cap for the total SSE payload read while waiting for one response.
pub(crate) const MAX_SSE_BYTES: usize = 8 * 1024 * 1024;
/// Non-2xx bodies are only used as a short diagnostic (truncated to 300 chars),
/// so they are read with a much smaller cap than a successful response.
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

pub(crate) struct HttpTransport {
    name: String,
    url: String,
    headers: BTreeMap<String, String>,
    client: reqwest::Client,
    session: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl HttpTransport {
    pub(crate) fn new(
        name: String,
        url: String,
        headers: BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("could not build the HTTP client: {e}"))?;
        Ok(Self {
            name,
            url,
            headers,
            client,
            session: Mutex::new(None),
            next_id: AtomicU64::new(0),
        })
    }

    fn session(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_session(&self, value: Option<String>) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, ACCEPT)
            .json(body);
        for (k, v) in &self.headers {
            request = request.header(k.as_str(), v.as_str());
        }
        if let Some(session) = self.session() {
            request = request.header(SESSION_HEADER, session);
        }
        request
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn open(&self) -> Result<(), McpError> {
        // Nothing to spawn; validate the URL shape so the error is actionable.
        let parsed = reqwest::Url::parse(&self.url).map_err(|e| {
            McpError::Transport(format!(
                "MCP server `{}` has an invalid url `{}`: {e}",
                self.name, self.url
            ))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(McpError::Transport(format!(
                "MCP server `{}` url `{}` must be http or https",
                self.name, self.url
            )));
        }
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let body = request_body(id, method, params);
        let exchange = async {
            let response = self.post(&body).send().await.map_err(|e| {
                // `e` never contains request headers; keep the message short.
                McpError::Transport(format!("request to MCP server `{}` failed: {e}", self.name))
            })?;
            if let Some(session) = response
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                self.set_session(Some(session.to_string()));
            }
            let status = response.status();
            if !status.is_success() {
                let text = read_body_capped(response, MAX_ERROR_BODY_BYTES, &self.name)
                    .await
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                return Err(McpError::Http {
                    status: status.as_u16(),
                    body: truncate(text.trim(), 300),
                });
            }
            let is_sse = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_ascii_lowercase().starts_with("text/event-stream"))
                .unwrap_or(false);
            if is_sse {
                read_sse(response, id, &self.name).await
            } else {
                let bytes = read_body_capped(response, MAX_HTTP_BODY_BYTES, &self.name).await?;
                let text = String::from_utf8_lossy(&bytes);
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| McpError::Protocol(format!("invalid JSON body: {e}")))?;
                pick_response(value, id)
            }
        };
        bounded(exchange, timeout, cancel)
            .await
            .map_err(|error| error.redacted(&self.headers))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.notify_unbounded(method, params)
            .await
            .map_err(|error| error.redacted(&self.headers))
    }

    async fn close(&self) {
        self.set_session(None);
    }

    fn alive(&self) -> bool {
        true
    }

    fn kind(&self) -> &'static str {
        "http"
    }
}

impl HttpTransport {
    async fn notify_unbounded(&self, method: &str, params: Value) -> Result<(), McpError> {
        let body = notification_body(method, params);
        let response = self.post(&body).send().await.map_err(|e| {
            McpError::Transport(format!("request to MCP server `{}` failed: {e}", self.name))
        })?;
        if let Some(session) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            self.set_session(Some(session.to_string()));
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = read_body_capped(response, MAX_ERROR_BODY_BYTES, &self.name)
                .await
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            return Err(McpError::Http {
                status,
                body: truncate(text.trim(), 300),
            });
        }
        debug!(server = %self.name, method, "MCP notification delivered over HTTP");
        Ok(())
    }
}

/// Read a response body, refusing to buffer more than `max` bytes.
///
/// The `Content-Length` header is checked first so an honest oversized response
/// is rejected without reading it; chunked/streamed bodies are counted as they
/// arrive. Exceeding the cap is a protocol error, not a silent truncation: a
/// truncated JSON/SSE payload cannot be parsed and the server would look broken.
async fn read_body_capped(
    response: reqwest::Response,
    max: usize,
    name: &str,
) -> Result<Vec<u8>, McpError> {
    if let Some(len) = response.content_length() {
        if len > max as u64 {
            return Err(too_large(name, max, "a response body"));
        }
    }
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            McpError::Transport(format!(
                "reading the response of MCP server `{name}` failed: {e}"
            ))
        })?;
        if body.len().saturating_add(chunk.len()) > max {
            return Err(too_large(name, max, "a response body"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn too_large(name: &str, max: usize, what: &str) -> McpError {
    McpError::Protocol(format!(
        "MCP server `{name}` sent {what} larger than the {max} byte limit; the response was \
         rejected without buffering it"
    ))
}

/// Read an SSE response until the JSON-RPC response for `id` arrives.
///
/// The body is buffered as **bytes** and decoded only once a complete line is
/// available. Decoding each TCP chunk on its own — which is what
/// `String::from_utf8_lossy(&chunk)` did — corrupts any character whose UTF-8
/// encoding straddles a chunk boundary: the leading bytes of one write and the
/// continuation bytes of the next both decode to `U+FFFD`, the `data:` payload
/// becomes invalid JSON, `dispatch_event` drops it, and the call fails with
/// "no JSON-RPC response for request N in the SSE stream". A `write()` boundary
/// is not a protocol boundary, so nothing may rely on it.
async fn read_sse(response: reqwest::Response, id: u64, name: &str) -> Result<Value, McpError> {
    if let Some(len) = response.content_length() {
        if len > MAX_SSE_BYTES as u64 {
            return Err(too_large(name, MAX_SSE_BYTES, "an SSE stream"));
        }
    }
    let mut stream = response.bytes_stream();
    // Raw bytes, not a `String`: a line is decoded only when it is complete.
    let mut buffer: Vec<u8> = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut total: usize = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            McpError::Transport(format!(
                "reading the SSE body of MCP server `{name}` failed: {e}"
            ))
        })?;
        total = total.saturating_add(chunk.len());
        if total > MAX_SSE_BYTES {
            return Err(too_large(name, MAX_SSE_BYTES, "an SSE stream"));
        }
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
            // The slice up to and including the newline is a complete line, so
            // it must be valid UTF-8 on its own. An invalid one is a broken
            // producer, not a chunking artefact: say so instead of guessing.
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let line = std::str::from_utf8(&line).map_err(|_| {
                McpError::Protocol(format!(
                    "MCP server `{name}` sent an SSE line that is not valid UTF-8"
                ))
            })?;
            if let Some(result) = handle_sse_line(line, &mut data_lines, id) {
                return result;
            }
        }
    }
    // A stream that ends without a trailing blank line still carries one event.
    if let Some(result) = dispatch_event(&mut data_lines, id) {
        return result;
    }
    Err(McpError::Protocol(format!(
        "no JSON-RPC response for request {id} in the SSE stream of MCP server `{name}`"
    )))
}

/// Feed one complete SSE line to the event accumulator.
///
/// Returns `Some` only when a blank line completed an event that carried the
/// response for `id`. `event:`, `id:`, `retry:` and `:comment` lines are not
/// needed here.
fn handle_sse_line(
    line: &str,
    data_lines: &mut Vec<String>,
    id: u64,
) -> Option<Result<Value, McpError>> {
    let line = line.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return dispatch_event(data_lines, id);
    }
    if let Some(rest) = line.strip_prefix("data:") {
        data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
    }
    None
}

fn dispatch_event(data_lines: &mut Vec<String>, id: u64) -> Option<Result<Value, McpError>> {
    if data_lines.is_empty() {
        return None;
    }
    let payload = data_lines.join("\n");
    data_lines.clear();
    let value: Value = serde_json::from_str(&payload).ok()?;
    take_response(&value, id)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transport() -> HttpTransport {
        HttpTransport::new(
            "demo".into(),
            "http://127.0.0.1:1/mcp".into(),
            BTreeMap::from([("Authorization".to_string(), "Bearer sk-secret".to_string())]),
        )
        .unwrap()
    }

    #[test]
    fn session_header_is_carried_and_cleared() {
        let t = transport();
        assert!(t.session().is_none());
        t.set_session(Some("abc".into()));
        assert_eq!(t.session().as_deref(), Some("abc"));
        t.set_session(None);
        assert!(t.session().is_none());
    }

    #[test]
    fn errors_never_contain_configured_headers() {
        let t = transport();
        let err = McpError::Http {
            status: 401,
            body: "unauthorized".into(),
        };
        let text = err.describe();
        assert!(!text.contains("sk-secret"), "{text}");
        let msg = err.tool_message("demo", "plugin__x", Duration::from_secs(1));
        assert!(!msg.contains("Bearer"), "{msg}");
        assert!(t.kind() == "http" && t.alive());
    }

    #[test]
    fn sse_events_dispatch_only_matching_responses() {
        let mut data =
            vec![json!({"jsonrpc": "2.0", "method": "notifications/progress"}).to_string()];
        assert!(dispatch_event(&mut data, 7).is_none());
        assert!(data.is_empty());

        let mut data = vec![json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": 1}}).to_string()];
        let got = dispatch_event(&mut data, 7).unwrap().unwrap();
        assert_eq!(got, json!({"ok": 1}));
    }

    #[tokio::test]
    async fn invalid_url_is_rejected_before_any_request() {
        let t = HttpTransport::new("demo".into(), "not-a-url".into(), BTreeMap::new()).unwrap();
        let err = t.open().await.unwrap_err();
        assert!(err.describe().contains("invalid url"), "{}", err.describe());
    }

    #[test]
    fn sse_lines_accumulate_into_one_event() {
        let mut data = Vec::new();
        assert!(handle_sse_line("event: message\n", &mut data, 3).is_none());
        assert!(handle_sse_line("data: {\"a\":\n", &mut data, 3).is_none());
        assert!(handle_sse_line("data: 1}\n", &mut data, 3).is_none());
        assert_eq!(data, vec!["{\"a\":".to_string(), "1}".to_string()]);
        assert!(handle_sse_line("\n", &mut data, 3).is_none());
        assert!(data.is_empty(), "the event was consumed on the blank line");

        // A blank line that completes the wanted response returns it.
        let mut data = Vec::new();
        handle_sse_line(
            "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"ok\":true}}\n",
            &mut data,
            9,
        );
        let got = handle_sse_line("\r\n", &mut data, 9).unwrap().unwrap();
        assert_eq!(got, json!({"ok": true}));

        // An unrelated event is dropped and the accumulator reset.
        let mut data = Vec::new();
        handle_sse_line("data: not json\n", &mut data, 9);
        assert!(handle_sse_line("\n", &mut data, 9).is_none());
        assert!(data.is_empty());
    }

    #[test]
    fn truncate_keeps_char_boundaries() {
        assert_eq!(truncate("héllo", 3), "hé...");
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn body_caps_are_eight_mebibytes_and_errors_are_explicit() {
        assert_eq!(MAX_HTTP_BODY_BYTES, 8 * 1024 * 1024);
        assert_eq!(MAX_SSE_BYTES, 8 * 1024 * 1024);
        let message = too_large("demo", MAX_SSE_BYTES, "an SSE stream").describe();
        assert!(message.contains("larger than"), "{message}");
        assert!(message.contains("8388608"), "{message}");
        assert!(message.contains("rejected"), "{message}");
    }
}
