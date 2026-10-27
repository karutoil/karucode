//! Subagent spawning, concurrency limits and status reporting.
//!
//! The runtime registers one built-in delegate tool, [`SPAWN_AGENT_TOOL`], into
//! the tool view it exposes. Its invocation runs a *nested* agent loop with:
//!
//! * a fresh message history (system prompt marked `is_subagent`, task as the
//!   user turn) persisted with the agent id so it never pollutes the parent
//!   conversation;
//! * iterations bounded by `settings.subagents.max_iterations` and wall clock by
//!   `timeout_secs` (with a hard backstop on top of the in-loop deadline);
//! * a tool set that excludes `spawn_agent` unless recursion is explicitly
//!   enabled *and* the depth budget allows it;
//! * approvals that keep the parent policy but carry the agent id;
//! * a concurrency limit of `settings.subagents.max_concurrent`.
//!
//! A subagent failure is reported as a failed tool result; it never fails the
//! parent run. Cancelling the parent cancels children through a child token.

use crate::agent::{LoopOutcome, LoopSpec};
use crate::approvals::RuntimeApprovalGate;
use crate::events::now_ms;
use crate::RuntimeInner;
use async_trait::async_trait;
use futures_util::FutureExt;
use kc_core::error::Result;
use kc_core::events::Event;
use kc_core::ids::AgentId;
use kc_core::message::Message;
use kc_core::store::{AgentRecord, AgentStatus, NewMessage, RunStatus};
use kc_core::tool::{
    truncate_output, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolRegistry, ToolSpec,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Name of the built-in delegate tool.
pub const SPAWN_AGENT_TOOL: &str = "spawn_agent";

/// How long a timed-out or cancelled subagent's tools get to observe the
/// cancellation token and kill their process groups before the loop future is
/// dropped. Dropping first would only kill the leader process (`kill_on_drop`),
/// leaving children behind.
const SUBAGENT_CANCEL_GRACE: Duration = Duration::from_millis(500);

/// Upper bound for a subagent summary returned to the parent and stored on the
/// agent record.
pub const MAX_SUMMARY_BYTES: usize = 8_000;

/// Counting limiter for concurrent subagents.
///
/// The limit is read from live settings on every acquire call, so a settings
/// change applies to the next spawn without restarting the runtime.
pub struct SubagentLimiter {
    pub config: kc_core::config::SubagentConfig,
    active: Mutex<usize>,
    peak: AtomicUsize,
    notify: Notify,
}

impl SubagentLimiter {
    pub fn new(config: kc_core::config::SubagentConfig) -> Self {
        Self {
            config,
            active: Mutex::new(0),
            peak: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    pub fn active(&self) -> usize {
        *self.active.lock().expect("subagent limiter")
    }

    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Wait until a slot is free. Returns `None` if `cancel` fires first.
    pub async fn acquire(
        self: &Arc<Self>,
        max: usize,
        cancel: &CancellationToken,
    ) -> Option<SubagentPermit> {
        let max = max.max(1);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut active = self.active.lock().expect("subagent limiter");
                if *active < max {
                    *active += 1;
                    self.peak.fetch_max(*active, Ordering::Relaxed);
                    return Some(SubagentPermit {
                        limiter: Arc::clone(self),
                    });
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = notified => {}
            }
        }
    }
}

/// Holds one concurrency slot; released on drop, even on early return.
pub struct SubagentPermit {
    limiter: Arc<SubagentLimiter>,
}

impl Drop for SubagentPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.limiter.active.lock() {
            *active = active.saturating_sub(1);
        }
        self.limiter.notify.notify_waiters();
    }
}

/// Build the `spawn_agent` tool bound to this runtime.
pub(crate) fn spawn_tool(inner: &Arc<RuntimeInner>) -> Arc<dyn Tool> {
    Arc::new(SpawnAgentTool {
        inner: Arc::downgrade(inner),
    })
}

pub(crate) struct SpawnAgentTool {
    inner: Weak<RuntimeInner>,
}

fn spec_definition() -> ToolSpec {
    ToolSpec {
        name: SPAWN_AGENT_TOOL.to_string(),
        description: "Spawn a focused subagent that works on one self-contained task in the same \
                      workspace and reports back a summary. Use it to parallelise independent \
                      investigations or reviews. The subagent gets its own conversation, cannot see \
                      this conversation, and cannot ask you questions."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Self-contained instruction: what to inspect or change, and what to report back."
                },
                "name": { "type": "string", "description": "Short display name, e.g. `reviewer`." },
                "role": { "type": "string", "description": "Role label shown in the UI, e.g. `reviewer`." },
                "owned_paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Workspace paths this subagent intends to modify; used to warn about overlaps."
                },
                "model": { "type": "string", "description": "Optional model override for this subagent." }
            },
            "required": ["task"],
            "additionalProperties": false
        }),
        origin: kc_core::tool::ToolOrigin::Builtin,
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn spec(&self) -> ToolSpec {
        spec_definition()
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down; cannot spawn a subagent");
        };
        match run_subagent(&inner, &request, &ctx).await {
            Ok(outcome) => outcome,
            Err(e) => ToolOutcome::err(format!("spawn_agent failed: {e}")),
        }
    }
}

