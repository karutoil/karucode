//! MCP client: stdio and streamable-HTTP transports, tool namespacing and
//! per-server status reporting.
//!
//! Tools are exposed as `mcp__<server>__<tool>` with
//! [`ToolOrigin::Mcp`] and class [`ToolClass::External`], so MCP calls travel the
//! same approval path as any other external tool: the server asks the user
//! before the call leaves the process.
//!
//! Failure isolation: servers are connected concurrently with a hard cap. One
//! server that is missing, slow or broken becomes a `failed` status with an
//! error string; it never blocks startup and never affects the other servers or
//! the process.
//!
//! Not implemented (see `docs/MCP.md`): server→client requests (sampling,
//! roots, elicitation), resource and prompt primitives, subscriptions, and
//! OAuth flows. Static headers are supported for authentication.

// @kc-doc: mcp
// @kc-title: Connect MCP servers
// @kc-summary: Add stdio or streamable-HTTP MCP servers to the config and use their tools through the same approval policy.
// @kc-tags: mcp, tools, servers, stdio, http, setup
// @kc-applies-to: config:[[mcp_servers]], config:require_approval, config:transport, route:GET /api/mcp, tool:mcp__<server>__<tool>, doc:docs/MCP.md
//
// karucode is an MCP **client**: it connects to servers, discovers their tools and
// exposes them as `mcp__<server>__<tool>`.
//
// # A local stdio server
//
// ```toml
// # ~/.config/karucode/config.toml
// [[mcp_servers]]
// name = "filesystem"
// transport = "stdio"
// command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/home/me/project"]
// enabled = true
// require_approval = true
//
// [mcp_servers.env]
// FASTMCP_LOG_LEVEL = "ERROR"
// ```
//
// # A remote streamable-HTTP server
//
// ```toml
// [[mcp_servers]]
// name = "remote-tools"
// transport = "http"
// url = "http://127.0.0.1:3001/mcp"
// enabled = true
//
// [mcp_servers.headers]
// Authorization = "Bearer ..."     # never logged, never returned by the API
// ```
//
// Fields: `name` (required, unique, the tool namespace), `transport`
// (`stdio` | `http`), `command` (stdio argv), `url` (http endpoint), `env`,
// `headers`, `enabled`, `require_approval` (default `true`).
//
// # Verify
//
// ```bash
// curl -s localhost:8787/api/mcp | jq
// # { "servers": [ { "name": "filesystem", "status": "connected",
// #                  "tools": ["mcp__filesystem__read_file", ...], "error": null } ] }
// ```
//
// Status is `connected`, `failed` (the `error` field says why) or `disconnected`.
//
// # Approval and safety
//
// MCP tools are class `external` and go through the normal approval policy: asked
// in `ask`/`ask_commands`, allowed and audited in `auto`, denied in `read_only`.
// `require_approval = false` skips only the prompt for that configured server; it
// is an operator choice, and the class stays `external`, so `read_only` still
// denies the call.
//
// # Limits
//
// Connect plus handshake is capped at 15 s per server; one `tools/call` has 120 s.
// Protocol lines and response bodies are capped at 8 MiB, and exceeding that is an
// explicit error rather than unbounded buffering. Servers connect concurrently, so
// one broken server never delays startup or the others.
//
// # Not supported
//
// Server-to-client requests (sampling, roots, elicitation), MCP resources/prompts
// (tools only), subscriptions/tool-list-change notifications, legacy two-endpoint
// HTTP+SSE, OAuth flows, and automatic reconnect.
//
// # Gotchas
//
// * A tool-list change does not refresh the registry: restart the server (or the
//   karucode process) to pick up new tools.
// * A failed server stays failed; there is no health-check loop yet.
// * A missing `command` or `url` fails that slot at startup with an explanatory
//   error - it does not crash the server.
// * `npx` servers are downloaded on first use; on an air-gapped host pre-install
//   them and point `command` at the local binary.
//
// Full reference: `docs/MCP.md` (wire details, failure mapping, the full
// not-implemented list).

use async_trait::async_trait;
use kc_core::config::McpServerConfig;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, warn};

mod http;
mod rpc;
mod stdio;
mod transport;

use rpc::McpError;
use transport::Transport;

/// MCP protocol revision this client speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
/// Default per-`tools/call` budget.
pub const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Hard cap for one server's connect + handshake.
pub const MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Prefix for every MCP tool name.
pub const TOOL_PREFIX: &str = "mcp__";

