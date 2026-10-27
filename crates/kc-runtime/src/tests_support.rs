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
use kc_core::config::{ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
use kc_core::error::{Error, Result};
use kc_core::events::EventEnvelope;
use kc_core::hooks::NoHooks;
use kc_core::message::ToolCall;
use kc_core::paths::Workspace;
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderFactory, ProviderStream};
use kc_core::settings::Settings;
use kc_core::store::{
    AgentRecord, ApprovalRecord, ClaimRecord, NewMessage, RunRecord, RunStatus, Session,
    SessionSummary, Store, StoredMessage,
};
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolRegistry, ToolSpec};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn now_ms() -> i64 {
    crate::events::now_ms()
}

// ---------------------------------------------------------------------------
// FakeStore
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Db {
    sessions: Vec<Session>,
    messages: Vec<StoredMessage>,
    next_message_id: i64,
    runs: Vec<RunRecord>,
    agents: Vec<AgentRecord>,
    approvals: Vec<ApprovalRecord>,
    claims: Vec<ClaimRecord>,
    settings: BTreeMap<String, String>,
}

pub(crate) struct FakeStore {
    db: Mutex<Db>,
    fail_appends: std::sync::atomic::AtomicBool,
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
        }
    }

    /// Make the next `append_message` calls fail, to exercise storage errors.
    pub(crate) fn fail_appends(&self, fail: bool) {
        self.fail_appends.store(fail, Ordering::SeqCst);
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Db> {
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
}

#[async_trait]
impl Store for FakeStore {
    async fn create_session(&self, id: &str, title: &str, workspace: &str) -> Result<Session> {
        let now = now_ms();
        let session = Session {
            id: id.to_string(),
            title: title.to_string(),
            workspace: workspace.to_string(),
            created_at: now,
            updated_at: now,
        };
        self.db().sessions.push(session.clone());
        Ok(session)
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
                created_at: s.created_at,
                updated_at: s.updated_at,
                message_count: 0,
                last_run_status: None,
                running: false,
            })
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

    async fn append_message(&self, message: &NewMessage) -> Result<i64> {
        if self.fail_appends.load(Ordering::SeqCst) {
            return Err(Error::Store("disk is full".into()));
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
            .filter(|m| m.session_id == session_id)
            .cloned()
            .collect();
        if limit > 0 && out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        Ok(out)
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
            .filter(|m| m.session_id == session_id && m.id > after_id)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn create_run(&self, id: &str, session_id: &str, model: &str, prompt: &str) -> Result<()> {
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

    async fn create_approval(&self, record: &ApprovalRecord) -> Result<()> {
        self.db().approvals.push(record.clone());
        Ok(())
    }

    async fn resolve_approval(&self, id: &str, decision: &str) -> Result<()> {
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

    async fn claim_paths(
        &self,
        session_id: &str,
        agent_id: &str,
        paths: &[String],
        mode: &str,
    ) -> Result<Vec<ClaimRecord>> {
        let mut db = self.db();
        let mut created = Vec::new();
        for path in paths {
            let claim = ClaimRecord {
                path: path.clone(),
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                mode: mode.to_string(),
                created_at: now_ms(),
            };
            db.claims.push(claim.clone());
            created.push(claim);
        }
        Ok(created)
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
}

// ---------------------------------------------------------------------------
// Scripted provider
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) enum Script {
    /// Stream these chunks, then end the stream normally.
    Chunks(Vec<ProviderChunk>),
    /// Emit these chunks and then never finish (used for cancellation tests).
    Hang(Vec<ProviderChunk>),
    /// Fail the request itself.
    Fail(String),
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

struct ScriptedProvider {
    scripts: Arc<Mutex<std::collections::VecDeque<Script>>>,
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
        self.requests.lock().expect("requests lock").push(request);
        let script = self.scripts.lock().expect("script lock").pop_front();
        match script {
            None => Err(Error::Provider(
                "scripted provider has no script left for this call".into(),
            )),
            Some(Script::Fail(message)) => Err(Error::Provider(message)),
            Some(Script::Chunks(chunks)) => Ok(Box::pin(futures_util::stream::iter(
                chunks.into_iter().map(Ok),
            ))),
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
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    builds: Arc<AtomicUsize>,
}

impl ScriptedFactory {
    pub(crate) fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            builds: Arc::new(AtomicUsize::new(0)),
        }
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
        }
    }
}

impl FakeSettings {
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
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
}

#[async_trait]
impl Settings for FakeSettings {
    async fn model(&self) -> ModelConfig {
        self.model.clone()
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
}

impl HarnessBuilder {
    pub(crate) fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts,
            settings: FakeSettings::default(),
            tools: Vec::new(),
        }
    }

    pub(crate) fn settings(mut self, settings: FakeSettings) -> Self {
        self.settings = settings;
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
            hooks: Arc::new(NoHooks),
            workspace,
        });
        let events = runtime.subscribe();
        let session_id = format!("s_{}", uuid::Uuid::new_v4().simple());
        store
            .create_session(&session_id, "test session", &dir.to_string_lossy())
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
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.workspace_dir);
    }
}

/// A `RuntimeInner` for unit tests that only need event plumbing.
pub(crate) fn test_inner() -> Arc<RuntimeInner> {
    let workspace = Arc::new(Workspace::new(std::env::temp_dir()).expect("workspace"));
    Arc::new(RuntimeInner {
        deps: RuntimeDeps {
            store: Arc::new(FakeStore::new()),
            providers: Arc::new(ScriptedFactory::new(Vec::new())),
            tools: Arc::new(ToolRegistry::new()),
            settings: Arc::new(FakeSettings::default()),
            hooks: Arc::new(NoHooks),
            workspace,
        },
        events: broadcast::channel(64).0,
        tools: RwLock::new(Arc::new(ToolRegistry::new())),
        runs: Mutex::new(HashMap::new()),
        approvals: ApprovalRegistry::new(),
        subagents: Arc::new(SubagentLimiter::new(Default::default())),
        seq: AtomicU64::new(0),
        events_emitted: AtomicU64::new(0),
        session_allow: Mutex::new(std::collections::HashSet::new()),
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
    }
}