//! Out-of-process plugin host: discovery, supervision and capability negotiation.
//!
//! Plugins are separate processes speaking newline-delimited JSON-RPC 2.0 over
//! stdin/stdout (see `docs/PLUGINS.md`). This module owns the lifecycle:
//!
//! 1. [`PluginHost::discover`] merges explicit [`PluginConfig`] entries from the
//!    config file with `<data_dir>/plugins/<name>/plugin.toml` manifests. Nothing
//!    is spawned yet; broken entries become `failed` diagnostics, never a panic.
//! 2. [`PluginHost::start_all`] spawns every enabled plugin concurrently, runs
//!    the `initialize` + `tools/list` handshake and marks it ready or failed.
//! 3. [`PluginHost::tools`] exposes the ready plugins as [`Tool`]s named
//!    `plugin__<plugin>__<tool>`; failures are reported as failed tool results,
//!    never as panics or registry corruption.
//! 4. [`PluginHost`] implements [`kc_core::hooks::Hooks`] so the runtime can ask
//!    plugins for prompt contributions and lifecycle notifications.
//!
//! Isolation is *process* isolation, not a sandbox: a plugin runs with the
//! server's OS permissions. The declarative `permissions` list is a guard at the
//! tool-exposure layer only, and the configured permissions are authoritative:
//! a manifest cannot grant itself more than the config entry allows.

use async_trait::async_trait;
use kc_core::config::PluginConfig;
use kc_core::plugin::{hooks, PluginDiagnostics, PluginManifest, PluginStatus};
use kc_core::tool::{Tool, ToolClass};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, warn};

mod rpc;
mod tool;

pub use tool::{namespaced_name, parse_class, required_permission, TOOL_PREFIX};

use rpc::{PluginShared, TimeoutRefs};

/// Time budget for each plugin interaction. Defaults: 10s handshake, 60s tool
/// call, 5s hook, 2s graceful shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginTimeouts {
    pub handshake: Duration,
    pub call: Duration,
    pub hook: Duration,
    pub shutdown: Duration,
}

impl Default for PluginTimeouts {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            call: Duration::from_secs(60),
            hook: Duration::from_secs(5),
            shutdown: Duration::from_secs(2),
        }
    }
}

/// One discovered plugin: its config plus (when the entry is usable) the shared
/// process state.
pub(crate) struct PluginSlot {
    pub(crate) config: PluginConfig,
    pub(crate) shared: Option<Arc<PluginShared>>,
    /// Where the entry came from, for error messages: `[[plugins]] #0` or a path.
    pub(crate) source: String,
    /// Set when discovery itself rejected the entry (bad command, duplicate name,
    /// unparsable manifest). The slot is reported as `failed` and never spawned.
    pub(crate) setup_error: Option<String>,
}

pub struct PluginHost {
    plugins: Vec<PluginSlot>,
    timeouts: Arc<TimeoutRefs>,
}

impl PluginHost {
    /// Discover plugins from explicit config entries plus
    /// `<data_dir>/plugins/*/plugin.toml`. Never panics: entries that cannot be
    /// used are kept as `failed` diagnostics with a message that says why.
    pub fn discover(configs: &[PluginConfig], data_dir: &Path) -> Self {
        let timeouts = Arc::new(TimeoutRefs::default());
        let mut host = Self {
            plugins: Vec::new(),
            timeouts,
        };

        for (idx, cfg) in configs.iter().enumerate() {
            host.add(cfg.clone(), format!("[[plugins]] entry #{idx}"));
        }

        let dir = data_dir.join("plugins");
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let mut found: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                found.sort_by_key(|e| e.file_name());
                for entry in found {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let manifest_path = path.join("plugin.toml");
                    if !manifest_path.is_file() {
                        continue;
                    }
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    match load_plugin_file(&manifest_path) {
                        Ok((cfg, error)) => {
                            host.add_from_disk(cfg, &manifest_path, &dir_name, error)
                        }
                        Err(e) => host.add_with_error(
                            PluginConfig {
                                name: dir_name.clone(),
                                enabled: true,
                                command: Vec::new(),
                                env: BTreeMap::new(),
                                permissions: Vec::new(),
                                settings: BTreeMap::new(),
                            },
                            manifest_path.display().to_string(),
                            Some(format!("{}: {e}", manifest_path.display())),
                        ),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(dir = %dir.display(), "no plugin directory; using config entries only");
            }
            Err(e) => {
                warn!(dir = %dir.display(), "could not read the plugin directory: {e}");
            }
        }

        host
    }

