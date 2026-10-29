//! Shared application state: the concrete store, runtime, provider factory,
//! plugin host, MCP manager and the live settings handle.
//!
//! [`AppState`] is a cheap `Arc` clone so axum can hand it to every handler.

// @kc-doc: projects
// @kc-title: Work on more than one project
// @kc-summary: How projects map to directories, how sessions are confined to one, and what happens when a project's directory moves or is deleted.
// @kc-tags: projects, sessions, workspace, directories
// @kc-applies-to: route:GET /api/projects, route:POST /api/projects, route:PATCH /api/projects/:id, route:DELETE /api/projects/:id, route:GET /api/sessions, route:PATCH /api/sessions/:id/model, config:workspace, env:KARUCODE_WORKSPACE, cmd:--workspace, ui:ProjectRail, concept:session workspace
//
// A **project** is a canonical directory plus a display name. It is not a git
// repository, a branch or a worktree - just the root every run in its sessions is
// confined to.
//
// # The default project
//
// On first start karucode creates one project for the configured workspace
// (`--workspace` / `KARUCODE_WORKSPACE`, default: the directory the server was
// started in). Sessions from before projects existed are attached to the project
// matching their recorded directory, created if necessary.
//
// # Creating and switching
//
// Use the project rail in the UI, or:
//
// ```bash
// curl -s -X POST localhost:8787/api/projects \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"path":"/home/me/other-repo","name":"other-repo"}'
// ```
//
// Creating a project does not move or copy files. Switching project changes which
// sessions are listed and which directory workspace, git and file requests answer
// for (`?project=<id>` on those routes).
//
// # Confinement
//
// A run's file tools resolve paths inside its session's project directory only:
// `..`, absolute paths outside it and symlink escapes are refused. Subagents
// inherit the workspace of the run that spawned them. The workspace explorer and
// `/api/workspace/*` use the same rule.
//
// # Deleting
//
// Deleting a project removes its rows from SQLite and never touches files on
// disk. It is refused while the project still has sessions unless you pass
// `?force=true`, in which case those sessions (messages, runs, agents, approvals,
// claims) are deleted with it; a session with a run in flight is never killed, so
// stop it first. This is deliberate: a misclick must not delete source code.
//
// # Gotchas
//
// * Deleting a directory outside karucode leaves the project row in place; it
//   keeps its history and fails to resolve with a message naming the path. Point
//   it at a new directory or delete the project.
// * The active model is install-wide by default, not per project: every project
//   shares it unless a session is pinned. A per-session pin
//   (`PATCH /api/sessions/:id/model`) travels with the session, not the project,
//   so two sessions in one project can run different models.
// * There is no per-session workspace override: the project (or the default
//   workspace) decides.
// * Symlinked project roots are canonicalised on insert, so the stored path is the
//   real one.
//
// Full reference: `README.md` (Projects), `docs/API.md` (projects).

use crate::catalog::CatalogCache;
use crate::host::HostAllowlist;
use crate::settings::SettingsHandle;
use crate::static_ui::WebAssets;
use async_trait::async_trait;
use kc_core::error::{Error, Result};
use kc_core::hooks::Hooks;
use kc_core::paths::Workspace;
use kc_core::plugin::{PluginDiagnostics, PluginStatus};
use kc_core::provider::ProviderFactory;
use kc_core::store::Store;
use kc_core::tool::{Tool, ToolRegistry};
use kc_mcp::McpManager;
use kc_plugin_host::{namespaced_name, PluginHost};
use kc_runtime::Runtime;
use kc_runtime::WorkspaceProvider;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => 0,
    }
}

/// Workspaces for the projects the server serves.
///
/// Projects live in SQLite, but resolving a project to a `Workspace` on every
/// tool call would mean hitting the database per file operation. The registry is
/// the in-memory, already-canonicalised view: it is loaded at startup and kept
/// current by the project routes. The default workspace (the `--workspace` /
/// `KARUCODE_WORKSPACE` directory) always exists and backs sessions that are not
/// attached to a project yet.
pub struct ProjectRegistry {
    default_workspace: Arc<Workspace>,
    projects: RwLock<HashMap<String, Arc<Workspace>>>,
}

