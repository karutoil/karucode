//! Process-wide run lifecycle counters.
//!
//! These live in `kc-core` rather than in the server's metrics registry because
//! the run lifecycle is owned by `kc-runtime`, which must not depend on
//! `kc-server` (that would be a cycle). The server reads them when it renders
//! `/metrics`; the runtime increments them as runs start and reach a terminal
//! state.
//!
//! Deliberately just two monotonic counters. Anything richer (per-model, per
//! status) belongs in the event log, which is already persisted and queryable;
//! these exist so an operator can answer "is this install doing work, and is it
//! finishing it" from a scrape without reading the database.

use std::sync::atomic::{AtomicU64, Ordering};

static RUNS_STARTED: AtomicU64 = AtomicU64::new(0);
static RUNS_FINISHED: AtomicU64 = AtomicU64::new(0);

/// Count a run that just started (top-level or subagent).
pub fn record_run_started() {
    RUNS_STARTED.fetch_add(1, Ordering::Relaxed);
}

/// Count a run that reached a terminal state, whatever it was.
pub fn record_run_finished() {
    RUNS_FINISHED.fetch_add(1, Ordering::Relaxed);
}

/// `(started, finished)` since process start.
pub fn run_totals() -> (u64, u64) {
    (
        RUNS_STARTED.load(Ordering::Relaxed),
        RUNS_FINISHED.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_monotonic() {
        let (started, finished) = run_totals();
        record_run_started();
        record_run_finished();
        let (started_after, finished_after) = run_totals();
        assert_eq!(started_after, started + 1);
        assert_eq!(finished_after, finished + 1);
    }
}
