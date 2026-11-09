//! Shared application state: the concrete store, runtime, provider factory,
//! plugin host, MCP manager and the live settings handle.
//!
//! [`AppState`] is a cheap `Arc` clone so axum can hand it to every handler.

// @kc-doc: projects
// @kc-title: Work on more than one project
// @kc-summary: How projects map to directories, how sessions are confined to one, and what happens when a project's directory moves or is deleted.
// @kc-tags: projects, sessions, workspace, directories
// @kc-applies-to: route:GET /api/projects, route:POST /api/projects, route:PATCH /api/projects/:id, route:PATCH /api/projects/:id/preferences, route:DELETE /api/projects/:id, route:GET /api/projects/:id/favicon, route:GET /api/sessions, route:PATCH /api/sessions/:id/model, config:workspace, env:KARUCODE_WORKSPACE, cmd:--workspace, ui:ProjectRail, concept:session workspace, concept:favicon.ico
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
// The one exception is the system temporary directory, which every workspace
// also allows (`allow_temp_dir`, on by default) so scratch files, build output
// and screenshots in `/tmp` are usable. It is an additional root, not a widening
// of the project's: `..` out of it and symlinks pointing elsewhere are still
// refused. `project_roots` below is a different setting - it decides where a
// project may be *created*, not what a run's tools may touch.
//
// # Which directories a project may be created in
//
// Any existing directory, except `/`. There is no allowlist by default: adding a
// second checkout that sits next to the workspace is the normal case and must not
// need an environment variable. `/` itself is refused, because a project's
// directory is the confinement root for its sessions and `/` would make every
// file tool able to read the whole disk.
//
// Pin the list with `project_roots` (`KARUCODE_PROJECT_ROOTS`, colon-separated)
// when the server is reachable by more than one person, or when you want the API
// unable to register a directory outside your projects. Paths are compared
// component-wise on canonical paths, so `/srv/app-evil` is not inside `/srv/app`.
//
// # The Workbench result preference
//
// The wide Workbench result divider stores its chosen pixel width on the project,
// via `PATCH /api/projects/:id/preferences` with `{"result_panel_width":580}`.
// `None`/an omitted field retains automatic sizing. The value is in the ordinary
// project list, not browser storage, so other devices and fresh profiles restore it.
// This preference never changes confinement, sessions or terminal state.
//
// # The project rail icon
//
// Each square in the project rail draws the project's own `favicon.ico` when it
// has one, and its initials when it does not. The file is found by
// `GET /api/projects/:id/favicon`, which searches that project's directory:
//
// * only the name `favicon.ico`, matched case-insensitively (a `favicon.svg` or
//   `apple-touch-icon.png` is not considered);
// * `.gitignore` rules apply - the project's own, nested ones and
//   `.git/info/exclude` - even outside a git repository, so a favicon in ignored
//   build output or a vendored copy is skipped. Your *global* gitignore is not
//   read, so the answer does not depend on which machine serves the project;
// * hidden directories, `node_modules` and `target` are skipped, the walk stops
//   after 6 levels and 20000 entries, and the shallowest match wins
//   (`favicon.ico` beats `public/favicon.ico` beats `web/public/favicon.ico`);
// * a file over 512 KiB, a symlink pointing outside the project, and an empty
//   file are all treated as "no icon".
//
// No icon is an empty `204 No Content`, which is the normal case and is not an
// error: the rail falls back to initials. It is deliberately not a `404` - a
// browser logs every 4xx as `Failed to load resource`, so an expected absence
// would look like a broken deployment. An unknown project id is still a `404`,
// because that one is a caller mistake.
//
// Nothing is cached server-side, so replacing a favicon shows up on the next
// load of the page. The bytes are fetched with the bearer token (an `<img src>`
// cannot send one) and turned into an object URL, so this works on a deployment
// with `KARUCODE_AUTH_TOKEN` set - and it is why the page CSP must allow
// `blob:` in `img-src`.
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
// * Ordinary project sessions have no per-session override: their project decides.
//   A control chat (or another project-less session) instead retains its recorded
//   workspace. Uploads without an explicit `project` follow that record and never
//   silently land in the default project.
// * Symlinked project roots are canonicalised on insert, so the stored path is the
//   real one.
//
// Full reference: `docs/ARCHITECTURE.md` (Workspace per run), `docs/API.md`
// (projects).

