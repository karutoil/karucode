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
// # Environment
//
// A stdio server does **not** inherit karucode's environment. It gets a cleared
// environment plus an allowlist (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
// `LANG`, `LC_ALL`, `TERM`, `TMPDIR`, `PWD`, `TZ`, each only when set), then the
// server's own `[mcp_servers.env]` table, which wins over the allowlist.
//
// Inherited secrets are never passed on: `KARUCODE_*`, `AWS_*`, `OPENAI_*`,
// `ANTHROPIC_*`, `GITHUB_TOKEN`, `GH_TOKEN`, `NPM_TOKEN` and names ending in
// `_API_KEY`, `_TOKEN`, `_SECRET`, `_PASSWORD` are dropped. This matters because
// `npx` servers and their `postinstall` scripts run arbitrary code at startup.
// The `env` table is *not* filtered: a credential an operator configures for a
// specific server is intentional, and that is how a server receives its own key.
// Every configured header and env **value** is nevertheless treated as secret on
// the way back: connection/status diagnostics, transport/JSON-RPC errors and
// successful or `isError` tool content are scrubbed even if the peer reflects the
// value (including a rewritten Bearer scheme). Dropping a live `McpManager` sends
// a synchronous kill signal to each stdio process group; explicit `shutdown()`
// awaits the retained supervisor so the actual child is reaped before it returns.
//
// HTTP errors never reproduce the configured URL: URL userinfo/query may contain
// credentials. Reqwest diagnostics have their URL field removed before they are
// formatted, so invalid-scheme, non-2xx and connection errors expose no userinfo.
// A JSON response with one wrong or explicit-null id is rejected; the only
// compatibility exception is a sole response whose id key is genuinely absent.
//
// # Limits
//
// Connect plus handshake is capped at 15 s per server; one `tools/call` has 120 s.
// Protocol lines and response bodies are capped at 8 MiB, and exceeding that is an
// explicit error rather than unbounded buffering. Servers connect concurrently, so
// one broken server never delays startup or the others. SSE retains raw bytes until
// a complete UTF-8 line arrives and drains the consumed prefix once per network
// chunk, so many short lines take linear work rather than quadratic copying.
//
// A tool result larger than `limits.max_tool_output_bytes` (24000 by default) is
// cut by karucode on its way to the model, not by the server: the text ends with a
// marker stating how many bytes and lines were dropped and naming that key. MCP
// has no argument karucode can narrow for you, so a server whose tool returns
// something huge should page it itself (take its own `offset`/`limit`) or return a
// file path the agent can open with `read_file`.
//
// # Not supported
//
// Server-to-client requests (sampling, roots, elicitation), MCP resources/prompts
// (tools only), subscriptions/tool-list-change notifications, legacy two-endpoint
// HTTP+SSE, OAuth flows, and automatic reconnect.
//
// # Gotchas
//
// * A tool-list change on a connected server does not refresh the registry by
//   itself: `POST /api/mcp/:name/reload` (or `mcp_watch`) replaces the connection
//   and re-reads the tools.
// * A failed server stays failed until it is reloaded; there is no health-check
//   loop, but `POST /api/mcp/reload` retries an unchanged entry whose connection
//   is not alive.
// * A missing `command` or `url` fails that slot at startup with an explanatory
//   error - it does not crash the server.
// * `npx` servers are downloaded on first use; on an air-gapped host pre-install
//   them and point `command` at the local binary.
//
// Full reference: `docs/MCP.md` (wire details, failure mapping, the full
// not-implemented list).

