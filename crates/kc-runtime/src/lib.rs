//! Agent runtime: the multi-step loop, cancellation, approvals, subagents,
//! event fan-out and context budgeting.

use kc_core::config::SubagentConfig;
use kc_core::error::Result;
use kc_core::events::EventEnvelope;
use kc_core::ids::{RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::provider::ProviderFactory;
use kc_core::settings::Settings;
use kc_core::store::Store;
use kc_core::tool::ToolRegistry;
use std::sync::Arc;
use tokio::sync::broadcast;

pub mod agent;
pub mod approvals;
pub mod context;
pub mod events;
pub mod subagent;

pub struct RuntimeDeps {
    pub store: Arc<dyn Store>,
    pub providers: Arc<dyn ProviderFactory>,
    pub tools: Arc<ToolRegistry>,
    pub settings: Arc<dyn Settings>,
    pub hooks: Arc<dyn kc_core::hooks::Hooks>,
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
    pub tools: std::sync::RwLock<Arc<ToolRegistry>>,
    pub runs: std::sync::Mutex<std::collections::HashMap<String, RunState>>,
    pub approvals: approvals::ApprovalRegistry,
    pub subagents: subagent::SubagentLimiter,
    pub seq: std::sync::atomic::AtomicU64,
}

pub(crate) struct RunState {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub cancel: tokio_util::sync::CancellationToken,
    pub started_at: i64,
    pub iterations: u32,
}

impl Runtime {
    pub fn new(deps: RuntimeDeps) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(RuntimeInner {
                deps,
                events: broadcast::channel(2048).0,
                tools: std::sync::RwLock::new(Arc::new(ToolRegistry::new())),
                runs: std::sync::Mutex::new(std::collections::HashMap::new()),
                approvals: approvals::ApprovalRegistry::new(),
                subagents: subagent::SubagentLimiter::new(SubagentConfig::default()),
                seq: std::sync::atomic::AtomicU64::new(0),
            }),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.inner.events.subscribe()
    }

    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.inner.tools.read().expect("tools lock").clone()
    }

    pub fn set_tools(&self, tools: Arc<ToolRegistry>) {
        *self.inner.tools.write().expect("tools lock") = tools;
    }

    pub async fn recover(&self) -> Result<RecoveryReport> {
        todo!("kc-runtime: recover interrupted state")
    }

    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: String) -> Result<RunHandle> {
        let _ = (session_id, prompt);
        todo!("kc-runtime: submit run")
    }

    pub fn cancel(&self, session_id: &str) -> bool {
        let _ = session_id;
        todo!("kc-runtime: cancel run")
    }

    pub fn active_runs(&self) -> Vec<ActiveRun> {
        Vec::new()
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.inner
            .runs
            .lock()
            .map(|r| r.values().any(|s| s.session_id.0 == session_id && !s.cancel.is_cancelled()))
            .unwrap_or(false)
    }

    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        decision: kc_core::approval::ApprovalDecision,
    ) -> Result<()> {
        let _ = (approval_id, decision);
        todo!("kc-runtime: resolve approval")
    }

    pub async fn shutdown(&self) {
        if let Ok(runs) = self.inner.runs.lock() {
            for run in runs.values() {
                run.cancel.cancel();
            }
        }
    }
}
