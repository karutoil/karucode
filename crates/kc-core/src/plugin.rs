//! Plugin and MCP protocol types (versioned, JSON-shaped).
//!
//! Plugins are separate processes speaking newline-delimited JSON-RPC 2.0 over
//! stdin/stdout. The core supervises them, so a panicking or hanging plugin
//! cannot take the harness down. This is *process* isolation, not a sandbox: a
//! plugin runs with the server's OS permissions.

use serde::{Deserialize, Serialize};

pub const PLUGIN_PROTOCOL_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "protocol_version")]
    pub protocol: String,
    /// Capabilities the plugin provides.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Permissions the plugin declares; the core records them and refuses tools
    /// whose origin was not declared. Not an OS sandbox.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Lifecycle hooks the plugin wants to receive.
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub tool_prefix: Option<String>,
    /// Panels this plugin contributes to the web UI's inspector.
    ///
    /// Every field is defaulted, so a manifest that declares none keeps working
    /// unchanged: the panel capability is additive, not a protocol bump.
    #[serde(default)]
    pub panels: Vec<PluginPanel>,
}

fn protocol_version() -> String {
    PLUGIN_PROTOCOL_VERSION.into()
}

/// One panel a plugin contributes to the inspector.
///
/// The host never interprets [`PluginPanelData::data`]: it is opaque JSON the
/// frontend renders with a component registered for that panel type. That is
/// what keeps a plugin from executing code in the app's origin — the property
/// the CSP and `frame-ancestors 'none'` exist to protect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginPanel {
    /// Stable id, unique within the plugin. Hosted as `plugin__<plugin>__<id>`.
    #[serde(default)]
    pub id: String,
    /// Tab label shown in the inspector.
    #[serde(default)]
    pub title: String,
    /// Optional leading glyph name from the app's icon set.
    #[serde(default)]
    pub icon: Option<String>,
    /// How the payload is produced: [`PANEL_SOURCE_TOOL`] (the plugin may run
    /// `tool` to build it) or [`PANEL_SOURCE_STATIC`] (a document it keeps). Both
    /// are answered by `panels/read`; the value is documentation for the host and
    /// the settings screen.
    #[serde(default = "panel_source_tool")]
    pub source: String,
    /// For `source = "tool"`, the tool a plugin may run to build the payload.
    #[serde(default)]
    pub tool: Option<String>,
    /// Suggested polling interval in ms; 0 means "only on explicit refresh".
    /// The host clamps it (see [`clamp_refresh_ms`]).
    #[serde(default)]
    pub refresh_ms: u64,
}

fn panel_source_tool() -> String {
    PANEL_SOURCE_TOOL.to_string()
}

impl Default for PluginPanel {
    /// A declaration with the documented defaults: `source = "tool"`, no icon,
    /// no tool, no polling. Hand-written rather than derived so `..Default::default()`
    /// cannot produce a `source` that the host refuses.
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            icon: None,
            source: panel_source_tool(),
            tool: None,
            refresh_ms: 0,
        }
    }
}

/// `source` value: the payload is built by running the named tool.
pub const PANEL_SOURCE_TOOL: &str = "tool";
/// `source` value: the payload is a document the plugin already keeps.
pub const PANEL_SOURCE_STATIC: &str = "static";

/// Prefix of every hosted panel id, mirroring the tool prefix.
pub const PANEL_PREFIX: &str = "plugin__";

/// The id a panel is hosted under: `plugin__<plugin>__<panel>`.
///
/// The prefix keeps a plugin panel from colliding with a built-in inspector tab
/// (`workspace`, `changes`, `commands`, `agents`) or with another plugin's panel.
pub fn hosted_panel_id(plugin: &str, panel: &str) -> String {
    format!("{PANEL_PREFIX}{plugin}__{panel}")
}

/// Whether `id` is usable as a panel id.
///
/// A panel id ends up in a URL path segment (`GET /api/plugins/:name/panels/:id`)
/// and in a DOM id, so it is restricted to `a-z`, `0-9`, `-`, `_` and `.`; it may
/// not be empty or contain whitespace, `/`, `?`, `#` or `%`.
pub fn valid_panel_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
}

