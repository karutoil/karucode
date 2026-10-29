//! SDK for out-of-process karucode plugins.
//!
//! A plugin is a normal executable that speaks newline-delimited JSON-RPC 2.0 on
//! stdin/stdout (protocol version [`PLUGIN_PROTOCOL_VERSION`], defined in
//! [`kc_core::plugin`] and shared by the host and this SDK). It never links the
//! harness: the only coupling is the wire protocol, so anything that can spawn a
//! process and write JSON can be a plugin.
//!
//! # Minimal plugin
//!
//! ```no_run
//! use kc_plugin_sdk::{
//!     hooks, PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest,
//! };
//! use serde_json::json;
//!
//! fn main() -> std::io::Result<()> {
//!     PluginBuilder::new("greeter", "0.1.0")
//!         .description("Greets people")
//!         .tool(
//!             "hello",
//!             "Greet someone.",
//!             json!({"type": "object", "properties": {"name": {"type": "string"}}}),
//!             |req: ToolRequest| {
//!                 let name = req.arguments.get("name").and_then(|v| v.as_str()).unwrap_or("world");
//!                 PluginToolResult::ok(format!("Hello, {name}!"))
//!             },
//!         )
//!         .hook(hooks::BEFORE_PROMPT, |_payload| json!("The greeter plugin is active."))
//!         .run()
//! }
//! ```
//!
//! # Rules
//!
//! * **stdout is the protocol.** One JSON object per line, nothing else. Write
//!   logs to stderr; the host drains them into its own log at debug level.
//! * Answer every request with the same `id`, either `result` or `error`.
//! * `initialize` must return a manifest whose `name` matches the name the host
//!   was configured with and whose `protocol` is `"1"`.
//! * Only advertise a hook you handle; the host only calls advertised hooks.
//! * Plugins run with the server's OS permissions. `permissions` is declarative
//!   and used by the host when deciding which tools to expose; it is not a
//!   sandbox.
//!
//! [`PluginBuilder`] implements the loop for you; [`Plugin::handle_request`] is
//! available if you want to embed the protocol in your own runtime.

