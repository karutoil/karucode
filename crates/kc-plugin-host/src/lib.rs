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
//! tool-exposure layer only, and it is **fail-closed**: an entry that lists no
//! permissions grants none, whatever the manifest asks for. Only an explicit
//! `permissions = ["*"]` (or `settings.trust = true`) honours the manifest's own
//! declaration. Every plugin tool is exposed as `external`, so it always goes
//! through the approval policy; a plugin cannot exempt itself by declaring
//! `read_only`.

use async_trait::async_trait;
use kc_core::config::PluginConfig;
use kc_core::plugin::{
    hooks, hosted_panel_id, PluginDiagnostics, PluginManifest, PluginPanel, PluginStatus,
    ToolCallContext,
};
use kc_core::tool::{Tool, ToolClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, warn};

mod rpc;
mod tool;

pub use tool::{namespaced_name, parse_class, required_permission, TOOL_PREFIX};

use rpc::{PluginShared, TimeoutRefs};

pub use rpc::PanelPayload;

/// One panel as the host exposes it to the UI: the manifest declaration plus
/// the id it is hosted under.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelSummary {
    /// The bare id from the manifest (`todo`).
    pub id: String,
    /// `plugin__<plugin>__<id>` — what the UI keys its tab and its request on.
    pub hosted_id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub source: String,
    /// Clamped to 1 000..=300 000, or 0 for "only on explicit refresh".
    pub refresh_ms: u64,
}

impl PanelSummary {
    pub fn new(plugin: &str, panel: PluginPanel) -> Self {
        Self {
            hosted_id: hosted_panel_id(plugin, &panel.id),
            id: panel.id,
            title: panel.title,
            icon: panel.icon,
            source: panel.source,
            refresh_ms: panel.refresh_ms,
        }
    }
}

/// One plugin's panels. Grouping lets the UI show which plugin a tab belongs to
/// (and its version) without a second request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginPanels {
    pub plugin: String,
    pub version: String,
    pub panels: Vec<PanelSummary>,
}

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
    /// The files this slot's process was built from, as of the last discovery,
    /// so a re-read of the sources can tell "same plugin" from "rebuilt plugin"
    /// (see [`Fingerprint`]).
    pub(crate) fingerprint: Fingerprint,
    /// The `plugin.toml` this slot came from, if any. Kept so the watch stamp can
    /// re-fingerprint the files the *process* is made of, not only the config
    /// entries that were cached at discovery time.
    pub(crate) manifest: Option<PathBuf>,
}

/// Everything a slot is built from, as discovery found it. Re-reading the
/// sources produces these, and [`PluginHost::rescan`] diffs them against the
/// slots that are already running.
pub(crate) struct DiscoveredEntry {
    pub(crate) config: PluginConfig,
    pub(crate) source: String,
    pub(crate) setup_error: Option<String>,
    pub(crate) fingerprint: Fingerprint,
    /// The manifest this entry was read from, when it came from disk.
    pub(crate) manifest: Option<PathBuf>,
}

/// The files a plugin's process is actually made of: its manifest (when it came
/// from disk) and every command argument that resolves to a file.
///
/// This is what makes "rebuild the plugin binary, then reload" work: the config
/// entry is byte-identical, so a config diff alone cannot notice that the process
/// the host is supervising is stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    /// `(path, mtime_nanos, len_bytes)`, sorted by path so the comparison is
    /// independent of the order the arguments were written in.
    stamps: Vec<(String, u64, u64)>,
}

impl Fingerprint {
    fn of(config: &PluginConfig, manifest: Option<&Path>) -> Self {
        let mut stamps = Vec::new();
        if let Some(path) = manifest {
            if let Some((mtime, len)) = file_stamp(path) {
                stamps.push((path.display().to_string(), mtime, len));
            }
        }
        for arg in &config.command {
            let Some(path) = resolve_command(arg) else {
                continue;
            };
            let Some((mtime, len)) = file_stamp(&path) else {
                continue;
            };
            let key = path.display().to_string();
            if stamps.iter().any(|(existing, _, _)| *existing == key) {
                continue;
            }
            stamps.push((key, mtime, len));
        }
        stamps.sort();
        Self { stamps }
    }

