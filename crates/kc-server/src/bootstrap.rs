//! Process wiring: config -> data dir -> store -> settings -> runtime ->
//! plugins/MCP -> router -> bind -> graceful shutdown.
//!
//! Every startup failure names the thing that failed and the next action (port
//! in use, bad workspace, unreadable config), and exits non-zero through
//! `main`'s `anyhow::Result`.

use crate::settings::SettingsHandle;
use crate::state::{base_tool_registry, AppState, AppStateDeps, PluginHooks, ProjectRegistry};
use crate::static_ui::WebAssets;
use anyhow::{anyhow, bail};
use kc_core::config::Config;
use kc_core::ids::ProjectId;
use kc_core::paths::Workspace;
use kc_core::provider::ProviderFactory;
use kc_core::settings::Settings;
use kc_core::store::Project;
use kc_core::store::Store;
use kc_mcp::McpManager;
use kc_plugin_host::PluginHost;
use kc_provider::ProviderFactoryImpl;
use kc_runtime::{Runtime, RuntimeDeps};
use kc_store::SqliteStore;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// Parsed command line. Values override environment variables and the config
/// file (see `docs/` precedence: UI overrides > env > file > defaults; the CLI
/// sits above env for the process it starts).
#[derive(Debug, Default, Clone)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub workspace: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CliAction {
    Help,
    Version,
    Run(Cli),
}

impl Cli {
    pub fn usage() -> &'static str {
        "karucode - self-hosted coding agent

Usage: karucode [OPTIONS]

Options:
      --config <PATH>      Config file (default: $KARUCODE_CONFIG or ~/.config/karucode/config.toml)
      --host <HOST>        Bind address (default: 127.0.0.1)
      --port <PORT>        Listen port (default: 8787)
      --workspace <PATH>   Workspace root the agent may read and edit (default: current directory)
      --data-dir <PATH>    Data directory for the SQLite database and plugins
      --no-open            Accepted for compatibility; the server never opens a browser
  -h, --help               Print this help and exit
  -V, --version            Print the version and exit

Environment: KARUCODE_CONFIG, KARUCODE_HOST, KARUCODE_PORT, KARUCODE_WORKSPACE,
KARUCODE_DATA_DIR, KARUCODE_AUTH_TOKEN, KARUCODE_LOCK_SETTINGS, KARUCODE_LOG,
KARUCODE_API_KEY, KARUCODE_BASE_URL, KARUCODE_MODEL, KARUCODE_WEB_DIR,
KARUCODE_ALLOWED_HOSTS.

The server binds 127.0.0.1 by default. Binding a non-loopback address requires
KARUCODE_AUTH_TOKEN (at least 16 characters).

When the server is bound to a loopback address and no KARUCODE_AUTH_TOKEN is
configured, requests are only served for Host 127.0.0.1, localhost or [::1]
(DNS-rebinding defence). KARUCODE_ALLOWED_HOSTS adds reverse-proxy names as a
comma-separated list; entries are host, host:port or *.example.com."
    }

    /// Parse `args` (without the program name). `--help`/`--version` are
    /// handled before anything touches the filesystem or the environment.
    pub fn parse<I>(args: I) -> anyhow::Result<CliAction>
    where
        I: IntoIterator<Item = OsString>,
    {
        let args: Vec<OsString> = args.into_iter().collect();
        let mut cli = Cli::default();
        let mut i = 0usize;
        while i < args.len() {
            let raw = args[i].to_string_lossy().to_string();
            let (flag, inline) = match raw.split_once('=') {
                Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
                None => (raw.clone(), None),
            };
            match flag.as_str() {
                "-h" | "--help" => return Ok(CliAction::Help),
                "-V" | "--version" => return Ok(CliAction::Version),
                "--no-open" => {
                    if inline.is_some() {
                        bail!("`--no-open` does not take a value\n\n{}", Cli::usage());
                    }
                }
                "--config" | "--port" | "--host" | "--workspace" | "--data-dir" => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            i += 1;
                            let next = args.get(i).ok_or_else(|| {
                                anyhow!("`{flag}` requires a value\n\n{}", Cli::usage())
                            })?;
                            next.to_string_lossy().to_string()
                        }
                    };
                    if value.is_empty() {
                        bail!("`{flag}` requires a non-empty value");
                    }
                    match flag.as_str() {
                        "--config" => cli.config = Some(PathBuf::from(value)),
                        "--host" => cli.host = Some(value),
                        "--workspace" => cli.workspace = Some(PathBuf::from(value)),
                        "--data-dir" => cli.data_dir = Some(PathBuf::from(value)),
                        "--port" => {
                            cli.port = Some(value.parse::<u16>().map_err(|_| {
                                anyhow!(
                                    "`--port` must be a number between 1 and 65535 (got `{value}`)"
                                )
                            })?)
                        }
                        _ => unreachable!(),
                    }
                }
                other => {
                    if other.starts_with('-') {
                        bail!("unknown option `{other}`\n\n{}", Cli::usage());
                    }
                    bail!(
                        "unexpected argument `{other}`: karucode takes no positional arguments\n\n{}",
                        Cli::usage()
                    );
                }
            }
            i += 1;
        }
        Ok(CliAction::Run(cli))
    }
}