async fn run_subagent(
    inner: &Arc<RuntimeInner>,
    request: &ToolCallRequest,
    ctx: &ToolCtx,
) -> Result<ToolOutcome> {
    let task = request
        .opt_str("task")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| kc_core::Error::BadRequest("missing non-empty `task` argument".into()))?;
    let role = request
        .opt_str("role")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "worker".to_string());
    let owned_paths: Vec<String> = match request.arguments.get("owned_paths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest("`owned_paths` must be an array of strings".into())
        })?,
    };
    let model_override = request
        .opt_str("model")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let cfg = ctx.settings.subagent_config().await;
    let child_depth = ctx.depth.saturating_add(1);
    if ctx.depth > 0 && !cfg.allow_recursive {
        return Ok(ToolOutcome::err(
            "recursive subagents are disabled (settings.subagents.allow_recursive = false)",
        ));
    }
    if child_depth > cfg.max_depth {
        return Ok(ToolOutcome::err(format!(
            "subagent depth limit reached (settings.subagents.max_depth = {})",
            cfg.max_depth
        )));
    }

    let Some(_permit) = inner
        .subagents
        .acquire(cfg.max_concurrent, &ctx.cancel)
        .await
    else {
        return Ok(ToolOutcome::err(
            "cancelled while waiting for a free subagent slot",
        ));
    };

    // --- register the agent ---------------------------------------------------
    let agent_id = AgentId::new();
    let short_id: String = {
        let tail: String = agent_id.0.chars().rev().take(8).collect();
        tail.chars().rev().collect()
    };
    let name = request
        .opt_str("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{role}-{short_id}"));
    let mut model = inner.deps.settings.model().await;
    if let Some(override_model) = &model_override {
        model.model = override_model.clone();
    }
    let limits = inner.deps.settings.limits().await;
    let policy = inner.deps.settings.approval_policy().await;
    let store = inner.deps.store.clone();
    let session_id = ctx.approvals.session_id.clone();
    let parent_run_id = ctx.approvals.run_id.clone();

    let mut record = AgentRecord {
        id: agent_id.0.clone(),
        session_id: session_id.0.clone(),
        parent_run_id: parent_run_id.0.clone(),
        parent_agent_id: ctx.approvals.agent_id.as_ref().map(|a| a.0.clone()),
        name: name.clone(),
        role: role.clone(),
        task: task.clone(),
        status: AgentStatus::Running.as_str().to_string(),
        model: model.model.clone(),
        iterations: 0,
        summary: String::new(),
        error: None,
        owned_paths: owned_paths.clone(),
        created_at: now_ms(),
        updated_at: now_ms(),
        finished_at: None,
    };
    store.upsert_agent(&record).await?;
    ctx.emit(Event::AgentSpawned {
        agent_id: agent_id.0.clone(),
        name: name.clone(),
        role: role.clone(),
        task: task.clone(),
        model: model.model.clone(),
        parent_run_id: parent_run_id.0.clone(),
    });

    // --- claims ---------------------------------------------------------------
    let mut warnings: Vec<String> = Vec::new();
    if !owned_paths.is_empty() {
        if let Err(e) = store
            .claim_paths(&session_id.0, &agent_id.0, &owned_paths, "write")
            .await
        {
            warnings.push(format!("could not record path claims: {e}"));
        }
        match store.list_claims(&session_id.0).await {
            Ok(claims) => {
                for claim in claims {
                    if claim.agent_id == agent_id.0 {
                        continue;
                    }
                    for path in &owned_paths {
                        if claim.path == *path {
                            warnings.push(format!(
                                "path `{path}` is also claimed by agent {}",
                                claim.agent_id
                            ));
                        }
                    }
                }
            }
            Err(e) => warnings.push(format!("could not list existing claims: {e}")),
        }
    }

    // --- child conversation ---------------------------------------------------
    let child_cancel = ctx.cancel.child_token();
    let mut task_message = NewMessage::from_message(
        &session_id.0,
        Some(&parent_run_id.0),
        &Message::user(task.clone()),
    );
    task_message.agent_id = Some(agent_id.0.clone());
    if let Err(e) = store.append_message(&task_message).await {
        record.status = AgentStatus::Failed.as_str().to_string();
        record.error = Some(format!("could not persist the subagent task: {e}"));
        record.updated_at = now_ms();
        record.finished_at = Some(now_ms());
        if store.upsert_agent(&record).await.is_ok() {
            ctx.emit(Event::AgentFinished {
                agent_id: agent_id.0.clone(),
                status: AgentStatus::Failed.as_str().to_string(),
                summary: String::new(),
                error: record.error.clone(),
            });
        }
        return Err(e);
    }

    // --- nested loop ----------------------------------------------------------
    let child_tools =
        child_tool_registry(inner, cfg.allow_recursive && child_depth < cfg.max_depth);
    let gate = RuntimeApprovalGate::new(
        Arc::clone(inner),
        session_id.clone(),
        parent_run_id.clone(),
        Some(agent_id.clone()),
        child_cancel.clone(),
        child_depth,
    );
    let approval_scope = gate.scope();
    let gate: Arc<dyn kc_core::tool::ApprovalGate> = Arc::new(gate);
    let child_state = Arc::new(crate::RunState::new(
        parent_run_id.clone(),
        session_id.clone(),
        Some(agent_id.clone()),
        child_cancel.clone(),
        false,
    ));
    let child_key = child_state.key();
    inner
        .runs
        .lock()
        .expect("runs lock")
        .insert(child_key.clone(), Arc::clone(&child_state));

    let timeout_secs = cfg.timeout_secs.max(1);
    let spec = LoopSpec {
        session_id: session_id.clone(),
        run_id: parent_run_id.clone(),
        agent_id: Some(agent_id.clone()),
        depth: child_depth,
        is_subagent: true,
        max_iterations: cfg.max_iterations.max(1),
        time_budget_secs: timeout_secs,
        deadline: Instant::now() + Duration::from_secs(timeout_secs),
        model: model.clone(),
        limits,
        approval_mode: policy.mode.as_str().to_string(),
        gate,
        approval_scope,
        tools: Arc::new(child_tools),
        cancel: child_cancel.clone(),
        history: vec![Message::user(task.clone())],
        run_state: Some(Arc::clone(&child_state)),
    };

    let guarded = std::panic::AssertUnwindSafe(crate::agent::run_loop(Arc::clone(inner), spec))
        .catch_unwind();
    tokio::pin!(guarded);
    let hard_deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(hard_deadline);

    enum Exit {
        Completed(LoopOutcome),
        TimedOut,
        ParentCancelled,
    }

    let exit = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Exit::ParentCancelled,
        _ = &mut hard_deadline => Exit::TimedOut,
        result = guarded.as_mut() => Exit::Completed(match result {
            Ok(outcome) => outcome,
            Err(payload) => LoopOutcome {
                status: RunStatus::Failed,
                error: Some(format!(
                    "subagent panicked: {}",
                    crate::agent::panic_message(payload.as_ref())
                )),
                text: String::new(),
                iterations: child_state.iterations.load(Ordering::Relaxed),
            },
        }),
    };

    let timed_out = matches!(exit, Exit::TimedOut);
    let outcome = match exit {
        Exit::Completed(outcome) => outcome,
        Exit::TimedOut | Exit::ParentCancelled => {
            // Fire the child token, then keep polling the loop for a short grace
            // period: in-flight tools select on the token, kill their process
            // groups and return. Only then is the future dropped.
            child_cancel.cancel();
            let _ = tokio::time::timeout(SUBAGENT_CANCEL_GRACE, guarded.as_mut()).await;

            let iterations = child_state.iterations.load(Ordering::Relaxed);
            if timed_out {
                LoopOutcome {
                    status: RunStatus::Failed,
                    error: Some(format!(
                        "subagent timed out after {timeout_secs}s (settings.subagents.timeout_secs)"
                    )),
                    text: String::new(),
                    iterations,
                }
            } else {
                LoopOutcome {
                    status: RunStatus::Cancelled,
                    error: None,
                    text: String::new(),
                    iterations,
                }
            }
        }
    };
    inner.runs.lock().expect("runs lock").remove(&child_key);

    // --- finish ---------------------------------------------------------------
    let (summary_raw, _) = truncate_output(&outcome.text, MAX_SUMMARY_BYTES);
    let summary = if summary_raw.trim().is_empty() {
        outcome.error.clone().unwrap_or_default()
    } else {
        summary_raw
    };
    let agent_status = match outcome.status {
        RunStatus::Completed => AgentStatus::Completed,
        RunStatus::Cancelled => AgentStatus::Cancelled,
        RunStatus::Failed => AgentStatus::Failed,
        RunStatus::LimitReached => AgentStatus::Failed,
        RunStatus::Interrupted => AgentStatus::Interrupted,
        RunStatus::Running => AgentStatus::Running,
    };
    let error = match outcome.status {
        RunStatus::LimitReached => Some(
            outcome
                .error
                .clone()
                .unwrap_or_else(|| "subagent stopped at its iteration or time limit".to_string()),
        ),
        _ => outcome.error.clone(),
    };

    record.status = agent_status.as_str().to_string();
    record.iterations = outcome.iterations;
    record.summary = summary.clone();
    record.error = error.clone();
    record.updated_at = now_ms();
    record.finished_at = Some(now_ms());
    if let Err(e) = store.upsert_agent(&record).await {
        warnings.push(format!("could not update the agent record: {e}"));
    }
    if let Err(e) = store.release_claims(&agent_id.0).await {
        warnings.push(format!("could not release path claims: {e}"));
    }
    ctx.emit(Event::AgentFinished {
        agent_id: agent_id.0.clone(),
        status: agent_status.as_str().to_string(),
        summary: summary.clone(),
        error: error.clone(),
    });

    let mut content = String::new();
    content.push_str(&format!(
        "subagent `{}` ({name}, {role}) \u{2014} {}\niterations: {}\n",
        agent_id.0,
        agent_status.as_str(),
        outcome.iterations
    ));
    if !warnings.is_empty() {
        content.push_str("warnings:\n");
        for warning in &warnings {
            content.push_str(&format!("- {warning}\n"));
        }
    }
    content.push('\n');
    content.push_str(if summary.trim().is_empty() {
        "(no summary produced)"
    } else {
        &summary
    });
    let (content, cut) = truncate_output(&content, MAX_SUMMARY_BYTES);

    let ok = matches!(agent_status, AgentStatus::Completed);
    Ok(ToolOutcome {
        ok,
        content,
        truncated: cut,
        meta: Some(serde_json::json!({
            "agent_id": agent_id.0,
            "status": agent_status.as_str(),
            "iterations": outcome.iterations,
            "error": error,
            "warnings": warnings,
        })),
    })
}