// @kc-doc: plugins
// @kc-title: Write and install a plugin
// @kc-summary: Build an out-of-process plugin that adds tools and hooks, register it, and debug it when it fails to start.
// @kc-tags: plugins, extension, sdk, tools, hooks, jsonrpc
// @kc-applies-to: config:[[plugins]], file:~/.local/share/karucode/plugins/<name>/plugin.toml, dir:plugins/example-hello, tool:plugin__<plugin>__<tool>, cmd:cargo build -p example-hello, cmd:karucode plugin init, cmd:karucode plugin sdk, cmd:karucode plugin sdk --check, file:src/sdk.rs, file:src/protocol.rs, doc:docs/PLUGINS.md, env:KARUCODE_PLUGIN_NAME, env:KARUCODE_PLUGIN_PROTOCOL, env:KARUCODE_PLUGIN_PERMISSIONS, env:KARUCODE_PLUGIN_SETTINGS
//
// A plugin is a separate executable that speaks newline-delimited JSON-RPC 2.0 on
// stdin/stdout, protocol version `1`. It never links the harness, so it can be
// written in Rust with this SDK, in Python, in Go or in shell. The host spawns,
// supervises and restarts it; a crash or hang fails that plugin only.
//
// # 1. Write it with the Rust SDK
//
// ```bash
// karucode plugin init ./wordy     # the name comes from the directory
// karucode plugin init ./wordy --name wordy        # explicit name
// karucode plugin init ./wordy --force             # replace an existing scaffold
// cd wordy && cargo build --release
// ```
//
// The scaffolder writes the SDK **into** the plugin (`src/sdk.rs` and
// `src/protocol.rs` are copies of this build), so a Rust plugin depends on
// nothing from karucode: there is no path into a checkout and no published crate
// to wait for. Edit `src/main.rs`:
//
// ```rust
// use sdk::{PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest};
// use serde_json::json;
//
// fn main() -> std::io::Result<()> {
//     PluginBuilder::new("wordy", "0.1.0")
//         .description("Counts words")
//         .tool(
//             "count",
//             "Count the words in `text`.",
//             json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
//             |req: ToolRequest| {
//                 let text = req.str_arg("text");
//                 if text.is_empty() {
//                     return PluginToolResult::err("`text` is required");
//                 }
//                 PluginToolResult::ok(format!("{} words", text.split_whitespace().count()))
//             },
//         )
//         .hook(sdk::hooks::BEFORE_PROMPT, |_payload| json!("The wordy plugin is active."))
//         .run()
// }
// ```
//
// Add a tool with `.tool(…)`, a hook with `.hook(hooks::BEFORE_PROMPT, …)`, and
// declare what it touches with `.permission(permissions::FILES_WRITE)`. After
// upgrading karucode, refresh the vendored copy with `karucode plugin sdk <dir>`;
// `--check` exits non-zero when it is stale, writes nothing and starts no server,
// so a plugin's CI can guard it.
//
// Inside this repository `plugins/example-hello` is a workspace member and uses
// `kc-plugin-sdk` by relative path (`cargo build -p example-hello`). Copy its
// sources if you like, never its `Cargo.toml`: that path only resolves from
// inside this checkout.
//
// # 2. Register it
//
// Either in the config file:
//
// ```toml
// [[plugins]]
// name = "example-hello"            # must match the plugin's manifest name
// command = ["/abs/path/to/target/release/example-hello"]
// enabled = true
// permissions = ["files.read"]
// env = { RUST_LOG = "debug" }
// settings = { greeting = "hi" }
// ```
//
// or as a manifest the host discovers on disk at
// `<data_dir>/plugins/<name>/plugin.toml` (same keys, `command` required). A
// duplicate name fails the slot with an error naming both sources instead of
// silently running one of them.
//
// # 3. Run and inspect
//
// ```bash
// curl -s localhost:8787/api/plugins | jq        # status, tools, error, restart_count
// curl -s -X POST localhost:8787/api/plugins/example-hello/enable \
//      -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{"enabled":true}'
// ```
//
// Status is `stopped`, `starting`, `ready`, `failed` or `disabled`. Enabling resets
// the restart budget.
//
// # 4. Debug it without the server
//
// The protocol is just lines of JSON, so drive it by hand:
//
// ```bash
// printf '%s\n' \
//   '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"1"}}' \
//   '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
//   | ./target/release/example-hello
// ```
//
// # Permissions
//
// `permissions` is a declarative list (`files.read`, `files.write`, `shell`,
// `network`, `session.read`, or free-form). The **config entry is authoritative**:
// a manifest cannot grant itself a permission the operator did not list. A
// `mutating` tool without `files.write`, or a `shell` tool without `shell`, is not
// exposed at all.
//
// It is **not** a sandbox. The plugin runs with the server's OS permissions and
// can do anything that user can do. Install plugins you trust, exactly like shell
// scripts.
//
// # Gotchas
//
// * stdout is the protocol. Anything else printed there is logged and ignored;
//   logs belong on stderr.
// * `initialize` must report the configured name and protocol `"1"`, or the
//   plugin fails with an error naming both values.
// * A dead process is restarted once per failed call, at most three times per
//   plugin per server lifetime; after that calls fail with an explanatory message
//   until you fix it or restart the server.
// * There is no hot reload: replacing the binary requires disabling and enabling
//   the plugin (or a server restart).
// * Results are truncated and rendered as text; a plugin cannot inject UI.
//
// Full reference: `docs/PLUGINS.md` (wire protocol, hooks, timeouts, error codes).

// Everything below the marker is copied verbatim into a scaffolded plugin by
// `karucode plugin init` (see `src/embed.rs`). Keep it free of harness-only
// dependencies, and refer to the wire types as `kc_core::plugin` — the emitter
// rewrites that path to the plugin's own `crate::protocol` module.
mod embed;
pub use embed::{
    files, name_from_path, protocol_source, sdk_source, validate_name, EmbedError,
    SDK_SOURCE_MARKER,
};

// ---- BEGIN EMBEDDED SDK SOURCE ----
pub use kc_core::plugin::*;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

