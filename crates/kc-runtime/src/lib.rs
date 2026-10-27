//! Agent runtime: the multi-step loop, cancellation, approvals, subagents,
//! event fan-out and context budgeting.
//!
//! The runtime implements the agent loop purely against the traits in
//! [`kc_core`] (`Store`, `ProviderFactory`, `Settings`, `Tool`, `Hooks`). The
//! server wires the concrete implementations in; tests wire fakes in.
//!
//! # Built-in tools
//!
//! On construction the runtime installs its own delegate tool (`spawn_agent`)
//! into the tool *view* it exposes through [`Runtime::tools`]. The registry the
//! server passes in [`RuntimeDeps::tools`] is left untouched, and
//! [`Runtime::set_tools`] re-installs the built-ins on top of whatever the server
//! supplies, so polling `Runtime::tools()` is always the authoritative set to
//! show to the model. [`Runtime::builtin_tool_names`] lists them.

use kc_core::error::Result;
use kc_core::events::EventEnvelope;
use kc_core::hooks::Hooks;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::provider::ProviderFactory;
use kc_core::settings::Settings;
use kc_core::store::Store;
use kc_core::tool::ToolRegistry;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod agent;
pub mod approvals;
pub mod context;
pub mod events;
pub(crate) mod history;
pub mod subagent;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_support;

/// How many stored messages a new run loads as conversation history.
pub const HISTORY_LIMIT: usize = 400;

/// Broadcast channel capacity: a slow SSE consumer lags and is skipped rather
/// than blocking the run.
pub const EVENT_CHANNEL_CAPACITY: usize = 2048;

pub struct RuntimeDeps {
    pub store: Arc<dyn Store>,
    pub providers: Arc<dyn ProviderFactory>,
    pub tools: Arc<ToolRegistry>,
    pub settings: Arc<dyn Settings>,
    pub hooks: Arc<dyn Hooks>,
    pub workspace: Arc<Workspace>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunHandle {
    pub run_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveRun {
    pub run_id: String,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub started_at: i64,
    pub iterations: u32,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RecoveryReport {
    pub interrupted_runs: usize,
    pub interrupted_agents: usize,
    pub expired_approvals: usize,
}

pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

pub(crate) struct RuntimeInner {
    pub deps: RuntimeDeps,
    pub events: broadcast::Sender<EventEnvelope>,
    /// The authoritative tool view: server tools plus runtime built-ins.
    pub tools: RwLock<Arc<ToolRegistry>>,
    /// In-flight runs, keyed by run id for top-level runs and by agent id for
    /// subagent loops.
    pub runs: Mutex<HashMap<String, Arc<RunState>>>,
    pub approvals: approvals::ApprovalRegistry,
    pub subagents: Arc<subagent::SubagentLimiter>,
    pub seq: AtomicU64,
    pub events_emitted: AtomicU64,
    /// `allow_session` approvals, keyed by session + kind + target.
    pub session_allow: Mutex<HashSet<String>>,
}

pub(crate) struct RunState {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub agent_id: Option<AgentId>,
    pub cancel: CancellationToken,
    pub started_at: i64,
    pub iterations: AtomicU32,
    /// True for a user-submitted run (one per session at a time), false for a
    /// subagent loop.
    pub top_level: bool,
}

impl RunState {
    pub(crate) fn new(
        run_id: RunId,
        session_id: SessionId,
        agent_id: Option<AgentId>,
        cancel: CancellationToken,
        top_level: bool,
    ) -> Self {
        Self {
            run_id,
            session_id,
            agent_id,
            cancel,
            started_at: events::now_ms(),
            iterations: AtomicU32::new(0),
            top_level,
        }
    }

    pub(crate) fn key(&self) -> String {
        match &self.agent_id {
            Some(a) => a.0.clone(),
            None => self.run_id.0.clone(),
        }
    }
}

impl Runtime {
    pub fn new(deps: RuntimeDeps) -> Arc<Self> {
        let inner = Arc::new(RuntimeInner {
            events: broadcast::channel(EVENT_CHANNEL_CAPACITY).0,
            tools: RwLock::new(Arc::new(ToolRegistry::new())),
            runs: Mutex::new(HashMap::new()),
            approvals: approvals::ApprovalRegistry::new(),
            subagents: Arc::new(subagent::SubagentLimiter::new(Default::default())),
            seq: AtomicU64::new(0),
            events_emitted: AtomicU64::new(0),
            session_allow: Mutex::new(HashSet::new()),
            deps,
        });
        let view = Arc::new(Self::tool_view_with_builtins(&inner, &inner.deps.tools));
        *inner.tools.write().expect("tools lock") = view;
        Arc::new(Self { inner })
    }

    /// Names of the tools the runtime itself contributes to the registry view.
    pub fn builtin_tool_names() -> &'static [&'static str] {
        &[subagent::SPAWN_AGENT_TOOL]
    }

    pub(crate) fn shared(&self) -> &Arc<RuntimeInner> {
        &self.inner
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.inner.events.subscribe()
    }

    /// The authoritative tool registry: server-registered tools plus the
    /// runtime built-ins.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.inner.tools.read().expect("tools lock").clone()
    }

    /// Replace the tool registry. Built-ins are re-installed on top of the new
    /// set, so `spawn_agent` cannot be accidentally dropped by a reload.
    pub fn set_tools(&self, tools: Arc<ToolRegistry>) {
        let view = Arc::new(Self::tool_view_with_builtins(&self.inner, &tools));
        *self.inner.tools.write().expect("tools lock") = view;
    }

    fn tool_view_with_builtins(inner: &Arc<RuntimeInner>, base: &ToolRegistry) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for name in base.names() {
            if Self::builtin_tool_names().contains(&name.as_str()) {
                continue;
            }
            if let Some(tool) = base.get(&name) {
                registry.register(tool);
            }
        }
        registry.register(subagent::spawn_tool(inner));
        registry
    }

