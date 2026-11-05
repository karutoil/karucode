//! In-memory fakes used by the crate's tests.
//!
//! Nothing here is compiled into the binary: the runtime is tested entirely
//! against `kc_core` traits, so no SQLite, HTTP or filesystem-walking tool is
//! needed to exercise the loop.

#![allow(dead_code)]

use crate::approvals::ApprovalRegistry;
use crate::subagent::SubagentLimiter;
use crate::{ActiveRun, Runtime, RuntimeDeps, RuntimeInner};
use async_trait::async_trait;
use kc_core::approval::ApprovalKind;
use kc_core::config::{ApprovalPolicy, Limits, ModelConfig, SessionModelSelection, SubagentConfig};
use kc_core::error::{Error, Result};
use kc_core::events::EventEnvelope;
use kc_core::hooks::{Hooks, NoHooks};
use kc_core::message::ToolCall;
use kc_core::paths::Workspace;
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderFactory, ProviderStream};
use kc_core::settings::Settings;
use kc_core::store::{
    AgentRecord, AgentSummary, ApprovalRecord, ClaimRecord, NewMessage, Project, RunRecord,
    RunStatus, Session, SessionSummary, Store, StoredMessage, TerminalSession, UsageRecord,
};
use kc_core::tool::{
    Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolRegistry, ToolSpec,
};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn now_ms() -> i64 {
    crate::events::now_ms()
}

/// Same normalisation the SQLite store applies to claimed paths.
fn normalize_claim_path(raw: &str) -> String {
    let mut p = raw.trim();
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    let mut p = p.trim_end_matches('/');
    if p == "." {
        p = "";
    }
    p.to_string()
}

/// Identical paths or ancestor/descendant relationships overlap.
fn claim_paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_under_b = a
        .strip_prefix(b)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1);
    let b_under_a = b
        .strip_prefix(a)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1);
    a_under_b || b_under_a
}

// ---------------------------------------------------------------------------
// FakeStore
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Db {
    sessions: Vec<Session>,
    projects: Vec<Project>,
    messages: Vec<StoredMessage>,
    next_message_id: i64,
    runs: Vec<RunRecord>,
    agents: Vec<AgentRecord>,
    approvals: Vec<ApprovalRecord>,
    claims: Vec<ClaimRecord>,
    settings: BTreeMap<String, String>,
    terminals: BTreeMap<String, TerminalSession>,
    checkpoints: Vec<kc_core::checkpoint::FileCheckpoint>,
    next_checkpoint_id: i64,
    /// One entry per recorded provider call, in arrival order.
    usage: Vec<UsageRecord>,
}

pub(crate) struct StoreFaultGate {
    entered: AtomicBool,
    entered_notify: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

impl StoreFaultGate {
    fn new() -> Self {
        Self {
            entered: AtomicBool::new(false),
            entered_notify: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        }
    }

    pub(crate) async fn wait_entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.entered_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.entered.load(Ordering::SeqCst) {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("store call never reached its fault barrier");
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

pub(crate) struct FakeStore {
    db: Mutex<Db>,
    fail_appends: std::sync::atomic::AtomicBool,
    fail_checkpoints: AtomicBool,
    fail_checkpoint_binds: AtomicBool,
    fail_approval_resolutions: AtomicBool,
    operation_gates: Mutex<HashMap<&'static str, Arc<StoreFaultGate>>>,
    /// Fail `append_message` for content containing this needle (one failure
    /// mode for exercising a single failed tool-result persist).
    fail_append_content: Mutex<Option<String>>,
}

impl Default for FakeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeStore {
    pub(crate) fn new() -> Self {
        Self {
            db: Mutex::new(Db::default()),
            fail_appends: std::sync::atomic::AtomicBool::new(false),
            fail_checkpoints: AtomicBool::new(false),
            fail_checkpoint_binds: AtomicBool::new(false),
            fail_approval_resolutions: AtomicBool::new(false),
            operation_gates: Mutex::new(HashMap::new()),
            fail_append_content: Mutex::new(None),
        }
    }

    pub(crate) fn fail_approval_resolutions(&self, fail: bool) {
        self.fail_approval_resolutions.store(fail, Ordering::SeqCst);
    }

    pub(crate) fn gate_next(&self, operation: &'static str) -> Arc<StoreFaultGate> {
        let gate = Arc::new(StoreFaultGate::new());
        let previous = self
            .operation_gates
            .lock()
            .expect("fake store gates lock")
            .insert(operation, Arc::clone(&gate));
        assert!(
            previous.is_none(),
            "a gate for {operation} is already armed"
        );
        gate
    }

    async fn wait_at_gate(&self, operation: &'static str) {
        let gate = self
            .operation_gates
            .lock()
            .expect("fake store gates lock")
            .remove(operation);
        let Some(gate) = gate else {
            return;
        };
        gate.entered.store(true, Ordering::SeqCst);
        gate.entered_notify.notify_waiters();
        let permit = tokio::time::timeout(Duration::from_secs(5), gate.release.acquire())
            .await
            .expect("store fault barrier was never released")
            .expect("store fault barrier semaphore closed");
        permit.forget();
    }

    /// Make the next `append_message` calls fail, to exercise storage errors.
    pub(crate) fn fail_appends(&self, fail: bool) {
        self.fail_appends.store(fail, Ordering::SeqCst);
    }

    pub(crate) fn fail_checkpoints(&self, fail: bool) {
        self.fail_checkpoints.store(fail, Ordering::SeqCst);
    }

    pub(crate) fn fail_checkpoint_binds(&self, fail: bool) {
        self.fail_checkpoint_binds.store(fail, Ordering::SeqCst);
    }

    /// Fail only appends whose content contains `needle`.
    pub(crate) fn fail_appends_containing(&self, needle: &str) {
        *self.fail_append_content.lock().expect("fake store lock") = Some(needle.to_string());
    }

    pub(crate) fn stop_failing_appends(&self) {
        *self.fail_append_content.lock().expect("fake store lock") = None;
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Db> {
        self.db.lock().expect("fake store lock")
    }

    fn db_mut(&self) -> std::sync::MutexGuard<'_, Db> {
        self.db.lock().expect("fake store lock")
    }

    pub(crate) fn messages(&self) -> Vec<StoredMessage> {
        self.db().messages.clone()
    }

    pub(crate) fn messages_for_agent(&self, agent_id: &str) -> Vec<StoredMessage> {
        self.db()
            .messages
            .iter()
            .filter(|m| m.agent_id.as_deref() == Some(agent_id))
            .cloned()
            .collect()
    }

    pub(crate) fn runs(&self) -> Vec<RunRecord> {
        self.db().runs.clone()
    }

    pub(crate) fn agents(&self) -> Vec<AgentRecord> {
        self.db().agents.clone()
    }

    pub(crate) fn approvals(&self) -> Vec<ApprovalRecord> {
        self.db().approvals.clone()
    }

    pub(crate) fn claims(&self) -> Vec<ClaimRecord> {
        self.db().claims.clone()
    }

    /// Every provider call the runtime recorded, in arrival order.
    pub(crate) fn usage(&self) -> Vec<UsageRecord> {
        self.db().usage.clone()
    }
}

#[async_trait]
impl Store for FakeStore {
    async fn create_session(
        &self,
        id: &str,
        title: &str,
        workspace: &str,
        project_id: Option<&str>,
    ) -> Result<Session> {
        let now = now_ms();
        let session = Session {
            id: id.to_string(),
            title: title.to_string(),
            workspace: workspace.to_string(),
            project_id: project_id.map(str::to_string),
            model: None,
            created_at: now,
            updated_at: now,
        };
        self.db().sessions.push(session.clone());
        Ok(session)
    }

    async fn recovery_sessions(
        &self,
        cutoff: i64,
        limit: usize,
        include_agents: bool,
    ) -> Result<(usize, Vec<String>)> {
        let db = self.db();
        let mut candidates: Vec<_> = db
            .sessions
            .iter()
            .filter_map(|s| {
                let interrupted = db
                    .runs
                    .iter()
                    .any(|r| r.session_id == s.id && r.status == "running")
                    || (include_agents
                        && db
                            .agents
                            .iter()
                            .any(|a| a.session_id == s.id && a.status == "running"));
                (interrupted || s.updated_at >= cutoff).then_some((
                    interrupted,
                    s.updated_at,
                    s.id.clone(),
                ))
            })
            .collect();
        candidates.sort_by(|a, b| b.cmp(a));
        Ok((
            db.sessions.len(),
            candidates
                .into_iter()
                .take(limit)
                .map(|(_, _, id)| id)
                .collect(),
        ))
    }

    async fn running_agent_ids(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .db()
            .agents
            .iter()
            .filter(|a| a.status == "running")
            .map(|a| (a.session_id.clone(), a.id.clone()))
            .collect())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self
            .db()
            .sessions
            .iter()
            .map(|s| SessionSummary {
                id: s.id.clone(),
                title: s.title.clone(),
                workspace: s.workspace.clone(),
                project_id: s.project_id.clone(),
                model: s.model.clone(),
                created_at: s.created_at,
                updated_at: s.updated_at,
                message_count: 0,
                last_run_status: None,
                running: false,
            })
            .collect())
    }