// @kc-doc: mcp-hot-reload
// @kc-title: Reload MCP servers without a restart
// @kc-summary: Apply an added, removed or edited [[mcp_servers]] entry with a reload route or the mcp_watch timer instead of restarting karucode.
// @kc-tags: mcp, reload, restart, hot reload, watch, config
// @kc-applies-to: config:[[mcp_servers]], config:mcp_watch, config:mcp_watch_secs, env:KARUCODE_MCP_WATCH, env:KARUCODE_MCP_WATCH_SECS, route:POST /api/mcp/reload, route:POST /api/mcp/:name/reload, route:GET /api/mcp, ui:Settings > Plugins & MCP, doc:docs/MCP.md
//
// Editing `[[mcp_servers]]` does not require restarting karucode. The config file
// is re-read on demand, and the running connections are reconciled with it.
//
// # Apply an edit now
//
// ```bash
// # after adding, removing or editing an entry in ~/.config/karucode/config.toml
// curl -s -X POST localhost:8787/api/mcp/reload \
//   -H "X-Karucode-Admin-Token: $KARUCODE_ADMIN_TOKEN" | jq
// ```
//
// The answer names what happened, in configuration order:
//
// ```json
// {
//   "report": {
//     "added": ["filesystem"],
//     "removed": ["old-server"],
//     "restarted": ["github"],
//     "retried": [],
//     "unchanged": ["remote-tools"]
//   },
//   "servers": [ { "name": "filesystem", "status": "connected", "tools": ["mcp__filesystem__read_file"] } ]
// }
// ```
//
// * `added` - a new name was started.
// * `removed` - the entry disappeared; its process was stopped and its tools are gone.
// * `restarted` - the entry changed; the old connection was closed and a new one opened.
// * `retried` - the entry did not change but its connection was not alive; it was rebuilt.
// * `unchanged` - left running exactly as it was.
//
// An empty `[[mcp_servers]]` table means **no servers**: deleting the last entry
// stops it, so there is no "keep what we had" fallback. A config file that cannot
// be read or parsed keeps the servers already running and logs a warning.
//
// # One server
//
// ```bash
// # after rebuilding a server binary or editing one entry
// curl -s -X POST localhost:8787/api/mcp/filesystem/reload \
//   -H "X-Karucode-Admin-Token: $KARUCODE_ADMIN_TOKEN" | jq
// ```
//
// This closes that server's connection and starts it again from the current
// entry, whether or not the entry changed. A name that is configured but not
// running is started; an unknown name is `404 not_found`.
//
// # Apply edits automatically
//
// ```toml
// mcp_watch = true          # KARUCODE_MCP_WATCH=1
// mcp_watch_secs = 2        # KARUCODE_MCP_WATCH_SECS, clamped to 1..=600
// ```
//
// With `mcp_watch` on, the server stats the config file on that interval and
// applies a change once two polls agree, so a truncate-then-write is not read as
// an empty server list. It is off by default because a reload replaces
// processes. The same switch exists for plugins (`plugins_watch`).
//
// # What still needs care
//
// * A run already in flight keeps the tool list it started with; the new set
//   applies to the next run. No new session is needed.
// * A tool-list change on a connected server needs a reload: karucode does not
//   subscribe to `list_changed` notifications.
// * `require_approval`, the `external` class and secret scrubbing are unchanged
//   by a reload: a hot-added server is exactly as gated as one read at startup.
// * `POST` routes need the admin credential (an API key with `admin` scope or the
//   admin token); `GET /api/mcp` is readable with `read` scope.
//
// Full reference: `docs/MCP.md`.

use async_trait::async_trait;
use kc_core::config::McpServerConfig;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, warn};

mod http;
mod procgroup;
mod rpc;
mod stdio;
mod transport;

use rpc::{redact_secrets, McpError};
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

/// What one [`McpManager::rescan`] changed, in configuration order.
///
/// Serialised by `POST /api/mcp/reload` so the caller can log or show what
/// happened. `retried` is separate from `restarted` on purpose: an unchanged
/// entry whose connection failed is retried, and calling that a restart would
/// hide the fact that the operator changed nothing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpRescanReport {
    /// Servers that did not exist before and whose process/connection was started.
    pub added: Vec<String>,
    /// Servers whose entry disappeared; their process was stopped and their tools
    /// are gone.
    pub removed: Vec<String>,
    /// Servers whose entry changed; the old connection was closed and a new one
    /// opened from the new entry.
    pub restarted: Vec<String>,
    /// Servers whose entry did not change but whose connection was not alive; the
    /// connection was rebuilt and the handshake attempted again.
    pub retried: Vec<String>,
    /// Servers left exactly as they were.
    pub unchanged: Vec<String>,
}

