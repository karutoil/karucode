//! Human-in-the-loop approval plumbing.
//!
//! [`RuntimeApprovalGate`] turns an [`ApprovalRequest`] coming from a tool into
//! one of: an automatic decision derived from the approval policy, an in-memory
//! `allow_session` grant, or a pending request persisted in the store that the
//! UI resolves through [`Runtime::resolve_approval`](crate::Runtime).
//!
//! Ordering guarantees: the store row exists *before* `ApprovalRequested` is
//! emitted, so a client reacting to the event can always resolve the id it saw.

use crate::events::{now_ms, EmitCtx};
use crate::RuntimeInner;
use async_trait::async_trait;
use kc_core::approval::{
    ApprovalDecision, ApprovalKind, ApprovalPolicyView, ApprovalRequest, AutoDecision,
};
use kc_core::error::{Error, Result};
use kc_core::events::Event;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::store::ApprovalRecord;
use kc_core::tool::ApprovalGate;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// In-process waiters for pending approvals, keyed by approval id.
pub struct ApprovalRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
}

impl Default for ApprovalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register interest in an approval id and get the receiver to await.
    pub fn insert(&self, id: &str) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("approvals lock")
            .insert(id.to_string(), tx);
        rx
    }

    /// Deliver a decision. Returns false when nobody was waiting any more
    /// (timeout, cancellation or a duplicate resolve).
    pub fn resolve(&self, id: &str, decision: ApprovalDecision) -> bool {
        let tx = self.pending.lock().expect("approvals lock").remove(id);
        match tx {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    /// Fail every waiter with `Expired`.
    pub fn cancel_all(&self) {
        let mut guard = self.pending.lock().expect("approvals lock");
        for (_, tx) in guard.drain() {
            let _ = tx.send(ApprovalDecision::Expired);
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("approvals lock").len()
    }
}

/// Stable string for an approval kind, used in the session-allow key.
pub fn kind_key(kind: &ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::ShellCommand => "shell_command",
        ApprovalKind::FileWrite => "file_write",
        ApprovalKind::FileEdit => "file_edit",
        ApprovalKind::DeleteFile => "delete_file",
        ApprovalKind::NetworkTool => "network_tool",
        ApprovalKind::Plugin => "plugin",
        ApprovalKind::Mcp => "mcp",
        ApprovalKind::Other => "other",
    }
}

/// What an `allow_session` grant is remembered against: the command string, the
/// path, or the tool name.
pub fn approval_target(kind: &ApprovalKind, payload: &serde_json::Value, fallback: &str) -> String {
    let pick = |keys: &[&str]| -> Option<String> {
        for key in keys {
            if let Some(value) = payload.get(*key).and_then(|v| v.as_str()) {
                if !value.trim().is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    };
    let target = match kind {
        ApprovalKind::ShellCommand => pick(&["command"]).or_else(|| Some(fallback.to_string())),
        ApprovalKind::FileWrite | ApprovalKind::FileEdit | ApprovalKind::DeleteFile => {
            pick(&["path", "file"]).or_else(|| Some(fallback.to_string()))
        }
        ApprovalKind::Plugin
        | ApprovalKind::Mcp
        | ApprovalKind::NetworkTool
        | ApprovalKind::Other => {
            pick(&["tool", "name", "url"]).or_else(|| Some(fallback.to_string()))
        }
    };
    target.unwrap_or_default()
}

fn session_key(session_id: &str, kind: &ApprovalKind, target: &str) -> String {
    format!("{session_id}\u{1f}{}\u{1f}{target}", kind_key(kind))
}

impl RuntimeInner {
    pub(crate) fn remember_session_allow(
        &self,
        session_id: &str,
        kind: &ApprovalKind,
        target: &str,
    ) {
        self.session_allow
            .lock()
            .expect("session allow lock")
            .insert(session_key(session_id, kind, target));
    }

    pub(crate) fn has_session_allow(
        &self,
        session_id: &str,
        kind: &ApprovalKind,
        target: &str,
    ) -> bool {
        self.session_allow
            .lock()
            .expect("session allow lock")
            .contains(&session_key(session_id, kind, target))
    }

    pub(crate) fn clear_session_allows(&self, session_id: &str) {
        let prefix = format!("{session_id}\u{1f}");
        self.session_allow
            .lock()
            .expect("session allow lock")
            .retain(|key| !key.starts_with(&prefix));
    }
}

fn auto_decision(
    view: &ApprovalPolicyView,
    kind: &ApprovalKind,
    payload: &serde_json::Value,
) -> AutoDecision {
    match kind {
        ApprovalKind::ShellCommand => {
            let command = payload
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            view.for_shell(command)
        }
        ApprovalKind::FileWrite | ApprovalKind::FileEdit | ApprovalKind::DeleteFile => {
            let path = payload
                .get("path")
                .or_else(|| payload.get("file"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            view.for_write(&PathBuf::from(path))
        }
        ApprovalKind::Plugin
        | ApprovalKind::Mcp
        | ApprovalKind::NetworkTool
        | ApprovalKind::Other => view.for_external_tool(),
    }
}

/// The gate handed to tools for one run (or one subagent of that run).
pub(crate) struct RuntimeApprovalGate {
    inner: Arc<RuntimeInner>,
    session_id: SessionId,
    run_id: RunId,
    agent_id: Option<AgentId>,
    cancel: CancellationToken,
    depth: u8,
}

impl RuntimeApprovalGate {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        session_id: SessionId,
        run_id: RunId,
        agent_id: Option<AgentId>,
        cancel: CancellationToken,
        depth: u8,
    ) -> Self {
        Self {
            inner,
            session_id,
            run_id,
            agent_id,
            cancel,
            depth,
        }
    }

    fn ctx(&self) -> EmitCtx {
        EmitCtx::new(
            Arc::clone(&self.inner),
            self.session_id.0.clone(),
            self.run_id.0.clone(),
            self.agent_id.as_ref().map(|a| a.0.clone()),
        )
    }
}

#[async_trait]
impl ApprovalGate for RuntimeApprovalGate {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        let ctx = self.ctx();
        if self.cancel.is_cancelled() {
            return ApprovalDecision::Expired;
        }

        let policy = self.inner.deps.settings.approval_policy().await;
        let view = ApprovalPolicyView {
            mode: policy.mode.clone(),
            allow_commands: policy.allow_commands.clone(),
            auto_approve_paths: policy.auto_approve_paths.clone(),
        };

        let target = approval_target(&request.kind, &request.payload, &request.title);
        let session_id = self.session_id.0.clone();

        match auto_decision(&view, &request.kind, &request.payload) {
            AutoDecision::Allow => return ApprovalDecision::AllowOnce,
            AutoDecision::Deny => return ApprovalDecision::Deny,
            AutoDecision::Ask => {}
        }
        if self
            .inner
            .has_session_allow(&session_id, &request.kind, &target)
        {
            return ApprovalDecision::AllowOnce;
        }

        let approval_id = request.id.clone();
        let run_id = self.run_id.0.clone();
        let record = ApprovalRecord {
            id: approval_id.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            agent_id: self.agent_id.as_ref().map(|a| a.0.clone()),
            kind: kind_key(&request.kind).to_string(),
            title: request.title.clone(),
            detail: request.detail.clone(),
            payload: request.payload.clone(),
            status: "pending".to_string(),
            decision: None,
            created_at: now_ms(),
            resolved_at: None,
        };
        if let Err(e) = self.inner.deps.store.create_approval(&record).await {
            ctx.error(
                "store",
                format!("could not record the approval request: {e}"),
            );
            return ApprovalDecision::Expired;
        }

        let receiver = self.inner.approvals.insert(&approval_id);
        ctx.emit(Event::ApprovalRequested {
            approval: ApprovalRequest {
                id: approval_id.clone(),
                kind: request.kind.clone(),
                title: request.title.clone(),
                detail: request.detail.clone(),
                payload: request.payload.clone(),
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                agent_id: self.agent_id.as_ref().map(|a| a.0.clone()),
            },
        });
        tracing::debug!(
            approval = %approval_id,
            kind = kind_key(&request.kind),
            depth = self.depth,
            "waiting for approval"
        );

        let timeout = Duration::from_secs(policy.timeout_secs);
        let decision = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                self.expire(&run_id, "run cancelled").await;
                None
            }
            _ = tokio::time::sleep(timeout) => {
                self.expire(&run_id, "approval timed out").await;
                None
            }
            answer = receiver => answer.ok(),
        };

        match decision {
            Some(decision) => {
                if decision == ApprovalDecision::AllowSession {
                    self.inner
                        .remember_session_allow(&session_id, &request.kind, &target);
                }
                decision
            }
            None => {
                // `resolve()` already emitted the resolved event when the UI
                // answered; timeout and cancellation are reported here.
                ctx.emit(Event::ApprovalResolved {
                    approval_id,
                    decision: ApprovalDecision::Expired,
                });
                ApprovalDecision::Expired
            }
        }
    }
}

impl RuntimeApprovalGate {
    async fn expire(&self, run_id: &str, reason: &str) {
        // Only one approval is awaited at a time per run (tool calls are
        // sequential), so expiring the run's pending rows expires exactly this
        // one. Failures are surfaced as an event, never swallowed.
        if let Err(e) = self
            .inner
            .deps
            .store
            .expire_pending_approvals(run_id, reason)
            .await
        {
            self.ctx().error(
                "store",
                format!("could not expire the pending approval ({reason}): {e}"),
            );
        }
    }
}

/// Resolve a pending approval through the store and the in-process registry.
pub(crate) async fn resolve(
    inner: Arc<RuntimeInner>,
    approval_id: &str,
    decision: ApprovalDecision,
) -> Result<()> {
    let pending = inner.deps.store.list_pending_approvals(None).await?;
    let record = pending
        .into_iter()
        .find(|a| a.id == approval_id)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "approval {approval_id} is not pending (unknown, already resolved or expired)"
            ))
        })?;

    if !inner.approvals.resolve(approval_id, decision) {
        // The row was pending but nobody is waiting: the gate already timed out
        // or the run is gone. Report it instead of pretending it worked.
        return Err(Error::NotFound(format!(
            "approval {approval_id} is no longer waiting for a decision"
        )));
    }

    inner
        .deps
        .store
        .resolve_approval(approval_id, decision.as_str())
        .await?;

    inner.emit(
        Some(&record.session_id),
        Some(&record.run_id),
        record.agent_id.as_deref(),
        Event::ApprovalResolved {
            approval_id: approval_id.to_string(),
            decision,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::config::ApprovalMode;

    #[test]
    fn session_keys_are_scoped_per_session_and_kind() {
        let a = session_key("s1", &ApprovalKind::ShellCommand, "cargo test");
        let b = session_key("s2", &ApprovalKind::ShellCommand, "cargo test");
        let c = session_key("s1", &ApprovalKind::FileWrite, "cargo test");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("s1\u{1f}shell_command"));
    }

    #[test]
    fn target_prefers_payload_then_falls_back_to_title() {
        let payload = serde_json::json!({ "command": "cargo test" });
        assert_eq!(
            approval_target(&ApprovalKind::ShellCommand, &payload, "Run tests"),
            "cargo test"
        );
        assert_eq!(
            approval_target(
                &ApprovalKind::ShellCommand,
                &serde_json::json!({}),
                "Run tests"
            ),
            "Run tests"
        );
        assert_eq!(
            approval_target(
                &ApprovalKind::FileWrite,
                &serde_json::json!({ "path": "src/main.rs" }),
                "write"
            ),
            "src/main.rs"
        );
    }

    #[test]
    fn auto_decision_follows_the_policy() {
        let view = ApprovalPolicyView {
            mode: ApprovalMode::Ask,
            allow_commands: vec!["cargo test".into()],
            auto_approve_paths: vec!["src/".into()],
        };
        assert_eq!(
            auto_decision(
                &view,
                &ApprovalKind::ShellCommand,
                &serde_json::json!({"command": "cargo test --all"})
            ),
            AutoDecision::Allow
        );
        assert_eq!(
            auto_decision(
                &view,
                &ApprovalKind::ShellCommand,
                &serde_json::json!({"command": "rm -rf /"})
            ),
            AutoDecision::Ask
        );
        assert_eq!(
            auto_decision(
                &view,
                &ApprovalKind::FileWrite,
                &serde_json::json!({"path": "src/lib.rs"})
            ),
            AutoDecision::Allow
        );

        let read_only = ApprovalPolicyView {
            mode: ApprovalMode::ReadOnly,
            allow_commands: vec![],
            auto_approve_paths: vec![],
        };
        assert_eq!(
            auto_decision(
                &read_only,
                &ApprovalKind::ShellCommand,
                &serde_json::json!({"command": "ls"})
            ),
            AutoDecision::Deny
        );
    }
}