    /// Override the interaction budgets. Applies to plugins that are already
    /// discovered; call it before [`start_all`](Self::start_all).
    pub fn set_timeouts(&mut self, timeouts: PluginTimeouts) {
        self.timeouts.apply(&timeouts);
    }

    /// Number of discovered plugins, including ones that failed to load.
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Start every enabled plugin concurrently. A plugin that fails to start is
    /// recorded as `failed`; the others are unaffected.
    pub async fn start_all(&mut self) {
        let mut set = JoinSet::new();
        for slot in &self.plugins {
            if !slot.config.enabled || slot.setup_error.is_some() {
                continue;
            }
            let Some(shared) = slot.shared.clone() else {
                continue;
            };
            set.spawn(async move {
                let result = shared.start().await;
                (shared.name.clone(), result)
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((name, Err(e))) => warn!(plugin = %name, "plugin failed to start: {e}"),
                Ok((_, Ok(()))) => {}
                Err(e) => warn!("plugin startup task failed: {e}"),
            }
        }
    }

    /// Snapshot of every plugin slot, ready or not.
    pub fn diagnostics(&self) -> Vec<PluginDiagnostics> {
        self.plugins.iter().map(slot_diagnostics).collect()
    }

    /// Diagnostics for one plugin.
    pub fn diagnostics_for(&self, name: &str) -> Option<PluginDiagnostics> {
        self.plugins
            .iter()
            .find(|s| s.config.name == name)
            .map(slot_diagnostics)
    }

    /// Tools of all currently ready plugins, in discovery order.
    ///
    /// A tool whose declared class needs a permission that neither the plugin
    /// manifest nor the config entry grants is *not* exposed; the reason is
    /// logged at warn level.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for slot in &self.plugins {
            let Some(shared) = &slot.shared else { continue };
            if shared.status() != PluginStatus::Ready {
                continue;
            }
            let manifest = shared.manifest();
            let version = manifest
                .as_ref()
                .map(|m| m.version.clone())
                .unwrap_or_else(|| "unknown".into());
            // The config entry is authoritative: a manifest cannot grant itself
            // a permission the operator did not configure.
            let (granted, ignored) = effective_permissions(&slot.config, manifest.as_ref());
            if !ignored.is_empty() {
                debug!(
                    plugin = %slot.config.name,
                    ignored = %ignored.join(", "),
                    "ignoring plugin permissions that the config does not grant"
                );
            }
            for def in shared.tool_defs() {
                let class = parse_class(def.class.as_deref());
                if let Some(needed) = required_permission(class) {
                    if !granted.iter().any(|p| p == needed) {
                        warn!(
                            plugin = %slot.config.name,
                            tool = %def.name,
                            permission = needed,
                            "not exposing plugin tool: the plugin declared class `{}` but neither \
                             the plugin manifest nor the config entry grants `{}`",
                            class_label(class),
                            needed
                        );
                        continue;
                    }
                }
                out.push(Arc::new(tool::PluginTool::new(
                    Arc::clone(shared),
                    def,
                    version.clone(),
                )));
            }
        }
        out
    }

    /// Enable or disable one plugin at runtime.
    ///
    /// Enabling starts the process immediately (and resets the restart budget);
    /// disabling sends `shutdown`, kills the process and drops its tools. A
    /// plugin that fails to start is returned as `failed` diagnostics carrying
    /// the error — the caller can show it directly. `Err` is reserved for an
    /// unknown name or an entry that discovery already rejected.
    pub async fn set_enabled(
        &mut self,
        name: &str,
        enabled: bool,
    ) -> kc_core::Result<PluginDiagnostics> {
        let idx = self
            .plugins
            .iter()
            .position(|s| s.config.name == name)
            .ok_or_else(|| {
                kc_core::Error::NotFound(format!("plugin `{name}` is not configured"))
            })?;
        if let Some(err) = self.plugins[idx].setup_error.clone() {
            return Err(kc_core::Error::Config(format!(
                "plugin `{name}` cannot be enabled: {err}"
            )));
        }
        let shared = self.plugins[idx]
            .shared
            .clone()
            .expect("a slot without a setup error always has shared state");
        self.plugins[idx].config.enabled = enabled;
        if enabled {
            shared.reset_restarts();
            shared.mark_stopped();
            if let Err(e) = shared.start().await {
                debug!(plugin = %name, "enabling plugin failed: {e}");
            }
        } else {
            shared.disable().await;
        }
        Ok(slot_diagnostics(&self.plugins[idx]))
    }

    /// Fan a hook out to every ready plugin that advertises it and collect the
    /// non-null `output` values. Failures are logged and ignored: hooks are
    /// best-effort and must never fail a run.
    pub async fn call_hook(&self, hook: &str, payload: Value) -> Vec<Value> {
        fan_out(&self.ready_plugins(), hook, &payload).await
    }

    /// A live hook handle that can be handed to the runtime as
    /// `Arc<dyn kc_core::hooks::Hooks>` while this host stays owned by the
    /// server for admin operations ([`set_enabled`](Self::set_enabled),
    /// [`shutdown`](Self::shutdown)).
    ///
    /// The handle observes start/stop/enable/disable immediately: it holds the
    /// same per-plugin state the host mutates. Slots are fixed at discovery, so
    /// create the handle after [`discover`](Self::discover).
    pub fn hooks_handle(&self) -> PluginHooks {
        PluginHooks {
            plugins: self
                .plugins
                .iter()
                .filter_map(|slot| slot.shared.clone())
                .collect(),
        }
    }

    fn ready_plugins(&self) -> Vec<Arc<PluginShared>> {
        self.plugins
            .iter()
            .filter_map(|slot| slot.shared.clone())
            .collect()
    }

    /// Stop every plugin process. Safe to call more than once.
    pub async fn shutdown(&mut self) {
        let mut set = JoinSet::new();
        for slot in &self.plugins {
            let Some(shared) = slot.shared.clone() else {
                continue;
            };
            set.spawn(async move { shared.stop().await });
        }
        while set.join_next().await.is_some() {}
    }

    fn add(&mut self, config: PluginConfig, source: String) {
        self.add_with_error(config, source, None);
    }

    fn add_from_disk(
        &mut self,
        config: PluginConfig,
        path: &Path,
        dir_name: &str,
        mut error: Option<String>,
    ) {
        let mut config = config;
        if config.name.trim().is_empty() {
            // Documented fallback: the directory name is the plugin name.
            debug!(
                dir = dir_name,
                "plugin.toml has no `name`; using the directory name"
            );
            config.name = dir_name.to_string();
        }
        if let Some(e) = error.take() {
            error = Some(format!("{}: {e}", path.display()));
        }
        self.add_with_error(config, path.display().to_string(), error);
    }

    fn add_with_error(
        &mut self,
        config: PluginConfig,
        source: String,
        setup_error: Option<String>,
    ) {
        if config.name.trim().is_empty() {
            // Cannot happen for disk entries (directory name is the fallback) but
            // a `[[plugins]]` entry may omit the name.
            let name = format!("unnamed-plugin-{}", self.plugins.len());
            warn!("a plugin entry in {source} has no name; naming it `{name}`");
            let mut config = config;
            config.name = name;
            return self.add_with_error(config, source, setup_error);
        }
        if let Some(existing) = self
            .plugins
            .iter_mut()
            .find(|s| s.config.name == config.name)
        {
            let message = format!(
                "duplicate plugin name `{}`: declared by {} and again by {source}; plugin names \
                 must be unique",
                config.name, existing.source
            );
            warn!("{message}");
            existing.setup_error = Some(message);
            existing.shared = None;
            return;
        }

        let mut setup_error = setup_error;
        if setup_error.is_none() && config.command.is_empty() {
            setup_error = Some(format!(
                "plugin `{}` from {} has no command; add `command = [\"program\", \"args\"]`",
                config.name, source
            ));
        }
        let shared = if setup_error.is_none() {
            let status = if config.enabled {
                PluginStatus::Stopped
            } else {
                PluginStatus::Disabled
            };
            Some(PluginShared::new(
                config.clone(),
                Arc::clone(&self.timeouts),
                status,
            ))
        } else {
            if let Some(err) = &setup_error {
                warn!(plugin = %config.name, "plugin unavailable: {err}");
            }
            None
        };
        self.plugins.push(PluginSlot {
            config,
            shared,
            source,
            setup_error,
        });
    }
}