    async fn list_sessions_for_project(&self, project_id: &str) -> Result<Vec<SessionSummary>> {
        Ok(self
            .list_sessions()
            .await?
            .into_iter()
            .filter(|s| s.project_id.as_deref() == Some(project_id))
            .collect())
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        Ok(self.db().sessions.iter().find(|s| s.id == id).cloned())
    }

    async fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        let mut db = self.db();
        match db.sessions.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.title = title.to_string();
                s.updated_at = now_ms();
                Ok(())
            }
            None => Err(Error::NotFound(format!("session {id}"))),
        }
    }

    async fn set_session_model(
        &self,
        id: &str,
        model: Option<&SessionModelSelection>,
    ) -> Result<()> {
        let mut db = self.db();
        match db.sessions.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.model = model.cloned();
                s.updated_at = now_ms();
                Ok(())
            }
            None => Err(Error::NotFound(format!("session {id}"))),
        }
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        let mut db = self.db();
        db.sessions.retain(|s| s.id != id);
        db.messages.retain(|m| m.session_id != id);
        Ok(())
    }

    async fn touch_session(&self, id: &str) -> Result<()> {
        if let Some(s) = self.db().sessions.iter_mut().find(|s| s.id == id) {
            s.updated_at = now_ms();
        }
        Ok(())
    }

    async fn assign_session_project(&self, session_id: &str, project_id: &str) -> Result<()> {
        let mut db = self.db();
        let path = db
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.path.clone())
            .ok_or_else(|| Error::NotFound(format!("project {project_id}")))?;
        match db.sessions.iter_mut().find(|s| s.id == session_id) {
            Some(session) => {
                session.project_id = Some(project_id.to_string());
                session.workspace = path;
                session.updated_at = now_ms();
                Ok(())
            }
            None => Err(Error::NotFound(format!("session {session_id}"))),
        }
    }

    async fn create_project(&self, id: &str, name: &str, path: &str) -> Result<Project> {
        let mut db = self.db();
        if db.projects.iter().any(|p| p.path == path) {
            return Err(Error::Store(format!("a project already uses {path}")));
        }
        let now = now_ms();
        let project = Project {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            created_at: now,
            updated_at: now,
            session_count: 0,
        };
        db.projects.push(project.clone());
        Ok(project)
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        let db = self.db();
        Ok(db
            .projects
            .iter()
            .map(|project| Project {
                session_count: db
                    .sessions
                    .iter()
                    .filter(|s| s.project_id.as_deref() == Some(project.id.as_str()))
                    .count() as i64,
                ..project.clone()
            })
            .collect())
    }

    async fn get_project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self.db().projects.iter().find(|p| p.id == id).cloned())
    }

    async fn rename_project(&self, id: &str, name: &str) -> Result<()> {
        let mut db = self.db();
        match db.projects.iter_mut().find(|p| p.id == id) {
            Some(project) => {
                project.name = name.to_string();
                project.updated_at = now_ms();
                Ok(())
            }
            None => Err(Error::NotFound(format!("project {id}"))),
        }
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        let mut db = self.db();
        db.projects.retain(|p| p.id != id);
        for session in db.sessions.iter_mut() {
            if session.project_id.as_deref() == Some(id) {
                session.project_id = None;
            }
        }
        Ok(())
    }

    async fn append_message(&self, message: &NewMessage) -> Result<i64> {
        if self.fail_appends.load(Ordering::SeqCst) {
            return Err(Error::Store("disk is full".into()));
        }
        if let Some(needle) = self
            .fail_append_content
            .lock()
            .expect("fake store lock")
            .as_ref()
        {
            if message.content.contains(needle.as_str()) {
                return Err(Error::Store("disk is full".into()));
            }
        }
        let mut db = self.db();
        db.next_message_id += 1;
        let id = db.next_message_id;
        let stored = StoredMessage {
            id,
            session_id: message.session_id.clone(),
            run_id: message.run_id.clone(),
            agent_id: message.agent_id.clone(),
            role: message.role.clone(),
            content: message.content.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            name: message.name.clone(),
            images: message.images.clone(),
            reasoning_content: message.reasoning_content.clone(),
            rewound_at: None,
            created_at: now_ms(),
        };
        db.messages.push(stored);
        Ok(id)
    }

    async fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<StoredMessage>> {
        let db = self.db();
        let mut out: Vec<StoredMessage> = db
            .messages
            .iter()
            .filter(|m| m.session_id == session_id && m.rewound_at.is_none())
            .cloned()
            .collect();
        if limit > 0 && out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        Ok(out)
    }

    async fn list_history_page(
        &self,
        session_id: &str,
        before_id: Option<i64>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<StoredMessage>> {
        let db = self.db();
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for row in db
            .messages
            .iter()
            .rev()
            .filter(|m| {
                m.session_id == session_id
                    && m.rewound_at.is_none()
                    && m.agent_id.is_none()
                    && m.role != "system"
                    && before_id.is_none_or(|id| m.id < id)
            })
            .take(limit)
        {
            let mut row = row.clone();
            if row.role == "tool" && row.content.len() > max_bytes / 2 {
                row.content = "[tool result omitted: exceeds history byte budget]".into();
            }
            let size = row.content.len()
                + serde_json::to_string(&row.tool_calls).unwrap().len()
                + serde_json::to_string(&row.images).unwrap().len()
                + row.reasoning_content.as_ref().map_or(0, String::len)
                + row.tool_call_id.as_ref().map_or(0, String::len)
                + row.name.as_ref().map_or(0, String::len);
            if !out.is_empty() && bytes.saturating_add(size) > max_bytes {
                break;
            }
            out.push(row);
            bytes = bytes.saturating_add(size);
        }
        Ok(out)
    }

    async fn rewind_tool_names(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<String>> {
        let db = self.db();
        let mut remaining = max_bytes;
        let mut names = Vec::new();
        for m in db
            .messages
            .iter()
            .filter(|m| m.session_id == session_id && m.id > after_id && m.rewound_at.is_none())
            .take(limit)
        {
            let size = serde_json::to_string(&m.tool_calls).unwrap().len();
            if size > remaining {
                return Err(kc_core::error::Error::BadRequest(
                    "rewind tool scan exceeds byte limit".into(),
                ));
            }
            remaining -= size;
            names.extend(m.tool_calls.iter().map(|c| c.name.clone()));
        }
        Ok(names)
    }

    async fn list_messages_after(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let db = self.db();
        Ok(db
            .messages
            .iter()
            .filter(|m| m.session_id == session_id && m.id > after_id && m.rewound_at.is_none())
            .take(limit)
            .cloned()
            .collect())
    }

    async fn create_run(
        &self,
        id: &str,
        session_id: &str,
        model: &str,
        prompt: &str,
    ) -> Result<()> {
        self.db().runs.push(RunRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            status: RunStatus::Running.as_str().to_string(),
            model: model.to_string(),
            prompt: prompt.to_string(),
            error: None,
            started_at: now_ms(),
            finished_at: None,
        });
        Ok(())
    }

    async fn finish_run(&self, run_id: &str, status: RunStatus, error: Option<&str>) -> Result<()> {
        self.wait_at_gate("finish_run").await;
        let mut db = self.db();
        match db.runs.iter_mut().find(|r| r.id == run_id) {
            Some(run) => {
                run.status = status.as_str().to_string();
                run.error = error.map(|e| e.to_string());
                run.finished_at = Some(now_ms());
                Ok(())
            }
            None => Err(Error::NotFound(format!("run {run_id}"))),
        }
    }

    async fn mark_interrupted_runs(&self) -> Result<usize> {
        let mut db = self.db();
        let mut count = 0;
        for run in db.runs.iter_mut() {
            if run.status == RunStatus::Running.as_str() {
                run.status = RunStatus::Interrupted.as_str().to_string();
                run.finished_at = Some(now_ms());
                count += 1;
            }
        }
        Ok(count)
    }

    async fn list_runs(&self, session_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let db = self.db();
        let mut out: Vec<RunRecord> = db
            .runs
            .iter()
            .filter(|r| r.session_id == session_id)
            .cloned()
            .collect();
        if limit > 0 && out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        Ok(out)
    }

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()> {
        let mut db = self.db();
        match db.agents.iter_mut().find(|a| a.id == agent.id) {
            Some(existing) => *existing = agent.clone(),
            None => db.agents.push(agent.clone()),
        }
        Ok(())
    }

    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>> {
        Ok(self
            .db()
            .agents
            .iter()
            .filter(|a| a.session_id == session_id)
            .cloned()
            .collect())
    }

    async fn list_agents_all(&self, project_id: Option<&str>) -> Result<Vec<AgentSummary>> {
        let db = self.db();
        let session_for =
            |agent: &AgentRecord| db.sessions.iter().find(|s| s.id == agent.session_id);
        let mut out: Vec<AgentSummary> = db
            .agents
            .iter()
            .filter(|agent| match project_id {
                None => true,
                Some(project) => {
                    session_for(agent).and_then(|s| s.project_id.as_deref()) == Some(project)
                }
            })
            .map(|agent| {
                let session = session_for(agent);
                AgentSummary {
                    agent: agent.clone(),
                    session_title: session.map(|s| s.title.clone()).unwrap_or_default(),
                    workspace: session.map(|s| s.workspace.clone()).unwrap_or_default(),
                    project_id: session.and_then(|s| s.project_id.clone()),
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.agent
                .created_at
                .cmp(&a.agent.created_at)
                .then_with(|| b.agent.id.cmp(&a.agent.id))
        });
        Ok(out)
    }

    async fn get_agent(&self, id: &str) -> Result<Option<AgentSummary>> {
        let db = self.db();
        let Some(agent) = db.agents.iter().find(|a| a.id == id) else {
            return Ok(None);
        };
        let session = db.sessions.iter().find(|s| s.id == agent.session_id);
        Ok(Some(AgentSummary {
            agent: agent.clone(),
            session_title: session.map(|s| s.title.clone()).unwrap_or_default(),
            workspace: session.map(|s| s.workspace.clone()).unwrap_or_default(),
            project_id: session.and_then(|s| s.project_id.clone()),
        }))
    }

    async fn list_agent_messages(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let db = self.db();
        let mut out: Vec<StoredMessage> = db
            .messages
            .iter()
            .filter(|m| m.agent_id.as_deref() == Some(agent_id))
            .cloned()
            .collect();
        out.sort_by_key(|m| m.id);
        if limit > 0 && out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        Ok(out)
    }

    async fn mark_interrupted_agents(&self) -> Result<usize> {
        let mut db = self.db();
        let mut count = 0;
        for agent in db.agents.iter_mut() {
            if agent.status == "running" {
                agent.status = "interrupted".to_string();
                agent.finished_at = Some(now_ms());
                count += 1;
            }
        }
        Ok(count)
    }

    async fn record_usage(&self, record: &UsageRecord) -> Result<()> {
        self.db().usage.push(record.clone());
        Ok(())
    }

    async fn create_approval(&self, record: &ApprovalRecord) -> Result<()> {
        self.db().approvals.push(record.clone());
        Ok(())
    }

    async fn resolve_approval(&self, id: &str, decision: &str) -> Result<()> {
        self.wait_at_gate("resolve_approval").await;
        if self.fail_approval_resolutions.load(Ordering::SeqCst) {
            return Err(Error::Store("approval store unavailable".into()));
        }
        let mut db = self.db();
        match db.approvals.iter_mut().find(|a| a.id == id) {
            Some(approval) if approval.status == "pending" => {
                approval.status = "resolved".to_string();
                approval.decision = Some(decision.to_string());
                approval.resolved_at = Some(now_ms());
                Ok(())
            }
            Some(_) => Err(Error::NotFound(format!("approval {id} is not pending"))),
            None => Err(Error::NotFound(format!("approval {id}"))),
        }
    }

    async fn expire_pending_approvals(&self, run_id: &str, _reason: &str) -> Result<usize> {
        let mut db = self.db();
        let mut count = 0;
        for approval in db.approvals.iter_mut() {
            if approval.run_id == run_id && approval.status == "pending" {
                approval.status = "expired".to_string();
                approval.decision = Some("expired".to_string());
                approval.resolved_at = Some(now_ms());
                count += 1;
            }
        }
        Ok(count)
    }

    async fn list_pending_approvals(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<ApprovalRecord>> {
        Ok(self
            .db()
            .approvals
            .iter()
            .filter(|a| a.status == "pending")
            .filter(|a| session_id.map(|s| a.session_id == s).unwrap_or(true))
            .cloned()
            .collect())
    }

    async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.db().settings.get(key).cloned())
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db()
            .settings
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn all_settings(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .db()
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    async fn create_terminal(&self, terminal: &TerminalSession) -> Result<TerminalSession> {
        let mut db = self.db_mut();
        let stored = TerminalSession {
            created_at: if terminal.created_at == 0 {
                now_ms()
            } else {
                terminal.created_at
            },
            updated_at: now_ms(),
            last_used_at: now_ms(),
            ..terminal.clone()
        };
        db.terminals.insert(stored.id.clone(), stored.clone());
        Ok(stored)
    }

    async fn list_terminals(&self) -> Result<Vec<TerminalSession>> {
        let db = self.db_mut();
        let mut terminals: Vec<TerminalSession> = db.terminals.values().cloned().collect();
        terminals.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then(a.created_at.cmp(&b.created_at))
        });
        Ok(terminals)
    }

    async fn get_terminal(&self, id: &str) -> Result<Option<TerminalSession>> {
        Ok(self.db_mut().terminals.get(id).cloned())
    }

    async fn rename_terminal(&self, id: &str, title: Option<&str>) -> Result<()> {
        let mut db = self.db_mut();
        let Some(existing) = db.terminals.get(id).cloned() else {
            return Err(Error::NotFound(format!("no terminal with id `{id}`")));
        };
        let title = match title {
            Some(title) => title.to_string(),
            None => TerminalSession::default_title(&existing.cwd, existing.position),
        };
        db.terminals
            .insert(id.to_string(), TerminalSession { title, ..existing });
        Ok(())
    }

    async fn touch_terminal(&self, id: &str) -> Result<()> {
        let mut db = self.db_mut();
        if let Some(existing) = db.terminals.get(id).cloned() {
            db.terminals.insert(
                id.to_string(),
                TerminalSession {
                    last_used_at: now_ms(),
                    updated_at: now_ms(),
                    ..existing
                },
            );
        }
        Ok(())
    }

    async fn delete_terminal(&self, id: &str) -> Result<()> {
        self.db_mut().terminals.remove(id);
        Ok(())
    }

    async fn terminal_ids(&self) -> Result<Vec<String>> {
        let db = self.db_mut();
        let mut ids: Vec<(i64, i64, String)> = db
            .terminals
            .values()
            .map(|t| (t.position, t.created_at, t.id.clone()))
            .collect();
        ids.sort();
        Ok(ids.into_iter().map(|(_, _, id)| id).collect())
    }

    async fn claim_paths(
        &self,
        session_id: &str,
        agent_id: &str,
        paths: &[String],
        mode: &str,
    ) -> Result<Vec<ClaimRecord>> {
        // Mirror the SQLite store: normalise paths, insert-or-replace this
        // agent's claims, and return other agents' overlapping claims.
        let paths: Vec<String> = paths
            .iter()
            .map(|p| normalize_claim_path(p))
            .filter(|p| !p.is_empty())
            .collect();
        let mut db = self.db();
        for path in &paths {
            db.claims.retain(|c| {
                !(c.session_id == session_id && c.agent_id == agent_id && c.path == *path)
            });
            db.claims.push(ClaimRecord {
                path: path.clone(),
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                mode: mode.to_string(),
                created_at: now_ms(),
            });
        }
        let conflicts: Vec<ClaimRecord> = db
            .claims
            .iter()
            .filter(|claim| claim.session_id == session_id && claim.agent_id != agent_id)
            .filter(|claim| {
                paths
                    .iter()
                    .any(|path| claim_paths_overlap(path, &claim.path))
            })
            .cloned()
            .collect();
        Ok(conflicts)
    }

    async fn list_claims(&self, session_id: &str) -> Result<Vec<ClaimRecord>> {
        Ok(self
            .db()
            .claims
            .iter()
            .filter(|c| c.session_id == session_id)
            .cloned()
            .collect())
    }

    async fn release_claims(&self, agent_id: &str) -> Result<()> {
        self.db().claims.retain(|c| c.agent_id != agent_id);
        Ok(())
    }

    // ---- rewind ----------------------------------------------------------

    async fn rewind_point(
        &self,
        session_id: &str,
        message_id: i64,
    ) -> Result<kc_core::store::RewindPoint> {
        let db = self.db();
        let row = db
            .messages
            .iter()
            .find(|m| m.id == message_id)
            .ok_or_else(|| Error::NotFound(format!("no message with id {message_id}")))?;
        if row.session_id != session_id {
            return Err(Error::NotFound(format!(
                "message {message_id} does not belong to session {session_id}"
            )));
        }
        let (user_id, prompt) = if row.role == "user" {
            (row.id, row.content.clone())
        } else {
            let before = db
                .messages
                .iter()
                .filter(|m| m.session_id == session_id && m.id < message_id && m.role == "user")
                .max_by_key(|m| m.id)
                .ok_or_else(|| {
                    Error::BadRequest(format!(
                        "message {message_id} has no user message before it; \
                         there is nothing to rewind to"
                    ))
                })?;
            (before.id, before.content.clone())
        };
        let messages_removed = db
            .messages
            .iter()
            .filter(|m| m.session_id == session_id && m.id > user_id && m.rewound_at.is_none())
            .count();
        Ok(kc_core::store::RewindPoint {
            session_id: session_id.to_string(),
            user_message_id: user_id,
            prompt,
            keep_through: user_id,
            before_prompt: db
                .messages
                .iter()
                .filter(|m| m.session_id == session_id && m.id < user_id)
                .map(|m| m.id)
                .max()
                .unwrap_or(0),
            messages_removed,
        })
    }

    async fn mark_messages_rewound(&self, session_id: &str, keep_through: i64) -> Result<usize> {
        self.wait_at_gate("rewind").await;
        let mut db = self.db();
        let now = now_ms();
        let mut hidden = 0;
        for message in db.messages.iter_mut() {
            if message.session_id == session_id
                && message.id > keep_through
                && message.rewound_at.is_none()
            {
                message.rewound_at = Some(now);
                hidden += 1;
            }
        }
        Ok(hidden)
    }

    async fn record_checkpoint(
        &self,
        checkpoint: &kc_core::checkpoint::FileCheckpoint,
    ) -> Result<i64> {
        if self.fail_checkpoints.load(Ordering::SeqCst) {
            return Err(Error::Store("injected checkpoint insert failure".into()));
        }
        let mut db = self.db();
        db.next_checkpoint_id += 1;
        let mut row = checkpoint.clone();
        row.id = db.next_checkpoint_id;
        // Like the real store, the placement happens in a second step.
        row.message_id = 0;
        db.checkpoints.push(row);
        Ok(db.next_checkpoint_id)
    }

    async fn bind_checkpoints(&self, ids: &[i64], message_id: i64) -> Result<usize> {
        if self.fail_checkpoint_binds.load(Ordering::SeqCst) {
            return Err(Error::Store("injected checkpoint bind failure".into()));
        }
        let mut db = self.db();
        let mut bound = 0;
        for row in &mut db.checkpoints {
            if row.message_id == 0 && ids.contains(&row.id) {
                row.message_id = message_id;
                bound += 1;
            }
        }
        Ok(bound)
    }

    async fn list_checkpoints(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<kc_core::checkpoint::FileCheckpoint>> {
        let db = self.db();
        Ok(db
            .checkpoints
            .iter()
            .filter(|c| {
                c.session_id == session_id && (c.message_id == 0 || c.message_id > after_id)
            })
            .cloned()
            .collect())
    }

    async fn list_checkpoint_metadata(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<kc_core::checkpoint::FileCheckpoint>> {
        let db = self.db();
        Ok(db
            .checkpoints
            .iter()
            .filter(|c| {
                c.session_id == session_id && (c.message_id == 0 || c.message_id > after_id)
            })
            .map(|c| {
                let mut row = c.clone();
                row.content.clear();
                row
            })
            .collect())
    }

    async fn checkpoint_content_bounded(
        &self,
        id: i64,
        max_bytes: usize,
    ) -> Result<Option<String>> {
        let db = self.db();
        let row = db
            .checkpoints
            .iter()
            .find(|c| c.id == id && c.existed && !c.capture_failed);
        if row.is_some_and(|c| c.content.len() > max_bytes) {
            return Err(kc_core::error::Error::BadRequest(
                "rewind preview before-images exceed byte limit".into(),
            ));
        }
        Ok(row.map(|c| c.content.clone()))
    }

    async fn checkpoint_content(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .checkpoints
            .iter()
            .find(|c| c.id == id && c.existed && !c.capture_failed)
            .map(|c| c.content.clone()))
    }

    async fn delete_checkpoint(&self, id: i64) -> Result<()> {
        self.db().checkpoints.retain(|c| c.id != id);
        Ok(())
    }

    async fn set_checkpoint_message_id(&self, id: i64, message_id: i64) -> Result<()> {
        let mut db = self.db();
        if let Some(row) = db.checkpoints.iter_mut().find(|c| c.id == id) {
            row.message_id = message_id;
        }
        Ok(())
    }

    async fn last_rewind_at(&self, session_id: &str) -> Result<Option<i64>> {
        let db = self.db();
        Ok(db
            .messages
            .iter()
            .filter(|m| m.session_id == session_id)
            .filter_map(|m| m.rewound_at)
            .max())
    }

    async fn prune_messages_older_than(&self, _days: u64) -> Result<kc_core::store::PruneReport> {
        // The in-memory double never retains on disk, so a retention pass has
        // nothing to delete. Implemented (rather than todo!()) so tests that walk
        // every Store method cannot panic on the fake.
        Ok(kc_core::store::PruneReport::default())
    }

    fn retention_keep_messages(&self) -> usize {
        0
    }

    async fn vacuum(&self) -> Result<()> {
        Ok(())
    }

    async fn incremental_vacuum(&self) -> Result<()> {
        Ok(())
    }

    async fn db_size_bytes(&self) -> Result<u64> {
        Ok(0)
    }

    async fn integrity_check(&self) -> Result<kc_core::store::IntegrityReport> {
        Ok(kc_core::store::IntegrityReport {
            ok: true,
            problems: vec!["ok".to_string()],
            checked_at: 0,
        })
    }

    fn last_integrity_check(&self) -> Option<i64> {
        None
    }
}

// ---------------------------------------------------------------------------
// Scripted provider
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) enum Script {
    /// Stream these chunks, then end the stream normally.
    Chunks(Vec<ProviderChunk>),
    /// Stream these chunks, then die the way a dropped gateway connection does
    /// (`error decoding response body`, a truncated SSE frame, a reset).
    ChunksThenFail(Vec<ProviderChunk>, String),
    /// Emit these chunks and then never finish (used for cancellation tests).
    Hang(Vec<ProviderChunk>),
    /// Fail the request itself.
    Fail(String),
    /// Panic inside `stream_chat` (exercises the runtime's panic guard).
    Panic(String),
}