/// Clamp a plugin's suggested `refresh_ms`.
///
/// A plugin must not be able to make the UI hammer the server: anything between
/// 1 and 999 becomes 1000, and anything above 300 000 becomes 300 000. 0 stays 0
/// and means "only on explicit refresh".
pub fn clamp_refresh_ms(ms: u64) -> u64 {
    const MIN_NON_ZERO: u64 = 1_000;
    const MAX: u64 = 300_000;
    if ms == 0 {
        return 0;
    }
    ms.clamp(MIN_NON_ZERO, MAX)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    #[serde(default = "empty_schema")]
    pub parameters: serde_json::Value,
    /// Optional side-effect class: read_only | mutating | shell | external.
    #[serde(default)]
    pub class: Option<String>,
}

fn empty_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl PluginError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Messages sent from the plugin to the core.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InboundRaw {
    Response {
        id: u64,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<PluginError>,
    },
    Notification {
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
}

impl InboundRaw {
    /// Parse one inbound frame from the plugin process.
    pub fn parse(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

/// Well-known method names for both directions.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const HOOKS_RUN: &str = "hooks/run";
    pub const SHUTDOWN: &str = "shutdown";
    pub const LOG: &str = "log";
    /// Host asks the plugin for the payload of a panel it declared.
    pub const PANELS_READ: &str = "panels/read";
}

/// Hooks the core can invoke on a plugin. Unsupported hooks are documented as
/// reserved; a plugin that does not advertise a hook never receives it.
pub mod hooks {
    pub const ON_RUN_START: &str = "on_run_start";
    pub const ON_RUN_END: &str = "on_run_end";
    pub const ON_TOOL_CALL: &str = "on_tool_call";
    pub const ON_TOOL_RESULT: &str = "on_tool_result";
    pub const ON_MODEL_REQUEST: &str = "on_model_request";
    pub const ON_MODEL_RESPONSE: &str = "on_model_response";
    /// Hooks that may contribute text to the system prompt.
    pub const BEFORE_PROMPT: &str = "before_prompt";
    pub const ON_SESSION_CREATE: &str = "on_session_create";

    pub const IMPLEMENTED: &[&str] = &[BEFORE_PROMPT, ON_RUN_END, ON_TOOL_RESULT];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Disabled,
    Starting,
    Ready,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostics {
    pub name: String,
    pub status: PluginStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restart_count: u32,
}

/// Reserved permission strings a plugin may declare.
pub mod permissions {
    pub const FILES_READ: &str = "files.read";
    pub const FILES_WRITE: &str = "files.write";
    pub const SHELL: &str = "shell";
    pub const NETWORK: &str = "network";
    pub const SESSION_READ: &str = "session.read";
}

/// Response payload of a `tools/call` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolResult {
    pub ok: bool,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

/// Response payload of a `hooks/run` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHookResult {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
}

/// Response payload of a `panels/read` request.
///
/// `data` is opaque: the host forwards it to the browser untouched and the
/// frontend renders it with a registered component (or a generic JSON view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginPanelData {
    /// Title the plugin wants shown. Falls back to the manifest's `title`.
    #[serde(default)]
    pub title: Option<String>,
    /// The panel's payload. Any JSON; never interpreted by the host.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Context forwarded with a `panels/read` call and (since protocol v1 is
/// additive) with every `tools/call`, so a workspace-scoped plugin does not have
/// to guess the session or the project from the last `before_prompt`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolCallContext {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Absolute path of the run's project root.
    #[serde(default)]
    pub workspace: String,
    /// Project id, when the session belongs to one.
    #[serde(default)]
    pub project: Option<String>,
    /// Subagent nesting depth; 0 for the main agent.
    #[serde(default)]
    pub depth: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_defaults_protocol_version() {
        let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1.0"}"#).unwrap();
        assert_eq!(m.protocol, PLUGIN_PROTOCOL_VERSION);
        assert!(m.capabilities.is_empty());
    }

    #[test]
    fn inbound_response_and_notification_parse() {
        let v = serde_json::json!({"id": 3, "result": {"ok": true}});
        let m = InboundRaw::parse(v).unwrap();
        assert!(matches!(m, InboundRaw::Response { id: 3, .. }));

        let v = serde_json::json!({"method": "log", "params": {"message": "hi"}});
        let m = InboundRaw::parse(v).unwrap();
        assert!(matches!(m, InboundRaw::Notification { .. }));
    }

    #[test]
    fn plugin_error_serialises() {
        let e = PluginError::new(-32000, "boom");
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("boom"));
    }

    #[test]
    fn a_manifest_without_panels_keeps_working() {
        // The whole point of the capability being additive: a plugin that knows
        // nothing about panels is unaffected, and so is every stored manifest.
        let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1.0"}"#).unwrap();
        assert!(m.panels.is_empty());

        let round = serde_json::to_value(&m).unwrap();
        assert_eq!(round["panels"], serde_json::json!([]));
        let back: PluginManifest = serde_json::from_value(round).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_panel_declaration_defaults_every_field() {
        // `{"id": "todo"}` alone must be a valid declaration: a plugin should not
        // have to spell out `source`/`refresh_ms` to be understood.
        let panels: Vec<PluginPanel> =
            serde_json::from_str(r#"[{"id":"todo","title":"Todo"}]"#).unwrap();
        let panel = &panels[0];
        assert_eq!(panel.id, "todo");
        assert_eq!(panel.title, "Todo");
        assert_eq!(panel.source, PANEL_SOURCE_TOOL);
        assert_eq!(panel.icon, None);
        assert_eq!(panel.tool, None);
        assert_eq!(panel.refresh_ms, 0);

        let full: PluginPanel = serde_json::from_str(
            r#"{"id":"todo","title":"Todo","icon":"check","source":"static",
                "tool":"todo_list","refresh_ms":5000}"#,
        )
        .unwrap();
        assert_eq!(full.source, PANEL_SOURCE_STATIC);
        assert_eq!(full.tool.as_deref(), Some("todo_list"));
        assert_eq!(full.refresh_ms, 5000);

        let manifest: PluginManifest = serde_json::from_str(
            r#"{"name":"x","version":"1.0","panels":[{"id":"todo","title":"Todo"}]}"#,
        )
        .unwrap();
        assert_eq!(manifest.panels.len(), 1);
    }

    #[test]
    fn panel_ids_are_namespaced_and_validated() {
        assert_eq!(hosted_panel_id("kc-todo", "todo"), "plugin__kc-todo__todo");
        // The prefix is what makes a collision with a built-in tab impossible.
        for builtin in ["workspace", "changes", "commands", "agents"] {
            assert!(hosted_panel_id("p", builtin).starts_with(PANEL_PREFIX));
            assert_ne!(hosted_panel_id("p", builtin), builtin);
        }

        for good in ["todo", "tasks-2", "my_panel", "v1.2"] {
            assert!(valid_panel_id(good), "{good} must be accepted");
        }
        for bad in ["", "To do", "tasks/2", "a?b", "a#b", "100%", "Ünicode"] {
            assert!(!valid_panel_id(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn refresh_intervals_are_clamped() {
        // 0 means "only on explicit refresh" and stays 0.
        assert_eq!(clamp_refresh_ms(0), 0);
        // Anything impatient is raised, anything greedy is lowered.
        assert_eq!(clamp_refresh_ms(1), 1_000);
        assert_eq!(clamp_refresh_ms(999), 1_000);
        assert_eq!(clamp_refresh_ms(5_000), 5_000);
        assert_eq!(clamp_refresh_ms(10_000_000), 300_000);
    }

    #[test]
    fn panel_data_round_trips_and_keeps_opaque_json() {
        let value = serde_json::json!({
            "title": "Todo",
            "data": {"items": [{"id": "t2", "status": "done"}], "count": 1},
        });
        let parsed: PluginPanelData = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Todo"));
        assert_eq!(parsed.data["count"], serde_json::json!(1));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);

        // A payload with no title still parses; `data` defaults to null.
        let bare: PluginPanelData = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(bare.title, None);
        assert!(bare.data.is_null());
    }

    #[test]
    fn tool_call_context_survives_a_missing_new_field() {
        let ctx: ToolCallContext = serde_json::from_str(r#"{"session_id":"s_1"}"#).unwrap();
        assert_eq!(ctx.session_id, "s_1");
        assert_eq!(ctx.depth, 0);
        assert_eq!(ctx.project, None);
        // An older host that sends no context at all is representable.
        let none: Option<ToolCallContext> = None;
        assert!(none.is_none());
    }

    #[test]
    fn panels_read_is_a_well_known_method() {
        assert_eq!(methods::PANELS_READ, "panels/read");
    }
}
