//! Process wiring: config -> data dir -> store -> settings -> runtime ->
//! plugins/MCP -> router -> bind -> graceful shutdown.
//!
//! Every startup failure names the thing that failed and the next action (port
//! in use, bad workspace, unreadable config), and exits non-zero through
//! `main`'s `anyhow::Result`.

// @kc-doc: run-server
// @kc-title: Build, run and stop the server
// @kc-summary: Build the UI and the binary, start it against a workspace, and know what each CLI flag and startup failure means.
// @kc-tags: install, build, run, cli, startup, server
// @kc-applies-to: cmd:karucode, cmd:--workspace, cmd:--port, cmd:--host, cmd:--data-dir, cmd:--config, cmd:--help, env:KARUCODE_HOST, env:KARUCODE_PORT, env:KARUCODE_WORKSPACE, env:KARUCODE_DATA_DIR, env:KARUCODE_AUTH_TOKEN, env:KARUCODE_LOG, env:KARUCODE_WEB_DIR, env:KARUCODE_ALLOWED_HOSTS, env:KARUCODE_SKIP_WEB_BUILD, env:KARUCODE_WEB_DIST, env:KARUCODE_NPM, cmd:npm run build, cmd:npm ci, route:GET /api/health, file:web/dist, file:crates/kc-server/build.rs
//
// # Build and run
//
// ```bash
// cargo run --release -p kc-server -- --workspace /path/to/your/project
// # open http://127.0.0.1:8787
// ```
//
// That is the whole build. `crates/kc-server/build.rs` runs `npm ci` (only when
// the dependency tree is missing or the lockfile moved) and `npm run build` when
// the frontend sources are newer than `web/dist`, and `rust-embed` then compiles
// `web/dist` into the binary. Debug and release builds both embed it, so the
// resulting `karucode` file runs on its own: no `web/dist` directory, no Node, no
// `KARUCODE_WEB_DIR`. Node.js 20+ is a *build-time* dependency, and only when you
// build the frontend from source.
//
// No Node on the build host, or a bundle built elsewhere?
//
// ```bash
// KARUCODE_WEB_DIST=/path/to/web/dist cargo build --release  # embed a prebuilt bundle
// KARUCODE_SKIP_WEB_BUILD=1 cargo build -p kc-server          # Rust only, placeholder page
// cargo build --release -p kc-server --no-default-features    # no UI embedded at all
// ```
//
// `KARUCODE_NPM=/path/to/npm` overrides the npm binary and `KARUCODE_WEB_DIST`
// replaces `web/dist` with the bundle you point at. A frontend build that fails
// fails `cargo build`, and a missing `web/dist` with no npm to rebuild it stops
// with the four options above instead of shipping a UI-less binary.
//
// While you are changing the UI, skip that round trip: `npm run dev` in `web/`
// serves Vite on :5173 and proxies `/api` to :8787, and
// `KARUCODE_WEB_DIR=web/dist` makes the server read the folder from disk instead
// of the embedded snapshot.
//
// # Flags
//
// | Flag | Default | Meaning |
// | --- | --- | --- |
// | `--workspace <PATH>` | none | optional default workspace; without it the server starts with no project and you create the first one in the web UI |
// | `--host <ADDR>` | `127.0.0.1` | bind address; a non-loopback bind requires `KARUCODE_AUTH_TOKEN` |
// | `--port <PORT>` | `8787` | listen port |
// | `--data-dir <PATH>` | `~/.local/share/karucode` | SQLite database and plugin data, forced to `0700` |
// | `--config <PATH>` | `$KARUCODE_CONFIG` or `~/.config/karucode/config.toml` | config file; must exist |
// | `--no-open` | - | accepted for compatibility; the server never opens a browser |
// | `-h`, `--version` | - | usage and version, handled before touching the filesystem |
//
// Every flag has an environment equivalent (`KARUCODE_HOST`, `KARUCODE_PORT`,
// `KARUCODE_WORKSPACE`, `KARUCODE_DATA_DIR`, `KARUCODE_CONFIG`); flags win for the
// process they start. `KARUCODE_LOG` sets the tracing filter
// (`info,karucode=debug`) and `KARUCODE_WEB_DIR` serves the UI from disk instead
// of the embedded copy, which is what you want while editing the frontend.
//
// # Subcommands
//
// `karucode plugin init <dir> [--name <name>] [--force]` writes a plugin project
// whose SDK is vendored, so it depends on nothing from karucode; see the
// `plugins` topic for the layout and the next steps. `karucode plugin sdk <dir>
// [--check]` refreshes that vendored copy after a karucode upgrade, and `--check`
// exits non-zero on drift so a plugin's CI can guard it.
//
// Neither subcommand starts the server, reads the config file or touches the
// data directory, and an existing file is never overwritten without `--force`.
//
// # Startup order
//
// Config -> data dir (`0700`) -> SQLite (`<data_dir>/karucode.db`, WAL) ->
// settings -> provider factory -> plugin host and MCP servers (concurrently) ->
// project registry -> recovery of interrupted runs -> bind -> serve. The single
// startup log line reports the URL, workspace, model, tool count, plugin/MCP
// status, auth mode, recovery counts and where the UI came from.
//
// # Health check
//
// ```bash
// curl -s localhost:8787/api/health | jq
// ```
//
// It answers before any model is configured, which makes it the right probe for a
// supervisor or container healthcheck.
//
// # Stopping
//
// `SIGINT`/`SIGTERM` cancel runs, expire pending approvals, shut plugins and MCP
// children down and checkpoint SQLite. Graceful drain is bounded at 10 s so a
// stuck keep-alive connection cannot hold the process forever. On the next start,
// leftover `running` runs and agents are marked `interrupted` and pending
// approvals `expired`; nothing is silently retried.
//
// # Startup failures you will actually see
//
// * `workspace ... is not usable` - you named a workspace that does not exist
//   (`--workspace`, `KARUCODE_WORKSPACE`, or `workspace =` in the config file).
//   Fix the path, or drop it and start with no project.
// * `refusing to bind ... without KARUCODE_AUTH_TOKEN` - remote binds need a
//   token of at least 16 characters.
// * `could not open the database at ...` - permissions or a corrupt/foreign
//   file; move it aside to start fresh.
// * `port ... in use` - another instance is running (plugin/MCP children are
//   stopped before exiting).
//
// # Starting without a workspace
//
// `--workspace` is optional. Without it the server starts with **no project**, an
// empty project rail, and `workspace: null` in `GET /api/health`; the first
// project is created in the web UI (`POST /api/projects`). Until then a session
// that names no project is refused with `400 bad_request` rather than running in
// some arbitrary directory.
//
// The one thing that is still fatal is naming a workspace that does not resolve:
// a typo in `--workspace` must not silently degrade to "start empty".
//
// # Gotchas
//
// * There is no built-in TLS. Bind loopback, or put a reverse proxy in front and
//   set `KARUCODE_ALLOWED_HOSTS` for its host names.
// * `--workspace` sets the *default* workspace; projects created in the UI have
//   their own directories.
// * The UI embed is a compile-time snapshot in debug builds too: after changing
//   the frontend, run `cargo build -p kc-server` again, or serve the folder
//   live with `KARUCODE_WEB_DIR=web/dist`.
// * `KARUCODE_WEB_DIR` is tried first but never required: if it points at a
//   directory with no `index.html`, the embedded copy is still served.
//
// Full reference: `README.md` (Quick start, Configuration).

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
    /// `karucode plugin init <dir> [--name <name>] [--force]`
    PluginInit(crate::scaffold::PluginInitArgs),
    /// `karucode plugin sdk <dir> [--check]`
    PluginSdk(crate::scaffold::PluginSdkArgs),
}