impl Script {
    pub(crate) fn text(text: &str) -> Self {
        Script::Chunks(vec![
            ProviderChunk::Text(text.to_string()),
            ProviderChunk::Finish {
                reason: "stop".into(),
            },
        ])
    }

    /// One assistant turn that asks for the given tool calls.
    pub(crate) fn call_text(text: &str, calls: &[(&str, &str, &str)]) -> Self {
        let mut chunks = Vec::new();
        if !text.is_empty() {
            chunks.push(ProviderChunk::Text(text.to_string()));
        }
        for (index, (id, name, arguments)) in calls.iter().enumerate() {
            chunks.push(ProviderChunk::ToolCallDelta {
                index,
                id: Some((*id).to_string()),
                name: Some((*name).to_string()),
                arguments_delta: Some((*arguments).to_string()),
            });
        }
        chunks.push(ProviderChunk::Finish {
            reason: "tool_calls".into(),
        });
        Script::Chunks(chunks)
    }

    pub(crate) fn call(calls: &[(&str, &str, &str)]) -> Self {
        Script::call_text("", calls)
    }
}

/// A script queue belonging to one agent.
///
/// `needle` is matched against the conversation's user turns. A spawned child's
/// first user turn *is* its task, which is unique by construction, so the task
/// text is what identifies "this child's script". Without this, a fan-out has two
/// loops popping from one queue: the second child gets the first child's answer
/// and the batch is unpredictable by design, which is exactly what a concurrency
/// test must not be.
type KeyedScripts = Arc<Mutex<Vec<(String, std::collections::VecDeque<Script>)>>>;