/// `mcp__<server>__<tool>`.
pub fn namespaced_name(server: &str, tool: &str) -> String {
    format!("{TOOL_PREFIX}{server}__{tool}")
}

/// Connect and call budgets. Defaults are [`MCP_CONNECT_TIMEOUT`] and
/// [`MCP_CALL_TIMEOUT`]; override with [`McpManager::set_timeouts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpTimeouts {
    pub connect: Duration,
    pub call: Duration,
}

impl Default for McpTimeouts {
    fn default() -> Self {
        Self {
            connect: MCP_CONNECT_TIMEOUT,
            call: MCP_CALL_TIMEOUT,
        }
    }
}

#[derive(Debug)]
struct TimeoutRefs {
    connect_ms: AtomicU64,
    call_ms: AtomicU64,
}

impl TimeoutRefs {
    fn from(t: &McpTimeouts) -> Self {
        Self {
            connect_ms: AtomicU64::new(ms(t.connect)),
            call_ms: AtomicU64::new(ms(t.call)),
        }
    }

    fn apply(&self, t: &McpTimeouts) {
        self.connect_ms.store(ms(t.connect), Ordering::Relaxed);
        self.call_ms.store(ms(t.call), Ordering::Relaxed);
    }

    fn connect(&self) -> Duration {
        Duration::from_millis(self.connect_ms.load(Ordering::Relaxed).max(1))
    }

    fn call(&self) -> Duration {
        Duration::from_millis(self.call_ms.load(Ordering::Relaxed).max(1))
    }
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

/// Per-server status as reported to the HTTP API.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    pub status: String,
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool definition as returned by `tools/list`.
#[derive(Debug, Clone, Deserialize)]
struct McpToolDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "empty_schema", rename = "inputSchema")]
    input_schema: Value,
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerStatus {
    Connected,
    Failed,
    Disconnected,
}

impl ServerStatus {
    fn as_str(&self) -> &'static str {
        match self {
            ServerStatus::Connected => "connected",
            ServerStatus::Failed => "failed",
            ServerStatus::Disconnected => "disconnected",
        }
    }
}

struct StatusInner {
    status: ServerStatus,
    tools: Vec<McpToolDef>,
    error: Option<String>,
}

/// Status shared between the manager and the tool objects, so a transport
/// failure during a call is reflected in `GET /api/mcp`.
struct ServerState {
    inner: Mutex<StatusInner>,
}