impl ProjectRegistry {
    pub fn new(default_workspace: Arc<Workspace>) -> Self {
        Self {
            default_workspace,
            projects: RwLock::new(HashMap::new()),
        }
    }

    pub fn default_workspace(&self) -> Arc<Workspace> {
        Arc::clone(&self.default_workspace)
    }

    /// Canonicalise `root` and register it. The path must exist and be a
    /// directory; a project whose directory disappears later keeps its row but
    /// fails to resolve, with a message naming the directory.
    pub fn insert(&self, id: &str, root: &Path) -> Result<()> {
        let workspace = Arc::new(Workspace::new(root)?);
        self.projects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), workspace);
        Ok(())
    }

    /// Drop a project from the cache. Its rows/sessions are the store's job.
    pub fn remove(&self, id: &str) {
        self.projects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    pub fn contains(&self, id: &str) -> bool {
        self.projects
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.projects
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every registered project id and its root, sorted by id.
    pub fn roots(&self) -> Vec<(String, PathBuf)> {
        let projects = self.projects.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(String, PathBuf)> = projects
            .iter()
            .map(|(id, workspace)| (id.clone(), workspace.root().to_path_buf()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

impl WorkspaceProvider for ProjectRegistry {
    fn default_workspace(&self) -> Arc<Workspace> {
        ProjectRegistry::default_workspace(self)
    }

    fn workspace_for(&self, project_id: Option<&str>) -> Result<Arc<Workspace>> {
        let Some(id) = project_id else {
            return Ok(self.default_workspace());
        };
        self.projects
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "project `{id}` is not available; its directory may have been removed"
                ))
            })
    }
}

/// Adapter that lets the runtime use the (mutably owned) plugin host as its
/// [`Hooks`] implementation. Hooks are best-effort; a poisoned/slow host must
/// not fail a run, so locking uses the async mutex and calls are bounded inside
/// the plugin host itself.
pub struct PluginHooks {
    host: Arc<Mutex<PluginHost>>,
}

impl PluginHooks {
    pub fn new(host: Arc<Mutex<PluginHost>>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl Hooks for PluginHooks {
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        self.host
            .lock()
            .await
            .before_prompt(session_id, workspace)
            .await
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        self.host
            .lock()
            .await
            .on_run_end(session_id, run_id, status)
            .await
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        self.host
            .lock()
            .await
            .on_tool_result(session_id, tool, ok)
            .await
    }
}

/// Everything the HTTP layer needs. Constructed by the bootstrap with concrete
/// implementations; tests construct it with the same fields and fakes.
pub struct AppStateDeps {
    pub settings: Arc<SettingsHandle>,
    pub store: Arc<dyn Store>,
    pub runtime: Arc<Runtime>,
    pub providers: Arc<dyn ProviderFactory>,
    pub workspace: Arc<Workspace>,
    /// Project id -> workspace, already seeded by the bootstrap.
    pub projects: Arc<ProjectRegistry>,
    pub plugins: Arc<Mutex<PluginHost>>,
    pub mcp: Arc<Mutex<McpManager>>,
    /// First-party tools (plus test extras). Kept so `reload_tools` can rebuild
    /// the registry after a plugin is enabled or disabled.
    pub builtin_tools: Vec<Arc<dyn Tool>>,
    pub web_dir: Option<PathBuf>,
    pub db_path: PathBuf,
    /// The address the server bound (`--host` / `KARUCODE_HOST`). Together with
    /// the (live) auth token this decides whether the `Host` allowlist applies.
    pub bind_host: String,
    /// Raw `KARUCODE_ALLOWED_HOSTS` value; `None` when unset. Parsed once here so
    /// a malformed entry cannot be re-interpreted later.
    pub allowed_hosts: Option<String>,
}

pub struct AppStateInner {
    pub settings: Arc<SettingsHandle>,
    pub store: Arc<dyn Store>,
    pub runtime: Arc<Runtime>,
    pub providers: Arc<dyn ProviderFactory>,
    pub workspace: Arc<Workspace>,
    pub plugins: Arc<Mutex<PluginHost>>,
    pub mcp: Arc<Mutex<McpManager>>,
    pub builtin_tools: Vec<Arc<dyn Tool>>,
    pub web: WebAssets,
    pub db_path: PathBuf,
    /// models.dev catalog cache (provider/model metadata for the settings UI).
    pub catalog: Arc<CatalogCache>,
    /// Project id -> confined workspace, plus the default workspace.
    pub projects: Arc<ProjectRegistry>,
    /// DNS-rebinding `Host` allowlist (see [`crate::host`]).
    pub host_allowlist: HostAllowlist,
    pub started: Instant,
    /// Cancelled on SIGINT/SIGTERM so SSE streams end and the server can stop.
    pub shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) inner: Arc<AppStateInner>,
}

impl AppState {
    pub fn new(deps: AppStateDeps) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                settings: deps.settings,
                store: deps.store,
                runtime: deps.runtime,
                providers: deps.providers,
                workspace: Arc::clone(&deps.workspace),
                plugins: deps.plugins,
                mcp: deps.mcp,
                builtin_tools: deps.builtin_tools,
                web: WebAssets::new(deps.web_dir),
                db_path: deps.db_path,
                catalog: Arc::new(CatalogCache::new()),
                projects: deps.projects,
                host_allowlist: HostAllowlist::new(&deps.bind_host, deps.allowed_hosts.as_deref()),
                started: Instant::now(),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    pub fn settings(&self) -> &Arc<SettingsHandle> {
        &self.inner.settings
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.inner.runtime
    }

    pub fn store(&self) -> Arc<dyn Store> {
        Arc::clone(&self.inner.store)
    }

    pub fn workspace(&self) -> &Arc<Workspace> {
        &self.inner.workspace
    }

    /// The in-memory project registry (workspace resolution per project).
    pub fn projects(&self) -> &Arc<ProjectRegistry> {
        &self.inner.projects
    }

    /// Resolve a project id to its workspace. Unknown ids are a 404; a missing
    /// directory is reported with the path so the operator can fix it.
    pub fn workspace_for_project(
        &self,
        project_id: Option<&str>,
    ) -> std::result::Result<Arc<Workspace>, crate::error::ApiError> {
        self.inner
            .projects
            .workspace_for(project_id)
            .map_err(crate::error::ApiError::from)
    }

    pub fn providers(&self) -> &Arc<dyn ProviderFactory> {
        &self.inner.providers
    }

    pub fn plugins(&self) -> &Arc<Mutex<PluginHost>> {
        &self.inner.plugins
    }

    pub fn mcp(&self) -> &Arc<Mutex<McpManager>> {
        &self.inner.mcp
    }

    pub fn web(&self) -> &WebAssets {
        &self.inner.web
    }

    pub fn catalog(&self) -> &Arc<CatalogCache> {
        &self.inner.catalog
    }

    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    pub fn auth_token(&self) -> Option<String> {
        self.inner.settings.auth_token().filter(|t| !t.is_empty())
    }

    /// The parsed `Host` allowlist (loopback bind + `KARUCODE_ALLOWED_HOSTS`).
    pub fn host_allowlist(&self) -> &HostAllowlist {
        &self.inner.host_allowlist
    }

    /// True when a request must present an allowlisted `Host` header:
    /// loopback bind **and** no bearer token. A rebound page never has the token,
    /// so with a token configured the `Origin == Host` CSRF rule is enough and
    /// reverse proxies are free to rewrite `Host`.
    pub fn enforce_host_allowlist(&self) -> bool {
        self.inner.host_allowlist.bind_is_loopback() && self.auth_token().is_none()
    }

    pub fn started(&self) -> Instant {
        self.inner.started
    }

    pub fn uptime_secs(&self) -> u64 {
        self.inner.started.elapsed().as_secs()
    }

    pub fn redact(&self, text: &str) -> String {
        self.inner.settings.redact(text)
    }

    /// The authoritative tool count (server tools + runtime built-ins).
    pub fn tool_count(&self) -> usize {
        self.inner.runtime.tools().len()
    }

    /// Rebuild the registry from built-ins + ready plugins + MCP tools and hand
    /// it to the runtime. The runtime re-adds its own built-ins on top.
    pub async fn reload_tools(&self) {
        let mut registry = ToolRegistry::new();
        registry.register_all(self.inner.builtin_tools.clone());
        {
            let host = self.inner.plugins.lock().await;
            registry.register_all(host.tools());
        }
        {
            let mcp = self.inner.mcp.lock().await;
            registry.register_all(mcp.tools());
        }
        self.inner.runtime.set_tools(Arc::new(registry));
    }

    /// `GET/PATCH /api/config` payload.
    pub async fn config_payload(&self) -> serde_json::Value {
        let mut payload = self.inner.settings.public_config().await;
        let plugins: Vec<serde_json::Value> = {
            let host = self.inner.plugins.lock().await;
            host.diagnostics().iter().map(plugin_summary).collect()
        };
        let mcp_servers: Vec<serde_json::Value> = {
            let mcp = self.inner.mcp.lock().await;
            mcp.statuses().iter().map(mcp_summary).collect()
        };
        if let Some(object) = payload.as_object_mut() {
            // The ids `PATCH /api/config` accepts for `model.provider`; the UI
            // selector is driven by this list instead of hard-coding them.
            object.insert(
                "available_providers".into(),
                json!(self.inner.providers.supported()),
            );
            object.insert("plugins".into(), json!(plugins));
            object.insert("mcp_servers".into(), json!(mcp_servers));
        }
        payload
    }

    /// Health probe for the database. Returns a short status string, never an
    /// error body (and never the settings values it reads).
    pub async fn db_status(&self) -> &'static str {
        match self.inner.store.all_settings().await {
            Ok(_) => "ok",
            Err(e) => {
                tracing::warn!("database health probe failed: {e}");
                "error"
            }
        }
    }
}