struct ScriptedProvider {
    scripts: Arc<Mutex<std::collections::VecDeque<Script>>>,
    keyed: KeyedScripts,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        _cancel: CancellationToken,
    ) -> Result<ProviderStream> {
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        let script = {
            // A keyed queue wins when its needle appears in this request's own
            // conversation; the shared queue is the parent's (and everyone
            // else's) fallback, so existing tests are unaffected.
            let text: String = request
                .messages
                .iter()
                .filter(|message| message.role == kc_core::message::Role::User)
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\u{1}");
            let mut keyed = self.keyed.lock().expect("keyed lock");
            let mut found = None;
            for (needle, queue) in keyed.iter_mut() {
                if text.contains(needle.as_str()) {
                    if let Some(script) = queue.pop_front() {
                        found = Some(script);
                        break;
                    }
                }
            }
            match found {
                Some(script) => Some(script),
                None => self.scripts.lock().expect("script lock").pop_front(),
            }
        };
        match script {
            None => Err(Error::Provider(
                "scripted provider has no script left for this call".into(),
            )),
            Some(Script::Fail(message)) => Err(Error::Provider(message)),
            Some(Script::Panic(message)) => panic!("scripted provider panic: {message}"),
            Some(Script::Chunks(chunks)) => Ok(Box::pin(futures_util::stream::iter(
                chunks.into_iter().map(Ok),
            ))),
            Some(Script::ChunksThenFail(chunks, message)) => {
                use futures_util::StreamExt;
                let head = futures_util::stream::iter(chunks.into_iter().map(Ok));
                let tail = futures_util::stream::iter(vec![Err(Error::Provider(message))]);
                Ok(Box::pin(head.chain(tail)))
            }
            Some(Script::Hang(chunks)) => {
                use futures_util::StreamExt;
                let head = futures_util::stream::iter(chunks.into_iter().map(Ok));
                Ok(Box::pin(head.chain(futures_util::stream::pending())))
            }
        }
    }
}