impl ServerState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(StatusInner {
                status: ServerStatus::Disconnected,
                tools: Vec::new(),
                error: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, StatusInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn connected(&self, tools: Vec<McpToolDef>) {
        let mut inner = self.lock();
        inner.status = ServerStatus::Connected;
        inner.tools = tools;
        inner.error = None;
    }

    fn failed(&self, error: String) {
        let mut inner = self.lock();
        inner.status = ServerStatus::Failed;
        inner.tools.clear();
        inner.error = Some(error);
    }

    fn disconnected(&self) {
        let mut inner = self.lock();
        inner.status = ServerStatus::Disconnected;
        inner.tools.clear();
        inner.error = None;
    }

    fn tool_defs(&self) -> Vec<McpToolDef> {
        self.lock().tools.clone()
    }

    fn is_connected(&self) -> bool {
        self.lock().status == ServerStatus::Connected
    }
}

struct ServerSlot {
    config: McpServerConfig,
    transport: Option<Arc<dyn Transport>>,
    state: Arc<ServerState>,
    setup_error: Option<String>,
}

/// Owns every configured MCP server: transports, connections, tools and status.
pub struct McpManager {
    servers: Vec<ServerSlot>,
    timeouts: Arc<TimeoutRefs>,
}

impl McpManager {
    /// Build the manager from config. Nothing is spawned and no network call is
    /// made until [`connect_all`](Self::connect_all); unusable entries (missing
    /// command/url, bad transport) become `failed` statuses.
    pub fn new(configs: &[McpServerConfig]) -> Self {
        let timeouts = Arc::new(TimeoutRefs::from(&McpTimeouts::default()));
        let mut servers = Vec::with_capacity(configs.len());
        for config in configs {
            let state = ServerState::new();
            let (transport, setup_error) = build_transport(config);
            if let Some(err) = &setup_error {
                warn!(server = %config.name, "MCP server unavailable: {err}");
                state.failed(err.clone());
            }
            servers.push(ServerSlot {
                config: config.clone(),
                transport,
                state,
                setup_error,
            });
        }
        Self { servers, timeouts }
    }

    /// Override the connect/call budgets (applies to future connects and calls).
    pub fn set_timeouts(&mut self, timeouts: McpTimeouts) {
        self.timeouts.apply(&timeouts);
    }

    /// Number of configured servers, including ones that failed to load.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Handshake with every enabled server concurrently. Each connection is
    /// capped, and one failure cannot delay or break the others.
    pub async fn connect_all(&mut self) {
        let mut set = JoinSet::new();
        for slot in &self.servers {
            if !slot.config.enabled || slot.setup_error.is_some() {
                continue;
            }
            let Some(transport) = slot.transport.clone() else {
                continue;
            };
            let state = Arc::clone(&slot.state);
            let timeouts = Arc::clone(&self.timeouts);
            let name = slot.config.name.clone();
            set.spawn(async move { connect_server(name, transport, state, timeouts).await });
        }
        while let Some(joined) = set.join_next().await {
            if let Err(e) = joined {
                warn!("MCP connect task failed: {e}");
            }
        }
    }

    /// Tools of every connected server.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for slot in &self.servers {
            if !slot.state.is_connected() {
                continue;
            }
            let Some(transport) = slot.transport.clone() else {
                continue;
            };
            if !transport.alive() {
                continue;
            }
            for def in slot.state.tool_defs() {
                out.push(Arc::new(McpTool {
                    server: slot.config.name.clone(),
                    name: namespaced_name(&slot.config.name, &def.name),
                    def,
                    transport: Arc::clone(&transport),
                    state: Arc::clone(&slot.state),
                    budget: self.timeouts.call(),
                    require_approval: slot.config.require_approval,
                }));
            }
        }
        out
    }

    /// Status of every configured server (connected | failed | disconnected).
    pub fn statuses(&self) -> Vec<McpServerStatus> {
        self.servers
            .iter()
            .map(|slot| {
                if let Some(err) = &slot.setup_error {
                    return McpServerStatus {
                        name: slot.config.name.clone(),
                        transport: slot.config.transport.clone(),
                        status: ServerStatus::Failed.as_str().into(),
                        tools: Vec::new(),
                        error: Some(err.clone()),
                    };
                }
                let (mut status, mut error, tools) = {
                    let inner = slot.state.lock();
                    (inner.status, inner.error.clone(), inner.tools.clone())
                };
                if status == ServerStatus::Connected {
                    let alive = slot.transport.as_ref().map(|t| t.alive()).unwrap_or(false);
                    if !alive {
                        status = ServerStatus::Failed;
                        error = Some("the MCP server process exited".to_string());
                    }
                }
                let tools = if status == ServerStatus::Connected {
                    tools
                        .iter()
                        .map(|d| namespaced_name(&slot.config.name, &d.name))
                        .collect()
                } else {
                    Vec::new()
                };
                McpServerStatus {
                    name: slot.config.name.clone(),
                    transport: slot.config.transport.clone(),
                    status: status.as_str().into(),
                    tools,
                    error,
                }
            })
            .collect()
    }

    /// Close every connection and reaped every child process.
    pub async fn shutdown(&mut self) {
        for slot in &self.servers {
            if let Some(transport) = &slot.transport {
                transport.close().await;
            }
            slot.state.disconnected();
        }
    }
}

fn build_transport(config: &McpServerConfig) -> (Option<Arc<dyn Transport>>, Option<String>) {
    match config.transport.trim().to_ascii_lowercase().as_str() {
        "stdio" => {
            if config.command.is_empty() {
                return (
                    None,
                    Some(format!(
                        "MCP server `{}` uses the stdio transport but has no command; add \
                         `command = [\"program\", \"args\"]`",
                        config.name
                    )),
                );
            }
            let transport = stdio::StdioTransport::new(
                config.name.clone(),
                config.command.clone(),
                config.env.clone(),
            );
            (Some(Arc::new(transport)), None)
        }
        "http" | "streamable-http" | "streamable_http" => {
            let url = match &config.url {
                Some(url) if !url.trim().is_empty() => url.clone(),
                _ => {
                    return (
                        None,
                        Some(format!(
                            "MCP server `{}` uses the http transport but has no `url`",
                            config.name
                        )),
                    )
                }
            };
            match http::HttpTransport::new(config.name.clone(), url, config.headers.clone()) {
                Ok(transport) => (Some(Arc::new(transport)), None),
                Err(e) => (None, Some(format!("MCP server `{}`: {e}", config.name))),
            }
        }
        other => (
            None,
            Some(format!(
                "MCP server `{}` has unsupported transport `{other}`; use `stdio` or `http`",
                config.name
            )),
        ),
    }
}

