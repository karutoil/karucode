//! Integration tests for the MCP client.
//!
//! stdio is exercised against `tests/fixtures/fake_mcp_server.sh` (POSIX sh +
//! python3 only, no cargo-built binary). The HTTP transport is exercised against
//! a canned HTTP/1.1 server built directly on `tokio::net::TcpListener`, so the
//! tests need no web framework.

use async_trait::async_trait;
use kc_core::approval::{ApprovalDecision, ApprovalRequest};
use kc_core::config::{ApprovalPolicy, Limits, McpServerConfig, ModelConfig, SubagentConfig};
use kc_core::ids::{RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use kc_core::tool::{
    ApprovalGate, ApprovalHandle, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin,
    ToolOutcome,
};
use kc_mcp::{McpManager, McpTimeouts, MCP_CALL_TIMEOUT};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/fake_mcp_server.sh"
);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-mcp-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stdio_server(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: "stdio".into(),
        command: vec!["sh".to_string(), FIXTURE.to_string()],
        url: None,
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        enabled: true,
        require_approval: true,
    }
}

struct TestSettings;

#[async_trait]
impl Settings for TestSettings {
    async fn model(&self) -> ModelConfig {
        ModelConfig::default()
    }
    async fn limits(&self) -> Limits {
        Limits::default()
    }
    async fn approval_policy(&self) -> ApprovalPolicy {
        ApprovalPolicy::default()
    }
    async fn subagent_config(&self) -> SubagentConfig {
        SubagentConfig::default()
    }
}

struct AllowGate;

#[async_trait]
impl ApprovalGate for AllowGate {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::AllowOnce
    }
}

fn ctx(root: &Path, cancel: CancellationToken) -> ToolCtx {
    ToolCtx {
        workspace: Arc::new(Workspace::new(root).unwrap()),
        cancel,
        approvals: ApprovalHandle::new(
            Arc::new(AllowGate),
            SessionId::from("s_test"),
            RunId::from("r_test"),
            None,
        ),
        events: kc_core::events::noop_sink(),
        settings: Arc::new(TestSettings),
        depth: 0,
        supports_images: false,
    }
}

fn find(tools: &[Arc<dyn Tool>], name: &str) -> Arc<dyn Tool> {
    tools
        .iter()
        .find(|t| t.spec().name == name)
        .unwrap_or_else(|| {
            panic!(
                "tool `{name}` not exposed; got {:?}",
                tools.iter().map(|t| t.spec().name).collect::<Vec<_>>()
            )
        })
        .clone()
}

async fn call_tool(tool: Arc<dyn Tool>, root: &Path, args: Value) -> ToolOutcome {
    tool.invoke(
        ToolCallRequest {
            call_id: "call_1".into(),
            name: tool.spec().name.clone(),
            arguments: args,
        },
        ctx(root, CancellationToken::new()),
    )
    .await
}