impl Cli {
    pub fn usage() -> &'static str {
        "karucode - self-hosted coding agent

Usage: karucode [OPTIONS]

Options:
      --config <PATH>      Config file (default: $KARUCODE_CONFIG or ~/.config/karucode/config.toml)
      --host <HOST>        Bind address (default: 127.0.0.1)
      --port <PORT>        Listen port (default: 8787)
      --workspace <PATH>   Optional default workspace (default: none; create a project in the UI)
      --data-dir <PATH>    Data directory for the SQLite database and plugins
      --no-open            Accepted for compatibility; the server never opens a browser
  -h, --help               Print this help and exit
  -V, --version            Print the version and exit

Commands:
  plugin init <dir>        Scaffold a plugin project; the SDK is vendored, so it
                           depends on nothing from karucode
  plugin sdk <dir>         Refresh a plugin's vendored SDK (--check exits 1 on drift)

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
        // `karucode plugin ...` is a subcommand, not a server flag: dispatch it
        // before the flag loop so the two grammars never mix.
        if args
            .first()
            .map(|a| a.to_string_lossy() == "plugin")
            .unwrap_or(false)
        {
            return parse_plugin(&args[1..]);
        }
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

/// Split `--flag=value` into its parts; `--flag value` is handled by
/// [`take_value`].
fn split_flag(word: &str) -> (String, Option<String>) {
    match word.split_once('=') {
        Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
        None => (word.to_string(), None),
    }
}

/// Value of a flag, either inline (`--name=x`) or the next word (`--name x`).
fn take_value(
    flag: &str,
    inline: Option<String>,
    words: &[String],
    i: &mut usize,
) -> anyhow::Result<String> {
    match inline {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => bail!("`{flag}` requires a non-empty value"),
        None => {
            *i += 1;
            let value = words
                .get(*i)
                .ok_or_else(|| anyhow!("`{flag}` requires a value\n\n{}", Cli::usage()))?;
            Ok(value.clone())
        }
    }
}

/// `karucode plugin <command> ...`: scaffolding only, never server startup, and
/// never touching the config file or the data directory.
fn parse_plugin(args: &[OsString]) -> anyhow::Result<CliAction> {
    let mut words = args.iter().map(|a| a.to_string_lossy().to_string());
    let command = match words.next() {
        Some(command) => command,
        None => bail!(
            "`karucode plugin` needs a command: `plugin init <dir>` or `plugin sdk <dir>`\n\n{}",
            Cli::usage()
        ),
    };
    if command == "-h" || command == "--help" || command == "help" {
        return Ok(CliAction::Help);
    }
    let words: Vec<String> = words.collect();
    match command.as_str() {
        "init" => {
            let (dir, name, force) = parse_plugin_init(&words)?;
            Ok(CliAction::PluginInit(crate::scaffold::PluginInitArgs {
                dir,
                name,
                force,
            }))
        }
        "sdk" => {
            let (dir, check) = parse_plugin_sdk(&words)?;
            Ok(CliAction::PluginSdk(crate::scaffold::PluginSdkArgs {
                dir,
                check,
            }))
        }
        other => bail!(
            "unknown `plugin` command `{other}`; expected `init` or `sdk`\n\n{}",
            Cli::usage()
        ),
    }
}

fn parse_plugin_init(words: &[String]) -> anyhow::Result<(PathBuf, Option<String>, bool)> {
    let mut dir: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut force = false;
    let mut i = 0usize;
    while i < words.len() {
        let (flag, inline) = split_flag(&words[i]);
        match flag.as_str() {
            "--name" => name = Some(take_value("--name", inline, words, &mut i)?),
            "--force" | "-f" => {
                if inline.is_some() {
                    bail!("`--force` does not take a value");
                }
                force = true;
            }
            other if other.starts_with('-') => {
                bail!("unknown option `{other}`\n\n{}", Cli::usage())
            }
            _ => {
                if dir.is_some() {
                    bail!(
                        "`plugin init` takes exactly one target directory; `{}` is extra\n\n{}",
                        words[i],
                        Cli::usage()
                    );
                }
                dir = Some(PathBuf::from(&words[i]));
            }
        }
        i += 1;
    }
    let dir = dir.ok_or_else(|| {
        anyhow!(
            "`plugin init` needs a target directory, e.g. `karucode plugin init ./my-plugin`\n\n{}",
            Cli::usage()
        )
    })?;
    Ok((dir, name, force))
}

fn parse_plugin_sdk(words: &[String]) -> anyhow::Result<(PathBuf, bool)> {
    let mut dir: Option<PathBuf> = None;
    let mut check = false;
    let mut i = 0usize;
    while i < words.len() {
        let (flag, inline) = split_flag(&words[i]);
        match flag.as_str() {
            "--check" => {
                if inline.is_some() {
                    bail!("`--check` does not take a value");
                }
                check = true;
            }
            other if other.starts_with('-') => {
                bail!("unknown option `{other}`\n\n{}", Cli::usage())
            }
            _ => {
                if dir.is_some() {
                    bail!(
                        "`plugin sdk` takes exactly one plugin directory; `{}` is extra\n\n{}",
                        words[i],
                        Cli::usage()
                    );
                }
                dir = Some(PathBuf::from(&words[i]));
            }
        }
        i += 1;
    }
    let dir = dir.ok_or_else(|| {
        anyhow!(
            "`plugin sdk` needs the plugin directory, e.g. `karucode plugin sdk ./my-plugin`\n\n{}",
            Cli::usage()
        )
    })?;
    Ok((dir, check))
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
        config.workspace_explicit = true;
    }
    if let Some(data_dir) = cli.data_dir {
        config.data_dir = data_dir;
    }

