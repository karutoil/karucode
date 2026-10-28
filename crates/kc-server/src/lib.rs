//! HTTP API, SSE event stream and static frontend serving.
//!
//! The binary is a thin wrapper around [`run`]. Everything testable lives in
//! modules: [`settings`] (layered config), [`state`] (wiring types), [`app`]
//! (router + security middleware), [`routes`] (handlers), [`static_ui`] (asset
//! serving) and [`bootstrap`] (process startup/shutdown).
//!
//! # Router construction
//!
//! ```no_run
//! # async fn example(state: kc_server::state::AppState) {
//! let router = kc_server::build_router(state);
//! # let _ = router;
//! # }
//! ```
//!
//! `Router::new().nest("/api", routes::api_router()).fallback(static_ui::serve_spa)`
//! with four layers, outermost first: security headers, bearer auth, the
//! `Host` guard (DNS rebinding, see [`host`]) and CSRF.

pub mod app;
pub mod bootstrap;
pub mod catalog;
pub mod error;
pub mod host;
pub mod routes;
pub mod settings;
pub mod state;
pub mod static_ui;

pub use app::build_router;
pub use bootstrap::{Cli, CliAction};
pub use catalog::CatalogCache;
pub use host::HostAllowlist;
pub use settings::{ConfigPatch, SettingsHandle};
pub use state::{AppState, AppStateDeps};

/// Crate version, reported by `GET /api/health` and `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parse the process arguments and either print help/version or run the server.
///
/// `main.rs` is intentionally a two-liner; keeping this here makes the CLI
/// parseable in unit tests and leaves room for a future library embedding.
pub async fn run() -> anyhow::Result<()> {
    match Cli::parse(std::env::args_os().skip(1))? {
        CliAction::Help => {
            println!("{}", Cli::usage());
            Ok(())
        }
        CliAction::Version => {
            println!("karucode {VERSION}");
            Ok(())
        }
        CliAction::Run(cli) => bootstrap::run(cli).await,
    }
}
