//! Exposure of plugin tools through the [`Tool`] trait.
//!
//! Tool names are namespaced as `plugin__<plugin>__<tool>` so a plugin can never
//! shadow a first-party tool. Every failure mode (dead process, timeout,
//! protocol garbage, remote error) becomes a [`ToolOutcome::err`] with text the
//! model (and the user) can act on; nothing here can panic the registry.

use async_trait::async_trait;
use kc_core::config::PluginConfig;
use kc_core::plugin::{methods, permissions, PluginToolDef, PluginToolResult, ToolCallContext};
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolSpec};
use serde_json::json;
use std::sync::Arc;

use crate::rpc::{PluginShared, RpcError};

/// Prefix for every plugin tool name.
pub const TOOL_PREFIX: &str = "plugin__";

/// `settings` key that lets an operator downgrade specific plugin tools to
/// `read_only`, and therefore skip the approval prompt for them.
pub const READ_ONLY_TOOLS_SETTING: &str = "read_only_tools";

/// `plugin__<plugin>__<tool>`.
pub fn namespaced_name(plugin: &str, tool: &str) -> String {
    format!("{TOOL_PREFIX}{plugin}__{tool}")
}

/// Map a manifest/def class string onto a [`ToolClass`]. Anything unknown is
/// treated as `external`, the conservative choice: it always needs approval.
///
/// This only *reads* what a plugin declared. The declared class is a hint and is
/// never the class the host exposes; see [`effective_class`].
pub fn parse_class(class: Option<&str>) -> ToolClass {
    match class.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("read_only") | Some("read-only") | Some("readonly") => ToolClass::ReadOnly,
        Some("mutating") | Some("write") => ToolClass::Mutating,
        Some("shell") | Some("command") => ToolClass::Shell,
        _ => ToolClass::External,
    }
}

/// The class the host actually uses for a plugin tool.
///
/// A plugin's declared class is a **hint only**. `Tool::requires_approval`
/// defaults to `!matches!(class, ReadOnly)`, so honouring a self-declared
/// `read_only` would let a plugin skip the approval prompt entirely — while its
/// code is free to write files, spawn processes or open sockets. Every plugin
/// tool is therefore [`ToolClass::External`], which always needs approval,
/// unless the operator explicitly downgrades it by listing it (bare or
/// namespaced) in the plugin's `settings.read_only_tools` array.
pub fn effective_class(config: &PluginConfig, plugin: &str, tool: &str) -> ToolClass {
    let downgraded = config
        .settings
        .get(READ_ONLY_TOOLS_SETTING)
        .and_then(|v| v.as_array())
        .map(|names| {
            names.iter().filter_map(|n| n.as_str()).any(|n| {
                let n = n.trim();
                n == tool || n == namespaced_name(plugin, tool)
            })
        })
        .unwrap_or(false);
    if downgraded {
        ToolClass::ReadOnly
    } else {
        ToolClass::External
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

/// The session/workspace context forwarded with a plugin call.
///
/// A plugin used to have no way to know which project a call belonged to: the
/// documented workaround was to remember the most recent `before_prompt`
/// workspace, which cross-talks as soon as two sessions run at once. This sends
/// the answer instead of a guess. It is additive — a plugin that ignores
/// `context` behaves exactly as before.
///
/// The project **id** is not on [`ToolCtx`]: it carries the `Workspace` (the
/// canonically resolved root), and the id lives in the server's registry. The
/// absolute path is what a workspace-scoped plugin actually needs, and it is
/// already exposed to the model in the system prompt.
pub fn call_context(ctx: &ToolCtx) -> ToolCallContext {
    ToolCallContext {
        session_id: ctx.approvals.session_id.0.clone(),
        run_id: ctx.approvals.run_id.0.clone(),
        agent_id: ctx.approvals.agent_id.as_ref().map(|a| a.0.clone()),
        workspace: ctx.workspace.root().to_string_lossy().into_owned(),
        project: None,
        depth: ctx.depth,
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
        // The plugin's own `class` is a hint; the operator decides. See
        // [`effective_class`].
        let class = effective_class(&shared.config(), &shared.name, &def.name);
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
        let (perms, _) =
            crate::effective_permissions(&self.shared.config(), self.shared.manifest().as_ref());
        if !perms.is_empty() {
            out.push_str(&format!(" Effective permissions: {}.", perms.join(", ")));
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
        let params = json!({
            "name": self.def.name,
            "arguments": request.arguments,
            // Additive: existing plugins ignore parameters they do not know. A
            // workspace-scoped plugin reads `context.workspace` instead of
            // guessing the project from the last `before_prompt`, which was
            // wrong as soon as two sessions ran concurrently.
            "context": call_context(&ctx),
        });
        // Cancellation is handled inside `request_cancellable`, which removes the
        // pending entry on every exit path. Dropping the future from outside
        // (as `tokio::select!` here used to) left a dead waiter in the map.
        let reply = self
            .shared
            .request_cancellable(methods::TOOLS_CALL, params, budget, Some(&ctx.cancel))
            .await;
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

    fn config_with(settings: &[(&str, serde_json::Value)]) -> PluginConfig {
        PluginConfig {
            name: "p".into(),
            enabled: true,
            command: vec!["true".into()],
            env: Default::default(),
            permissions: Vec::new(),
            settings: settings
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn a_declared_read_only_class_does_not_exempt_a_tool_from_approval() {
        // The plugin says `read_only`; the host still exposes it as `external`,
        // which `Tool::requires_approval` maps to "ask first".
        let config = config_with(&[]);
        assert_eq!(
            effective_class(&config, "p", "peek"),
            ToolClass::External,
            "a self-declared read_only class must not be honoured"
        );
    }

    #[test]
    fn the_operator_can_downgrade_a_tool_to_read_only() {
        // Bare tool name.
        let config = config_with(&[(
            READ_ONLY_TOOLS_SETTING,
            serde_json::json!(["peek", "other"]),
        )]);
        assert_eq!(effective_class(&config, "p", "peek"), ToolClass::ReadOnly);
        assert_eq!(effective_class(&config, "p", "other"), ToolClass::ReadOnly);
        // Anything not listed stays external.
        assert_eq!(effective_class(&config, "p", "write"), ToolClass::External);

        // Namespaced name works too.
        let config = config_with(&[(
            READ_ONLY_TOOLS_SETTING,
            serde_json::json!(["plugin__p__peek"]),
        )]);
        assert_eq!(effective_class(&config, "p", "peek"), ToolClass::ReadOnly);

        // A malformed setting is ignored rather than trusted.
        let config = config_with(&[(READ_ONLY_TOOLS_SETTING, serde_json::json!("peek"))]);
        assert_eq!(effective_class(&config, "p", "peek"), ToolClass::External);
        let config = config_with(&[(READ_ONLY_TOOLS_SETTING, serde_json::json!([1, 2]))]);
        assert_eq!(effective_class(&config, "p", "peek"), ToolClass::External);
    }
}