    validate_config(&config)?;
    init_tracing(&config.log_level);

    // The workspace is optional. A fresh install starts with none and the
    // operator creates the first project in the web UI; only an explicitly
    // requested `--workspace` that does not resolve is an error, because silently
    // ignoring it would hide a typo.
    let workspace = match Workspace::with_extra_roots(&config.workspace_root, config.extra_roots())
    {
        Ok(workspace) => {
            let workspace = Arc::new(workspace);
            // Canonicalise so `/api/config` and `/api/health` report the same
            // path the file tools actually use.
            config.workspace_root = workspace.root().to_path_buf();
            Some(workspace)
        }
        Err(error) if workspace_was_requested(&config) => {
            anyhow::bail!(
                "workspace {} is not usable: {error}. Pass --workspace <path> or set \
                 KARUCODE_WORKSPACE to an existing directory, or start without one and create \
                 a project in the web UI.",
                config.workspace_root.display()
            )
        }
        Err(_) => {
            tracing::info!(
                attempted = %config.workspace_root.display(),
                "no default workspace; create a project in the web UI"
            );
            None
        }
    };

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
    // Subscribe *before* `start_all`: a plugin that raises a notification during
    // its own startup would otherwise have it dropped on the floor.
    let plugin_notices = plugin_host.listen_notifications();
    let mut mcp_manager = McpManager::new(&config.mcp_servers);
    tokio::join!(plugin_host.start_all(), mcp_manager.connect_all());
    let plugins = Arc::new(Mutex::new(plugin_host));
    let mcp = Arc::new(Mutex::new(mcp_manager));