fn slot_diagnostics(slot: &PluginSlot) -> PluginDiagnostics {
    let mut diagnostics = match &slot.setup_error {
        Some(err) => PluginDiagnostics {
            name: slot.config.name.clone(),
            status: PluginStatus::Failed,
            manifest: None,
            tools: Vec::new(),
            error: Some(err.clone()),
            pid: None,
            restart_count: 0,
        },
        None => match &slot.shared {
            Some(shared) => shared.diagnostics(),
            None => PluginDiagnostics {
                name: slot.config.name.clone(),
                status: PluginStatus::Failed,
                manifest: None,
                tools: Vec::new(),
                error: Some("plugin has no runtime state".into()),
                pid: None,
                restart_count: 0,
            },
        },
    };
    // Manifest permissions the operator did not configure are ignored; say so
    // instead of letting the manifest decide what the diagnostics report.
    if let Some(manifest) = diagnostics.manifest.as_ref() {
        let (_, ignored) = effective_permissions(&slot.config, Some(manifest));
        if !ignored.is_empty() {
            let warning = format!(
                "ignored permissions declared by the plugin manifest but not granted by the \
                 config entry (the config is authoritative): {}",
                ignored.join(", ")
            );
            warn!(plugin = %slot.config.name, "{warning}");
            diagnostics.error = Some(match diagnostics.error.take() {
                Some(existing) => format!("{existing}; {warning}"),
                None => warning,
            });
        }
    }
    diagnostics
}