impl McpRescanReport {
    /// Whether anything was added, removed, replaced or retried.
    pub fn changed(&self) -> bool {
        !(self.added.is_empty()
            && self.removed.is_empty()
            && self.restarted.is_empty()
            && self.retried.is_empty())
    }
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

/// Every value the operator deliberately gave this server and a remote peer can
/// therefore reflect. Prefixing keys avoids an env var accidentally replacing a
/// header with the same name: redaction only reads the values.
fn configured_secrets(config: &McpServerConfig) -> BTreeMap<String, String> {
    let mut secrets: BTreeMap<String, String> = config
        .headers
        .iter()
        .map(|(name, value)| (format!("header:{name}"), value.clone()))
        .collect();
    secrets.extend(
        config
            .env
            .iter()
            .map(|(name, value)| (format!("env:{name}"), value.clone())),
    );
    secrets
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
        let servers = configs.iter().map(Self::slot_for).collect();
        Self { servers, timeouts }
    }

    /// One slot for one entry: transport built, setup error recorded, nothing
    /// started. Shared by [`new`](Self::new), [`rescan`](Self::rescan) and
    /// [`reload`](Self::reload) so a hot-added server is built exactly like one
    /// read at startup.
    fn slot_for(config: &McpServerConfig) -> ServerSlot {
        let state = ServerState::new();
        let (transport, setup_error) = build_transport(config);
        if let Some(err) = &setup_error {
            warn!(server = %config.name, "MCP server unavailable: {err}");
            state.failed(err.clone());
        }
        ServerSlot {
            config: config.clone(),
            transport,
            state,
            setup_error,
        }
    }

    /// Close one slot's transport and mark it disconnected. Used by
    /// [`rescan`](Self::rescan), [`reload`](Self::reload) and
    /// [`shutdown`](Self::shutdown) so a departing process is reaped the same way
    /// everywhere.
    async fn close_slot(slot: &ServerSlot) {
        if let Some(transport) = &slot.transport {
            transport.close().await;
        }
        slot.state.disconnected();
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
        self.connect_slots((0..self.servers.len()).collect()).await;
    }

    /// Handshake with the slots at `indices` concurrently.
    ///
    /// The indices are resolved against the current slot list, so a caller that
    /// just replaced slots (a rescan or a single reload) can start exactly the
    /// ones that need starting without a second pass over the config.
    async fn connect_slots(&self, indices: Vec<usize>) {
        let mut set = JoinSet::new();
        for index in indices {
            let Some(slot) = self.servers.get(index) else {
                continue;
            };
            if !slot.config.enabled || slot.setup_error.is_some() {
                continue;
            }
            let Some(transport) = slot.transport.clone() else {
                continue;
            };
            let state = Arc::clone(&slot.state);
            let timeouts = Arc::clone(&self.timeouts);
            let name = slot.config.name.clone();
            let secrets = configured_secrets(&slot.config);
            set.spawn(
                async move { connect_server(name, transport, state, timeouts, secrets).await },
            );
        }
        while let Some(joined) = set.join_next().await {
            if let Err(e) = joined {
                warn!("MCP connect task failed: {e}");
            }
        }
    }