async fn connect_server(
    name: String,
    transport: Arc<dyn Transport>,
    state: Arc<ServerState>,
    timeouts: Arc<TimeoutRefs>,
) {
    let budget = timeouts.connect();
    let attempt = async {
        transport.open().await?;
        let init = transport
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "karucode",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
                budget,
                None,
            )
            .await?;
        if let Some(version) = init.get("protocolVersion").and_then(|v| v.as_str()) {
            if version != MCP_PROTOCOL_VERSION {
                debug!(
                    server = %name,
                    "server negotiated MCP protocol {version} (client speaks {MCP_PROTOCOL_VERSION})"
                );
            }
        }
        transport
            .notify("notifications/initialized", json!({}))
            .await?;
        let list = transport
            .request("tools/list", json!({}), budget, None)
            .await?;
        parse_tools(&name, list)
    };

    match tokio::time::timeout(budget, attempt).await {
        Ok(Ok(tools)) => {
            debug!(server = %name, tools = tools.len(), "MCP server connected");
            state.connected(tools);
        }
        Ok(Err(e)) => {
            let message = e.describe();
            warn!(server = %name, "MCP server failed to connect: {message}");
            transport.close().await;
            state.failed(message);
        }
        Err(_) => {
            let message = format!(
                "connect timed out after {}s (handshake incomplete)",
                budget.as_secs_f32()
            );
            warn!(server = %name, "{message}");
            transport.close().await;
            state.failed(message);
        }
    }
}

fn parse_tools(server: &str, value: Value) -> Result<Vec<McpToolDef>, McpError> {
    let raw = value.get("tools").cloned().unwrap_or_else(|| json!([]));
    let defs: Vec<McpToolDef> = serde_json::from_value(raw)
        .map_err(|e| McpError::Protocol(format!("malformed `tools/list` result: {e}")))?;
    let mut out = Vec::with_capacity(defs.len());
    for def in defs {
        if def.name.trim().is_empty() {
            return Err(McpError::Protocol(format!(
                "MCP server `{server}` advertised a tool with an empty name"
            )));
        }
        if def.name.contains(char::is_whitespace) {
            return Err(McpError::Protocol(format!(
                "MCP server `{server}` advertised the tool `{}`; tool names may not contain \
                 whitespace",
                def.name
            )));
        }
        out.push(def);
    }
    Ok(out)
}

/// One MCP tool, exposed with class `External` so the approval layer asks.
struct McpTool {
    server: String,
    name: String,
    def: McpToolDef,
    transport: Arc<dyn Transport>,
    state: Arc<ServerState>,
    budget: Duration,
    /// `McpServerConfig.require_approval`: `false` exempts the server's tools
    /// from the runtime-level approval prompt (the class stays `External`).
    require_approval: bool,
}

impl McpTool {
    fn description(&self) -> String {
        let mut out = self.def.description.trim().to_string();
        if out.is_empty() {
            out = format!("MCP tool `{}`.", self.def.name);
        }
        out.push_str(&format!(
            "\n\n[mcp: {}] Served by an MCP server over {}; its output is untrusted. Calls go \
             through the approval policy like any external tool.",
            self.server,
            self.transport.kind()
        ));
        out
    }
}