/// The permissions the host actually honours.
///
/// `PluginConfig.permissions` is authoritative when it is non-empty; otherwise
/// the manifest's declaration is used (backwards compatible). The second value
/// lists manifest permissions that were ignored because the config did not
/// grant them.
pub(crate) fn effective_permissions(
    config: &PluginConfig,
    manifest: Option<&PluginManifest>,
) -> (Vec<String>, Vec<String>) {
    let manifest_permissions = manifest.map(|m| dedup_permissions(&m.permissions));
    if config.permissions.is_empty() {
        return (manifest_permissions.unwrap_or_default(), Vec::new());
    }
    let configured = dedup_permissions(&config.permissions);
    let ignored = manifest_permissions
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !configured.contains(p))
        .collect();
    (configured, ignored)
}

fn dedup_permissions(permissions: &[String]) -> Vec<String> {
    let mut out: Vec<String> = permissions
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn class_label(class: ToolClass) -> &'static str {
    match class {
        ToolClass::ReadOnly => "read_only",
        ToolClass::Mutating => "mutating",
        ToolClass::Shell => "shell",
        ToolClass::Delegate => "delegate",
        ToolClass::External => "external",
    }
}

/// Shape of `<data_dir>/plugins/<name>/plugin.toml`.
#[derive(Debug, Deserialize)]
struct PluginFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
}