/// One `tools/call` invocation handed to a tool handler.
pub struct ToolRequest {
    pub name: String,
    pub arguments: Value,
}

impl ToolRequest {
    /// String argument (empty string when absent).
    pub fn str_arg(&self, key: &str) -> &str {
        self.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }

    /// Integer argument (0 when absent or not an integer).
    pub fn int_arg(&self, key: &str) -> i64 {
        self.arguments
            .get(key)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }
}

/// Constructors for [`PluginToolResult`].
///
/// `PluginToolResult` is a wire type shared with the host, so the ergonomic
/// constructors arrive as an extension trait instead of inherent methods. Bring
/// it into scope (`use kc_plugin_sdk::PluginToolResultExt;`) and
/// `PluginToolResult::ok("done")` works.
pub trait PluginToolResultExt: Sized {
    /// Successful result.
    fn ok(content: impl Into<String>) -> Self;
    /// Failed result: the harness shows `content` as the failure reason. It is
    /// not a transport error, so the plugin stays connected.
    fn err(content: impl Into<String>) -> Self;
    /// Replace the content of an existing result.
    fn content(self, content: impl Into<String>) -> Self;
    /// Attach structured metadata (echoed back in `ToolOutcome::meta`).
    fn with_meta(self, meta: Value) -> Self;
}

impl PluginToolResultExt for PluginToolResult {
    fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            meta: None,
        }
    }

    fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            meta: None,
        }
    }

    fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    fn with_meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }
}

type ToolHandler = Box<dyn Fn(ToolRequest) -> PluginToolResult + Send + Sync>;
type HookHandler = Box<dyn Fn(Value) -> Value + Send + Sync>;

struct ToolEntry {
    def: PluginToolDef,
    handler: ToolHandler,
}

/// A plugin assembled by [`PluginBuilder`], ready to serve requests.
pub struct Plugin {
    manifest: PluginManifest,
    tools: BTreeMap<String, ToolEntry>,
    hooks: BTreeMap<String, HookHandler>,
    shutdown: bool,
}

impl Plugin {
    /// Manifest reported to the host by `initialize`.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// True once a `shutdown` request was handled.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Handle one decoded JSON-RPC message.
    ///
    /// Returns the response object for requests, and `None` for notifications
    /// (no `id`). This is the whole protocol surface; [`Plugin::run`] is just
    /// stdin/stdout plumbing around it.
    pub fn handle_request(&mut self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (e.g. `notifications/initialized`) get no response.
        let id = id?;

        let response = match method.as_str() {
            methods::INITIALIZE => Ok(json!(self.manifest)),
            methods::TOOLS_LIST => {
                let list: Vec<PluginToolDef> = self.tools.values().map(|t| t.def.clone()).collect();
                Ok(json!({ "tools": list }))
            }
            methods::TOOLS_CALL => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match self.tools.get(&name) {
                    Some(entry) => {
                        let result = (entry.handler)(ToolRequest {
                            name: name.clone(),
                            arguments,
                        });
                        Ok(serde_json::to_value(result).unwrap_or_else(
                            |_| json!({"ok": false, "content": "serialisation error"}),
                        ))
                    }
                    None => Err(PluginError::new(-32601, format!("unknown tool `{name}`"))),
                }
            }
            methods::HOOKS_RUN => {
                let hook = params
                    .get("hook")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                match self.hooks.get(&hook) {
                    Some(handler) => Ok(json!({ "output": handler(payload) })),
                    None => Ok(json!({ "output": null })),
                }
            }
            methods::SHUTDOWN => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            other => Err(PluginError::new(
                -32601,
                format!("unknown method `{other}`"),
            )),
        };

        Some(match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        })
    }

    /// Serve requests on stdin/stdout until EOF or `shutdown`.
    pub fn run(&mut self) -> std::io::Result<()> {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut reader = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let request: Value = match serde_json::from_str(trimmed) {
                Ok(value) => value,
                Err(e) => {
                    // A malformed line is not recoverable per-request (we cannot
                    // know its id), so report and keep going.
                    let error = json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": {"code": -32700, "message": format!("parse error: {e}")},
                    });
                    writeln!(stdout, "{error}")?;
                    stdout.flush()?;
                    continue;
                }
            };
            let response = self.handle_request(request);
            if let Some(response) = response {
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
            }
            if self.shutdown {
                return Ok(());
            }
        }
    }
}