/// Load config, apply CLI overrides, validate, then start the server.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    // `Config::load` resolves the config path from the environment, so the CLI
    // flag is expressed as the highest-priority override.
    if let Some(path) = &cli.config {
        if !path.exists() {
            bail!(
                "config file {} does not exist; create it or omit --config",
                path.display()
            );
        }
        std::env::set_var("KARUCODE_CONFIG", path);
    }

    let mut config = Config::load().map_err(|e| anyhow!("could not load configuration: {e}"))?;

    if let Some(host) = cli.host {
        config.host = host;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(workspace) = cli.workspace {
        config.workspace_root = workspace;
    }
    if let Some(data_dir) = cli.data_dir {
        config.data_dir = data_dir;
    }

    validate_config(&config)?;
    init_tracing(&config.log_level);

    let workspace = Arc::new(Workspace::new(&config.workspace_root).map_err(|e| {
        anyhow!(
            "workspace {} is not usable: {e}. Pass --workspace <path> or set KARUCODE_WORKSPACE to an existing directory.",
            config.workspace_root.display()
        )
    })?);
    // Canonicalise so `/api/config` and `/api/health` report the same path the
    // file tools actually use.
    config.workspace_root = workspace.root().to_path_buf();

    ensure_private_dir(&config.data_dir)?;

    let db_path = config.db_path();
    let store: Arc<dyn Store> = Arc::new(
        SqliteStore::open(&db_path)
            .await
            .map_err(|e| anyhow!("could not open the database at {}: {e}", db_path.display()))?,
    );

    let settings = SettingsHandle::new(config.clone(), Arc::clone(&store))
        .await
        .map_err(|e| anyhow!("invalid settings: {e}"))?;

    let providers: Arc<dyn ProviderFactory> = Arc::new(ProviderFactoryImpl);

    // Plugin host and MCP manager start concurrently: a slow plugin must not
    // delay MCP servers and vice versa.
    let mut plugin_host = PluginHost::discover(&config.plugins, &config.data_dir);
    let mut mcp_manager = McpManager::new(&config.mcp_servers);
    tokio::join!(plugin_host.start_all(), mcp_manager.connect_all());
    let plugins = Arc::new(Mutex::new(plugin_host));
    let mcp = Arc::new(Mutex::new(mcp_manager));

    // Projects: seed the default one for the configured workspace, attach
    // sessions created before projects existed, then load the registry the
    // runtime resolves runs against.
    let projects = Arc::new(ProjectRegistry::new(Arc::clone(&workspace)));
    seed_projects(&store, &projects, &workspace).await?;

    let plugin_tools = {
        let host = plugins.lock().await;
        host.tools()
    };
    let mcp_tools = {
        let manager = mcp.lock().await;
        manager.tools()
    };
    let builtin_tools = kc_tools::builtin_tools();
    let tools = base_tool_registry(builtin_tools.clone(), plugin_tools, mcp_tools);

    let runtime = Runtime::new(RuntimeDeps {
        store: Arc::clone(&store),
        providers: Arc::clone(&providers),
        tools,
        settings: settings.clone(),
        hooks: Arc::new(PluginHooks::new(Arc::clone(&plugins))),
        workspaces: Arc::clone(&projects) as Arc<dyn kc_runtime::WorkspaceProvider>,
    });

    let recovery = match runtime.recover().await {
        Ok(recovery) => recovery,
        Err(e) => {
            // Plugin/MCP children are already running; stop them before exiting.
            plugins.lock().await.shutdown().await;
            mcp.lock().await.shutdown().await;
            return Err(anyhow!("startup recovery failed: {e}"));
        }
    };

    let state = AppState::new(AppStateDeps {
        settings: settings.clone(),
        store: Arc::clone(&store),
        runtime: Arc::clone(&runtime),
        providers,
        workspace,
        projects: Arc::clone(&projects),
        plugins: Arc::clone(&plugins),
        mcp: Arc::clone(&mcp),
        builtin_tools,
        web_dir: config.web_dir.clone(),
        db_path: db_path.clone(),
        bind_host: config.host.clone(),
        allowed_hosts: std::env::var(crate::host::ALLOWED_HOSTS_ENV).ok(),
    });
    state.reload_tools().await;

    // Log the allowlist when it actually gates requests, so an operator who
    // reaches the server through a proxy sees the 403 reason in the log.
    if state.enforce_host_allowlist() {
        tracing::info!(
            allowed_hosts = state.host_allowlist().description(),
            "loopback bind without a token: only allowlisted Host headers are served \
             (extend with KARUCODE_ALLOWED_HOSTS)"
        );
    }

    // Bind after the wiring so the startup summary can report real tool/plugin
    // counts, but stop any child processes if the port is taken.
    let listener = match bind(&config).await {
        Ok(listener) => listener,
        Err(e) => {
            plugins.lock().await.shutdown().await;
            mcp.lock().await.shutdown().await;
            return Err(e);
        }
    };
    let url = format!("http://{}", host_for_url(&config.host, config.port));

    let plugin_ready = {
        let host = plugins.lock().await;
        let diagnostics = host.diagnostics();
        let ready = diagnostics
            .iter()
            .filter(|d| d.status == kc_core::plugin::PluginStatus::Ready)
            .count();
        format!("{ready}/{} ready", diagnostics.len())
    };
    let mcp_status = {
        let manager = mcp.lock().await;
        let statuses = manager.statuses();
        let connected = statuses.iter().filter(|s| s.status == "connected").count();
        format!("{connected}/{} connected", statuses.len())
    };

    // The single startup summary line.
    let model = settings.model().await;
    let auth_label = if state.auth_token().is_some() {
        "bearer"
    } else {
        "off"
    };
    tracing::info!(
        url = %url,
        workspace = %state.workspace().root().display(),
        model = %model.model,
        base_url = %model.base_url,
        tools = state.tool_count(),
        plugins = %plugin_ready,
        mcp = %mcp_status,
        auth = %auth_label,
        interrupted_runs = recovery.interrupted_runs,
        interrupted_agents = recovery.interrupted_agents,
        expired_approvals = recovery.expired_approvals,
        web = %web_source(&config.web_dir),
        "karucode server listening"
    );

    let shutdown = state.shutdown_token();
    let signal_task = tokio::spawn(watch_signals(shutdown.clone()));

    let app = crate::app::build_router(state.clone());
    let serve = axum::serve(listener, app).with_graceful_shutdown({
        let token = shutdown.clone();
        async move { token.cancelled().await }
    });

    // The drain is bounded so a stuck keep-alive connection cannot hold the
    // process forever. Crucially this timer only starts after the shutdown token
    // fires: it must never bound the normal serving lifetime.
    let drain_deadline = {
        let token = shutdown.clone();
        async move {
            token.cancelled().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    };
    let serve_result = tokio::select! {
        result = serve => result.map_err(|e| anyhow!("server error: {e}")),
        _ = drain_deadline => {
            tracing::warn!("graceful shutdown timed out after 10s; closing remaining connections");
            Ok(())
        }
    };
    signal_task.abort();

    shutdown.cancel();
    runtime.shutdown().await;
    plugins.lock().await.shutdown().await;
    mcp.lock().await.shutdown().await;
    // Dropping the final store handle closes SQLite, which checkpoints the WAL.
    drop(state);
    drop(runtime);
    drop(settings);
    drop(store);

    if serve_result.is_ok() {
        tracing::info!("karucode stopped");
    }
    serve_result
}

fn validate_config(config: &Config) -> anyhow::Result<()> {
    if config.host.trim().is_empty() {
        bail!("host may not be empty (use --host 127.0.0.1)");
    }
    if config.port == 0 {
        bail!("port may not be 0; pass --port <1-65535>");
    }
    if let Some(token) = &config.auth_token {
        if token.len() < 16 {
            bail!(
                "auth token is too short ({} characters); use at least 16, e.g. `openssl rand -hex 24`",
                token.len()
            );
        }
    }
    if !config.is_local_bind() && config.auth_token.as_deref().unwrap_or("").is_empty() {
        bail!(
            "refusing to bind {} without KARUCODE_AUTH_TOKEN: remote access requires a bearer token of at least 16 characters",
            config.host
        );
    }
    crate::settings::validate_settings(config)
        .map_err(|e| anyhow!("invalid configuration: {e}"))?;
    Ok(())
}

fn init_tracing(level: &str) {
    let filter = match EnvFilter::try_new(level) {
        Ok(filter) => filter,
        Err(e) => {
            eprintln!("warning: invalid log filter `{level}` ({e}); using info");
            EnvFilter::new("info")
        }
    };
    // `try_init` keeps this harmless if a test harness already installed one.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Create the data directory with 0700 permissions (`docs/SECURITY.md`: the
/// settings table may hold an API key).
fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow!("cannot create the data directory {}: {e}", dir.display()))?;
    if !dir.is_dir() {
        bail!("data directory {} is not a directory", dir.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(dir)
            .map_err(|e| anyhow!("cannot inspect the data directory {}: {e}", dir.display()))?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
                anyhow!(
                    "cannot restrict the data directory {} to 0700: {e}",
                    dir.display()
                )
            })?;
        }
    }
    Ok(())
}

