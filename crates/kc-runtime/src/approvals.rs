//! Human-in-the-loop approval plumbing.
//!
//! [`RuntimeApprovalGate`] turns an [`ApprovalRequest`] coming from a tool into
//! one of: an automatic decision derived from the approval policy, an in-memory
//! `allow_session` grant, or a pending request persisted in the store that the
//! UI resolves through [`Runtime::resolve_approval`](crate::Runtime).
//!
//! Ordering guarantees: the store row exists *before* `ApprovalRequested` is
//! emitted, so a client reacting to the event can always resolve the id it saw.

// @kc-doc: approval-expiry-recovery
// @kc-title: Understand approval timeout and storage-failure recovery
// @kc-summary: Each approval has an independent expiry scope, and its in-memory waiter survives until expiry is durable.
// @kc-tags: approvals, timeout, persistence, subagents, recovery, safety
// @kc-applies-to: config:[approvals], config:approvals.timeout_secs, env:KARUCODE_APPROVAL_TIMEOUT_SECS, route:GET /api/approvals, route:POST /api/approvals/:id, event:approval_resolved
//
// # Retry or answer an approval after an expiry problem
//
// `approvals.timeout_secs` bounds one request, not every request sharing a run id.
// A parent and its subagents therefore time out independently. When the timer
// fires, only that approval expires and its action is treated as denied; sibling
// cards keep waiting for their own decisions.
//
// ```toml
// [approvals]
// mode = "ask"
// allow_commands = []
// auto_approve_paths = []
// timeout_secs = 300
// ```
//
// Durable expiry precedes `approval_resolved` and waiter release. Expiry uses a
// conditional exact-Pending transition for each candidate: a human Allow that
// commits after the pending-list read wins outright, with no later Expired waiter,
// event or expiry count. If a store read or write fails, the scope keeps the id and
// the in-memory registration instead of draining them, so later scope teardown
// retries expiry and a still-pending card remains answerable through
// `POST /api/approvals/:id`. No false Expired event is published for a row that is
// still durable-pending or was already resolved. A late answer never executes an
// action whose gate already returned `expired`; if the waiter has disappeared, the
// API reports that race even though the store closed the pending row.
//
// Clients normally need no repair command: answer the pending card again after
// storage recovers, or let loop teardown retry. Pending rows left by a process
// crash are expired by startup recovery. Approval modes and automatic decisions
// are documented by `approvals`; route payloads are listed in `http-api`.

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
use std::collections::{HashMap, HashSet};
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

    /// Whether a gate is still parked on this approval.
    ///
    /// Resolution checks this before writing the durable decision. It is only
    /// a preflight: cancellation may still win while the store write is in
    /// flight, and the final `resolve` remains the authority that wakes (or
    /// declines to wake) the receiver.
    pub fn contains(&self, id: &str) -> bool {
        self.pending
            .lock()
            .expect("approvals lock")
            .contains_key(id)
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

    /// Drop a waiter without delivering a decision.
    ///
    /// Used on gate exit paths where the request already knows the outcome
    /// (timeout, cancellation, store failure); leaving the sender behind would
    /// leak an entry for the rest of the process lifetime.
    pub fn discard(&self, id: &str) {
        self.pending.lock().expect("approvals lock").remove(id);
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

/// The approval ids one loop scope (session + run + agent) requested.
///
/// A subagent shares its parent's `run_id`, so expiring by run alone would also
/// expire a concurrent sibling's request. The gate records every id it created
/// here and expires *exactly* those, both when a single request gives up and when
/// the loop tears down.
#[derive(Clone, Default)]
pub(crate) struct ApprovalScope {
    ids: Arc<Mutex<HashSet<String>>>,
}

impl ApprovalScope {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn record(&self, id: &str) {
        self.ids
            .lock()
            .expect("approval scope lock")
            .insert(id.to_string());
    }

    fn forget(&self, id: &str) {
        self.ids.lock().expect("approval scope lock").remove(id);
    }

    fn snapshot(&self) -> Vec<String> {
        self.ids
            .lock()
            .expect("approval scope lock")
            .iter()
            .cloned()
            .collect()
    }

    /// Expire the approvals this scope requested that are still pending. Returns
    /// only ids whose expiry was durably written by this call.
    ///
    /// Snapshot, never drain: after a list/write failure the id and its in-process
    /// waiter stay registered so a later scope teardown or a client's explicit
    /// resolve can retry. A per-id waiter is released only after its own durable
    /// write succeeds, never merely because a timeout fired or storage failed.
    ///
    /// `run_wide` is exclusively for whole-run teardown. One request timing out
    /// always passes false: a top-level parent and its children share a run id but
    /// have independent approval deadlines.
    pub(crate) async fn expire_pending(
        &self,
        inner: &Arc<RuntimeInner>,
        session_id: &str,
        run_id: &str,
        run_wide: bool,
        reason: &str,
    ) -> Vec<String> {
        let ids = self.snapshot();
        if ids.is_empty() {
            return Vec::new();
        }
        let store = &inner.deps.store;
        let pending = match store.list_pending_approvals(Some(session_id)).await {
            Ok(pending) => pending,
            Err(e) => {
                tracing::warn!(
                    session = %session_id,
                    run = %run_id,
                    error = %e,
                    "could not list pending approvals while expiring scope; keeping waiters for retry"
                );
                return Vec::new();
            }
        };
        let pending_for_run: Vec<&ApprovalRecord> =
            pending.iter().filter(|a| a.run_id == run_id).collect();
        // Anything no longer pending was deliberately resolved elsewhere. It no
        // longer belongs in this expiry scope, but do not invent an Expired event
        // for what could have been an Allow decision.
        for id in &ids {
            if !pending_for_run.iter().any(|approval| &approval.id == id) {
                self.forget(id);
            }
        }
        let ours: Vec<&ApprovalRecord> = pending_for_run
            .iter()
            .filter(|a| ids.contains(&a.id))
            .copied()
            .collect();

        // A pending-list snapshot is only a candidate set, never evidence that an
        // expiry happened. A human can commit Allow after the list and before a
        // bulk update affects zero rows; the conditional per-id transition is the
        // sole authority for the waiter, event and returned ids. Whole-run teardown
        // considers child rows too, while one request's timeout remains scoped.
        let candidates = if run_wide { pending_for_run } else { ours };
        let mut expired = Vec::new();
        for approval in candidates {
            match store
                .resolve_pending_approval(&approval.id, ApprovalDecision::Expired.as_str())
                .await
            {
                Ok(true) => {
                    self.forget(&approval.id);
                    scope_expired(inner, approval);
                    expired.push(approval.id.clone());
                }
                // Another resolver won the exact Pending transition. Its durable
                // decision and event are authoritative; never append Expired.
                Ok(false) | Err(Error::NotFound(_)) => self.forget(&approval.id),
                Err(e) => tracing::warn!(
                    approval = %approval.id,
                    error = %e,
                    "could not expire the scoped approval; keeping its waiter for retry"
                ),
            }
        }
        if !expired.is_empty() {
            tracing::debug!(run = %run_id, expired = expired.len(), reason, "expired pending approvals");
        }
        expired
    }
}

/// Publish one expiry only after its durable transition succeeded.
fn scope_expired(inner: &Arc<RuntimeInner>, approval: &ApprovalRecord) {
    inner
        .approvals
        .resolve(&approval.id, ApprovalDecision::Expired);
    inner.emit(
        Some(&approval.session_id),
        Some(&approval.run_id),
        approval.agent_id.as_deref(),
        Event::ApprovalResolved {
            approval_id: approval.id.clone(),
            decision: ApprovalDecision::Expired,
        },
    );
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
    scope: ApprovalScope,
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
            scope: ApprovalScope::new(),
        }
    }

    /// Handle to the ids this gate requested, so the loop can expire exactly
    /// them when it ends or is cancelled.
    pub(crate) fn scope(&self) -> ApprovalScope {
        self.scope.clone()
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

        self.scope.record(&approval_id);
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
        // Only the app-level card notifies: a subagent's approval is raised through
        // its parent's card, and two notifications for one card is a bug.
        if self.agent_id.is_none() {
            crate::notify::approval_requested(
                &self.inner,
                &ctx,
                &approval_id,
                &request.title,
                &request.detail,
            )
            .await;
        }
        tracing::debug!(
            approval = %approval_id,
            kind = kind_key(&request.kind),
            depth = self.depth,
            "waiting for approval"
        );

        let timeout = Duration::from_secs(policy.timeout_secs);
        let (decision, expiry_written) = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                (None, self.expire_request(&approval_id, "run cancelled").await)
            }
            _ = tokio::time::sleep(timeout) => {
                (None, self.expire_request(&approval_id, "approval timed out").await)
            }
            answer = receiver => (answer.ok(), true),
        };
        if decision.is_none() && expiry_written {
            // A successful expiry resolves/removes the waiter too; discard is only
            // a no-op safeguard. On storage failure retain both ids for a retry.
            self.inner.approvals.discard(&approval_id);
        }

        match decision {
            Some(ApprovalDecision::Expired) => {
                // e.g. `ApprovalRegistry::cancel_all` during shutdown: nobody
                // updated the store row, so close exactly this request as well.
                // A scope-level/run-wide expiry that woke us already published it.
                self.expire_request(&approval_id, "approval expired").await;
                ApprovalDecision::Expired
            }
            Some(decision) => {
                // Resolved by the UI: nothing left to expire for this id.
                self.scope.forget(&approval_id);
                if decision == ApprovalDecision::AllowSession {
                    self.inner
                        .remember_session_allow(&session_id, &request.kind, &target);
                }
                decision
            }
            None => {
                // `expire_request` publishes only after durable expiry. If storage
                // failed, leave the card pending and resolvable on recovery/retry:
                // an Expired event here would falsely claim persistence succeeded.
                ApprovalDecision::Expired
            }
        }
    }
}