/// Builds a [`Plugin`]: declares the manifest, the tools and the hooks.
pub struct PluginBuilder {
    manifest: PluginManifest,
    tools: BTreeMap<String, ToolEntry>,
    hooks: BTreeMap<String, HookHandler>,
}

impl PluginBuilder {
    /// Start a plugin that reports `name`/`version` in its manifest. The name
    /// must match the `[[plugins]].name` (or `plugin.toml` name) the host is
    /// configured with.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            manifest: PluginManifest {
                name: name.into(),
                version: version.into(),
                description: String::new(),
                protocol: PLUGIN_PROTOCOL_VERSION.into(),
                capabilities: vec!["tools".into()],
                permissions: Vec::new(),
                hooks: Vec::new(),
                tool_prefix: None,
            },
            tools: BTreeMap::new(),
            hooks: BTreeMap::new(),
        }
    }

    /// Human-readable description shown in diagnostics.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.manifest.description = description.into();
        self
    }

    /// Declare a permission (`files.read`, `files.write`, `shell`, `network`,
    /// `session.read`). Declarative: the host uses it to decide which tools to
    /// expose. It is **not** an OS sandbox.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.manifest.permissions.push(permission.into());
        self
    }

    /// Declare a capability (free-form; `tools` is declared by default).
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        let capability = capability.into();
        if !self.manifest.capabilities.contains(&capability) {
            self.manifest.capabilities.push(capability);
        }
        self
    }

    /// Register a tool with the default class (`external`, which always needs
    /// approval).
    pub fn tool(
        self,
        name: &str,
        description: &str,
        parameters: Value,
        handler: impl Fn(ToolRequest) -> PluginToolResult + Send + Sync + 'static,
    ) -> Self {
        self.tool_with_class(name, description, parameters, None, handler)
    }

    /// Register a tool with an explicit side-effect class:
    /// `read_only`, `mutating`, `shell` or `external`.
    ///
    /// `mutating` needs the `files.write` permission and `shell` needs the
    /// `shell` permission before the host exposes the tool.
    pub fn tool_with_class(
        mut self,
        name: &str,
        description: &str,
        parameters: Value,
        class: Option<&str>,
        handler: impl Fn(ToolRequest) -> PluginToolResult + Send + Sync + 'static,
    ) -> Self {
        self.tools.insert(
            name.to_string(),
            ToolEntry {
                def: PluginToolDef {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters,
                    class: class.map(|c| c.to_string()),
                },
                handler: Box::new(handler),
            },
        );
        self
    }

    /// Register a lifecycle hook. Only hooks listed in
    /// [`kc_core::plugin::hooks::IMPLEMENTED`] are called by the host; the
    /// others are reserved. The handler's return value becomes the hook's
    /// `output`.
    pub fn hook(
        mut self,
        name: &str,
        handler: impl Fn(Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        if !self.manifest.hooks.iter().any(|h| h == name) {
            self.manifest.hooks.push(name.to_string());
        }
        self.hooks.insert(name.to_string(), Box::new(handler));
        self
    }

    /// Finish building without serving.
    pub fn build(self) -> Plugin {
        Plugin {
            manifest: self.manifest,
            tools: self.tools,
            hooks: self.hooks,
            shutdown: false,
        }
    }

    /// Serve requests on stdin/stdout until EOF or `shutdown`.
    pub fn run(self) -> std::io::Result<()> {
        self.build().run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin() -> Plugin {
        PluginBuilder::new("demo", "1.2.3")
            .description("test plugin")
            .permission(permissions::FILES_READ)
            .tool(
                "echo",
                "Echo.",
                json!({"type": "object", "properties": {"text": {"type": "string"}}}),
                |req: ToolRequest| PluginToolResult::ok(format!("echo: {}", req.str_arg("text"))),
            )
            .tool_with_class("deploy", "Deploy.", json!({}), Some("shell"), |_req| {
                PluginToolResult::err("not today")
            })
            .hook(hooks::BEFORE_PROMPT, |_payload| json!("demo note"))
            .build()
    }

    #[test]
    fn manifest_reflects_the_builder() {
        let plugin = plugin();
        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.description, "test plugin");
        assert_eq!(manifest.protocol, PLUGIN_PROTOCOL_VERSION);
        assert_eq!(manifest.permissions, vec!["files.read".to_string()]);
        assert_eq!(manifest.hooks, vec!["before_prompt".to_string()]);
        assert!(manifest.capabilities.contains(&"tools".to_string()));
    }

    #[test]
    fn initialize_returns_the_manifest() {
        let mut plugin = plugin();
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                                   "params": {"protocol": "1"}}))
            .unwrap();
        assert_eq!(response["id"], json!(1));
        assert_eq!(response["result"]["name"], json!("demo"));
    }

    #[test]
    fn tools_list_is_sorted_and_stable() {
        let mut plugin = plugin();
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["deploy", "echo"]);
        // The declared class travels with the definition.
        assert_eq!(response["result"]["tools"][0]["class"], json!("shell"));
        assert!(response["result"]["tools"][1]["class"].is_null());
    }

    #[test]
    fn tools_call_success_and_failure() {
        let mut plugin = plugin();
        let ok = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                   "params": {"name": "echo", "arguments": {"text": "hi"}}}))
            .unwrap();
        assert_eq!(ok["result"]["ok"], json!(true));
        assert_eq!(ok["result"]["content"], json!("echo: hi"));

        let failed = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                                   "params": {"name": "deploy", "arguments": {}}}))
            .unwrap();
        assert_eq!(failed["result"]["ok"], json!(false));
        assert_eq!(failed["result"]["content"], json!("not today"));
    }

    #[test]
    fn unknown_tool_and_method_are_jsonrpc_errors() {
        let mut plugin = plugin();
        let unknown_tool = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                                   "params": {"name": "nope", "arguments": {}}}))
            .unwrap();
        assert_eq!(unknown_tool["error"]["code"], json!(-32601));
        assert!(unknown_tool["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool `nope`"));

        let unknown_method = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 6, "method": "nope"}))
            .unwrap();
        assert_eq!(unknown_method["error"]["code"], json!(-32601));
    }

    #[test]
    fn hooks_return_output_and_unknown_hooks_return_null() {
        let mut plugin = plugin();
        let known = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 7, "method": "hooks/run",
                                   "params": {"hook": "before_prompt", "payload": {}}}))
            .unwrap();
        assert_eq!(known["result"]["output"], json!("demo note"));

        let unknown = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 8, "method": "hooks/run",
                                   "params": {"hook": "on_run_end", "payload": {}}}))
            .unwrap();
        assert!(unknown["result"]["output"].is_null());
    }

    #[test]
    fn notifications_get_no_response_and_shutdown_stops_the_loop() {
        let mut plugin = plugin();
        assert!(plugin
            .handle_request(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none());
        assert!(!plugin.is_shutdown());
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 9, "method": "shutdown"}))
            .unwrap();
        assert!(response["result"].is_null());
        assert!(plugin.is_shutdown());
    }

    #[test]
    fn result_constructors_are_complete() {
        let ok = PluginToolResult::ok("fine").with_meta(json!({"n": 1}));
        assert!(ok.ok);
        assert_eq!(ok.content, "fine");
        assert_eq!(ok.meta, Some(json!({"n": 1})));
        let err = PluginToolResult::err("broken");
        assert!(!err.ok);
        assert!(err.meta.is_none());
    }

    #[test]
    fn tool_request_argument_helpers() {
        let req = ToolRequest {
            name: "t".into(),
            arguments: json!({"text": "hi", "n": 4}),
        };
        assert_eq!(req.str_arg("text"), "hi");
        assert_eq!(req.str_arg("missing"), "");
        assert_eq!(req.int_arg("n"), 4);
        assert_eq!(req.int_arg("missing"), 0);
    }
}