// ---------------------------------------------------------------------------
// stdio
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stdio_handshake_and_tools_list() {
    let dir = temp_dir("stdio-handshake");
    let mut manager = McpManager::new(&[stdio_server("fake")]);
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].status, "connected", "{:?}", statuses[0].error);
    assert!(statuses[0].error.is_none());
    assert_eq!(
        statuses[0].tools,
        vec![
            "mcp__fake__echo".to_string(),
            "mcp__fake__sleep".to_string(),
            "mcp__fake__fail".to_string(),
            "mcp__fake__ghost".to_string(),
        ]
    );

    let tools = manager.tools();
    let echo = find(&tools, "mcp__fake__echo");
    let spec = echo.spec();
    assert_eq!(
        spec.origin,
        ToolOrigin::Mcp {
            server: "fake".into()
        }
    );
    // MCP tools always take the approval path of first-party external tools.
    assert_eq!(echo.class(), ToolClass::External);
    assert!(spec.description.contains("Echo text back."));
    assert!(spec.description.contains("[mcp: fake]"));
    assert_eq!(
        spec.parameters["properties"]["text"]["type"],
        json!("string")
    );

    manager.shutdown().await;
    assert_eq!(manager.statuses()[0].status, "disconnected");
    assert!(manager.tools().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn stdio_tools_call_joins_text_content() {
    let dir = temp_dir("stdio-call");
    let mut manager = McpManager::new(&[stdio_server("fake")]);
    manager.connect_all().await;
    let echo = find(&manager.tools(), "mcp__fake__echo");

    let out = call_tool(echo, &dir, json!({"text": "hi"})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "mcp echo: hi");

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn stdio_is_error_and_jsonrpc_errors_become_failed_outcomes() {
    let dir = temp_dir("stdio-errors");
    let mut manager = McpManager::new(&[stdio_server("fake")]);
    manager.connect_all().await;

    let failed = call_tool(find(&manager.tools(), "mcp__fake__fail"), &dir, json!({})).await;
    assert!(!failed.ok);
    assert!(
        failed.content.contains("mcp tool failed on purpose"),
        "{}",
        failed.content
    );
    assert!(
        failed.content.contains("mcp__fake__fail"),
        "{}",
        failed.content
    );

    // `ghost` is listed but not implemented: the server answers JSON-RPC error
    // -32601, which must surface as an actionable failure, not a panic.
    let ghost = call_tool(find(&manager.tools(), "mcp__fake__ghost"), &dir, json!({})).await;
    assert!(!ghost.ok);
    assert!(
        ghost.content.contains("unknown tool ghost"),
        "{}",
        ghost.content
    );
    assert!(ghost.content.contains("-32601"), "{}", ghost.content);

    // Remote errors do not disconnect the server.
    assert_eq!(manager.statuses()[0].status, "connected");
    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn stdio_failure_isolation_and_bounded_connect() {
    let dir = temp_dir("stdio-isolation");
    let mut broken = stdio_server("broken");
    broken.env.insert("FAKE_MCP_FAIL".into(), "1".into());
    let mut hanging = stdio_server("hanging");
    hanging.env.insert("FAKE_MCP_HANG".into(), "1".into());
    let healthy = stdio_server("healthy");

    let mut manager = McpManager::new(&[broken, hanging, healthy]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_millis(500),
        call: Duration::from_secs(5),
    });
    let started = Instant::now();
    manager.connect_all().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "connect_all must be bounded, took {:?}",
        started.elapsed()
    );

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "failed");
    assert!(statuses[0].tools.is_empty());
    assert!(statuses[0].error.is_some());
    assert_eq!(statuses[1].status, "failed");
    assert!(
        statuses[1].error.as_deref().unwrap().contains("timed out"),
        "{:?}",
        statuses[1].error
    );
    assert_eq!(statuses[2].status, "connected");
    assert!(!statuses[2].tools.is_empty());

    // The healthy server still works, and only its tools are exposed.
    let tools = manager.tools();
    assert!(tools
        .iter()
        .all(|t| t.spec().name.starts_with("mcp__healthy__")));
    let out = call_tool(
        find(&tools, "mcp__healthy__echo"),
        &dir,
        json!({"text": "ok"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn stdio_cancellation_abandons_the_call() {
    let dir = temp_dir("stdio-cancel");
    let mut slow = stdio_server("slow");
    slow.env.insert("FAKE_MCP_SLOW_SECS".into(), "10".into());
    let mut manager = McpManager::new(&[slow]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(5),
        call: Duration::from_secs(30),
    });
    manager.connect_all().await;
    let sleep = find(&manager.tools(), "mcp__slow__sleep");

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        token.cancel();
    });
    let started = Instant::now();
    let out = sleep
        .invoke(
            ToolCallRequest {
                call_id: "c1".into(),
                name: "mcp__slow__sleep".into(),
                arguments: json!({}),
            },
            ctx(&dir, cancel),
        )
        .await;
    assert!(!out.ok);
    assert!(out.content.contains("cancelled"), "{}", out.content);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancellation must be prompt, took {:?}",
        started.elapsed()
    );
    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn stdio_call_timeout_is_reported() {
    let dir = temp_dir("stdio-timeout");
    let mut slow = stdio_server("slow");
    slow.env.insert("FAKE_MCP_SLOW_SECS".into(), "10".into());
    let mut manager = McpManager::new(&[slow]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(5),
        call: Duration::from_millis(300),
    });
    manager.connect_all().await;
    let sleep = find(&manager.tools(), "mcp__slow__sleep");

    let started = Instant::now();
    let out = call_tool(sleep, &dir, json!({})).await;
    assert!(!out.ok);
    assert!(out.content.contains("timed out"), "{}", out.content);
    assert!(started.elapsed() < Duration::from_secs(5));
    // A timeout does not mark the server dead.
    assert_eq!(manager.statuses()[0].status, "connected");

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// HTTP transport (canned server on a tokio TcpListener)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn json_body(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

type Handler = Arc<dyn Fn(&HttpRequest) -> (u16, Vec<(String, String)>, String) + Send + Sync>;

struct CannedHttp {
    addr: SocketAddr,
    requests: Arc<std::sync::Mutex<Vec<HttpRequest>>>,
    stop: CancellationToken,
}

impl CannedHttp {
    async fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = CancellationToken::new();
        let stop_task = stop.clone();
        let recorded = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = stop_task.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((mut stream, _)) = accepted else { break };
                let handler = Arc::clone(&handler);
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    if let Some(request) = read_http_request(&mut stream).await {
                        recorded.lock().unwrap().push(request.clone());
                        let (status, headers, body) = handler(&request);
                        let reason = match status {
                            200 => "OK",
                            202 => "Accepted",
                            401 => "Unauthorized",
                            _ => "Error",
                        };
                        let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
                        for (k, v) in headers {
                            response.push_str(&format!("{k}: {v}\r\n"));
                        }
                        response.push_str(&format!(
                            "content-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        ));
                        response.push_str(&body);
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.flush().await;
                    }
                });
            }
        });
        Self {
            addr,
            requests,
            stop,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for CannedHttp {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Option<HttpRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_bytes(&buf, b"\r\n\r\n") {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim().to_string();
            if key == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((key, value));
        }
    }
    let mut body_bytes = buf[header_end + 4..].to_vec();
    while body_bytes.len() < content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..n]);
    }
    body_bytes.truncate(content_length);
    Some(HttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn http_config(name: &str, url: String, headers: BTreeMap<String, String>) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: "http".into(),
        command: Vec::new(),
        url: Some(url),
        env: BTreeMap::new(),
        headers,
        enabled: true,
        require_approval: true,
    }
}