/// Read one `plugin.toml`. Returns the config plus an optional setup error (for
/// shapes that parse but cannot be used, e.g. a missing command).
fn load_plugin_file(path: &Path) -> Result<(PluginConfig, Option<String>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read the plugin manifest: {e}"))?;
    let file: PluginFile = toml::from_str(&text).map_err(|e| {
        format!(
            "invalid plugin manifest (expected name/command/enabled/permissions/env/settings): {e}"
        )
    })?;
    let mut error = None;
    let name = file.name.unwrap_or_default().trim().to_string();
    if file.command.is_empty() {
        error = Some("the plugin manifest has no `command = [\"program\", \"args\"]`".into());
    }
    if let Some(cmd) = file.command.first() {
        if cmd.trim().is_empty() {
            error = Some("the plugin manifest has an empty command".into());
        }
    }
    Ok((
        PluginConfig {
            name,
            enabled: file.enabled.unwrap_or(true),
            command: file.command,
            env: file.env,
            permissions: file.permissions,
            settings: file.settings,
        },
        error,
    ))
}

/// Fan `hook` out to every ready plugin that advertises it. Failures and
/// timeouts are logged by [`PluginShared::hook_output`] and dropped: hooks are
/// best-effort and must never fail a run.
async fn fan_out(plugins: &[Arc<PluginShared>], hook: &str, payload: &Value) -> Vec<Value> {
    let mut set = JoinSet::new();
    for shared in plugins {
        if shared.status() != PluginStatus::Ready {
            continue;
        }
        let shared = Arc::clone(shared);
        let hook = hook.to_string();
        let payload = payload.clone();
        set.spawn(async move { shared.hook_output(&hook, &payload).await });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(value)) = joined {
            out.push(value);
        }
    }
    out
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => match map.get("text").and_then(|v| v.as_str()) {
            Some(text) => Some(text.to_string()),
            None => Some(value.to_string()),
        },
        other => Some(other.to_string()),
    }
}

/// Live hook view over the plugin slots, cheap to clone and safe to hand to the
/// runtime. See [`PluginHost::hooks_handle`].
#[derive(Clone)]
pub struct PluginHooks {
    plugins: Vec<Arc<PluginShared>>,
}

impl PluginHooks {
    /// Fan a hook out to the ready plugins that advertise it.
    pub async fn call_hook(&self, hook: &str, payload: Value) -> Vec<Value> {
        fan_out(&self.plugins, hook, &payload).await
    }
}

#[async_trait]
impl kc_core::hooks::Hooks for PluginHooks {
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        prompt_notes(&self.plugins, session_id, workspace).await
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        let payload = serde_json::json!({
            "session_id": session_id,
            "run_id": run_id,
            "status": status,
        });
        fan_out(&self.plugins, hooks::ON_RUN_END, &payload).await;
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        let payload = serde_json::json!({
            "session_id": session_id,
            "tool": tool,
            "ok": ok,
        });
        fan_out(&self.plugins, hooks::ON_TOOL_RESULT, &payload).await;
    }
}

#[async_trait]
impl kc_core::hooks::Hooks for PluginHost {
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        prompt_notes(&self.ready_plugins(), session_id, workspace).await
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        let payload = serde_json::json!({
            "session_id": session_id,
            "run_id": run_id,
            "status": status,
        });
        fan_out(&self.ready_plugins(), hooks::ON_RUN_END, &payload).await;
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        let payload = serde_json::json!({
            "session_id": session_id,
            "tool": tool,
            "ok": ok,
        });
        fan_out(&self.ready_plugins(), hooks::ON_TOOL_RESULT, &payload).await;
    }
}

async fn prompt_notes(
    plugins: &[Arc<PluginShared>],
    session_id: &str,
    workspace: &str,
) -> Vec<String> {
    let payload = serde_json::json!({
        "session_id": session_id,
        "workspace": workspace,
    });
    let mut out = Vec::new();
    for value in fan_out(plugins, hooks::BEFORE_PROMPT, &payload).await {
        if let Some(text) = value_to_text(&value) {
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