use crate::artifacts::ArtifactStore;
use crate::catalog::CatalogCache;
use crate::host::HostAllowlist;
use crate::settings::SettingsHandle;
use crate::static_ui::WebAssets;
use crate::terminals::TerminalRegistry;
use crate::web_auth::WebAuth;
use async_trait::async_trait;
use kc_core::error::{Error, Result};
use kc_core::hooks::Hooks;
use kc_core::paths::Workspace;
use kc_core::plugin::{PluginDiagnostics, PluginStatus};
use kc_core::provider::ProviderFactory;
use kc_core::store::Store;
use kc_core::tool::{Tool, ToolRegistry};
use kc_mcp::McpManager;
use kc_plugin_host::{namespaced_name, PluginHost, RescanReport};
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
///
/// Every workspace this registry builds carries the same *extra roots* (see
/// [`Workspace::with_extra_roots`]) - by default the system temporary directory,
/// because build tools, screenshots and browser profiles land there and refusing
/// them fails tool calls for no security gain. Holding them here, rather than
/// re-reading the settings per tool call, is what makes every project and every
/// subagent inherit the same allowlist.
pub struct ProjectRegistry {
    /// The workspace a session without a project runs in.
    ///
    /// `None` when the server was started without one: a fresh install has no
    /// project yet, and the operator picks the first one in the web UI. Callers
    /// that need a workspace must then resolve a project first.
    default_workspace: Option<Arc<Workspace>>,
    extra_roots: Vec<PathBuf>,
    projects: RwLock<HashMap<String, Arc<Workspace>>>,
}

impl ProjectRegistry {
    pub fn new(default_workspace: Arc<Workspace>) -> Self {
        Self::with_extra_roots(default_workspace, Vec::new())
    }

    /// A registry whose workspaces also allow `extra_roots`.
    ///
    /// The default workspace passed in is expected to have been built with the
    /// same roots (the bootstrap does that); this constructor re-applies them to
    /// every project it registers later.
    pub fn with_extra_roots(default_workspace: Arc<Workspace>, extra_roots: Vec<PathBuf>) -> Self {
        Self {
            default_workspace: Some(default_workspace),
            extra_roots,
            projects: RwLock::new(HashMap::new()),
        }
    }

    /// A registry with no default workspace: a session must name a project.
    pub fn without_default(extra_roots: Vec<PathBuf>) -> Self {
        Self {
            default_workspace: None,
            extra_roots,
            projects: RwLock::new(HashMap::new()),
        }
    }

    /// The extra roots every workspace in this registry allows.
    pub fn extra_roots(&self) -> &[PathBuf] {
        &self.extra_roots
    }

    /// The implicit workspace, when the server was started with one.
    pub fn default_workspace(&self) -> Option<Arc<Workspace>> {
        self.default_workspace.clone()
    }

