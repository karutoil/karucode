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
}

fn protocol_version() -> String {
    PLUGIN_PROTOCOL_VERSION.into()
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
enum InboundRaw {
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
}