/// A JSON-RPC handler that answers initialize / tools/list / tools/call.
fn json_handler(sse_for: Option<&'static str>) -> Handler {
    Arc::new(move |request: &HttpRequest| {
        let body = request.json_body();
        let method = body
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        if method == "notifications/initialized" {
            return (202, vec![], String::new());
        }
        let result = match method.as_str() {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "canned", "version": "0.1.0"},
            }),
            "tools/list" => json!({
                "tools": [
                    {"name": "echo", "description": "Echo.", "inputSchema": {"type": "object"}},
                    {"name": "fail", "description": "Fail.", "inputSchema": {"type": "object"}}
                ]
            }),
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or_default();
                if name == "fail" {
                    json!({"content": [{"type": "text", "text": "canned failure"}], "isError": true})
                } else {
                    json!({"content": [{"type": "text", "text": "canned echo"}], "isError": false})
                }
            }
            other => {
                let error = json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("unknown method {other}")}});
                return (
                    200,
                    vec![("content-type".into(), "application/json".into())],
                    error.to_string(),
                );
            }
        };
        let payload = json!({"jsonrpc": "2.0", "id": id, "result": result});
        let mut headers = Vec::new();
        if method == "initialize" {
            headers.push(("mcp-session-id".to_string(), "sess-1".to_string()));
        }
        if sse_for == Some(method.as_str()) {
            headers.push(("content-type".to_string(), "text/event-stream".to_string()));
            let body = format!("event: message\ndata: {}\n\n", payload);
            (200, headers, body)
        } else {
            headers.push(("content-type".to_string(), "application/json".to_string()));
            (200, headers, payload.to_string())
        }
    })
}