    // Projects: seed the default one for the configured workspace, attach
    // sessions created before projects existed, then load the registry the
    // runtime resolves runs against. The registry carries the same extra roots
    // as the default workspace, so a project created later (or a subagent, which
    // inherits its parent's workspace) can reach the temp dir too.
    let projects = Arc::new(match &workspace {
        Some(workspace) => ProjectRegistry::with_extra_roots(
            Arc::clone(workspace),
            workspace.extra_roots().to_vec(),
        ),
        None => ProjectRegistry::without_default(config.extra_roots()),
    });
    seed_projects(&store, &projects, workspace.as_ref()).await?;
    detach_control_sessions(&store, &config.data_dir).await?;

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
    for mission_id in state
        .store()
        .recover_mission_tasks()
        .await
        .map_err(|e| anyhow!("mission recovery failed: {e}"))?
    {
        crate::routes::missions::spawn_scheduler(state.clone(), mission_id);
    }

    // Plugin-raised notifications: drain the host channel into the same event
    // stream the runtime publishes on, so a plugin notification is
    // indistinguishable from a runtime one downstream.
    crate::plugin_notifications::spawn(state.clone(), plugin_notices);

    // Hot plugin reloads (`plugins_watch = true`): one `read_dir` plus a `stat`
    // per plugin command, on a timer. Off by default — a reload replaces
    // processes, so an operator opts in. The task stops itself on shutdown.
    if config.plugins_watch {
        let interval = Duration::from_secs(config.plugins_watch_secs);
        crate::plugins_watch::spawn(state.clone(), interval);
        tracing::info!(
            interval_secs = config.plugins_watch_secs,
            "plugin watcher enabled: changes to plugin manifests and command files are applied \
             without a restart"
        );
    }

    // Retention: drop transcript rows outside `retention_days` and hand the
    // freed pages back. Off by default (`0` = keep everything), so the task is
    // only started when the operator opted in at startup; it re-reads the
    // setting every tick, so changing the window (or turning it off) from the
    // settings sheet takes effect on the next pass. Turning it *on* for a server
    // started with `0` needs a restart, which is the price of not running a
    // timer for an install that never enabled the feature. The task stops itself
    // on shutdown.
    if crate::retention::should_prune(config.retention_days) {
        crate::retention::spawn(state.clone(), crate::retention::PRUNE_INTERVAL);
        tracing::info!(
            days = config.retention_days,
            interval_secs = crate::retention::PRUNE_INTERVAL.as_secs(),
            "retention enabled: messages older than this window are pruned, keeping the \
             newest messages of every session"
        );
    }