impl RuntimeApprovalGate {
    /// Expire one timed-out/cancelled request, never its whole run. The bool says
    /// whether this call durably wrote the expiry; false keeps the scope id and
    /// waiter available for a retry.
    async fn expire_request(&self, approval_id: &str, reason: &str) -> bool {
        self.scope
            .expire_pending(
                &self.inner,
                &self.session_id.0,
                &self.run_id.0,
                false,
                reason,
            )
            .await
            .into_iter()
            .any(|id| id == approval_id)
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

    // Reject cards whose gate already durably closed. If expiry storage failed,
    // the waiter deliberately remains here so the durable pending card can still
    // be answered even though the tool has already treated the wait as expired.
    if !inner.approvals.contains(approval_id) {
        return Err(Error::NotFound(format!(
            "approval {approval_id} is no longer waiting for a decision"
        )));
    }

    // Persist *before* waking the gate. An allow delivered first would let the
    // tool execute even if this write subsequently failed, leaving no durable
    // record of the human's decision. On error the registry entry is untouched,
    // so timeout/cancellation still expires it and the caller may retry.
    inner
        .deps
        .store
        .resolve_approval(approval_id, decision.as_str())
        .await?;

    if !inner.approvals.resolve(approval_id, decision) {
        // The store has the decision, but no live tool was woken. This preserves
        // the cancellation-race error contract while still closing a durable
        // pending card whose timeout expiry previously failed to persist.
        return Err(Error::NotFound(format!(
            "approval {approval_id} expired while its decision was being stored"
        )));
    }

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
    use crate::tests_support::{FakeSettings, HarnessBuilder};
    use kc_core::config::{ApprovalMode, ApprovalPolicy};
    use tokio::time::Instant;

    fn scoped_request(id: &str, session: &str, run: &str, agent: Option<&str>) -> ApprovalRequest {
        ApprovalRequest {
            id: id.into(),
            kind: ApprovalKind::Mcp,
            title: format!("Call {id}"),
            detail: "test approval".into(),
            payload: serde_json::json!({"tool": id}),
            session_id: session.into(),
            run_id: run.into(),
            agent_id: agent.map(str::to_string),
        }
    }

    async fn wait_for_pending(inner: &Arc<RuntimeInner>, session: &str, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pending = inner
                .deps
                .store
                .list_pending_approvals(Some(session))
                .await
                .expect("list pending");
            if pending.len() == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "wanted {expected} pending approvals, got {pending:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_top_level_timeout_does_not_expire_an_independent_child() {
        let settings = FakeSettings::default().with_policy(ApprovalPolicy {
            mode: ApprovalMode::Ask,
            timeout_secs: 1,
            ..Default::default()
        });
        let h = HarnessBuilder::new(Vec::new())
            .settings(settings)
            .build()
            .await;
        let inner = Arc::clone(h.runtime.shared());
        let session = SessionId::from_string(h.session());
        let run = RunId::new();
        let parent = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            session.clone(),
            run.clone(),
            None,
            CancellationToken::new(),
            0,
        ));
        let child = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            session.clone(),
            run.clone(),
            Some(AgentId::from_string("a_child")),
            CancellationToken::new(),
            1,
        ));
        let parent_request = scoped_request("ap_parent", h.session(), &run.0, None);
        let parent_task = tokio::spawn(async move { parent.request(parent_request).await });
        wait_for_pending(&inner, h.session(), 1).await;

        // Start the child halfway through the parent's independent one-second
        // budget. The parent times out while this child still has time remaining.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let child_request = scoped_request("ap_child", h.session(), &run.0, Some("a_child"));
        let child_task = tokio::spawn(async move { child.request(child_request).await });
        wait_for_pending(&inner, h.session(), 2).await;

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), parent_task)
                .await
                .expect("parent request returned")
                .expect("parent task joined"),
            ApprovalDecision::Expired
        );
        let pending = inner
            .deps
            .store
            .list_pending_approvals(Some(h.session()))
            .await
            .expect("list after parent timeout");
        assert_eq!(
            pending
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ap_child"],
            "a run-wide timeout would incorrectly close the child"
        );
        assert!(inner.approvals.contains("ap_child"));
        assert_eq!(inner.approvals.pending_count(), 1);

        // Resolve the still-independent child through the normal durable path.
        resolve(Arc::clone(&inner), "ap_child", ApprovalDecision::Deny)
            .await
            .expect("resolve child after parent timeout");
        assert_eq!(
            child_task.await.expect("child joined"),
            ApprovalDecision::Deny
        );
        assert_eq!(inner.approvals.pending_count(), 0);
    }

    #[tokio::test]
    async fn failed_timeout_persistence_keeps_scope_and_pending_card_resolvable() {
        let settings = FakeSettings::default().with_policy(ApprovalPolicy {
            mode: ApprovalMode::Ask,
            timeout_secs: 1,
            ..Default::default()
        });
        let h = HarnessBuilder::new(Vec::new())
            .settings(settings)
            .build()
            .await;
        let inner = Arc::clone(h.runtime.shared());
        let session = SessionId::from_string(h.session());
        let run = RunId::new();
        let gate = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            session.clone(),
            run.clone(),
            Some(AgentId::from_string("a_store_failure")),
            CancellationToken::new(),
            1,
        ));
        let scope = gate.scope();
        let request = scoped_request(
            "ap_store_failure",
            h.session(),
            &run.0,
            Some("a_store_failure"),
        );
        let task = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.request(request).await })
        };
        wait_for_pending(&inner, h.session(), 1).await;
        h.store.fail_approval_resolutions(true);

        // The timed-out tool gets a safe denial, but neither its scope id nor its
        // registry entry can be thrown away while the durable row remains pending.
        assert_eq!(task.await.expect("gate joined"), ApprovalDecision::Expired);
        assert!(scope
            .ids
            .lock()
            .expect("scope")
            .contains("ap_store_failure"));
        assert!(inner.approvals.contains("ap_store_failure"));
        assert_eq!(
            h.store.approvals()[0].status,
            "pending",
            "the injected write failure must leave the row pending"
        );

        // A second expiry attempt during the same outage also changes no waiter.
        assert!(scope
            .expire_pending(&inner, h.session(), &run.0, false, "retry during outage",)
            .await
            .is_empty());
        assert!(inner.approvals.contains("ap_store_failure"));

        // Once storage recovers, normal resolution is the other retry path. The
        // late decision must not pretend to execute the already-expired tool; it
        // durably closes the card and removes the in-memory registration.
        h.store.fail_approval_resolutions(false);
        let late = resolve(
            Arc::clone(&inner),
            "ap_store_failure",
            ApprovalDecision::Deny,
        )
        .await;
        assert!(
            matches!(late, Err(Error::NotFound(_))),
            "the vanished waiter is not reported as woken: {late:?}"
        );
        assert!(!inner.approvals.contains("ap_store_failure"));
        assert_eq!(h.store.approvals()[0].status, "resolved");
        assert_eq!(h.store.approvals()[0].decision.as_deref(), Some("deny"));
        assert!(scope
            .expire_pending(&inner, h.session(), &run.0, false, "teardown retry")
            .await
            .is_empty());
        assert!(scope.ids.lock().expect("scope").is_empty());
    }

    #[tokio::test]
    async fn expiry_write_retry_closes_the_row_before_releasing_its_waiter() {
        let settings = FakeSettings::default().with_policy(ApprovalPolicy {
            mode: ApprovalMode::Ask,
            timeout_secs: 30,
            ..Default::default()
        });
        let h = HarnessBuilder::new(Vec::new())
            .settings(settings)
            .build()
            .await;
        let mut events = h.runtime.subscribe();
        let inner = Arc::clone(h.runtime.shared());
        let run = RunId::new();
        let cancel = CancellationToken::new();
        let gate = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            SessionId::from_string(h.session()),
            run.clone(),
            Some(AgentId::from_string("a_retry")),
            cancel.clone(),
            1,
        ));
        let scope = gate.scope();
        let request = scoped_request("ap_retry", h.session(), &run.0, Some("a_retry"));
        let task = tokio::spawn(async move { gate.request(request).await });
        wait_for_pending(&inner, h.session(), 1).await;

        h.store.fail_approval_resolutions(true);
        cancel.cancel();
        assert_eq!(task.await.expect("gate joined"), ApprovalDecision::Expired);
        assert!(inner.approvals.contains("ap_retry"));
        assert_eq!(h.store.approvals()[0].status, "pending");

        h.store.fail_approval_resolutions(false);
        let expired = scope
            .expire_pending(&inner, h.session(), &run.0, false, "teardown retry")
            .await;
        assert_eq!(expired, vec!["ap_retry"]);
        assert_eq!(h.store.approvals()[0].status, "expired");
        assert_eq!(h.store.approvals()[0].decision.as_deref(), Some("expired"));
        assert!(!inner.approvals.contains("ap_retry"));
        assert!(scope.ids.lock().expect("scope").is_empty());

        let mut published = Vec::new();
        while let Ok(event) = events.try_recv() {
            published.push(event);
        }
        assert_eq!(
            published
                .iter()
                .filter(|event| matches!(
                    &event.event,
                    Event::ApprovalResolved {
                        decision: ApprovalDecision::Expired,
                        ..
                    }
                ))
                .count(),
            1,
            "only the durable retry emits expiry: {published:?}"
        );
    }

    #[tokio::test]
    async fn allow_before_run_wide_expiry_never_publishes_or_reports_expired() {
        let h = HarnessBuilder::new(Vec::new())
            .settings(FakeSettings::default().with_policy(ApprovalPolicy {
                mode: ApprovalMode::Ask,
                timeout_secs: 30,
                ..Default::default()
            }))
            .build()
            .await;
        let mut events = h.runtime.subscribe();
        let inner = Arc::clone(h.runtime.shared());
        let run = RunId::new();
        let gate = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            SessionId::from_string(h.session()),
            run.clone(),
            None,
            CancellationToken::new(),
            0,
        ));
        let scope = gate.scope();
        let request = scoped_request("ap_allow_before_expiry", h.session(), &run.0, None);
        let request_task = tokio::spawn(async move { gate.request(request).await });
        wait_for_pending(&inner, h.session(), 1).await;

        // Freeze either implementation after it has listed the approval as pending
        // but before its durable transition. The human Allow therefore wins a real
        // storage race rather than merely preceding the call to expire the scope.
        let bulk_gate = h.store.gate_next("expire_pending_approvals");
        let conditional_gate = h.store.gate_next("resolve_pending_approval");
        let expiry = {
            let inner = Arc::clone(&inner);
            let scope = scope.clone();
            let session = h.session().to_string();
            let run_id = run.0.clone();
            tokio::spawn(async move {
                scope
                    .expire_pending(&inner, &session, &run_id, true, "review")
                    .await
            })
        };
        tokio::select! {
            _ = bulk_gate.wait_entered() => {}
            _ = conditional_gate.wait_entered() => {}
        }

        resolve(
            Arc::clone(&inner),
            "ap_allow_before_expiry",
            ApprovalDecision::AllowOnce,
        )
        .await
        .expect("human decision");
        assert_eq!(
            request_task.await.expect("approval request joined"),
            ApprovalDecision::AllowOnce
        );
        bulk_gate.release();
        conditional_gate.release();

        assert!(
            expiry.await.expect("expiry joined").is_empty(),
            "an Allow that already committed must not be reported as expired"
        );
        let approval = &h.store.approvals()[0];
        assert_eq!(approval.status, "resolved");
        assert_eq!(approval.decision.as_deref(), Some("allow_once"));
        let mut persisted_and_published = Vec::new();
        while let Ok(envelope) = events.try_recv() {
            if let Event::ApprovalResolved {
                approval_id,
                decision,
            } = envelope.event
            {
                if approval_id == "ap_allow_before_expiry" {
                    persisted_and_published.push((approval.decision.clone(), decision));
                }
            }
        }
        assert_eq!(
            persisted_and_published,
            vec![(Some("allow_once".to_string()), ApprovalDecision::AllowOnce)],
            "published decisions must not contradict the durable Allow"
        );
        assert!(scope
            .expire_pending(&inner, h.session(), &run.0, true, "teardown retry")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn normal_pending_expiry_is_reported_once_and_teardown_retry_is_a_noop() {
        let h = HarnessBuilder::new(Vec::new())
            .settings(FakeSettings::default().with_policy(ApprovalPolicy {
                mode: ApprovalMode::Ask,
                timeout_secs: 30,
                ..Default::default()
            }))
            .build()
            .await;
        let mut events = h.runtime.subscribe();
        let inner = Arc::clone(h.runtime.shared());
        let run = RunId::new();
        let gate = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(&inner),
            SessionId::from_string(h.session()),
            run.clone(),
            None,
            CancellationToken::new(),
            0,
        ));
        let scope = gate.scope();
        let request = scoped_request("ap_normal_expiry", h.session(), &run.0, None);
        let request_task = tokio::spawn(async move { gate.request(request).await });
        wait_for_pending(&inner, h.session(), 1).await;

        assert_eq!(
            scope
                .expire_pending(&inner, h.session(), &run.0, true, "run finished")
                .await,
            vec!["ap_normal_expiry"]
        );
        assert_eq!(
            request_task.await.expect("approval request joined"),
            ApprovalDecision::Expired
        );
        assert!(scope
            .expire_pending(&inner, h.session(), &run.0, true, "teardown retry")
            .await
            .is_empty());
        let approval = &h.store.approvals()[0];
        assert_eq!(approval.status, "expired");
        assert_eq!(approval.decision.as_deref(), Some("expired"));
        assert_eq!(inner.approvals.pending_count(), 0);

        let mut expired_events = 0;
        while let Ok(envelope) = events.try_recv() {
            if matches!(
                envelope.event,
                Event::ApprovalResolved {
                    ref approval_id,
                    decision: ApprovalDecision::Expired,
                } if approval_id == "ap_normal_expiry"
            ) {
                expired_events += 1;
            }
        }
        assert_eq!(expired_events, 1, "the waiter retry must not publish twice");
    }

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