async fn bind(config: &Config) -> anyhow::Result<tokio::net::TcpListener> {
    // Brackets IPv6 literals; `TcpListener::bind` accepts a `host:port` string.
    let addr = host_for_url(&config.host, config.port);
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => Ok(listener),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => bail!(
            "port {} on {} is already in use: another karucode (or other process) is listening. Stop it or pass --port <other>.",
            config.port,
            config.host
        ),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => bail!(
            "cannot bind {}: permission denied. Ports below 1024 need elevated privileges; use --port >= 1024.",
            addr
        ),
        Err(e) => bail!("cannot bind {addr}: {e}"),
    }
}

fn web_source(web_dir: &Option<PathBuf>) -> String {
    match web_dir {
        Some(dir) => format!("disk:{}", dir.display()),
        None => {
            let assets = WebAssets::new(None);
            if assets.has_index() {
                "embedded:web/dist".to_string()
            } else {
                "placeholder (run `npm run build` in web/)".to_string()
            }
        }
    }
}

async fn watch_signals(shutdown: CancellationToken) {
    wait_for_signal().await;
    tracing::info!("shutdown signal received; draining connections");
    shutdown.cancel();
    // A second signal means "now"; cleanup gets no chance to hang.
    wait_for_signal().await;
    tracing::warn!("second shutdown signal; exiting immediately");
    std::process::exit(130);
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        Err(e) => {
            tracing::warn!("cannot listen for SIGTERM: {e}");
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Host:port for a URL, bracketing IPv6 literals.
fn host_for_url(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<CliAction> {
        Cli::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(parse(&["--help"]).unwrap(), CliAction::Help));
        assert!(matches!(parse(&["-h"]).unwrap(), CliAction::Help));
        assert!(matches!(parse(&["--version"]).unwrap(), CliAction::Version));
        assert!(matches!(parse(&["-V"]).unwrap(), CliAction::Version));
    }

    #[test]
    fn options_override_in_both_forms() {
        let CliAction::Run(cli) = parse(&[
            "--config",
            "/tmp/c.toml",
            "--port=9001",
            "--host",
            "0.0.0.0",
            "--workspace=/tmp/ws",
            "--data-dir",
            "/tmp/data",
            "--no-open",
        ])
        .unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/c.toml")));
        assert_eq!(cli.port, Some(9001));
        assert_eq!(cli.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.workspace, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/data")));
    }

    #[test]
    fn bad_input_is_rejected_with_usage() {
        let err = parse(&["--port", "zero"]).unwrap_err();
        assert!(err.to_string().contains("--port"));
        let err = parse(&["--wat"]).unwrap_err();
        assert!(err.to_string().contains("unknown option"));
        let err = parse(&["positional"]).unwrap_err();
        assert!(err.to_string().contains("positional"));
        assert!(parse(&["--port"]).is_err());
        assert!(parse(&["--no-open=1"]).is_err());
    }

    #[test]
    fn remote_bind_needs_a_token() {
        let config = Config {
            host: "0.0.0.0".into(),
            auth_token: None,
            ..Config::default()
        };
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("AUTH_TOKEN"));

        let config = Config {
            host: "0.0.0.0".into(),
            auth_token: Some("0123456789abcdef".into()),
            ..Config::default()
        };
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn short_tokens_and_zero_port_are_rejected() {
        let config = Config {
            auth_token: Some("short".into()),
            ..Config::default()
        };
        assert!(validate_config(&config).is_err());
        let config = Config {
            port: 0,
            ..Config::default()
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        assert_eq!(host_for_url("::1", 8787), "[::1]:8787");
        assert_eq!(host_for_url("127.0.0.1", 8787), "127.0.0.1:8787");
    }
}

/// Make sure every session has a project and the registry knows every project.
///
/// * no projects at all -> create one for the configured workspace;
/// * a session without `project_id` (created before projects existed, or left
///   behind by a project delete) -> attach it to the project with the same
///   directory, creating that project if this is the first session in it.
///
/// A session whose old workspace no longer exists keeps its row (and its
/// history) and is logged; it will run in that project once the directory is
/// back, instead of being silently moved to a different repository.
async fn seed_projects(
    store: &Arc<dyn kc_core::store::Store>,
    registry: &ProjectRegistry,
    workspace: &Arc<Workspace>,
) -> anyhow::Result<()> {
    let mut projects = store.list_projects().await?;
    if projects.is_empty() {
        let id = ProjectId::new();
        let name = project_name_for(workspace.root());
        let project = store
            .create_project(id.as_str(), &name, &workspace.root().to_string_lossy())
            .await?;
        tracing::info!(project = %project.id, path = %project.path, "created the default project");
        projects.push(project);
    }

    for session in store.list_sessions().await? {
        if session.project_id.is_some() {
            continue;
        }
        let raw = session.workspace.trim();
        if raw.is_empty() {
            continue;
        }
        let project = match projects.iter().find(|project| project.path == raw) {
            Some(project) => project.clone(),
            None => {
                let path = PathBuf::from(raw);
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                let id = ProjectId::new();
                let name = unique_project_name(&projects, project_name_for(&canonical));
                match store
                    .create_project(id.as_str(), &name, &canonical.to_string_lossy())
                    .await
                {
                    Ok(project) => {
                        tracing::info!(
                            project = %project.id,
                            path = %project.path,
                            "adopted a session workspace as a project"
                        );
                        projects.push(project.clone());
                        project
                    }
                    Err(error) => {
                        tracing::warn!(
                            session = %session.id,
                            path = %canonical.display(),
                            %error,
                            "could not adopt a legacy session workspace"
                        );
                        continue;
                    }
                }
            }
        };
        if let Err(error) = store.assign_session_project(&session.id, &project.id).await {
            tracing::warn!(
                session = %session.id,
                project = %project.id,
                %error,
                "could not attach a legacy session to its project"
            );
        }
    }

    for project in store.list_projects().await? {
        if let Err(error) = registry.insert(&project.id, Path::new(&project.path)) {
            tracing::warn!(
                project = %project.id,
                path = %project.path,
                %error,
                "project directory is not usable; sessions in it will not run until it is fixed"
            );
        }
    }
    Ok(())
}

fn project_name_for(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Project".to_string())
}

fn unique_project_name(existing: &[Project], base: String) -> String {
    if !existing.iter().any(|project| project.name == base) {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base} ({suffix})");
        if !existing.iter().any(|project| project.name == candidate) {
            return candidate;
        }
    }
    // Unreachable in practice (1000 same-named projects); still unique.
    format!("{base} ({})", crate::state::now_ms())
}