/// Parent tool set minus `spawn_agent`, unless recursion is allowed at this
/// depth (in which case the delegate tool stays available).
fn child_tool_registry(inner: &Arc<RuntimeInner>, allow_recursive: bool) -> ToolRegistry {
    let parent = inner.tool_view();
    let mut registry = ToolRegistry::new();
    for name in parent.names() {
        if name == SPAWN_AGENT_TOOL {
            continue;
        }
        if let Some(tool) = parent.get(&name) {
            registry.register(tool);
        }
    }
    if allow_recursive {
        match parent.get(SPAWN_AGENT_TOOL) {
            Some(tool) => registry.register(tool),
            None => registry.register(spawn_tool(inner)),
        }
    }
    registry
}

/// Convenience for embedders that build their own registry: install the
/// runtime built-ins into `registry`.
pub fn install_builtins(registry: &mut ToolRegistry, runtime: &Arc<crate::Runtime>) {
    registry.register(spawn_tool(runtime.shared()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_counts_and_releases() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let a = limiter.acquire(1, &cancel).await.unwrap();
            assert_eq!(limiter.active(), 1);
            assert_eq!(limiter.peak(), 1);
            let blocked = Arc::clone(&limiter);
            let c2 = cancel.clone();
            let waiter = tokio::spawn(async move {
                let permit = blocked.acquire(1, &c2).await;
                permit.is_some()
            });
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!waiter.is_finished());
            drop(a);
            assert_eq!(limiter.active(), 0);
            assert!(waiter.await.unwrap());
            assert_eq!(limiter.peak(), 1);
        });
    }

    #[test]
    fn limiter_gives_up_on_cancel() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let _held = limiter.acquire(1, &cancel).await.unwrap();
            let c2 = cancel.clone();
            let blocked = Arc::clone(&limiter);
            let waiter = tokio::spawn(async move { blocked.acquire(1, &c2).await.is_some() });
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
            assert!(!waiter.await.unwrap());
        });
    }

    #[test]
    fn spec_is_a_delegate_tool_with_required_task() {
        struct Dummy;
        #[async_trait]
        impl Tool for Dummy {
            fn spec(&self) -> ToolSpec {
                spec_definition()
            }
            async fn invoke(&self, _r: ToolCallRequest, _c: ToolCtx) -> ToolOutcome {
                ToolOutcome::ok("")
            }
        }
        let spec = Dummy.spec();
        assert_eq!(spec.name, SPAWN_AGENT_TOOL);
        assert_eq!(Dummy.class(), ToolClass::ReadOnly); // default class, not the spec's
        assert_eq!(spec.origin, kc_core::tool::ToolOrigin::Builtin);
        assert_eq!(spec.parameters["required"], serde_json::json!(["task"]));
        assert!(spec.description.contains("subagent"));
    }
}