    /// How many files the fingerprint covers. `0` means the host could not see
    /// any of the files (e.g. a command resolved on `PATH` inside a container),
    /// so only a config or manifest change can trigger a restart.
    pub fn file_count(&self) -> usize {
        self.stamps.len()
    }
}

/// mtime in nanoseconds since the epoch, plus length, of a regular file.
fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

/// Resolve one command argument to a file on disk: an explicit path is used as
/// is, a bare program name is looked up on `PATH` exactly like the spawn does.
fn resolve_command(arg: &str) -> Option<PathBuf> {
    if arg.trim().is_empty() {
        return None;
    }
    let path = Path::new(arg);
    if path.is_absolute() || arg.contains(std::path::MAIN_SEPARATOR) {
        return Some(path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(arg))
        .find(|candidate| candidate.is_file())
}

/// `(path, mtime, len)` of every `<data_dir>/plugins/<name>/plugin.toml`.
///
/// Used for the watch stamp: this is the set of manifests a reload could pick
/// up, and a new, deleted or edited manifest changes it.
fn plugin_dir_stamps(data_dir: &Path) -> Vec<(String, u64, u64)> {
    let dir = data_dir.join("plugins");
    let mut stamps: Vec<(String, u64, u64)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return stamps;
    };
    let mut found: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    found.sort_by_key(|e| e.file_name());
    for entry in found {
        if !entry.path().is_dir() {
            continue;
        }
        let manifest = entry.path().join("plugin.toml");
        if let Some((mtime, len)) = file_stamp(&manifest) {
            stamps.push((manifest.display().to_string(), mtime, len));
        }
    }
    stamps
}

/// Whether two config entries describe the same process.
///
/// `enabled` is deliberately excluded: for a slot that already exists it is a
/// *runtime* toggle, and a config re-read must not silently undo the operator's
/// last click. It only decides the status of a newly discovered plugin.
fn same_source(a: &PluginConfig, b: &PluginConfig) -> bool {
    a.name == b.name
        && a.command == b.command
        && a.env == b.env
        && a.permissions == b.permissions
        && a.settings == b.settings
}

/// What one [`PluginHost::rescan`] changed, in discovery order. Serialised by
/// `POST /api/plugins/reload` so the caller can log or show what happened.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RescanReport {
    /// Plugin names that did not exist before and whose process was started.
    pub added: Vec<String>,
    /// Plugins whose sources disappeared; their process was stopped and their
    /// tools are gone.
    pub removed: Vec<String>,
    /// Plugins whose config or files changed; the process was replaced.
    pub restarted: Vec<String>,
    /// Plugins that were already correct and were left running, untouched.
    pub unchanged: Vec<String>,
}

impl RescanReport {
    /// Whether anything at all changed. A rescan that only reported `unchanged`
    /// entries did not disturb a single running process.
    pub fn changed(&self) -> bool {
        !(self.added.is_empty() && self.removed.is_empty() && self.restarted.is_empty())
    }
}

pub struct PluginHost {
    plugins: Vec<PluginSlot>,
    timeouts: Arc<TimeoutRefs>,
    /// Where the manifest-only plugins live, kept so a rescan can look again.
    data_dir: PathBuf,
}

