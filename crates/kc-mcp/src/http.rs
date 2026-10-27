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

pub(crate) struct HttpTransport {
    name: String,
    url: String,
    headers: Vec<(String, String)>,
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
            headers: headers.into_iter().collect(),
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
                let text = response.text().await.unwrap_or_default();
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
                let text = response.text().await.map_err(|e| {
                    McpError::Transport(format!(
                        "reading the response of MCP server `{}` failed: {e}",
                        self.name
                    ))
                })?;
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| McpError::Protocol(format!("invalid JSON body: {e}")))?;
                pick_response(value, id)
            }
        };
        bounded(exchange, timeout, cancel).await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
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
            let text = response.text().await.unwrap_or_default();
            return Err(McpError::Http {
                status,
                body: truncate(text.trim(), 300),
            });
        }
        debug!(server = %self.name, method, "MCP notification delivered over HTTP");
        Ok(())
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

/// Read an SSE response until the JSON-RPC response for `id` arrives.
async fn read_sse(response: reqwest::Response, id: u64, name: &str) -> Result<Value, McpError> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            McpError::Transport(format!(
                "reading the SSE body of MCP server `{name}` failed: {e}"
            ))
        })?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if let Some(result) = dispatch_event(&mut data_lines, id) {
                    return result;
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // `event:`, `id:`, `retry:` and `:comment` lines are not needed here.
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
    fn truncate_keeps_char_boundaries() {
        assert_eq!(truncate("héllo", 3), "hé...");
        assert_eq!(truncate("abc", 10), "abc");
    }
}