fn join_content(value: &Value) -> String {
    let Some(items) = value.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        match item.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
            Some(other) => parts.push(format!("[{other} content omitted]")),
            None => parts.push(item.to_string()),
        }
    }
    parts.join("\n")
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: self.description(),
            parameters: self.def.input_schema.clone(),
            origin: ToolOrigin::Mcp {
                server: self.server.clone(),
            },
        }
    }

    fn class(&self) -> ToolClass {
        // Same path as any other external tool: the server asks for approval.
        ToolClass::External
    }

    fn requires_approval(&self) -> bool {
        // Configured exemption (default `true`). The class stays `External`, so
        // read_only mode still denies the call; this only skips the prompt.
        self.require_approval
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let params = json!({ "name": self.def.name, "arguments": request.arguments });
        let result = self
            .transport
            .request("tools/call", params, self.budget, Some(&ctx.cancel))
            .await;
        match result {
            Ok(value) => {
                let text = join_content(&value);
                let is_error = value
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let content = if text.is_empty() {
                    "(MCP server returned no content)".to_string()
                } else {
                    text
                };
                if is_error {
                    ToolOutcome::err(format!(
                        "MCP server `{}` reported that tool `{}` failed: {content}",
                        self.server, self.name
                    ))
                } else {
                    ToolOutcome::ok(content)
                }
            }
            Err(e) => {
                if matches!(e, McpError::Transport(_) | McpError::Http { .. }) {
                    // Keep `GET /api/mcp` honest about a dead/unreachable server.
                    self.state.failed(e.describe());
                }
                ToolOutcome::err(e.tool_message(&self.server, &self.name, self.budget))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_namespaced() {
        assert_eq!(namespaced_name("fs", "read"), "mcp__fs__read");
    }

    #[test]
    fn missing_config_is_reported_not_panicked() {
        let manager = McpManager::new(&[
            McpServerConfig {
                name: "no-command".into(),
                transport: "stdio".into(),
                command: Vec::new(),
                url: None,
                env: Default::default(),
                headers: Default::default(),
                enabled: true,
                require_approval: true,
            },
            McpServerConfig {
                name: "no-url".into(),
                transport: "http".into(),
                command: Vec::new(),
                url: None,
                env: Default::default(),
                headers: Default::default(),
                enabled: true,
                require_approval: true,
            },
            McpServerConfig {
                name: "bad-transport".into(),
                transport: "carrier-pigeon".into(),
                command: Vec::new(),
                url: None,
                env: Default::default(),
                headers: Default::default(),
                enabled: true,
                require_approval: true,
            },
        ]);
        assert_eq!(manager.server_count(), 3);
        let statuses = manager.statuses();
        assert!(statuses.iter().all(|s| s.status == "failed"));
        assert!(statuses[0].error.as_deref().unwrap().contains("no command"));
        assert!(statuses[1].error.as_deref().unwrap().contains("no `url`"));
        assert!(statuses[2]
            .error
            .as_deref()
            .unwrap()
            .contains("unsupported transport"));
        assert!(manager.tools().is_empty());
    }

    #[test]
    fn disabled_servers_stay_disconnected() {
        let manager = McpManager::new(&[McpServerConfig {
            name: "off".into(),
            transport: "stdio".into(),
            command: vec!["sh".into()],
            url: None,
            env: Default::default(),
            headers: Default::default(),
            enabled: false,
            require_approval: true,
        }]);
        let statuses = manager.statuses();
        assert_eq!(statuses[0].status, "disconnected");
        assert!(statuses[0].error.is_none());
    }

    #[test]
    fn content_blocks_are_joined() {
        let value = json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "text", "text": "b"},
            {"type": "image", "data": "..."}
        ]});
        assert_eq!(join_content(&value), "a\nb\n[image content omitted]");
        assert_eq!(join_content(&json!({})), "");
    }

    #[test]
    fn tool_list_validation() {
        let ok = parse_tools(
            "s",
            json!({"tools": [{"name": "a", "inputSchema": {"type": "object"}}]}),
        )
        .unwrap();
        assert_eq!(ok.len(), 1);
        assert!(parse_tools("s", json!({"tools": [{"name": " "}]})).is_err());
        assert!(parse_tools("s", json!({"tools": [{"name": "a b"}]})).is_err());
        assert!(parse_tools("s", json!({"tools": "nope"})).is_err());
    }

    #[test]
    fn default_timeouts_match_the_documented_values() {
        let t = McpTimeouts::default();
        assert_eq!(t.connect, Duration::from_secs(15));
        assert_eq!(t.call, Duration::from_secs(120));
        let refs = TimeoutRefs::from(&t);
        assert_eq!(refs.call(), Duration::from_secs(120));
        refs.apply(&McpTimeouts {
            connect: Duration::from_millis(50),
            call: Duration::from_millis(60),
        });
        assert_eq!(refs.connect(), Duration::from_millis(50));
        assert_eq!(refs.call(), Duration::from_millis(60));
    }
}