impl PluginHost {
    /// Discover plugins from explicit config entries plus
    /// `<data_dir>/plugins/*/plugin.toml`. Never panics: entries that cannot be
    /// used are kept as `failed` diagnostics with a message that says why.
    pub fn discover(configs: &[PluginConfig], data_dir: &Path) -> Self {
        let timeouts = Arc::new(TimeoutRefs::default());
        let plugins = discover_entries(configs, data_dir)
            .into_iter()
            .map(|entry| entry.into_slot(Arc::clone(&timeouts), None))
            .collect();
        Self {
            plugins,
            timeouts,
            data_dir: data_dir.to_path_buf(),
        }
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
        // Say once, at startup, when a plugin asked for more than the operator
        // granted. Permissions are fail-closed, so anything not configured is
        // simply not granted — but silently ignoring a manifest's request would
        // leave the operator wondering why a tool is missing.
        for slot in &self.plugins {
            let Some(shared) = &slot.shared else { continue };
            let manifest = shared.manifest();
            let (_, ignored) = effective_permissions(&slot.config, manifest.as_ref());
            if !ignored.is_empty() {
                warn!(
                    plugin = %slot.config.name,
                    ignored = %ignored.join(", "),
                    "the plugin declares permissions the config does not grant; they are ignored. \
                     Add them to the [[plugins]] entry's `permissions` list, or set \
                     `permissions = [\"*\"]` / `settings.trust = true` to honour the manifest"
                );
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
    /// A tool whose *declared* class needs a permission the config entry does
    /// not grant is *not* exposed; the reason is logged at warn level. The class
    /// it is exposed with is always `external` (see [`tool::effective_class`]),
    /// so the declaration can only restrict exposure, never widen it.
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
                // The *declared* class still gates exposure: a tool that says it
                // needs `shell` is hidden unless the operator granted `shell`.
                // This can only restrict, never widen, so it stays safe even
                // though the class the tool is exposed with is always
                // `external` (see `tool::effective_class`).
                let declared = parse_class(def.class.as_deref());
                if let Some(needed) = required_permission(declared) {
                    if !granted.iter().any(|p| p == needed) {
                        warn!(
                            plugin = %slot.config.name,
                            tool = %def.name,
                            permission = needed,
                            "not exposing plugin tool: the plugin declared class `{}` but neither \
                             the plugin manifest nor the config entry grants `{}`",
                            class_label(declared),
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

    /// Every panel contributed by a **ready** plugin, in discovery order.
    ///
    /// This is what the web UI turns into inspector tabs. A plugin that declares
    /// no panels contributes nothing, and a failed/disabled plugin contributes
    /// nothing, so the list is always safe to render.
    pub fn panels(&self) -> Vec<PluginPanels> {
        panels_of(
            &self
                .plugins
                .iter()
                .filter_map(|slot| slot.shared.clone())
                .collect::<Vec<_>>(),
        )
    }

    /// Read one panel's payload.
    ///
    /// The plugin answers `panels/read` with opaque JSON. `Err` carries the
    /// [`kc_core::Error`] the HTTP layer maps exactly like it maps
    /// [`set_enabled`](Self::set_enabled): `NotFound` for an unknown plugin or
    /// panel, `Config` for a plugin that is not running, `Plugin` for a plugin
    /// that failed to answer.
    ///
    /// The server route uses [`panels_handle`](Self::panels_handle) instead, so
    /// it does not hold the host mutex while waiting on the plugin process.
    pub async fn panel(
        &self,
        name: &str,
        panel: &str,
        ctx: &ToolCallContext,
    ) -> kc_core::Result<PanelPayload> {
        let plugins: Vec<Arc<PluginShared>> = self.ready_plugins();
        read_panel(&plugins, name, panel, ctx).await
    }

    /// A live panel handle: the counterpart of [`hooks_handle`](Self::hooks_handle).
    ///
    /// Reading a panel means waiting for another process (up to the call
    /// timeout). Doing that while holding the host's `Mutex` would block every
    /// other `/api` request that touches plugins, so the route takes this cheap
    /// handle instead and drops the lock first.
    pub fn panels_handle(&self) -> PluginPanelsHandle {
        PluginPanelsHandle {
            plugins: self
                .plugins
                .iter()
                .filter_map(|slot| slot.shared.clone())
                .collect(),
        }
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

    /// Re-read the sources and reconcile the running processes with them.
    ///
    /// This is what makes a plugin install, edit or upgrade work without
    /// restarting the server: the config entries and the `<data_dir>/plugins`
    /// manifests are read again, and:
    ///
    /// * a name that is new is added and started (hot **load**);
    /// * a name that disappeared is stopped and dropped (hot **unload**);
    /// * a plugin whose config or files changed is stopped and started again
    ///   (hot **reload** — see [`Fingerprint`], which catches a rebuilt binary
    ///   even when the command line is identical);
    /// * everything else is left running untouched. A rescan of an unchanged
    ///   tree does not disturb a single process, so calling it from a watcher is
    ///   cheap morally as well as computationally.
    ///
    /// A runtime enable/disable (`set_enabled`) survives a rescan: `enabled` is
    /// only taken from the sources for a name that did not exist before.
    ///
    /// Objects already handed to the runtime (tools, panels) keep pointing at
    /// the process they were created for. Call the server's `reload_tools`
    /// afterwards so the registry exposes the new set — the same contract as
    /// [`set_enabled`](Self::set_enabled).
    pub async fn rescan(&mut self, configs: &[PluginConfig]) -> RescanReport {
        let desired = discover_entries(configs, &self.data_dir);
        let mut report = RescanReport::default();
        let mut next: Vec<PluginSlot> = Vec::with_capacity(desired.len());
        let mut stops: Vec<Arc<PluginShared>> = Vec::new();
        let mut starts: Vec<Arc<PluginShared>> = Vec::new();

        for entry in desired {
            let name = entry.config.name.clone();
            let existing = self
                .plugins
                .iter()
                .position(|slot| slot.config.name == name)
                .map(|idx| self.plugins.remove(idx));
            let Some(old) = existing else {
                let slot = entry.into_slot(Arc::clone(&self.timeouts), None);
                if let Some(shared) = started_slot(&slot) {
                    starts.push(shared);
                }
                report.added.push(name);
                next.push(slot);
                continue;
            };
            if old.setup_error == entry.setup_error
                && same_source(&old.config, &entry.config)
                && old.fingerprint == entry.fingerprint
            {
                report.unchanged.push(name);
                next.push(old);
                continue;
            }
            // The sources changed under a running process: stop it and build a
            // fresh slot. A restart is the only honest option — the old process
            // is executing code the operator has already replaced.
            if let Some(shared) = old.shared {
                stops.push(shared);
            }
            let slot = entry.into_slot(Arc::clone(&self.timeouts), Some(&old.config));
            if let Some(shared) = started_slot(&slot) {
                starts.push(shared);
            }
            report.restarted.push(name);
            next.push(slot);
        }

        for slot in self.plugins.drain(..) {
            if let Some(shared) = slot.shared {
                stops.push(shared);
            }
            report.removed.push(slot.config.name);
        }
        self.plugins = next;

        // Stop before start: a replacement must not race its predecessor for the
        // same name, port, lock file or socket.
        for shared in stops {
            shared.stop().await;
        }
        start_many(starts).await;
        report
    }

    /// Reload one plugin by name: re-read its sources and replace its process.
    ///
    /// This is the "I rebuilt it" / "I edited its manifest" path. The runtime
    /// `enabled` state is preserved, so reloading a plugin that is disabled in
    /// the UI does not start it. `Err` is reserved for an unknown name, a name
    /// whose sources disappeared, or a slot that discovery already rejected;
    /// a plugin that fails to start comes back as `failed` diagnostics, exactly
    /// like [`set_enabled`](Self::set_enabled).
    pub async fn reload(
        &mut self,
        name: &str,
        configs: &[PluginConfig],
    ) -> kc_core::Result<PluginDiagnostics> {
        let idx = self
            .plugins
            .iter()
            .position(|slot| slot.config.name == name)
            .ok_or_else(|| {
                kc_core::Error::NotFound(format!("plugin `{name}` is not configured"))
            })?;
        let entry = discover_entries(configs, &self.data_dir)
            .into_iter()
            .find(|entry| entry.config.name == name)
            .ok_or_else(|| {
                kc_core::Error::NotFound(format!(
                    "plugin `{name}` is no longer configured; reload the plugin list to drop it"
                ))
            })?;

        let old = self.plugins.remove(idx);
        if let Some(shared) = old.shared.clone() {
            shared.stop().await;
        }
        let slot = entry.into_slot(Arc::clone(&self.timeouts), Some(&old.config));
        let shared = slot.shared.clone();
        let enabled = slot.config.enabled;
        self.plugins.insert(idx, slot);
        if enabled {
            if let Some(shared) = shared {
                if let Err(e) = shared.start().await {
                    debug!(plugin = %name, "reloading plugin failed: {e}");
                }
            }
        }
        Ok(slot_diagnostics(&self.plugins[idx]))
    }

    /// A cheap number that changes when anything a reload depends on changes:
    /// the manifests under `<data_dir>/plugins` and the files of every configured
    /// command. The watcher polls it and calls [`rescan`](Self::rescan) when it
    /// moves; `rescan` decides what actually needs a restart.
    pub fn source_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for stamp in plugin_dir_stamps(&self.data_dir) {
            stamp.hash(&mut hasher);
        }
        for slot in &self.plugins {
            slot.source.hash(&mut hasher);
            // Re-fingerprint here rather than hashing the cached value: a rebuilt
            // binary is exactly what the cached value cannot see, and that is the
            // common case for "I changed my plugin".
            Fingerprint::of(&slot.config, slot.manifest.as_deref()).hash(&mut hasher);
        }
        hasher.finish()
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
}

impl DiscoveredEntry {
    /// Build the slot for this entry. `previous` is the config of the slot being
    /// replaced, if any: its `enabled` flag is a runtime toggle and therefore
    /// wins over whatever the config file says now.
    fn into_slot(self, timeouts: Arc<TimeoutRefs>, previous: Option<&PluginConfig>) -> PluginSlot {
        let DiscoveredEntry {
            mut config,
            source,
            setup_error,
            fingerprint,
            manifest,
        } = self;
        if let Some(previous) = previous {
            config.enabled = previous.enabled;
        }
        let status = if config.enabled {
            PluginStatus::Stopped
        } else {
            PluginStatus::Disabled
        };
        let shared = if setup_error.is_none() {
            Some(PluginShared::new(config.clone(), timeouts, status))
        } else {
            if let Some(err) = &setup_error {
                warn!(plugin = %config.name, "plugin unavailable: {err}");
            }
            None
        };
        PluginSlot {
            config,
            shared,
            source,
            setup_error,
            fingerprint,
            manifest,
        }
    }
}

/// The shared state of a slot that should be running, if it has any.
fn started_slot(slot: &PluginSlot) -> Option<Arc<PluginShared>> {
    if !slot.config.enabled {
        return None;
    }
    slot.shared.clone()
}

/// Start a set of plugin processes concurrently, logging failures instead of
/// returning them: one broken plugin must not stop the others from starting.
async fn start_many(starts: Vec<Arc<PluginShared>>) {
    let mut set = JoinSet::new();
    for shared in starts {
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

/// Read both plugin sources: explicit config entries, then
/// `<data_dir>/plugins/*/plugin.toml`, in that order.
///
/// Duplicate names are kept as a single `failed` entry naming both sources, so a
/// rescan can never silently switch which plugin a name refers to.
fn discover_entries(configs: &[PluginConfig], data_dir: &Path) -> Vec<DiscoveredEntry> {
    let mut out: Vec<DiscoveredEntry> = Vec::new();
    for (idx, cfg) in configs.iter().enumerate() {
        let source = format!("[[plugins]] entry #{idx}");
        let fingerprint = Fingerprint::of(cfg, None);
        push_entry(&mut out, cfg.clone(), source, None, fingerprint, None);
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
                    Ok((mut cfg, error)) => {
                        if cfg.name.trim().is_empty() {
                            // Documented fallback: the directory name is the name.
                            debug!(
                                dir = %dir_name,
                                "plugin.toml has no `name`; using the directory name"
                            );
                            cfg.name = dir_name;
                        }
                        let error = error.map(|e| format!("{}: {e}", manifest_path.display()));
                        let fingerprint = Fingerprint::of(&cfg, Some(&manifest_path));
                        push_entry(
                            &mut out,
                            cfg,
                            manifest_path.display().to_string(),
                            error,
                            fingerprint,
                            Some(manifest_path),
                        );
                    }
                    Err(e) => {
                        let cfg = PluginConfig {
                            name: dir_name.clone(),
                            enabled: true,
                            command: Vec::new(),
                            env: BTreeMap::new(),
                            permissions: Vec::new(),
                            settings: BTreeMap::new(),
                        };
                        let fingerprint = Fingerprint::of(&cfg, Some(&manifest_path));
                        push_entry(
                            &mut out,
                            cfg,
                            manifest_path.display().to_string(),
                            Some(format!("{}: {e}", manifest_path.display())),
                            fingerprint,
                            Some(manifest_path),
                        );
                    }
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
    out
}

/// Append one discovered entry, applying the same rules every source shares: an
/// entry without a name gets a synthetic one, an entry without a command is
/// unusable, and a duplicate name fails the *first* entry rather than running
/// either of them.
fn push_entry(
    out: &mut Vec<DiscoveredEntry>,
    config: PluginConfig,
    source: String,
    setup_error: Option<String>,
    fingerprint: Fingerprint,
    manifest: Option<PathBuf>,
) {
    let mut config = config;
    if config.name.trim().is_empty() {
        // Cannot happen for disk entries (directory name is the fallback) but a
        // `[[plugins]]` entry may omit the name.
        let name = format!("unnamed-plugin-{}", out.len());
        warn!("a plugin entry in {source} has no name; naming it `{name}`");
        config.name = name;
    }
    if let Some(existing) = out.iter_mut().find(|e| e.config.name == config.name) {
        let message = format!(
            "duplicate plugin name `{}`: declared by {} and again by {source}; plugin names must be \
             unique",
            config.name, existing.source
        );
        warn!("{message}");
        existing.setup_error = Some(message);
        return;
    }
    let setup_error = match setup_error {
        Some(e) => Some(e),
        None if config.command.is_empty() => Some(format!(
            "plugin `{}` from {source} has no command; add `command = [\"program\", \"args\"]`",
            config.name
        )),
        None if config
            .command
            .first()
            .is_some_and(|cmd| cmd.trim().is_empty()) =>
        {
            Some(format!(
                "plugin `{}` from {source} has an empty command",
                config.name
            ))
        }
        None => None,
    };
    out.push(DiscoveredEntry {
        config,
        source,
        setup_error,
        fingerprint,
        manifest,
    });
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
/// **Fail-closed.** `PluginConfig.permissions` is the only source of grants:
///
/// * empty (the common `[[plugins]] name = … command = …` entry) grants
///   **nothing**, whatever the manifest asks for;
/// * `"*"` — or `settings.trust = true` — honours the manifest's declaration
///   verbatim, which is the explicit opt-in for "I trust this plugin";
/// * otherwise exactly the listed permissions are granted.
///
/// The second value lists manifest permissions that were ignored because the
/// config did not grant them.
pub(crate) fn effective_permissions(
    config: &PluginConfig,
    manifest: Option<&PluginManifest>,
) -> (Vec<String>, Vec<String>) {
    let manifest_permissions = manifest.map(|m| dedup_permissions(&m.permissions));
    if trusts_manifest(config) {
        return (manifest_permissions.unwrap_or_default(), Vec::new());
    }
    let configured: Vec<String> = dedup_permissions(&config.permissions)
        .into_iter()
        .filter(|p| p != WILDCARD_PERMISSION)
        .collect();
    let ignored = manifest_permissions
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !configured.contains(p))
        .collect();
    (configured, ignored)
}

/// The permission string that means "honour whatever the manifest declares".
pub(crate) const WILDCARD_PERMISSION: &str = "*";

/// `settings` key equivalent to `permissions = ["*"]`.
pub(crate) const TRUST_SETTING: &str = "trust";

/// Whether the operator explicitly opted into the manifest's own permission
/// declaration, via `permissions = ["*"]` or `settings.trust = true`.
fn trusts_manifest(config: &PluginConfig) -> bool {
    let wildcard = config
        .permissions
        .iter()
        .any(|p| p.trim() == WILDCARD_PERMISSION);
    let trusted = config
        .settings
        .get(TRUST_SETTING)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    wildcard || trusted
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
///
/// Results come back in **plugin order**, not completion order. The
/// `before_prompt` output is joined into the system prompt on every model
/// call, so an order that depended on which process answered first would
/// rewrite message 0 between turns and cost a full prompt-cache miss.
async fn fan_out(plugins: &[Arc<PluginShared>], hook: &str, payload: &Value) -> Vec<Value> {
    let mut set = JoinSet::new();
    for (index, shared) in plugins.iter().enumerate() {
        if shared.status() != PluginStatus::Ready {
            continue;
        }
        let shared = Arc::clone(shared);
        let hook = hook.to_string();
        let payload = payload.clone();
        set.spawn(async move { (index, shared.hook_output(&hook, &payload).await) });
    }
    let mut indexed = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((index, Some(value))) = joined {
            indexed.push((index, value));
        }
    }
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, value)| value).collect()
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

/// Live panel view over the plugin slots. See [`PluginHost::panels_handle`].
///
/// Cheap to clone, holds no lock of its own, and reading a panel waits on the
/// plugin process rather than on the server's plugin mutex.
#[derive(Clone)]
pub struct PluginPanelsHandle {
    plugins: Vec<Arc<PluginShared>>,
}

impl PluginPanelsHandle {
    /// Every panel contributed by a ready plugin, in discovery order.
    pub fn panels(&self) -> Vec<PluginPanels> {
        panels_of(&self.plugins)
    }

    /// Read one panel's payload from a ready plugin.
    ///
    /// `Err` carries the [`kc_core::Error`] the HTTP layer maps exactly like it
    /// maps [`PluginHost::set_enabled`]: `NotFound` for an unknown plugin or
    /// panel, `Config` for a plugin that is not running, `Plugin` for a plugin
    /// that failed to answer.
    pub async fn panel(
        &self,
        name: &str,
        panel: &str,
        ctx: &ToolCallContext,
    ) -> kc_core::Result<PanelPayload> {
        read_panel(&self.plugins, name, panel, ctx).await
    }
}

/// The body of [`PluginPanelsHandle::panel`], shared with the handle the server
/// takes so both go through one set of error messages.
async fn read_panel(
    plugins: &[Arc<PluginShared>],
    name: &str,
    panel: &str,
    ctx: &ToolCallContext,
) -> kc_core::Result<PanelPayload> {
    {
        let shared = plugins.iter().find(|s| s.name == name).ok_or_else(|| {
            kc_core::Error::NotFound(format!("plugin `{name}` is not configured"))
        })?;
        match shared.status() {
            PluginStatus::Ready => {}
            PluginStatus::Disabled => {
                return Err(kc_core::Error::Config(format!(
                    "plugin `{name}` is disabled; enable it in the plugin settings to read its \
                     panels"
                )))
            }
            PluginStatus::Failed | PluginStatus::Stopped | PluginStatus::Starting => {
                let reason = shared
                    .diagnostics()
                    .error
                    .unwrap_or_else(|| "not running".into());
                return Err(kc_core::Error::Plugin(format!(
                    "plugin `{name}` is not ready ({reason})"
                )));
            }
        }
        let def = shared.panel_def(panel).ok_or_else(|| {
            let mut known: Vec<String> = shared
                .panel_defs()
                .into_iter()
                .map(|p| hosted_panel_id(name, &p.id))
                .collect();
            known.sort();
            kc_core::Error::NotFound(format!(
                "plugin `{name}` has no panel `{panel}`; it declares [{}]",
                known.join(", ")
            ))
        })?;
        match shared.panel_output(&def, ctx).await {
            Ok(payload) => Ok(payload),
            Err(e) => Err(kc_core::Error::Plugin(format!(
                "panel `{}` of plugin `{name}` could not be read: {}",
                def.id,
                e.describe()
            ))),
        }
    }
}

fn panels_of(plugins: &[Arc<PluginShared>]) -> Vec<PluginPanels> {
    let mut out = Vec::new();
    for shared in plugins {
        if shared.status() != PluginStatus::Ready {
            continue;
        }
        let declared = shared.panel_defs();
        if declared.is_empty() {
            continue;
        }
        out.push(PluginPanels {
            plugin: shared.name.clone(),
            version: shared
                .manifest()
                .map(|m| m.version)
                .unwrap_or_else(|| "unknown".into()),
            panels: declared
                .into_iter()
                .map(|panel| PanelSummary::new(&shared.name, panel))
                .collect(),
        });
    }
    out
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

/// Largest single hook contribution injected into the system prompt.
pub(crate) const MAX_HOOK_NOTE_BYTES: usize = 4 * 1024;
/// Largest total of all hook contributions injected into the system prompt.
pub(crate) const MAX_HOOK_NOTES_BYTES: usize = 16 * 1024;
/// Marker appended to a contribution that was cut short.
pub(crate) const TRUNCATION_MARKER: &str = " [… truncated]";

/// Truncate `text` to at most `max` bytes on a char boundary, appending
/// [`TRUNCATION_MARKER`]. Returns the text and whether it was cut.
fn cap_text(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{TRUNCATION_MARKER}", &text[..end]), true)
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
    let mut total = 0usize;
    for value in fan_out(plugins, hooks::BEFORE_PROMPT, &payload).await {
        let Some(text) = value_to_text(&value) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        // Hook output lands in the system prompt verbatim, so it is capped like
        // tool output: a plugin must not be able to blow the context budget (or
        // bury the harness's own instructions) by returning megabytes.
        let (text, cut) = cap_text(&text, MAX_HOOK_NOTE_BYTES);
        if cut {
            warn!(
                hook = hooks::BEFORE_PROMPT,
                limit = MAX_HOOK_NOTE_BYTES,
                "a plugin's hook output was truncated before it reached the system prompt"
            );
        }
        let remaining = MAX_HOOK_NOTES_BYTES.saturating_sub(total);
        if remaining <= TRUNCATION_MARKER.len() {
            warn!(
                hook = hooks::BEFORE_PROMPT,
                limit = MAX_HOOK_NOTES_BYTES,
                "dropping plugin hook output: the total budget for prompt contributions is spent"
            );
            break;
        }
        let (text, cut) = cap_text(&text, remaining);
        if cut {
            warn!(
                hook = hooks::BEFORE_PROMPT,
                limit = MAX_HOOK_NOTES_BYTES,
                "plugin hook output was truncated: the total budget for prompt contributions is \
                 spent"
            );
        }
        total += text.len();
        out.push(text);
    }
    out
}

#[cfg(test)]
mod tests;