    // Log the allowlist when it actually gates requests, so an operator who
    // reaches the server through a proxy sees the 403 reason in the log.
    if state.enforce_host_allowlist() {
        tracing::info!(
            allowed_hosts = state.host_allowlist().description(),
            "loopback bind without a token: only allowlisted Host headers are served \
             (extend with KARUCODE_ALLOWED_HOSTS)"
        );
    }

    // Reap terminal shells nobody is attached to. A forgotten tab on a laptop
    // closed for the night must not hold a shell open, but the *tab* and its
    // title stay: attaching later starts a fresh shell in the same directory.
    // The interval is a fraction of the timeout so a 12-hour setting is not
    // reaped hours late, floored at a minute so a short timeout is not a busy
    // loop.
    let idle_secs = config.terminals.idle_timeout_secs;
    if config.terminals.enabled && idle_secs > 0 {
        let state_for_reaper = state.clone();
        let token = state.shutdown_token();
        let every = Duration::from_secs((idle_secs / 10).max(60));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(every) => {}
                }
                let current = state_for_reaper.settings().snapshot().terminals;
                if !current.enabled {
                    continue;
                }
                let reaped = state_for_reaper
                    .terminals()
                    .reap_idle(current.idle_timeout_secs);
                if !reaped.is_empty() {
                    tracing::info!(
                        terminals = %reaped.join(", "),
                        idle_secs = current.idle_timeout_secs,
                        "closed terminal shells with no connected browser; the tabs and their \
                         titles are kept, and attaching starts a fresh shell"
                    );
                }
            }
        });
    }

    // Hand freed heap back to the kernel, so a burst of plugin JSON, a long
    // transcript or a wide diff does not stay resident for the life of the
    // process. The walk runs off the async workers, and a trim that releases
    // nothing is a no-op - `KARUCODE_MALLOC_TRIM_SECS=0` turns it off. See
    // `memory.rs` for the measurements behind both knobs.
    if let Some(every) = crate::memory::trim_interval() {
        let token = state.shutdown_token();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(every) => {}
                }
                let released = tokio::task::spawn_blocking(crate::memory::trim_now)
                    .await
                    .unwrap_or(false);
                if released {
                    tracing::debug!("returned free heap to the kernel");
                }
            }
        });
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
        workspace = %state
            .workspace_root_display()
            .unwrap_or_else(|| "(none yet - create a project in the UI)".into()),
        model = %model.model,
        base_url = %model.base_url,
        tools = state.tool_count(),
        plugins = %plugin_ready,
        mcp = %mcp_status,
        auth = %auth_label,
        interrupted_runs = recovery.interrupted_runs,
        interrupted_agents = recovery.interrupted_agents,
        expired_approvals = recovery.expired_approvals,
        web = %web_source(&config.web_dir).await,
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
    // Hang up every terminal shell: they are children of this process, and a
    // `make watch` left in a tab must not outlive the server that started it.
    state.terminals().shutdown_all();
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