    /// Reconcile the running servers with `configs` without restarting karucode.
    ///
    /// This is what makes editing `[[mcp_servers]]` a normal edit instead of a
    /// reboot:
    ///
    /// * a name that is new is added and started (hot **load**);
    /// * a name that disappeared is stopped and dropped (hot **unload**);
    /// * an entry whose fields changed is closed and started again from the new
    ///   entry (hot **reload**);
    /// * an unchanged entry whose connection is not alive is retried, because a
    ///   reload is the operator asking for another attempt;
    /// * everything else is left running untouched, so a rescan of an unchanged
    ///   file does not disturb a single process.
    ///
    /// A deterministic setup error (no `command`, no `url`, unsupported
    /// transport) is not retried while the entry is unchanged: the config would
    /// have to change for the outcome to differ.
    ///
    /// Call the server's `reload_tools` afterwards so the registry exposes the new
    /// set - the same contract as the plugin host's `rescan`.
    pub async fn rescan(&mut self, configs: &[McpServerConfig]) -> McpRescanReport {
        let mut report = McpRescanReport::default();
        let mut previous: Vec<Option<ServerSlot>> = self.servers.drain(..).map(Some).collect();
        let mut next: Vec<ServerSlot> = Vec::with_capacity(configs.len());
        let mut starts: Vec<usize> = Vec::new();
        let mut stops: Vec<ServerSlot> = Vec::new();

        for config in configs {
            let existing = previous
                .iter_mut()
                .find(|slot| {
                    slot.as_ref()
                        .is_some_and(|slot| slot.config.name == config.name)
                })
                .and_then(|slot| slot.take());
            let Some(old) = existing else {
                let slot = Self::slot_for(config);
                if slot.config.enabled && slot.setup_error.is_none() {
                    starts.push(next.len());
                }
                report.added.push(config.name.clone());
                next.push(slot);
                continue;
            };
            if old.config == *config {
                let running = old.config.enabled
                    && old.setup_error.is_none()
                    && old.state.is_connected()
                    && old
                        .transport
                        .as_ref()
                        .is_some_and(|transport| transport.alive());
                if running || old.setup_error.is_some() || !old.config.enabled {
                    report.unchanged.push(config.name.clone());
                    next.push(old);
                    continue;
                }
                stops.push(old);
                let slot = Self::slot_for(config);
                starts.push(next.len());
                report.retried.push(config.name.clone());
                next.push(slot);
                continue;
            }
            // The entry changed under a running process: the old process is
            // executing what the operator has already replaced, so it is closed
            // and a fresh slot is built from the new entry.
            stops.push(old);
            let slot = Self::slot_for(config);
            if slot.config.enabled && slot.setup_error.is_none() {
                starts.push(next.len());
            }
            report.restarted.push(config.name.clone());
            next.push(slot);
        }

        for slot in previous.into_iter().flatten() {
            report.removed.push(slot.config.name.clone());
            stops.push(slot);
        }

        // Stop before start: a replacement must not race its predecessor for the
        // same name, port, lock file or socket.
        for slot in &stops {
            Self::close_slot(slot).await;
        }
        self.servers = next;
        self.connect_slots(starts).await;
        report
    }

    /// Replace one server's slot from `configs`, whether or not its entry changed.
    ///
    /// This is the "I rebuilt the server binary" / "I edited its entry" path: the
    /// old process is closed and a fresh one is started from the current entry.
    /// `Err(NotFound)` means the name is not in `configs` at all - a name that is
    /// configured but not yet in the manager is added, so a reload can also be the
    /// first start.
    pub async fn reload(
        &mut self,
        name: &str,
        configs: &[McpServerConfig],
    ) -> kc_core::Result<McpServerStatus> {
        let config = configs
            .iter()
            .find(|config| config.name == name)
            .ok_or_else(|| {
                kc_core::Error::NotFound(format!("MCP server `{name}` is not configured"))
            })?;
        let index = self
            .servers
            .iter()
            .position(|slot| slot.config.name == name);
        if let Some(old) = index.map(|index| self.servers.remove(index)) {
            Self::close_slot(&old).await;
        }
        let slot = Self::slot_for(config);
        let connect = slot.config.enabled && slot.setup_error.is_none();
        let index = match index {
            Some(index) => {
                self.servers.insert(index, slot);
                index
            }
            None => {
                self.servers.push(slot);
                self.servers.len() - 1
            }
        };
        if connect {
            self.connect_slots(vec![index]).await;
        }
        self.statuses()
            .into_iter()
            .find(|status| status.name == name)
            .ok_or_else(|| {
                kc_core::Error::NotFound(format!("MCP server `{name}` is not configured"))
            })
    }

