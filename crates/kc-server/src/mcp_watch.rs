//! Background MCP watcher: with `mcp_watch = true` the server re-reads
//! `[[mcp_servers]]` on a timer and applies additions, removals and changes
//! without a restart.
//!
//! This is what makes editing an MCP server entry a normal edit instead of a
//! reboot: save the config file and the next poll reconciles it — see
//! [`McpManager::rescan`](kc_mcp::McpManager::rescan) for exactly what is added,
//! dropped and replaced.
//!
//! **Polling, deliberately, not inotify.** One `stat` of the config file every
//! [`Config::mcp_watch_secs`] is cheaper than a dependency, works on the network
//! filesystems and container bind mounts people actually keep config files on,
//! cannot miss a write that happened while the server was busy (event streams
//! coalesce and drop), and cannot be defeated by an editor that writes through a
//! rename. The cost of a false positive is one `stat`; the cost of a missed event
//! is a server that silently runs the old entry.
//!
//! Unlike the plugin watcher this one waits for the file to stop moving before it
//! applies it. A config file is often written by truncating and then writing, and
//! an empty intermediate read would mean "no servers" — every process stopped and
//! started again a tick later. Two agreeing polls cost one interval and remove
//! that churn.
//!
//! [`Config::mcp_watch_secs`]: kc_core::config::Config::mcp_watch_secs

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
        let mut stamp = state.mcp_source_stamp().await;
        'watch: loop {
            tokio::select! {
                _ = shutdown.cancelled() => break 'watch,
                _ = tokio::time::sleep(interval) => {}
            }
            let mut current = state.mcp_source_stamp().await;
            if current == stamp {
                continue;
            }
            // Wait until two consecutive polls agree: a truncate-then-write would
            // otherwise be applied as an empty `[[mcp_servers]]` table.
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break 'watch,
                    _ = tokio::time::sleep(interval) => {}
                }
                let next = state.mcp_source_stamp().await;
                if next == current {
                    break;
                }
                current = next;
            }
            stamp = current;
            let report = state.reload_mcp().await;
            if report.changed() {
                info!(
                    added = ?report.added,
                    removed = ?report.removed,
                    restarted = ?report.restarted,
                    retried = ?report.retried,
                    "MCP server entries changed; applied without a restart"
                );
            } else {
                // A file was touched but nothing needed doing (a rewrite with the
                // same content, a timestamp bump). Say it once at debug level so
                // the info log stays a record of real changes.
                debug!("MCP config moved but nothing needed reloading");
            }
        }
        debug!("MCP watcher stopped");
    })
}