    /// Canonicalise `root` and register it. The path must exist and be a
    /// directory; a project whose directory disappears later keeps its row but
    /// fails to resolve, with a message naming the directory.
    pub fn insert(&self, id: &str, root: &Path) -> Result<()> {
        let workspace = Arc::new(Workspace::with_extra_roots(root, self.extra_roots.clone())?);
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
    fn workspace_for(&self, project_id: Option<&str>) -> Result<Arc<Workspace>> {
        let Some(id) = project_id else {
            return self.default_workspace.clone().ok_or_else(|| {
                Error::BadRequest(
                    "no project selected: create or pick a project first (POST /api/projects)"
                        .into(),
                )
            });
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

    fn workspace_for_session(
        &self,
        project_id: Option<&str>,
        recorded: &str,
    ) -> Result<Arc<Workspace>> {
        if project_id.is_some() {
            return self.workspace_for(project_id);
        }
        // No project: the session's own recorded directory is the confinement
        // root. This is the control chat, which must never inherit a project.
        Ok(Arc::new(Workspace::with_extra_roots(
            recorded,
            self.extra_roots.clone(),
        )?))
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

    /// Obtain the live slot view, then release the administrative host lock before
    /// waiting on any out-of-process hook. The handle follows reload generations.
    async fn handle(&self) -> kc_plugin_host::PluginHooks {
        self.host.lock().await.hooks_handle()
    }
}

struct PluginNotificationForwarder {
    host: Arc<Mutex<PluginHost>>,
}

#[async_trait]
impl kc_core::hooks::NotificationObserver for PluginNotificationForwarder {
    async fn on_notify(&self, payload: serde_json::Value) {
        let observer = { self.host.lock().await.hooks_handle().notifier() };
        if let Some(observer) = observer {
            observer.on_notify(payload).await;
        }
    }
}

#[async_trait]
impl Hooks for PluginHooks {
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        self.handle()
            .await
            .before_prompt(session_id, workspace)
            .await
    }

    async fn before_prompt_for_turn(
        &self,
        session_id: &str,
        workspace: &str,
        turn: Option<&str>,
    ) -> Vec<String> {
        self.handle()
            .await
            .before_prompt_for_turn(session_id, workspace, turn)
            .await
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        self.handle()
            .await
            .on_run_end(session_id, run_id, status)
            .await
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        self.handle()
            .await
            .on_tool_result(session_id, tool, ok)
            .await
    }

    fn notifier(&self) -> Option<Arc<dyn kc_core::hooks::NotificationObserver>> {
        Some(Arc::new(PluginNotificationForwarder {
            host: Arc::clone(&self.host),
        }))
    }
}

/// Everything the HTTP layer needs. Constructed by the bootstrap with concrete
/// implementations; tests construct it with the same fields and fakes.
pub struct AppStateDeps {
    pub settings: Arc<SettingsHandle>,
    pub store: Arc<dyn Store>,
    pub runtime: Arc<Runtime>,
    pub providers: Arc<dyn ProviderFactory>,
    /// Project id -> workspace, already seeded by the bootstrap. It owns the
    /// implicit workspace too, which is `None` on a fresh install.
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
    /// Web-UI credentials and sessions.
    pub web_auth: Arc<WebAuth>,
}

pub struct AppStateInner {
    pub settings: Arc<SettingsHandle>,
    pub store: Arc<dyn Store>,
    pub runtime: Arc<Runtime>,
    pub providers: Arc<dyn ProviderFactory>,
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
    /// Live shells, keyed by terminal id. Empty after a restart by design: the
    /// processes are gone and a fresh shell starts on the next attach. The
    /// durable half is the `terminals` table.
    pub terminals: Arc<TerminalRegistry>,
    /// Chat artifact documents, keyed by the SHA-256 of their bytes. An in-memory
    /// cache with no persistence: the client re-posts what it is about to draw,
    /// so losing the store costs one round trip, not a message.
    pub artifacts: Arc<ArtifactStore>,
    pub started: Instant,
    /// Cancelled on SIGINT/SIGTERM so SSE streams end and the server can stop.
    pub shutdown: CancellationToken,
    /// One scheduler supervisor per mission, shared by all state clones.
    pub(crate) mission_schedulers:
        std::sync::Mutex<HashMap<String, Arc<crate::routes::missions::SchedulerWake>>>,
    /// Web-UI credentials and sessions (password, passkeys, cookies).
    pub web_auth: Arc<WebAuth>,
    /// The trusted-proxy policy, rebuilt when `[access]` changes.
    pub proxy: RwLock<CachedProxy>,
}

/// The trusted-proxy policy plus the `[access]` fingerprint it was built from.
///
/// Rebuilding on every request would parse 24 CIDR blocks for each one; not
/// rebuilding at all would make the "tunnel" setting a restart-only switch. The
/// fingerprint is the settings that feed the policy, so an in-place change is
/// picked up on the next request and an unchanged one costs a string compare.
pub struct CachedProxy {
    fingerprint: String,
    trust: crate::proxy::ProxyTrust,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) inner: Arc<AppStateInner>,
}

impl AppState {
    pub fn new(deps: AppStateDeps) -> Self {
        let web_auth = deps.web_auth;
        let trust = crate::proxy::ProxyTrust::from_access(&deps.settings.snapshot().access)
            .unwrap_or_else(|_| crate::proxy::ProxyTrust::disabled());
        Self {
            inner: Arc::new(AppStateInner {
                settings: deps.settings,
                store: deps.store,
                runtime: deps.runtime,
                providers: deps.providers,
                plugins: deps.plugins,
                mcp: deps.mcp,
                builtin_tools: deps.builtin_tools,
                web: WebAssets::new(deps.web_dir),
                db_path: deps.db_path,
                catalog: Arc::new(CatalogCache::new()),
                projects: deps.projects,
                host_allowlist: HostAllowlist::new(&deps.bind_host, deps.allowed_hosts.as_deref()),
                terminals: Arc::new(TerminalRegistry::new()),
                artifacts: Arc::new(ArtifactStore::new()),
                started: Instant::now(),
                shutdown: CancellationToken::new(),
                mission_schedulers: std::sync::Mutex::new(HashMap::new()),
                web_auth,
                proxy: RwLock::new(CachedProxy {
                    fingerprint: String::new(),
                    trust,
                }),
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

    /// The implicit workspace, when the server was started with one.
    ///
    /// `None` on a fresh install: nothing is chosen until the operator creates a
    /// project in the web UI.
    pub fn default_workspace(&self) -> Option<Arc<Workspace>> {
        self.inner.projects.default_workspace()
    }

    /// The implicit workspace, or the first registered project.
    ///
    /// Used for display and for the "which directory am I in" summary; callers
    /// that are about to *run* something must go through
    /// [`AppState::workspace_for_project`], which refuses when nothing is chosen.
    pub fn workspace_root_display(&self) -> Option<String> {
        self.default_workspace()
            .map(|workspace| workspace.root().to_string_lossy().into_owned())
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

    /// Resolve the workspace recorded on a session. Project-less sessions (most
    /// importantly control chats) keep their own root; they must never fall back
    /// to the install's default project merely because it exists.
    pub fn workspace_for_session(
        &self,
        project_id: Option<&str>,
        recorded: &str,
    ) -> std::result::Result<Arc<Workspace>, crate::error::ApiError> {
        self.inner
            .projects
            .workspace_for_session(project_id, recorded)
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

    /// The live shells. See [`TerminalRegistry`].
    pub fn terminals(&self) -> &Arc<TerminalRegistry> {
        &self.inner.terminals
    }

    /// The chat artifact documents. See [`ArtifactStore`].
    pub fn artifacts(&self) -> &Arc<ArtifactStore> {
        &self.inner.artifacts
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

    /// The web-UI credential store (password, passkeys, sessions).
    pub fn web_auth(&self) -> &Arc<WebAuth> {
        &self.inner.web_auth
    }

    /// The current `[access]` policy.
    pub fn access(&self) -> kc_core::config::AccessConfig {
        self.inner.settings.access()
    }

    /// Whether the web UI requires a password or passkey.
    ///
    /// True when `[access] enabled` is on **or** a bearer token is configured -
    /// either way the browser has to prove something before `/api` answers. The
    /// second half keeps a token-only deployment behaving exactly as before.
    pub fn sign_in_required(&self) -> bool {
        self.access().enabled || self.auth_token().is_some()
    }

    /// The trusted-proxy policy, rebuilt when the relevant settings change.
    pub fn proxy_trust(&self) -> crate::proxy::ProxyTrust {
        let access = self.access();
        let fingerprint = format!(
            "{}|{}|{}",
            access.tunnel.as_str(),
            access.public_url.as_deref().unwrap_or_default(),
            access.trusted_proxies.join(",")
        );
        {
            let cached = self.inner.proxy.read().unwrap_or_else(|e| e.into_inner());
            if cached.fingerprint == fingerprint {
                return cached.trust.clone();
            }
        }
        let trust = crate::proxy::ProxyTrust::from_access(&access)
            .unwrap_or_else(|_| crate::proxy::ProxyTrust::disabled());
        let mut cached = self.inner.proxy.write().unwrap_or_else(|e| e.into_inner());
        cached.fingerprint = fingerprint;
        cached.trust = trust.clone();
        trust
    }

    /// The configured `host[:port]` people reach this server at, if any.
    pub fn public_authority(&self) -> Option<String> {
        self.access().public_authority()
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
        registry.register(crate::control::tool(self.clone()));
        registry.register(crate::control::list_tool(self.clone()));
        for tool in crate::control_sessions::tools(self.clone()) {
            registry.register(tool);
        }
        registry.register(crate::mission_coordinator::tool(self.clone()));
        registry.register(crate::mission_coordinator::revise_tool(self.clone()));
        self.inner.runtime.set_tools(Arc::new(registry));
    }

    /// Re-read the plugin sources and reconcile the running processes with them.
    ///
    /// This is what `POST /api/plugins/reload` and the watcher call. It reads the
    /// `[[plugins]]` entries from the config file again (see
    /// [`Config::reload_plugin_entries`]) plus `<data_dir>/plugins/*/plugin.toml`,
    /// then adds, removes or replaces processes as needed and rebuilds the tool
    /// registry, so the model sees the new set on its very next call.
    ///
    /// Returns the report and the diagnostics of the lock is already dropped, so
    /// the caller can answer with both without waiting on another request.
    pub async fn reload_plugins(&self) -> (RescanReport, Vec<kc_core::plugin::PluginDiagnostics>) {
        let configs = self.inner.settings.snapshot().reload_plugin_entries();
        let (report, diagnostics) = {
            let mut host = self.inner.plugins.lock().await;
            let report = host.rescan(&configs).await;
            (report, host.diagnostics())
        };
        self.reload_tools().await;
        (report, diagnostics)
    }

    /// Reload one plugin by name: re-read its sources and replace its process.
    ///
    /// `Err` carries the plugin host's own error (`NotFound` for an unknown
    /// name), which the HTTP layer maps exactly like `set_enabled` does.
    pub async fn reload_plugin(
        &self,
        name: &str,
    ) -> kc_core::Result<kc_core::plugin::PluginDiagnostics> {
        let configs = self.inner.settings.snapshot().reload_plugin_entries();
        let diagnostics = {
            let mut host = self.inner.plugins.lock().await;
            host.reload(name, &configs).await?
        };
        self.reload_tools().await;
        Ok(diagnostics)
    }

    /// A cheap number that changes when a plugin source changes: the manifests
    /// and command files the host knows about, plus the config file itself (which
    /// is where `[[plugins]]` entries live). The watcher polls it and reloads only
    /// when it moves — unlike a filesystem event stream it cannot miss a write
    /// that happened while the server was busy.
    pub async fn plugin_source_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        {
            let host = self.inner.plugins.lock().await;
            host.source_stamp().hash(&mut hasher);
        }
        if let Some(path) = self.inner.settings.snapshot().config_path {
            path.hash(&mut hasher);
            if let Ok(meta) = std::fs::metadata(&path) {
                meta.len().hash(&mut hasher);
                meta.modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// `GET/PATCH /api/config` payload.
    pub async fn config_payload(&self) -> serde_json::Value {
        let mut payload = self.inner.settings.public_config().await;
        let plugins: Vec<serde_json::Value> = {
            let host = self.inner.plugins.lock().await;
            host.diagnostics().iter().map(plugin_summary).collect()
        };
        // Panels are grouped per plugin so the inspector knows which plugin (and
        // version) a tab belongs to without a second request. A plugin that
        // declares none contributes nothing.
        let plugin_panels = {
            let host = self.inner.plugins.lock().await;
            host.panels()
        };
        // The declarative UI layer: buttons, palette commands and hide requests,
        // plus the catalogues of slots and hideable elements this build knows.
        // Empty by default, so a server with no UI-contributing plugin sends an
        // object the frontend can ignore wholesale.
        let plugin_ui = {
            let host = self.inner.plugins.lock().await;
            host.ui()
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
            object.insert("plugin_panels".into(), json!(plugin_panels));
            object.insert("plugin_ui".into(), json!(plugin_ui));
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