pub(crate) struct ScriptedFactory {
    scripts: Arc<Mutex<std::collections::VecDeque<Script>>>,
    keyed: KeyedScripts,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    builds: Arc<AtomicUsize>,
}

impl ScriptedFactory {
    pub(crate) fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
            keyed: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            builds: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Give one conversation its own script queue, keyed by a needle that appears
    /// in its user turns (a subagent's task text).
    pub(crate) fn key(&self, needle: &str, scripts: Vec<Script>) {
        self.keyed
            .lock()
            .expect("keyed lock")
            .push((needle.to_string(), scripts.into()));
    }

    pub(crate) fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    pub(crate) fn builds(&self) -> usize {
        self.builds.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl ProviderFactory for ScriptedFactory {
    async fn build(&self, _config: &ModelConfig) -> Result<Arc<dyn Provider>> {
        self.builds.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(ScriptedProvider {
            scripts: Arc::clone(&self.scripts),
            keyed: Arc::clone(&self.keyed),
            requests: Arc::clone(&self.requests),
        }))
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub(crate) struct FakeSettings {
    pub model: ModelConfig,
    pub limits: Limits,
    pub policy: ApprovalPolicy,
    pub subagents: SubagentConfig,
    pub notifications: kc_core::config::NotificationsConfig,
}

impl Default for FakeSettings {
    fn default() -> Self {
        Self {
            model: ModelConfig {
                model: "fake-model".into(),
                ..Default::default()
            },
            limits: Limits::default(),
            policy: ApprovalPolicy {
                mode: kc_core::config::ApprovalMode::Auto,
                ..Default::default()
            },
            subagents: SubagentConfig::default(),
            notifications: kc_core::config::NotificationsConfig::default(),
        }
    }
}

impl FakeSettings {
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub(crate) fn with_model(mut self, model: ModelConfig) -> Self {
        self.model = model;
        self
    }

    pub(crate) fn with_policy(mut self, policy: ApprovalPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub(crate) fn with_subagents(mut self, subagents: SubagentConfig) -> Self {
        self.subagents = subagents;
        self
    }

    pub(crate) fn with_notifications(
        mut self,
        notifications: kc_core::config::NotificationsConfig,
    ) -> Self {
        self.notifications = notifications;
        self
    }
}

#[async_trait]
impl Settings for FakeSettings {
    async fn model(&self) -> ModelConfig {
        self.model.clone()
    }

    /// Mirrors the server's resolution closely enough to prove the runtime
    /// *uses* a session's selection: the selected model id wins, everything else
    /// stays whatever the fake config says.
    async fn resolve_model(&self, selection: Option<&SessionModelSelection>) -> ModelConfig {
        let mut model = self.model.clone();
        if let Some(selection) = selection {
            model.model = selection.model.clone();
            if let Some(effort) = &selection.reasoning_effort {
                model.extra_body.insert(
                    "reasoning_effort".to_string(),
                    serde_json::Value::String(effort.clone()),
                );
            }
        }
        model
    }

    async fn limits(&self) -> Limits {
        self.limits.clone()
    }

    async fn approval_policy(&self) -> ApprovalPolicy {
        self.policy.clone()
    }

    async fn subagent_config(&self) -> SubagentConfig {
        self.subagents.clone()
    }

    async fn notify_config(&self) -> kc_core::config::NotificationsConfig {
        self.notifications.clone()
    }
}

// ---------------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------------

/// Returns its arguments back, so tests can assert the round-trip.
pub(crate) struct EchoTool {
    pub name: String,
}

impl EchoTool {
    pub(crate) fn boxed(name: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
        })
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Echo the arguments back to the caller.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        ToolOutcome::ok(format!("echoed:{}", request.arguments))
    }
}

/// Steers the session from *inside* a tool call.
///
/// This is the honest reproduction of a user typing while the agent works: the
/// run is in flight (a tool is executing), so `steer` must be accepted, and the
/// next iteration must pick the message up. Doing it from the test body instead
/// would race the loop and make the test flaky in the direction of "sometimes
/// accepted after the run ended", which proves nothing.
pub(crate) struct SteerTool {
    pub name: String,
    pub runtime: Arc<crate::Runtime>,
    pub session: String,
    pub message: String,
}

impl SteerTool {
    pub(crate) fn boxed(
        name: &str,
        runtime: Arc<crate::Runtime>,
        session: &str,
        message: &str,
    ) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
            runtime,
            session: session.to_string(),
            message: message.to_string(),
        })
    }
}