#[tokio::test]
async fn http_transport_handshakes_calls_and_carries_the_session_id() {
    let server = CannedHttp::start(json_handler(None)).await;
    let mut headers = BTreeMap::new();
    headers.insert(
        "Authorization".to_string(),
        "Bearer sk-super-secret".to_string(),
    );
    let mut manager = McpManager::new(&[http_config("remote", server.url("/mcp"), headers)]);
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "connected", "{:?}", statuses[0].error);
    assert_eq!(statuses[0].transport, "http");
    assert_eq!(
        statuses[0].tools,
        vec![
            "mcp__remote__echo".to_string(),
            "mcp__remote__fail".to_string()
        ]
    );
    // Status never leaks configured headers or keys.
    let rendered = serde_json::to_string(&statuses).unwrap();
    assert!(!rendered.contains("sk-super-secret"), "{rendered}");

    let echo = find(&manager.tools(), "mcp__remote__echo");
    assert_eq!(echo.class(), ToolClass::External);
    let dir = temp_dir("http-call");
    let out = call_tool(echo, &dir, json!({"text": "hi"})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "canned echo");

    let failed = call_tool(find(&manager.tools(), "mcp__remote__fail"), &dir, json!({})).await;
    assert!(!failed.ok);
    assert!(
        failed.content.contains("canned failure"),
        "{}",
        failed.content
    );

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        5,
        "{:?}",
        requests.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
    for request in &requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp");
        assert_eq!(
            request.header("accept"),
            Some("application/json, text/event-stream")
        );
        assert_eq!(
            request.header("authorization"),
            Some("Bearer sk-super-secret")
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
    }
    // The session id returned by `initialize` travels on every later request.
    assert_eq!(requests[0].json_body()["method"], "initialize");
    let init_params = &requests[0].json_body()["params"];
    assert_eq!(init_params["protocolVersion"], "2024-11-05");
    assert_eq!(init_params["clientInfo"]["name"], "karucode");
    assert!(requests[0].header("mcp-session-id").is_none());
    for request in &requests[1..] {
        assert_eq!(request.header("mcp-session-id"), Some("sess-1"));
    }
    assert_eq!(
        requests[1].json_body()["method"],
        "notifications/initialized"
    );
    assert_eq!(requests[2].json_body()["method"], "tools/list");
    let call = requests
        .iter()
        .find(|r| r.json_body()["method"] == "tools/call")
        .expect("tools/call POSTed");
    assert_eq!(call.json_body()["params"]["name"], "echo");
    assert_eq!(call.json_body()["params"]["arguments"]["text"], "hi");

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn http_sse_responses_are_parsed() {
    let server = CannedHttp::start(json_handler(Some("tools/list"))).await;
    let mut manager = McpManager::new(&[http_config("sse", server.url("/mcp"), BTreeMap::new())]);
    manager.connect_all().await;
    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "connected", "{:?}", statuses[0].error);
    assert_eq!(statuses[0].tools.len(), 2);

    // The SSE event carried the tool call response too.
    let dir = temp_dir("http-sse");
    let out = call_tool(find(&manager.tools(), "mcp__sse__echo"), &dir, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "canned echo");

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn http_failures_are_reported_without_leaking_headers() {
    let handler: Handler = Arc::new(|_request: &HttpRequest| {
        (
            401,
            vec![("content-type".into(), "application/json".into())],
            r#"{"error":"bad key"}"#.to_string(),
        )
    });
    let server = CannedHttp::start(handler).await;
    let mut headers = BTreeMap::new();
    headers.insert(
        "Authorization".to_string(),
        "Bearer sk-very-secret".to_string(),
    );
    let mut manager = McpManager::new(&[http_config("guarded", server.url("/mcp"), headers)]);
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "failed");
    let error = statuses[0].error.clone().unwrap();
    assert!(error.contains("401"), "{error}");
    assert!(error.contains("bad key"), "{error}");
    assert!(!error.contains("sk-very-secret"), "{error}");
    assert!(!error.to_lowercase().contains("authorization"), "{error}");
    assert!(manager.tools().is_empty());
    manager.shutdown().await;
}

#[tokio::test]
async fn http_and_stdio_servers_coexist_and_failed_http_servers_do_not_block() {
    let server = CannedHttp::start(json_handler(None)).await;
    let dead_url = {
        // Bind then drop, so nothing is listening: connection refused.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}/mcp")
    };
    let mut dead = http_config("dead-http", dead_url, BTreeMap::new());
    dead.enabled = true;
    let mut manager = McpManager::new(&[
        http_config("canned", server.url("/mcp"), BTreeMap::new()),
        dead,
        stdio_server("fake"),
    ]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(3),
        call: Duration::from_secs(5),
    });
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "connected");
    assert_eq!(statuses[1].status, "failed");
    assert!(statuses[1].error.is_some());
    assert_eq!(statuses[2].status, "connected");

    let names: Vec<String> = manager.tools().iter().map(|t| t.spec().name).collect();
    assert!(names.contains(&"mcp__canned__echo".to_string()));
    assert!(names.contains(&"mcp__fake__echo".to_string()));

    let dir = temp_dir("mixed");
    let out = call_tool(
        find(&manager.tools(), "mcp__fake__echo"),
        &dir,
        json!({"text": "x"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn documented_call_timeout_is_two_minutes() {
    assert_eq!(MCP_CALL_TIMEOUT, Duration::from_secs(120));
}

// ---------------------------------------------------------------------------
// Resource limits (unbounded reads are a DoS)
// ---------------------------------------------------------------------------

/// The documented cap for one HTTP body / SSE stream / stdio line.
const EIGHT_MIB: usize = 8 * 1024 * 1024;

#[tokio::test]
async fn http_oversized_json_body_is_rejected_with_a_clear_error() {
    let handler: Handler = Arc::new(|_request: &HttpRequest| {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"pad": "y".repeat(EIGHT_MIB + 1024)},
        })
        .to_string();
        (
            200,
            vec![("content-type".into(), "application/json".into())],
            body,
        )
    });
    let server = CannedHttp::start(handler).await;
    let mut manager = McpManager::new(&[http_config("huge", server.url("/mcp"), BTreeMap::new())]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(5),
        call: Duration::from_secs(5),
    });
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "failed");
    let error = statuses[0].error.clone().unwrap_or_default();
    assert!(error.contains("larger than"), "{error}");
    assert!(error.contains(&EIGHT_MIB.to_string()), "{error}");
    assert!(manager.tools().is_empty());
    manager.shutdown().await;
}

