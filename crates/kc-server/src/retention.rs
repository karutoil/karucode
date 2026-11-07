//! Background retention: with `retention_days > 0` the server periodically drops
//! transcript rows outside the retention window and hands the freed pages back to
//! the filesystem.
//!
//! Without a caller the store's `prune_messages_older_than` is dead code, and the
//! database grows forever: it is append-only by design (a rewind never renumbers
//! the tail), so nothing else ever deletes a message. This is that caller.
//!
//! **Hourly, not continuous.** [`PRUNE_INTERVAL`] is a compromise between a
//! growing file and a writer-locked scan: the pass takes the single writer
//! transaction (it deletes from `messages`, `runs`, `agents`, `approvals`,
//! `checkpoints` and `checkpoint_blobs` together, because a half-pruned session is
//! worse than an unpruned one), so running it every minute would park every write
//! behind a scan of the whole transcript table for a handful of rows. Once an hour
//! removes at most an hour's worth of expired rows while keeping the window at
//! `retention_days` to within an hour.
//!
//! **The setting is re-read every tick, not captured.** `retention_days` lives in
//! the settings snapshot, which `PATCH /api/config` and the settings sheet change
//! at runtime; a task that captured it at boot would keep pruning after the
//! operator set it to 0 (or keep doing nothing after they enabled it). So the
//! loop asks the current snapshot each tick and
//! [`should_prune`] decides, which is also the part a unit test can pin without
//! waiting an hour.
//!
//! [`Config::retention_days`]: kc_core::config::Config::retention_days

use crate::state::AppState;
use kc_core::store::PruneReport;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// How often a pass is attempted. See the module comment for why it is an hour
/// and not a minute.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Should this tick run a retention pass?
///
/// `0` means "keep everything" and is the default, so the decision is a plain
/// comparison rather than a special case at the call site. Split out so the
/// policy can be tested without a timer.
pub fn should_prune(days: u64) -> bool {
    days > 0
}

/// Start the retention task. Returns the task handle so the caller can keep it
/// (the task also stops itself when the server's shutdown token fires).
pub fn spawn(state: AppState, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let shutdown = state.shutdown_token();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }
            // Re-read every tick: the operator can change (or disable) retention
            // from the settings sheet while the server runs.
            run_once(&state).await;
        }
        debug!("retention task stopped");
    })
}

/// One pass: read the current window, prune, then reclaim if anything went.
///
/// Separate from the loop so the pass itself - the part that talks to the store
/// and decides whether to vacuum - can be tested without waiting an hour.
pub async fn run_once(state: &AppState) -> PruneReport {
    let days = state.settings().snapshot().retention_days;
    if !should_prune(days) {
        debug!("retention is off (retention_days = 0); nothing to prune");
        return PruneReport::default();
    }
    let store = state.store();
    let report = match store.prune_messages_older_than(days).await {
        Ok(report) => report,
        Err(e) => {
            // A failed pass must never take the server down, and it must be
            // visible: silently not pruning is how a disk fills up.
            warn!(
                error = %e,
                days,
                "retention pass failed; will retry on the next tick"
            );
            return PruneReport::default();
        }
    };
    if report.is_empty() {
        return report;
    }
    info!(
        days,
        messages = report.messages,
        runs = report.runs,
        agents = report.agents,
        approvals = report.approvals,
        claims = report.claims,
        checkpoints = report.checkpoints,
        checkpoint_blobs = report.checkpoint_blobs,
        "pruned rows outside the retention window"
    );
    // The deletes free pages inside the file but leave it the same size on
    // disk; incremental vacuum returns them, and it only has work to do after
    // something was actually deleted.
    if let Err(e) = store.incremental_vacuum().await {
        warn!(error = %e, "retention: could not reclaim the freed pages");
    }
    report
}

#[cfg(test)]
mod tests {
    use super::should_prune;

    #[test]
    fn zero_keeps_everything_and_a_positive_window_prunes() {
        assert!(
            !should_prune(0),
            "0 is the documented 'keep everything' default"
        );
        assert!(should_prune(1));
        assert!(should_prune(30));
        assert!(should_prune(u64::MAX));
    }
}