pub fn plugin_status_str(status: &PluginStatus) -> &'static str {
    match status {
        PluginStatus::Disabled => "disabled",
        PluginStatus::Starting => "starting",
        PluginStatus::Ready => "ready",
        PluginStatus::Failed => "failed",
        PluginStatus::Stopped => "stopped",
    }
}

/// Summary used by `GET /api/config`.
fn plugin_summary(diagnostics: &PluginDiagnostics) -> serde_json::Value {
    let version = diagnostics
        .manifest
        .as_ref()
        .map(|m| m.version.clone())
        .unwrap_or_default();
    let permissions = diagnostics
        .manifest
        .as_ref()
        .map(|m| m.permissions.clone())
        .unwrap_or_default();
    let tools: Vec<String> = diagnostics
        .tools
        .iter()
        .map(|tool| namespaced_name(&diagnostics.name, &tool.name))
        .collect();
    json!({
        "name": diagnostics.name,
        "version": version,
        "status": plugin_status_str(&diagnostics.status),
        "tools": tools,
        "error": diagnostics.error,
        "permissions": permissions,
    })
}

/// Summary used by `GET /api/config` and `GET /api/mcp`.
pub fn mcp_summary(status: &kc_mcp::McpServerStatus) -> serde_json::Value {
    json!({
        "name": status.name,
        "transport": status.transport,
        "status": status.status,
        "tools": status.tools,
        "error": status.error,
    })
}

/// Build a fresh base registry (without runtime built-ins). Exposed for the
/// bootstrap so the initial [`kc_runtime::RuntimeDeps`] and the first
/// `reload_tools` agree.
pub fn base_tool_registry(
    builtin: Vec<Arc<dyn Tool>>,
    plugin_tools: Vec<Arc<dyn Tool>>,
    mcp_tools: Vec<Arc<dyn Tool>>,
) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register_all(builtin);
    registry.register_all(plugin_tools);
    registry.register_all(mcp_tools);
    Arc::new(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_strings_match_the_api() {
        assert_eq!(plugin_status_str(&PluginStatus::Ready), "ready");
        assert_eq!(plugin_status_str(&PluginStatus::Failed), "failed");
    }
}
