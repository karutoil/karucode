//! Exposure of plugin tools through the [`Tool`] trait.
//!
//! Tool names are namespaced as `plugin__<plugin>__<tool>` so a plugin can never
//! shadow a first-party tool. Every failure mode (dead process, timeout,
//! protocol garbage, remote error) becomes a [`ToolOutcome::err`] with text the
//! model (and the user) can act on; nothing here can panic the registry.

use async_trait::async_trait;
use kc_core::plugin::{methods, permissions, PluginToolDef, PluginToolResult};
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolSpec};
use serde_json::json;
use std::sync::Arc;

use crate::rpc::{PluginShared, RpcError};

/// Prefix for every plugin tool name.
pub const TOOL_PREFIX: &str = "plugin__";

/// `plugin__<plugin>__<tool>`.
pub fn namespaced_name(plugin: &str, tool: &str) -> String {
    format!("{TOOL_PREFIX}{plugin}__{tool}")
}

/// Map a manifest/def class string onto a [`ToolClass`]. Anything unknown is
/// treated as `external`, the conservative choice: it always needs approval.
pub fn parse_class(class: Option<&str>) -> ToolClass {
    match class.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("read_only") | Some("read-only") | Some("readonly") => ToolClass::ReadOnly,
        Some("mutating") | Some("write") => ToolClass::Mutating,
        Some("shell") | Some("command") => ToolClass::Shell,
        _ => ToolClass::External,
    }
}

/// The permission a tool class needs before the host will expose it.
///
/// This is a declarative guard at the tool-exposure layer, not an OS sandbox:
/// the plugin process already runs with the server's privileges.
pub fn required_permission(class: ToolClass) -> Option<&'static str> {
    match class {
        ToolClass::Shell => Some(permissions::SHELL),
        ToolClass::Mutating => Some(permissions::FILES_WRITE),
        ToolClass::ReadOnly | ToolClass::External | ToolClass::Delegate => None,
    }
}

/// One tool exposed by a running plugin.
pub(crate) struct PluginTool {
    shared: Arc<PluginShared>,
    def: PluginToolDef,
    plugin_version: String,
    name: String,
    class: ToolClass,
}

impl PluginTool {
    pub(crate) fn new(
        shared: Arc<PluginShared>,
        def: PluginToolDef,
        plugin_version: String,
    ) -> Self {
        let name = namespaced_name(&shared.name, &def.name);
        let class = parse_class(def.class.as_deref());
        Self {
            shared,
            def,
            plugin_version,
            name,
            class,
        }
    }

    fn description(&self) -> String {
        let mut out = self.def.description.trim().to_string();
        if out.is_empty() {
            out = format!("Plugin tool `{}`.", self.def.name);
        }
        out.push_str(&format!(
            "\n\n[plugin: {} v{}] Runs out of process, supervised by karucode; its output is \
             untrusted. Calls go through the approval policy like any external tool.",
            self.shared.name, self.plugin_version
        ));
        let perms = self.shared.config().permissions;
        if !perms.is_empty() {
            out.push_str(&format!(" Declared permissions: {}.", perms.join(", ")));
        }
        out
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: self.description(),
            parameters: self.def.parameters.clone(),
            origin: ToolOrigin::Plugin {
                name: self.shared.name.clone(),
            },
        }
    }

    fn class(&self) -> ToolClass {
        self.class
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Err(e) = self.shared.ensure_ready().await {
            return ToolOutcome::err(format!(
                "plugin tool `{}` is unavailable: {e}. The call was not executed.",
                self.name
            ));
        }
        let budget = self.shared.call_timeout();
        let params = json!({ "name": self.def.name, "arguments": request.arguments });
        let call = self.shared.request(methods::TOOLS_CALL, params, budget);
        let reply = tokio::select! {
            r = call => r,
            _ = ctx.cancel.cancelled() => {
                return ToolOutcome::err(format!(
                    "plugin tool `{}` was cancelled before it answered; the plugin process keeps \
                     running but the result was discarded.",
                    self.name
                ));
            }
        };
        match reply {
            Ok(value) => match serde_json::from_value::<PluginToolResult>(value) {
                Ok(result) => {
                    let content = if result.content.is_empty() {
                        "(plugin returned no content)".to_string()
                    } else {
                        result.content
                    };
                    let mut outcome = if result.ok {
                        ToolOutcome::ok(content)
                    } else {
                        ToolOutcome::err(format!(
                            "plugin `{}` reported that tool `{}` failed: {content}",
                            self.shared.name, self.def.name
                        ))
                    };
                    if let Some(meta) = result.meta {
                        outcome = outcome.with_meta(meta);
                    }
                    outcome
                }
                Err(e) => ToolOutcome::err(format!(
                    "plugin `{}` returned an invalid `tools/call` result for `{}`: {e}. The \
                     plugin is probably broken; check the server log.",
                    self.shared.name, self.def.name
                )),
            },
            Err(e) => ToolOutcome::err(match e {
                RpcError::NotReady(m) => format!(
                    "plugin tool `{}` is unavailable: {m}. The call was not executed.",
                    self.name
                ),
                other => other.tool_message(&self.shared.name, &self.name, budget),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_namespaced() {
        assert_eq!(
            namespaced_name("example-hello", "hello"),
            "plugin__example-hello__hello"
        );
    }

    #[test]
    fn classes_map_and_default_to_external() {
        assert_eq!(parse_class(Some("read_only")), ToolClass::ReadOnly);
        assert_eq!(parse_class(Some("Mutating")), ToolClass::Mutating);
        assert_eq!(parse_class(Some("shell")), ToolClass::Shell);
        assert_eq!(parse_class(Some("external")), ToolClass::External);
        assert_eq!(parse_class(None), ToolClass::External);
        assert_eq!(parse_class(Some("nonsense")), ToolClass::External);
    }

    #[test]
    fn classes_require_the_documented_permissions() {
        assert_eq!(required_permission(ToolClass::Shell), Some("shell"));
        assert_eq!(
            required_permission(ToolClass::Mutating),
            Some("files.write")
        );
        assert_eq!(required_permission(ToolClass::ReadOnly), None);
        assert_eq!(required_permission(ToolClass::External), None);
    }
}