#[tokio::test]
async fn http_oversized_sse_stream_is_rejected_with_a_clear_error() {
    let handler: Handler = Arc::new(|_request: &HttpRequest| {
        let body = format!("event: message\ndata: {}\n\n", "y".repeat(EIGHT_MIB + 1024));
        (
            200,
            vec![("content-type".into(), "text/event-stream".into())],
            body,
        )
    });
    let server = CannedHttp::start(handler).await;
    let mut manager =
        McpManager::new(&[http_config("huge-sse", server.url("/mcp"), BTreeMap::new())]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(5),
        call: Duration::from_secs(5),
    });
    manager.connect_all().await;

    let statuses = manager.statuses();
    assert_eq!(statuses[0].status, "failed");
    let error = statuses[0].error.clone().unwrap_or_default();
    assert!(error.contains("SSE stream"), "{error}");
    assert!(error.contains("larger than"), "{error}");
    assert!(error.contains(&EIGHT_MIB.to_string()), "{error}");
    manager.shutdown().await;
}

/// A stdio server whose `tools/call` answer is a single over-long line without
/// a newline. The host must fail the call with a protocol error instead of
/// buffering the line forever or waiting for the call timeout.
const OVERSIZED_LINE_SERVER: &str = r#"
import json, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "serverInfo": {"name": "huge", "version": "0"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "boom", "description": "Floods its stdout.",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        sys.stdout.write("y" * (8 * 1024 * 1024 + 4096))
        sys.stdout.flush()
"#;

#[tokio::test]
async fn stdio_oversized_line_is_a_prompt_protocol_error() {
    let dir = temp_dir("stdio-oversized");
    let script = dir.join("oversized_server.py");
    std::fs::write(&script, OVERSIZED_LINE_SERVER).unwrap();
    let mut config = stdio_server("huge");
    config.command = vec!["python3".to_string(), script.to_string_lossy().into_owned()];

    let mut manager = McpManager::new(&[config]);
    manager.set_timeouts(McpTimeouts {
        connect: Duration::from_secs(5),
        call: Duration::from_secs(5),
    });
    manager.connect_all().await;
    assert_eq!(
        manager.statuses()[0].status,
        "connected",
        "{:?}",
        manager.statuses()[0].error
    );

    let started = Instant::now();
    let out = call_tool(find(&manager.tools(), "mcp__huge__boom"), &dir, json!({})).await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains("larger than"),
        "expected the line cap to fire, got: {}",
        out.content
    );
    assert!(
        out.content.contains(&EIGHT_MIB.to_string()),
        "{}",
        out.content
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "an oversized line must fail fast, not hit the call timeout (took {:?})",
        started.elapsed()
    );

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn require_approval_false_exempts_the_server_but_keeps_the_external_class() {
    let dir = temp_dir("require-approval");
    let mut exempt = stdio_server("exempt");
    exempt.require_approval = false;
    let strict = stdio_server("strict");

    let mut manager = McpManager::new(&[exempt, strict]);
    manager.connect_all().await;
    let tools = manager.tools();

    let exempt_tool = find(&tools, "mcp__exempt__echo");
    assert!(
        !exempt_tool.requires_approval(),
        "require_approval = false must exempt the server"
    );
    // The exemption must not reclassify the tool: read_only mode still denies it.
    assert_eq!(exempt_tool.class(), ToolClass::External);

    let strict_tool = find(&tools, "mcp__strict__echo");
    assert!(
        strict_tool.requires_approval(),
        "the default (require_approval = true) must still ask"
    );
    assert_eq!(strict_tool.class(), ToolClass::External);

    manager.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}