    /// Tools of every connected server.
    ///
    /// # Name collisions
    ///
    /// `mcp__<server>__<tool>` only guarantees no clash with a first-party tool.
    /// Two servers can still produce the same string — a server `a` exposing a
    /// tool `b__c` and a server `a__b` exposing a tool `c` both yield
    /// `mcp__a__b__c` — and the tool registry is a `HashMap`, so an insert would
    /// silently drop one server's tool with no diagnostic anywhere.
    ///
    /// The first server in configuration order keeps the name; a later claimant
    /// is refused, logged, and reported in that server's [`McpServerStatus::error`]
    /// (see [`McpManager::collisions`]). Refusing rather than suffixing keeps a
    /// tool name's meaning stable for callers and for the approval record.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
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
                let name = namespaced_name(&slot.config.name, &def.name);
                if let Some(owner) = seen.get(&name) {
                    warn!(
                        server = %slot.config.name,
                        tool = %def.name,
                        name = %name,
                        claimed_by = %owner,
                        "not exposing MCP tool: `{name}` is already provided by server \
                         `{owner}`; two servers produced the same namespaced tool name (the \
                         `mcp__` prefix only prevents collisions with first-party tools). \
                         Rename the server or the tool"
                    );
                    continue;
                }
                seen.insert(name, slot.config.name.clone());
                out.push(Arc::new(McpTool {
                    server: slot.config.name.clone(),
                    name: namespaced_name(&slot.config.name, &def.name),
                    def,
                    transport: Arc::clone(&transport),
                    state: Arc::clone(&slot.state),
                    budget: self.timeouts.call(),
                    secrets: configured_secrets(&slot.config),
                    require_approval: slot.config.require_approval,
                }));
            }
        }
        out
    }

    /// Namespaced tool names claimed by more than one server, keyed by the
    /// colliding **name**: `name -> (owner server, later claimant servers)`.
    ///
    /// The tool name is refused in [`tools`](Self::tools); this is how that
    /// refusal reaches a human, through [`statuses`](Self::statuses).
    pub fn collisions(&self) -> BTreeMap<String, (String, Vec<String>)> {
        let mut claims: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        for slot in &self.servers {
            if !slot.state.is_connected() {
                continue;
            }
            for def in slot.state.tool_defs() {
                let name = namespaced_name(&slot.config.name, &def.name);
                match claims.get_mut(&name) {
                    Some((_, claimants)) => {
                        if !claimants.contains(&slot.config.name) {
                            claimants.push(slot.config.name.clone());
                        }
                    }
                    None => {
                        claims.insert(name, (slot.config.name.clone(), Vec::new()));
                    }
                }
            }
        }
        claims.retain(|_, (_, claimants)| !claimants.is_empty());
        claims
    }

    /// Status of every configured server (connected | failed | disconnected).
    pub fn statuses(&self) -> Vec<McpServerStatus> {
        let collisions = self.collisions();
        self.servers
            .iter()
            .map(|slot| {
                if let Some(err) = &slot.setup_error {
                    return McpServerStatus {
                        name: slot.config.name.clone(),
                        transport: slot.config.transport.clone(),
                        status: ServerStatus::Failed.as_str().into(),
                        tools: Vec::new(),
                        error: Some(redact_secrets(err, &configured_secrets(&slot.config))),
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
                // A refused tool name is a diagnostic the operator must see, so
                // it goes in `error` (the only free-text field the API exposes)
                // without changing the status: the server itself is fine.
                if let Some((name, (owner, _))) = collisions
                    .iter()
                    .find(|(_, (_, claimants))| claimants.contains(&slot.config.name))
                {
                    let note = format!(
                        "not exposing `{name}`: the name is already provided by server \
                         `{owner}`. Two servers produced the same namespaced tool name \
                         (`mcp__` only prevents collisions with first-party tools), so this one \
                         was refused rather than silently replacing the other. Rename the server \
                         or the tool"
                    );
                    error = Some(match error.take() {
                        Some(existing) => format!("{existing}; {note}"),
                        None => note,
                    });
                }
                McpServerStatus {
                    name: slot.config.name.clone(),
                    transport: slot.config.transport.clone(),
                    status: status.as_str().into(),
                    tools,
                    error: error
                        .map(|message| redact_secrets(&message, &configured_secrets(&slot.config))),
                }
            })
            .collect()
    }

    /// Close every connection and reap every child process.
    pub async fn shutdown(&mut self) {
        for slot in &self.servers {
            Self::close_slot(slot).await;
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for slot in &self.servers {
            if let Some(transport) = &slot.transport {
                transport.close_now();
            }
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
    secrets: BTreeMap<String, String>,
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
        list_all_tools(transport.as_ref(), &name, budget).await
    };

    match tokio::time::timeout(budget, attempt).await {
        Ok(Ok(tools)) => {
            debug!(server = %name, tools = tools.len(), "MCP server connected");
            state.connected(tools);
        }
        Ok(Err(e)) => {
            let message = redact_secrets(&e.describe(), &secrets);
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

/// Fetch every tools/list page under one wall-clock budget. A server controls the
/// cursor sequence, so each request gets only the time remaining and a repeated
/// cursor fails discovery instead of looping forever.
async fn list_all_tools(
    transport: &dyn Transport,
    server: &str,
    budget: Duration,
) -> Result<Vec<McpToolDef>, McpError> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut tools = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(McpError::Timeout(budget));
        }
        let params = match cursor.as_deref() {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let page = transport
            .request("tools/list", params, remaining, None)
            .await?;
        tools.extend(parse_tools(server, page.clone())?);
        let next = match page.get("nextCursor") {
            None | Some(Value::Null) => return Ok(tools),
            Some(Value::String(next)) => next,
            Some(_) => {
                return Err(McpError::Protocol(
                    "malformed `tools/list` result: `nextCursor` must be a string".into(),
                ))
            }
        };
        if !seen.insert(next.clone()) {
            return Err(McpError::Protocol(format!(
                "MCP server `{server}` returned a cyclic `tools/list` nextCursor; discovery was                  stopped before requesting the same page again"
            )));
        }
        cursor = Some(next.clone());
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
    /// Configured header and environment values are scrubbed again at the
    /// tool/model boundary, including a server reporting `isError` with a
    /// credential reflected in its content.
    secrets: BTreeMap<String, String>,
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
                let content = redact_secrets(&content, &self.secrets);
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
                    self.state
                        .failed(redact_secrets(&e.describe(), &self.secrets));
                }
                ToolOutcome::err(redact_secrets(
                    &e.tool_message(&self.server, &self.name, self.budget),
                    &self.secrets,
                ))
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
    fn headers_and_env_values_with_the_same_name_are_both_redacted() {
        let config = McpServerConfig {
            name: "secrets".into(),
            transport: "stdio".into(),
            command: vec!["sh".into()],
            url: None,
            env: BTreeMap::from([("Authorization".into(), "env-secret".into())]),
            headers: BTreeMap::from([("Authorization".into(), "header-secret".into())]),
            enabled: true,
            require_approval: true,
        };
        let secrets = configured_secrets(&config);
        let output = redact_secrets("header-secret then env-secret", &secrets);
        assert!(!output.contains("header-secret"));
        assert!(!output.contains("env-secret"));
        assert_eq!(secrets.len(), 2);
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