    /// Stamp with the runtime seq/ts, attach context and publish one event.
    ///
    /// Used by the server for session lifecycle events (created/renamed/deleted)
    /// so the SSE sequence stays monotonic across runtime and server events.
    pub fn emit(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
        agent_id: Option<&str>,
        event: kc_core::events::Event,
    ) -> EventEnvelope {
        self.inner.emit(session_id, run_id, agent_id, event)
    }

    /// Number of events published since process start.
    pub fn events_emitted(&self) -> u64 {
        self.inner.events_emitted.load(Ordering::Relaxed)
    }

    /// Forget every `allow_session` grant remembered for a session. Call this
    /// when a session is deleted so grants cannot be inherited by a new session
    /// that happens to reuse the id.
    pub fn clear_session_allows(&self, session_id: &str) {
        self.inner.clear_session_allows(session_id);
    }

    /// Startup recovery: leftover `running` runs and agents become
    /// `interrupted`; approvals still pending become `expired`. Returns counts.
    pub async fn recover(&self) -> Result<RecoveryReport> {
        let store = &self.inner.deps.store;
        let interrupted_runs = store.mark_interrupted_runs().await?;
        let interrupted_agents = store.mark_interrupted_agents().await?;

        // A crash can leave an assistant turn whose tool result was never
        // persisted. Repair every session's transcript before serving requests,
        // so the first run after a restart does not replay an invalid exchange.
        for session in store.list_sessions().await? {
            history::repair_history(store, &session.id, HISTORY_LIMIT).await?;
        }

        let pending = store.list_pending_approvals(None).await?;
        let mut run_ids: Vec<String> = Vec::new();
        for approval in &pending {
            if !run_ids.contains(&approval.run_id) {
                run_ids.push(approval.run_id.clone());
            }
        }
        let mut expired_approvals = 0usize;
        for run_id in run_ids {
            expired_approvals += store
                .expire_pending_approvals(&run_id, "server restarted")
                .await?;
        }

        Ok(RecoveryReport {
            interrupted_runs,
            interrupted_agents,
            expired_approvals,
        })
    }

    /// Start a run for `prompt` in `session_id`.
    ///
    /// The run row and the user message are written before this returns, so the
    /// session view is consistent as soon as the caller got the handle. The
    /// model loop itself runs in a background task.
    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: String) -> Result<RunHandle> {
        agent::start_run(self, session_id, prompt).await
    }

    /// Cancel every run belonging to `session_id` (including subagents).
    /// Returns `false` when nothing was running.
    pub fn cancel(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        let mut cancelled = false;
        for state in runs.values() {
            if state.session_id.0 == session_id {
                state.cancel.cancel();
                cancelled = true;
            }
        }
        cancelled
    }

    /// Every in-flight loop: user runs (`agent_id: None`) and nested subagent
    /// loops (`agent_id: Some`), sorted by session and run id.
    pub fn active_runs(&self) -> Vec<ActiveRun> {
        let runs = self.inner.runs.lock().expect("runs lock");
        let mut out: Vec<ActiveRun> = runs
            .values()
            .map(|state| ActiveRun {
                run_id: state.run_id.0.clone(),
                session_id: state.session_id.0.clone(),
                agent_id: state.agent_id.as_ref().map(|a| a.0.clone()),
                started_at: state.started_at,
                iterations: state.iterations.load(Ordering::Relaxed),
            })
            .collect();
        out.sort_by(|a, b| {
            (a.session_id.as_str(), a.run_id.as_str())
                .cmp(&(b.session_id.as_str(), b.run_id.as_str()))
        });
        out
    }

    /// True while a run for this session is registered and has not been
    /// cancelled. Cancelled-but-unwinding runs still block a new `submit`, which
    /// is why `submit` does its own check on the run map.
    pub fn is_running(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        runs.values()
            .any(|s| s.session_id.0 == session_id && !s.cancel.is_cancelled())
    }

    /// Resolve a pending approval. Unknown, already resolved or expired ids are
    /// a 404-style [`Error::NotFound`].
    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        decision: kc_core::approval::ApprovalDecision,
    ) -> Result<()> {
        approvals::resolve(Arc::clone(&self.inner), approval_id, decision).await
    }

    /// Cancel every run and wait briefly for the loops to unwind.
    pub async fn shutdown(&self) {
        {
            let runs = self.inner.runs.lock().expect("runs lock");
            for state in runs.values() {
                state.cancel.cancel();
            }
        }
        // Wake gates parked on an approval: their cancel token will also fire,
        // but this makes the unwinding immediate and releases every waiter.
        self.inner.approvals.cancel_all();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let empty = self.inner.runs.lock().map(|r| r.is_empty()).unwrap_or(true);
            if empty {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tracing::warn!("runtime shutdown: some runs did not stop within 5s");
    }
}

impl RuntimeInner {
    pub(crate) fn tool_view(&self) -> Arc<ToolRegistry> {
        self.tools.read().expect("tools lock").clone()
    }
}