#[async_trait]
impl Tool for SteerTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Steer the session, as if the user typed mid-run.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        match self.runtime.steer(&self.session, self.message.clone()) {
            Ok(steer) => ToolOutcome::ok(format!("steered:{}", steer.id)),
            Err(error) => ToolOutcome::err(format!("steer refused: {error}")),
        }
    }
}

/// Queues a follow-up from *inside* a tool call, for the same reason as
/// [`SteerTool`].
///
/// `messages` holds every prompt to queue, in order. One entry is the ordinary
/// "and then…" case; several is what catches a drain that starts the next run
/// before the previous one has deregistered (the queue used to be wiped after the
/// second entry was refused).
pub(crate) struct FollowUpTool {
    pub name: String,
    pub runtime: Arc<crate::Runtime>,
    pub session: String,
    pub messages: Vec<String>,
}

impl FollowUpTool {
    pub(crate) fn boxed(
        name: &str,
        runtime: Arc<crate::Runtime>,
        session: &str,
        message: &str,
    ) -> Arc<dyn Tool> {
        Self::boxed_many(name, runtime, session, vec![message.to_string()])
    }

    pub(crate) fn boxed_many(
        name: &str,
        runtime: Arc<crate::Runtime>,
        session: &str,
        messages: Vec<String>,
    ) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
            runtime,
            session: session.to_string(),
            messages,
        })
    }
}

#[async_trait]
impl Tool for FollowUpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Queue a follow-up prompt for this session.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        let mut ids = Vec::new();
        for message in &self.messages {
            match self
                .runtime
                .enqueue_follow_up(&self.session, message.clone())
            {
                Ok(follow_up) => ids.push(follow_up.id),
                Err(error) => return ToolOutcome::err(format!("follow-up refused: {error}")),
            }
        }
        ToolOutcome::ok(format!("queued:{}", ids.join(",")))
    }
}

/// Returns a body of the size it is asked for (`{"bytes": N}`), so a test can
/// drive the context budget without carrying a megabyte fixture.
pub(crate) struct BigOutputTool {
    pub name: String,
}

impl BigOutputTool {
    pub(crate) fn boxed(name: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
        })
    }
}

#[async_trait]
impl Tool for BigOutputTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Return a body of `bytes` characters.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "bytes": { "type": "integer" } }
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        let bytes = request
            .arguments
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1_000) as usize;
        ToolOutcome::ok("a".repeat(bytes))
    }
}

/// Returns an image, but only when the tool context says the model accepts one.
///
/// Mirrors what `read_file` does: the decision is the tool's, the capability is
/// the runtime's, and a text-only model must get a refusal rather than pixels.
pub(crate) struct ImageTool {
    pub name: String,
}

impl ImageTool {
    pub(crate) fn boxed(name: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
        })
    }
}

#[async_trait]
impl Tool for ImageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Return an image for the model to look at.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if !ctx.supports_images {
            return ToolOutcome::err("the active model cannot see images");
        }
        ToolOutcome::ok("shot.png — image/png").with_images(vec![
            kc_core::message::ImageAttachment {
                media_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
                bytes: 8,
                name: Some("shot.png".into()),
            },
        ])
    }
}

/// Asks for approval, then reports the decision.
pub(crate) struct ApprovalTool {
    pub name: String,
    pub kind: ApprovalKind,
    pub payload: serde_json::Value,
}

impl ApprovalTool {
    pub(crate) fn shell(name: &str, command: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
            kind: ApprovalKind::ShellCommand,
            payload: serde_json::json!({ "command": command }),
        })
    }

    pub(crate) fn write(name: &str, path: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
            kind: ApprovalKind::FileWrite,
            payload: serde_json::json!({ "path": path }),
        })
    }
}

#[async_trait]
impl Tool for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Requests approval and then performs the action.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Shell
    }

    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let detail = self
            .payload
            .get("command")
            .or_else(|| self.payload.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let decision = ctx
            .approvals
            .ask(
                self.kind.clone(),
                format!("{} approval", self.name),
                detail,
                self.payload.clone(),
            )
            .await;
        if decision.is_allowed() {
            ToolOutcome::ok(format!("allowed:{}", decision.as_str()))
        } else {
            ToolOutcome::err(format!("not allowed:{}", decision.as_str()))
        }
    }
}

/// A non-builtin tool (plugin/MCP) that records whether it was invoked.
///
/// The runtime must gate these itself: nothing in this tool asks for approval,
/// which mirrors the audited plugin/MCP implementations.
pub(crate) struct ExternalTool {
    pub name: String,
    pub origin: ToolOrigin,
    pub class: ToolClass,
    /// `None` keeps the trait default (`class != ReadOnly`).
    pub require_approval: Option<bool>,
    invocations: Arc<AtomicUsize>,
}

