//! Background plugin watcher: with `plugins_watch = true` the server re-reads the
//! plugin sources on a timer and applies additions, removals and upgrades without
//! a restart.
//!
//! This is what makes editing a plugin a normal edit instead of a reboot: rebuild
//! the binary (or drop a `<data_dir>/plugins/<name>/plugin.toml`, or delete the
//! directory) and the next poll reconciles it — see
//! [`PluginHost::rescan`](kc_plugin_host::PluginHost::rescan) for exactly what is
//! added, dropped and replaced.
//!
//! **Polling, deliberately, not inotify.** One `read_dir` plus a `stat` per
//! configured command every [`Config::plugins_watch_secs`] is cheaper than a
//! dependency, works on the network filesystems and container bind mounts people
//! actually keep plugins on, cannot miss a write that happened while the server
//! was busy (event streams coalesce and drop), and cannot be defeated by an
//! editor that writes through a rename. The cost of a false positive is one
//! `stat`; the cost of a missed event is a plugin that silently runs stale code.
//!
//! [`Config::plugins_watch_secs`]: kc_core::config::Config::plugins_watch_secs

use crate::state::AppState;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// Start the watcher. Returns the task handle so the caller can keep it (the task
/// also stops itself when the server's shutdown token fires).
pub fn spawn(state: AppState, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let shutdown = state.shutdown_token();
        // The first stamp is taken before the loop so the initial state is not
        // mistaken for a change on the first tick.
        let mut stamp = state.plugin_source_stamp().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }
            let current = state.plugin_source_stamp().await;
            if current == stamp {
                continue;
            }
            stamp = current;
            let (report, _) = state.reload_plugins().await;
            if report.changed() {
                info!(
                    added = ?report.added,
                    removed = ?report.removed,
                    restarted = ?report.restarted,
                    "plugin sources changed; applied without a restart"
                );
            } else {
                // A file was touched but nothing needed doing (a rewrite with the
                // same content, a timestamp bump). Say it once at debug level so
                // the info log stays a record of real changes.
                debug!("plugin sources moved but nothing needed reloading");
            }
        }
        debug!("plugin watcher stopped");
    })
}