async fn web_source(web_dir: &Option<PathBuf>) -> String {
    match web_dir {
        Some(dir) => format!("disk:{}", dir.display()),
        None => {
            let assets = WebAssets::new(None);
            if assets.has_index().await {
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

/// Did the operator actually name a workspace, or is `workspace_root` just the
/// guess `Config::default()` made from the current directory?
///
/// This decides whether an unusable workspace is fatal. `--workspace`,
/// `KARUCODE_WORKSPACE` and `workspace =` in the config file are explicit asks, so
/// a typo there must fail loudly. The bare "current directory" default is not an
/// ask: a service whose `WorkingDirectory=` is missing, or a first run from an odd
/// cwd, should start empty and let the operator pick a project in the UI.
fn workspace_was_requested(config: &Config) -> bool {
    config.workspace_explicit
}

/// Keep control sessions out of projects, and give each its own directory.
///
/// A chat created by an older version was bound to a project (often the first one)
/// and inherited that project's workspace. It is detached here rather than on the
/// next click, so it never shows up in a project's session list and its tools never
/// touch project files. Each chat gets `<data_dir>/control-workspaces/<id>`.
async fn detach_control_sessions(
    store: &Arc<dyn kc_core::store::Store>,
    data_dir: &Path,
) -> anyhow::Result<()> {
    let base = data_dir.join("control-workspaces");
    tokio::fs::create_dir_all(&base).await?;
    let base = std::fs::canonicalize(&base)?;
    for session in store.list_control_sessions().await? {
        let recorded = std::fs::canonicalize(&session.workspace).ok();
        let inside = recorded
            .as_ref()
            .is_some_and(|path| path.starts_with(&base));
        if session.project_id.is_none() && inside {
            continue;
        }
        let target = base.join(&session.id);
        tokio::fs::create_dir_all(&target).await?;
        store
            .detach_control_session(&session.id, &target.to_string_lossy())
            .await?;
        tracing::info!(
            session = %session.id,
            path = %target.display(),
            "detached a control chat from its project"
        );
    }
    Ok(())
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
    workspace: Option<&Arc<Workspace>>,
) -> anyhow::Result<()> {
    let mut projects = store.list_projects().await?;
    // With no default workspace there is nothing to seed: a fresh install shows
    // an empty project rail and the operator creates the first project.
    let Some(workspace) = workspace else {
        for project in &projects {
            registry.insert(&project.id, std::path::Path::new(&project.path))?;
        }
        return Ok(());
    };
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
        // The control chat lives in a server directory on purpose. Adopting it as
        // a project on the next restart would put it back inside the project rail
        // and defeat the separation, so it is never attached.
        if store.is_control_session(&session.id).await.unwrap_or(false) {
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
    fn plugin_init_parses_both_value_forms() {
        let CliAction::PluginInit(args) = parse(&["plugin", "init", "/tmp/wordy"]).unwrap() else {
            panic!("expected a plugin init action");
        };
        assert_eq!(args.dir, PathBuf::from("/tmp/wordy"));
        assert_eq!(args.name, None, "the directory name is the default");
        assert!(!args.force);

        let CliAction::PluginInit(args) =
            parse(&["plugin", "init", "wordy", "--name=greeter", "--force"]).unwrap()
        else {
            panic!("expected a plugin init action");
        };
        assert_eq!(args.name.as_deref(), Some("greeter"));
        assert!(args.force);

        let CliAction::PluginInit(args) =
            parse(&["plugin", "init", "wordy", "--name", "greeter", "-f"]).unwrap()
        else {
            panic!("expected a plugin init action");
        };
        assert_eq!(args.name.as_deref(), Some("greeter"));
        assert!(args.force);
    }

    #[test]
    fn plugin_sdk_parses_and_check_is_opt_in() {
        let CliAction::PluginSdk(args) = parse(&["plugin", "sdk", "/tmp/wordy"]).unwrap() else {
            panic!("expected a plugin sdk action");
        };
        assert_eq!(args.dir, PathBuf::from("/tmp/wordy"));
        assert!(!args.check);

        let CliAction::PluginSdk(args) =
            parse(&["plugin", "sdk", "--check", "/tmp/wordy"]).unwrap()
        else {
            panic!("expected a plugin sdk action");
        };
        assert!(args.check);
    }

    #[test]
    fn plugin_usage_errors_name_the_fix() {
        let err = parse(&["plugin"]).unwrap_err().to_string();
        assert!(err.contains("needs a command"), "{err}");

        let err = parse(&["plugin", "deploy", "x"]).unwrap_err().to_string();
        assert!(err.contains("unknown `plugin` command `deploy`"), "{err}");
        assert!(err.contains("expected `init` or `sdk`"), "{err}");

        let err = parse(&["plugin", "init"]).unwrap_err().to_string();
        assert!(err.contains("needs a target directory"), "{err}");

        let err = parse(&["plugin", "init", "a", "b"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("exactly one target directory"), "{err}");

        let err = parse(&["plugin", "init", "a", "--name"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("`--name` requires a value"), "{err}");

        let err = parse(&["plugin", "sdk", "a", "--bogus"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown option `--bogus`"), "{err}");
    }

    #[test]
    fn plugin_help_is_help() {
        assert!(matches!(
            parse(&["plugin", "--help"]).unwrap(),
            CliAction::Help
        ));
        assert!(matches!(
            parse(&["plugin", "help"]).unwrap(),
            CliAction::Help
        ));
    }

    #[test]
    fn the_usage_text_lists_the_subcommands() {
        let usage = Cli::usage();
        assert!(usage.contains("plugin init <dir>"), "{usage}");
        assert!(usage.contains("plugin sdk <dir>"), "{usage}");
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