impl ExternalTool {
    pub(crate) fn new(name: &str, origin: ToolOrigin, class: ToolClass) -> Self {
        Self {
            name: name.to_string(),
            origin,
            class,
            require_approval: None,
            invocations: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn mcp(name: &str, server: &str) -> Self {
        Self::new(
            name,
            ToolOrigin::Mcp {
                server: server.to_string(),
            },
            ToolClass::External,
        )
    }

    pub(crate) fn plugin(name: &str, plugin: &str) -> Self {
        Self::new(
            name,
            ToolOrigin::Plugin {
                name: plugin.to_string(),
            },
            ToolClass::External,
        )
    }

    pub(crate) fn with_require_approval(mut self, require: bool) -> Self {
        self.require_approval = Some(require);
        self
    }

    pub(crate) fn with_class(mut self, class: ToolClass) -> Self {
        self.class = class;
        self
    }

    /// Counter of successful invocations (the tool's "side effect").
    pub(crate) fn invocations(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.invocations)
    }
}

#[async_trait]
impl Tool for ExternalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "A non-builtin tool that records invocations.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: self.origin.clone(),
        }
    }

    fn class(&self) -> ToolClass {
        self.class
    }

    fn requires_approval(&self) -> bool {
        self.require_approval
            .unwrap_or(!matches!(self.class, ToolClass::ReadOnly))
    }

    async fn invoke(&self, request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        ToolOutcome::ok(format!("invoked:{}:{}", self.name, request.arguments))
    }
}

/// A tool whose schema declares required arguments, so the runtime's
/// schema check and its "how to call this" hint can be exercised.
///
/// Three modes, because they must be told apart: a call that runs and fails
/// (an execution failure, no hint), a call that refuses the request itself
/// (`Error::BadRequest`, hint), and a call that runs cleanly.
pub(crate) struct RequiredArgsTool {
    pub name: String,
    pub required: Vec<String>,
    mode: RequiredMode,
    invocations: Arc<AtomicUsize>,
    /// Declare `required` as `properties` without making any of them required.
    optional: bool,
}

enum RequiredMode {
    Ok,
    Fail,
    Refuse,
}

impl RequiredArgsTool {
    pub(crate) fn boxed(name: &str, required: &[&str]) -> Arc<Self> {
        Self::build(name, required, RequiredMode::Ok)
    }

    pub(crate) fn failing(name: &str, required: &[&str]) -> Arc<Self> {
        Self::build(name, required, RequiredMode::Fail)
    }

    pub(crate) fn refusing(name: &str, required: &[&str]) -> Arc<Self> {
        Self::build(name, required, RequiredMode::Refuse)
    }

    /// Refuses the request itself while declaring every argument *optional*, so
    /// schema validation lets the call through and the tool's own
    /// `Error::BadRequest` is what the loop sees.
    pub(crate) fn refusing_optional(name: &str, properties: &[&str]) -> Arc<Self> {
        let mut tool = Self::build(name, properties, RequiredMode::Refuse);
        Arc::get_mut(&mut tool)
            .expect("a fresh tool handle")
            .optional = true;
        tool
    }

    fn build(name: &str, required: &[&str], mode: RequiredMode) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            required: required.iter().map(|s| s.to_string()).collect(),
            mode,
            invocations: Arc::new(AtomicUsize::new(0)),
            optional: false,
        })
    }

    /// How many times the tool actually ran, so a test can assert that a call
    /// the schema rejected never reached it.
    pub(crate) fn invocations(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.invocations)
    }
}

#[async_trait]
impl Tool for RequiredArgsTool {
    fn spec(&self) -> ToolSpec {
        let mut properties = serde_json::Map::new();
        for key in &self.required {
            properties.insert(key.clone(), serde_json::json!({"type": "string"}));
        }
        ToolSpec {
            name: self.name.clone(),
            description: "Needs the declared arguments.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": if self.optional { serde_json::json!([]) } else { serde_json::json!(self.required) },
            }),
            origin: ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            RequiredMode::Ok => ToolOutcome::ok(format!("invoked:{}", request.name)),
            RequiredMode::Fail => ToolOutcome::err("boom"),
            RequiredMode::Refuse => ToolOutcome::err(
                Error::BadRequest("pattern may not be empty".to_string()).to_string(),
            ),
        }
    }
}

/// A tool that blocks until the run's cancellation token fires.
///
/// Used to prove that a timed-out or cancelled subagent gets the chance to
/// observe cancellation (and kill its process group) before its loop future is
/// dropped.
pub(crate) struct SlowCancelTool {
    pub name: String,
    started: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl SlowCancelTool {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            started: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn started(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.started)
    }

    pub(crate) fn cancelled(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    pub(crate) fn finished(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.finished)
    }
}

#[async_trait]
impl Tool for SlowCancelTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Blocks until the runtime cancels the run.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                self.cancelled.store(true, Ordering::SeqCst);
                ToolOutcome::err("cancelled by the runtime")
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                self.finished.store(true, Ordering::SeqCst);
                ToolOutcome::ok("finished after waiting")
            }
        }
    }
}

/// A tool that parks its caller until a test releases it.
///
/// A background subagent has to be observable *while it works*, and a test has no
/// other way to hold one still: the loop only stops between iterations, so
/// something inside an iteration has to be the thing that waits. `arrived` counts
/// the calls that got here and `release` lets them all go.
#[derive(Clone)]
pub(crate) struct GateTool {
    pub name: String,
    arrived: Arc<AtomicUsize>,
    /// Latched, not just signalled: a test that releases *before* the agent
    /// reaches the gate must still let it through, or the whole run parks on a
    /// wake-up that already happened.
    released: Arc<AtomicBool>,
    release: Arc<tokio::sync::Notify>,
}

impl GateTool {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            arrived: Arc::new(AtomicUsize::new(0)),
            released: Arc::new(AtomicBool::new(false)),
            release: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// The tool plus a handle on its gate, so a test can register it and still
    /// release the invocation it is waiting on.
    pub(crate) fn pair(name: &str) -> (Self, GateHandle) {
        let tool = Self::new(name);
        let handle = GateHandle {
            arrived: Arc::clone(&tool.arrived),
            released: Arc::clone(&tool.released),
            release: Arc::clone(&tool.release),
        };
        (tool, handle)
    }

    pub(crate) fn boxed(name: &str) -> Arc<dyn Tool> {
        Arc::new(Self::new(name))
    }
}

/// The far end of a [`GateTool`]: what a test holds while the agent holds a call.
#[derive(Clone)]
pub(crate) struct GateHandle {
    arrived: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    release: Arc<tokio::sync::Notify>,
}

impl GateHandle {
    /// How many calls have reached the gate.
    pub(crate) fn arrived(&self) -> usize {
        self.arrived.load(Ordering::SeqCst)
    }

    /// Let every parked call return, and every later one through.
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.release.notify_waiters();
    }
}

#[async_trait]
impl Tool for GateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Blocks until the test releases it.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        self.arrived.fetch_add(1, Ordering::SeqCst);
        loop {
            if self.released.load(Ordering::SeqCst) {
                return ToolOutcome::ok("released");
            }
            // Register before re-checking, so a release between the check and the
            // await cannot be missed.
            let notified = self.release.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.released.load(Ordering::SeqCst) {
                return ToolOutcome::ok("released");
            }
            tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => return ToolOutcome::err("cancelled while parked"),
                _ = notified => {}
            }
        }
    }
}

/// Panics on every invocation (exercises the tool panic guard).
pub(crate) struct PanicTool {
    pub name: String,
}

impl PanicTool {
    pub(crate) fn boxed(name: &str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name: name.to_string(),
        })
    }
}

#[async_trait]
impl Tool for PanicTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Always panics.".into(),
            parameters: serde_json::json!({ "type": "object" }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        panic!("this tool is broken on purpose")
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub(crate) struct Harness {
    pub runtime: Arc<Runtime>,
    pub store: Arc<FakeStore>,
    pub factory: Arc<ScriptedFactory>,
    pub settings: Arc<FakeSettings>,
    pub session_id: String,
    pub workspace_dir: PathBuf,
    pub events: broadcast::Receiver<EventEnvelope>,
}

pub(crate) struct HarnessBuilder {
    scripts: Vec<Script>,
    settings: FakeSettings,
    tools: Vec<Arc<dyn Tool>>,
    /// The project the harness session belongs to, when a test cares.
    ///
    /// `None` is the ordinary case: most tests are about one conversation, not
    /// about which project it lives in. A notification carries its session's
    /// project, so a test of that field opts in here.
    project_id: Option<String>,
    hooks: Arc<dyn Hooks>,
}

impl HarnessBuilder {
    pub(crate) fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts,
            settings: FakeSettings::default(),
            tools: Vec::new(),
            project_id: None,
            hooks: Arc::new(NoHooks),
        }
    }

    pub(crate) fn hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Put the harness session in a project, so emitted notifications name it.
    pub(crate) fn project(mut self, project_id: &str) -> Self {
        self.project_id = Some(project_id.to_string());
        self
    }

    pub(crate) fn settings(mut self, settings: FakeSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Replace the model config (sampling + retry budgets) the runtime sees.
    pub(crate) fn model(mut self, model: ModelConfig) -> Self {
        self.settings.model = model;
        self
    }

    pub(crate) fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub(crate) async fn build(self) -> Harness {
        let dir = std::env::temp_dir().join(format!("kc-runtime-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp workspace");
        let workspace = Arc::new(Workspace::new(&dir).expect("workspace"));
        let store = Arc::new(FakeStore::new());
        let factory = Arc::new(ScriptedFactory::new(self.scripts));
        let settings = Arc::new(self.settings);

        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.register(tool);
        }

        let runtime = Runtime::new(RuntimeDeps {
            store: store.clone(),
            providers: factory.clone(),
            tools: Arc::new(registry),
            settings: settings.clone(),
            hooks: self.hooks,
            workspaces: Arc::new(crate::SingleWorkspace::new(workspace)),
        });
        let events = runtime.subscribe();
        let session_id = format!("s_{}", uuid::Uuid::new_v4().simple());
        store
            .create_session(
                &session_id,
                "test session",
                &dir.to_string_lossy(),
                self.project_id.as_deref(),
            )
            .await
            .expect("create session");

        Harness {
            runtime,
            store,
            factory,
            settings,
            session_id,
            workspace_dir: dir,
            events,
        }
    }
}

impl Harness {
    pub(crate) fn session(&self) -> &str {
        &self.session_id
    }

    pub(crate) async fn submit(&mut self, prompt: &str) -> crate::RunHandle {
        self.runtime
            .submit(&self.session_id, prompt.to_string())
            .await
            .expect("submit")
    }

    /// Next event, failing the test instead of hanging.
    pub(crate) async fn next_event(&mut self) -> EventEnvelope {
        match tokio::time::timeout(Duration::from_secs(5), self.events.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => panic!("event stream closed: {e}"),
            Err(_) => panic!("timed out waiting for the next event"),
        }
    }

    /// Collect events until `RunFinished` arrives.
    pub(crate) async fn run_to_finish(&mut self, prompt: &str) -> Vec<EventEnvelope> {
        self.submit(prompt).await;
        self.collect_until_finished().await
    }

    pub(crate) async fn collect_until_finished(&mut self) -> Vec<EventEnvelope> {
        let mut collected = Vec::new();
        loop {
            let event = self.next_event().await;
            let finished = matches!(event.event, kc_core::events::Event::RunFinished { .. });
            collected.push(event);
            if finished {
                return collected;
            }
        }
    }

    /// Collect the frames still arriving until every loop (parent and child) is
    /// gone.
    ///
    /// `collect_until_finished` stops at the parent's `run_finished`, which a
    /// background child legitimately outlives. A test about *that child* needs the
    /// frames it emits afterwards, and they are already in flight by the time the
    /// parent settles.
    pub(crate) async fn collect_until_quiet(&mut self) -> Vec<EventEnvelope> {
        let mut collected = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !self.runtime.active_runs().is_empty() {
            match tokio::time::timeout(Duration::from_millis(50), self.events.recv()).await {
                Ok(Ok(event)) => collected.push(event),
                Ok(Err(e)) => panic!("event stream closed: {e}"),
                Err(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        // Drain whatever is already queued, without waiting for more.
        while let Ok(event) = self.events.try_recv() {
            collected.push(event);
        }
        collected
    }

    /// Wait (bounded) until no run is registered any more.
    pub(crate) async fn wait_idle(&self) -> Vec<ActiveRun> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let active = self.runtime.active_runs();
            if active.is_empty() || tokio::time::Instant::now() >= deadline {
                return active;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub(crate) async fn message_roles(&self) -> Vec<String> {
        self.store
            .messages()
            .into_iter()
            .filter(|m| m.session_id == self.session_id)
            .map(|m| m.role)
            .collect()
    }

    pub(crate) async fn session_messages(&self) -> Vec<StoredMessage> {
        self.store
            .messages()
            .into_iter()
            .filter(|m| m.session_id == self.session_id)
            .collect()
    }

    /// Accept a steering message for the session, as the HTTP route does.
    pub(crate) fn steer(&self, content: &str) -> crate::QueuedSteer {
        self.runtime
            .steer(&self.session_id, content.to_string())
            .expect("steering message accepted")
    }

    /// Accept a follow-up prompt for the session, as the HTTP route does.
    pub(crate) fn follow_up(&self, content: &str) -> crate::QueuedFollowUp {
        self.runtime
            .enqueue_follow_up(&self.session_id, content.to_string())
            .expect("follow-up accepted")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.workspace_dir);
    }
}

/// A `RuntimeInner` for unit tests that only need event plumbing.
pub(crate) fn test_inner() -> Arc<RuntimeInner> {
    test_inner_with_settings(FakeSettings::default())
}

pub(crate) fn test_inner_with_settings(settings: FakeSettings) -> Arc<RuntimeInner> {
    let workspace = Arc::new(Workspace::new(std::env::temp_dir()).expect("workspace"));
    Arc::new(RuntimeInner {
        deps: RuntimeDeps {
            store: Arc::new(FakeStore::new()),
            providers: Arc::new(ScriptedFactory::new(Vec::new())),
            tools: Arc::new(ToolRegistry::new()),
            settings: Arc::new(settings),
            hooks: Arc::new(NoHooks),
            workspaces: Arc::new(crate::SingleWorkspace::new(workspace)),
        },
        events: broadcast::channel(64).0,
        tools: RwLock::new(Arc::new(ToolRegistry::new())),
        runs: Mutex::new(HashMap::new()),
        session_lifecycle: Mutex::new(HashMap::new()),
        approvals: ApprovalRegistry::new(),
        subagents: Arc::new(SubagentLimiter::new(Default::default())),
        hub: Arc::new(crate::hub::AgentHub::new()),
        seq: Mutex::new(0),
        queue_publication: Mutex::new(()),
        events_emitted: AtomicU64::new(0),
        session_allow: Mutex::new(std::collections::HashSet::new()),
        steers: Mutex::new(HashMap::new()),
        follow_ups: Mutex::new(HashMap::new()),
    })
}

/// Helper for tools that want to record what they saw.
pub(crate) fn tool_call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

pub(crate) fn new_message(session: &str, role: &str, content: &str) -> NewMessage {
    NewMessage {
        session_id: session.to_string(),
        run_id: None,
        agent_id: None,
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        images: Vec::new(),
        reasoning_content: None,
    }
}
