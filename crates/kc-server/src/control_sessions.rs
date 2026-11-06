//! Control-chat tools over the whole server: audit, guide and start sessions.
//!
//! Every tool here is restricted to the single control session and every mutating
//! one passes through the ordinary approval policy (or is refused under
//! `read_only`). They act through the same store and runtime a user acts through;
//! none of them reads or writes project files directly.

use async_trait::async_trait;
use kc_core::{
    approval::ApprovalKind,
    config::SessionModelSelection,
    events::Event,
    store::{AgentStatus, AgentSummary, StoredMessage},
    tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

/// One message may not blow the transcript window handed to the model.
const MESSAGE_PREVIEW_CHARS: usize = 4_000;
/// …nor may the whole read.
const READ_BUDGET_CHARS: usize = 24_000;

/// Non-empty, trimmed and within bounds — the same rule the HTTP routes use.
fn check_text(value: &str, name: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max {
        return Err(format!("{name} must be non-empty and at most {max} bytes"));
    }
    Ok(())
}

async fn control_only(state: &AppState, ctx: &ToolCtx) -> Option<ToolOutcome> {
    if ctx.approvals.agent_id.is_some() {
        return Some(ToolOutcome::err(
            "Control tools belong to the control chat, not to subagents.",
        ));
    }
    match state
        .store()
        .is_control_session(ctx.approvals.session_id.as_str())
        .await
    {
        Ok(true) => None,
        Ok(false) => Some(ToolOutcome::err(
            "Only the control chat may use this tool; it is not available to ordinary sessions.",
        )),
        Err(e) => Some(ToolOutcome::err(e.to_string())),
    }
}

/// Apply the approval policy to one mutating control action.
async fn gate(
    ctx: &ToolCtx,
    title: String,
    detail: String,
    payload: serde_json::Value,
) -> Option<ToolOutcome> {
    match ctx.settings.approval_policy().await.mode {
        kc_core::config::ApprovalMode::ReadOnly => Some(ToolOutcome::err(format!(
            "`{}` is refused: the approval policy is read_only",
            title
        ))),
        kc_core::config::ApprovalMode::Ask | kc_core::config::ApprovalMode::AskCommands => {
            let decision = ctx
                .approvals
                .ask(ApprovalKind::Other, title, detail, payload)
                .await;
            if decision.is_allowed() {
                None
            } else {
                Some(ToolOutcome::err(format!(
                    "denied by user ({})",
                    decision.as_str()
                )))
            }
        }
        kc_core::config::ApprovalMode::Auto => None,
    }
}

fn project_name(projects: &[kc_core::store::Project], id: Option<&str>) -> String {
    id.and_then(|id| projects.iter().find(|project| project.id == id))
        .map(|project| project.name.clone())
        .unwrap_or_else(|| id.unwrap_or("no project").to_string())
}

// ---------------------------------------------------------------- list

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    project_id: Option<String>,
    #[serde(default)]
    running: Option<bool>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

pub struct ListSessions(pub AppState);

#[async_trait]
impl Tool for ListSessions {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_list_sessions".into(),
            description: "List sessions across every project, newest activity first: id, title, project, latest run status, whether it is running, message count and last update. Read-only; use it to audit what work exists before reading or guiding a session.".into(),
            parameters: json!({"type":"object","properties":{
                "project_id":{"type":"string","description":"Only sessions in this registered project"},
                "running":{"type":"boolean","description":"Only sessions with a run in flight"},
                "query":{"type":"string","description":"Case-insensitive match on the title"},
                "limit":{"type":"integer","minimum":1,"maximum":200}
            },"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: ListArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        let limit = args.limit.unwrap_or(50).clamp(1, 200) as usize;
        let projects = match self.0.store().list_projects().await {
            Ok(projects) => projects,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let sessions = match self.0.store().list_sessions().await {
            Ok(sessions) => sessions,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let query = args.query.as_deref().map(str::to_lowercase);
        let mut rows: Vec<_> = sessions
            .into_iter()
            .filter(|session| {
                args.project_id
                    .as_deref()
                    .is_none_or(|id| session.project_id.as_deref() == Some(id))
            })
            .filter(|session| {
                query
                    .as_deref()
                    .is_none_or(|needle| session.title.to_lowercase().contains(needle))
            })
            .map(|session| {
                let running = self.0.runtime().has_active_run(&session.id);
                (session, running)
            })
            .filter(|(_, running)| args.running.is_none_or(|want| want == *running))
            .collect();
        rows.sort_by_key(|(session, running)| (!*running, std::cmp::Reverse(session.updated_at)));
        let total = rows.len();
        let rows: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(|(session, running)| {
                json!({
                    "id": session.id,
                    "title": session.title,
                    "project_id": session.project_id,
                    "project": project_name(&projects, session.project_id.as_deref()),
                    "running": running,
                    "last_run_status": session.last_run_status,
                    "message_count": session.message_count,
                    "updated_at": session.updated_at,
                })
            })
            .collect();
        ToolOutcome::ok(json!({"total": total, "sessions": rows}).to_string())
    }
}

// ---------------------------------------------------------------- read

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    session_id: String,
    #[serde(default)]
    limit: Option<u32>,
}

pub struct ReadSession(pub AppState);

#[async_trait]
impl Tool for ReadSession {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_read_session".into(),
            description: "Read the newest messages of one session so you can audit what it is doing, with the session's own status and latest run. Read-only and bounded; long messages and transcripts are truncated with a marker.".into(),
            parameters: json!({"type":"object","properties":{
                "session_id":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":100,"description":"How many newest messages (default 30)"}
            },"required":["session_id"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: ReadArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if check_text(&args.session_id, "session_id", 128).is_err() {
            return ToolOutcome::err("session_id must be non-empty and at most 128 bytes");
        }
        let limit = args.limit.unwrap_or(30).clamp(1, 100);
        let session = match self.0.store().get_session(&args.session_id).await {
            Ok(Some(session)) => session,
            Ok(None) => return ToolOutcome::err("no such session"),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let messages = match self
            .0
            .store()
            .list_messages(&session.id, limit as usize + 1)
            .await
        {
            Ok(messages) => messages,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let truncated = messages.len() > limit as usize;
        let messages: Vec<StoredMessage> = messages
            .into_iter()
            .rev()
            .take(limit as usize)
            .rev()
            .collect();
        let runs = self
            .0
            .store()
            .list_runs(&session.id, 3)
            .await
            .unwrap_or_default();
        let mut budget = READ_BUDGET_CHARS;
        let mut rendered = Vec::new();
        for message in messages {
            if budget == 0 {
                break;
            }
            let mut content: String = message
                .content
                .chars()
                .take(MESSAGE_PREVIEW_CHARS)
                .collect();
            if content.chars().count() < message.content.chars().count() {
                content.push_str("\n… [truncated]");
            }
            if content.chars().count() > budget {
                content = content.chars().take(budget).collect::<String>();
                content.push_str("\n… [budget reached]");
            }
            budget = budget.saturating_sub(content.chars().count());
            rendered.push(json!({
                "role": message.role,
                "content": content,
                "tool_calls": message.tool_calls.iter().map(|call| call.name.clone()).collect::<Vec<_>>(),
                "name": message.name,
            }));
        }
        ToolOutcome::ok(
            json!({
                "session": {
                    "id": session.id,
                    "title": session.title,
                    "project_id": session.project_id,
                    "workspace": session.workspace,
                    "updated_at": session.updated_at,
                },
                "running": self.0.runtime().has_active_run(&session.id),
                "runs": runs.iter().map(|run| json!({"id":run.id,"status":run.status,"error":run.error})).collect::<Vec<_>>(),
                "messages_truncated": truncated,
                "messages": rendered,
            })
            .to_string(),
        )
    }
}

// ---------------------------------------------------------------- send

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    session_id: String,
    /// `send` starts a run; `steer` folds into the turn in flight; `follow_up`
    /// queues the next turn.
    mode: String,
    content: String,
}

pub struct SendMessage(pub AppState);

#[async_trait]
impl Tool for SendMessage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_send_message".into(),
            description: "Send a message to an existing session as its user. mode=send starts a run (refused while one is active), mode=steer folds the text into the running turn, mode=follow_up queues it for after the current run. Never targets the control chat itself. Mutating, so it follows the approval policy.".into(),
            parameters: json!({"type":"object","properties":{
                "session_id":{"type":"string"},
                "mode":{"type":"string","enum":["send","steer","follow_up"]},
                "content":{"type":"string"}
            },"required":["session_id","mode","content"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: SendArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Err(e) = check_text(&args.session_id, "session_id", 128) {
            return ToolOutcome::err(e);
        }
        if let Err(e) = check_text(&args.content, "content", 1024 * 1024) {
            return ToolOutcome::err(e);
        }
        if args.session_id == ctx.approvals.session_id.as_str() {
            return ToolOutcome::err(
                "Refusing to message the control chat from itself; talk to the user instead.",
            );
        }
        if !matches!(args.mode.as_str(), "send" | "steer" | "follow_up") {
            return ToolOutcome::err("mode must be `send`, `steer` or `follow_up`");
        }
        match self.0.store().get_session(&args.session_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return ToolOutcome::err("no such session"),
            Err(e) => return ToolOutcome::err(e.to_string()),
        }
        let running = self.0.runtime().has_active_run(&args.session_id);
        if let Some(err) = gate(
            &ctx,
            format!("Send to session: {}", args.session_id),
            format!(
                "mode={} · {} · running={running}",
                args.mode,
                snippet(&args.content)
            ),
            json!({"tool":"control_send_message","session_id":args.session_id,"mode":args.mode,"content":args.content}),
        )
        .await
        {
            return err;
        }
        match args.mode.as_str() {
            "send" => {
                if running {
                    return ToolOutcome::err(
                        "That session already has a run in flight; use mode=steer or follow_up.",
                    );
                }
                match self
                    .0
                    .runtime()
                    .submit(&args.session_id, args.content)
                    .await
                {
                    Ok(handle) => ToolOutcome::ok(
                        json!({"sent":true,"mode":"send","run_id":handle.run_id}).to_string(),
                    ),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            "steer" => {
                if !running {
                    return ToolOutcome::err(
                        "No run in flight in that session; use mode=send to start one.",
                    );
                }
                match self
                    .0
                    .runtime()
                    .steer(&args.session_id, kc_runtime::QueuedInput::text(args.content))
                {
                    Ok(steer) => ToolOutcome::ok(
                        json!({"sent":true,"mode":"steer","steer_id":steer.id,"queued":true,"note":"Queued at the next iteration; queued is not the same as read."}).to_string(),
                    ),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
            _ => {
                if !running {
                    return ToolOutcome::err(
                        "No run in flight in that session; use mode=send to start one.",
                    );
                }
                match self.0.runtime().enqueue_follow_up(
                    &args.session_id,
                    kc_runtime::QueuedInput::text(args.content),
                ) {
                    Ok(follow) => ToolOutcome::ok(
                        json!({"sent":true,"mode":"follow_up","follow_up_id":follow.id,"queued":true}).to_string(),
                    ),
                    Err(e) => ToolOutcome::err(e.to_string()),
                }
            }
        }
    }
}

fn snippet(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(160).collect()
}

// ---------------------------------------------------------------- cancel

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelArgs {
    session_id: String,
}

pub struct CancelRun(pub AppState);

#[async_trait]
impl Tool for CancelRun {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_cancel_run".into(),
            description: "Stop the run in flight in one session, as its user would. Mutating, so it follows the approval policy; reports when there was nothing to stop.".into(),
            parameters: json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: CancelArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Err(e) = check_text(&args.session_id, "session_id", 128) {
            return ToolOutcome::err(e);
        }
        if !matches!(
            self.0.store().get_session(&args.session_id).await,
            Ok(Some(_))
        ) {
            return ToolOutcome::err("no such session");
        }
        if let Some(err) = gate(
            &ctx,
            format!("Stop the run in session {}", args.session_id),
            "The session's current run is cancelled at its next safe point.".to_string(),
            json!({"tool":"control_cancel_run","session_id":args.session_id}),
        )
        .await
        {
            return err;
        }
        let cancelled = self.0.runtime().cancel(&args.session_id);
        ToolOutcome::ok(
            json!({"cancelled":cancelled,"session_id":args.session_id,
                   "note": if cancelled {"cancellation requested"} else {"no run in flight"}})
            .to_string(),
        )
    }
}

// ---------------------------------------------------------------- rename

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameArgs {
    session_id: String,
    title: String,
}

pub struct RenameSession(pub AppState);

#[async_trait]
impl Tool for RenameSession {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_rename_session".into(),
            description: "Rename one session, as its user would. Mutating, so it follows the approval policy.".into(),
            parameters: json!({"type":"object","properties":{"session_id":{"type":"string"},"title":{"type":"string"}},"required":["session_id","title"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: RenameArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Err(e) = check_text(&args.session_id, "session_id", 128) {
            return ToolOutcome::err(e);
        }
        if let Err(e) = check_text(&args.title, "title", 200) {
            return ToolOutcome::err(e);
        }
        if !matches!(
            self.0.store().get_session(&args.session_id).await,
            Ok(Some(_))
        ) {
            return ToolOutcome::err("no such session");
        }
        if let Some(err) = gate(
            &ctx,
            format!("Rename session {}", args.session_id),
            format!("New title: {}", args.title),
            json!({"tool":"control_rename_session","session_id":args.session_id,"title":args.title}),
        )
        .await
        {
            return err;
        }
        match self
            .0
            .store()
            .rename_session(&args.session_id, args.title.trim())
            .await
        {
            Ok(()) => ToolOutcome::ok(
                json!({"renamed":true,"session_id":args.session_id,"title":args.title.trim()})
                    .to_string(),
            ),
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------- agents

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAgentsArgs {
    /// One session's subagents. This is the only way to see the agents of a
    /// control chat, which belongs to no project.
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

pub struct ListAgents(pub AppState);

/// One row of `control_list_agents`.
///
/// The whole `AgentRecord` except its task: a supervisor needs to know *what* an
/// agent is doing (`note`, `summary`, `progress`) and whether it is still paying
/// (`status`), not to re-read the instructions it was given.
fn agent_row(summary: &AgentSummary) -> serde_json::Value {
    let agent = &summary.agent;
    json!({
        "id": agent.id,
        "session_id": agent.session_id,
        "session_title": summary.session_title,
        "project_id": summary.project_id,
        "name": agent.name,
        "role": agent.role,
        "status": agent.status,
        "model": agent.model,
        "iterations": agent.iterations,
        "note": agent.note,
        "progress": agent.progress,
        "created_at": agent.created_at,
        "updated_at": agent.updated_at,
        "finished_at": agent.finished_at,
        "background": agent.background,
        "summary": agent.summary,
        "error": agent.error,
    })
}

#[async_trait]
impl Tool for ListAgents {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_list_agents".into(),
            description: "List subagents across every project and every control chat: status, model, iterations, newest note, progress and their session's title. Pass `session_id` to see one session's subagents (the only way to see a control chat's own agents); omit it to see them all, optionally narrowed by `project_id` or `status`. Read-only; this is how you audit delegated work.".into(),
            parameters: json!({"type":"object","properties":{
                "session_id":{"type":"string","description":"Only subagents of this session, including a control chat"},
                "status":{"type":"string","enum":["running","paused","completed","failed","cancelled","interrupted"]},
                "project_id":{"type":"string","description":"Only subagents working in this registered project"},
                "limit":{"type":"integer","minimum":1,"maximum":100,"description":"How many rows (default 40); `total` counts all of them"}
            },"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: ListAgentsArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Some(session_id) = &args.session_id {
            if let Err(e) = check_text(session_id, "session_id", 128) {
                return ToolOutcome::err(e);
            }
        }
        // Validated rather than passed through: an unknown status would filter
        // every row away and read as "no agents", which is a lie.
        let status = match args
            .status
            .as_deref()
            .map(str::trim)
            .filter(|status| !status.is_empty())
        {
            Some(raw) => match AgentStatus::parse(raw) {
                Some(status) => Some(status.as_str().to_string()),
                None => {
                    return ToolOutcome::err(format!(
                        "status must be one of running, paused, completed, failed, cancelled, interrupted (got `{raw}`)"
                    ))
                }
            },
            None => None,
        };
        let limit = args.limit.unwrap_or(40).clamp(1, 100) as usize;
        let store = self.0.store();
        let mut agents = if let Some(session_id) = args.session_id.as_deref() {
            match store.list_agent_summaries_for_session(session_id).await {
                Ok(agents) => agents,
                Err(e) => return ToolOutcome::err(e.to_string()),
            }
        } else {
            let statuses: Vec<String> = status.clone().into_iter().collect();
            match store
                .list_agents_filtered(args.project_id.as_deref(), None, &statuses)
                .await
            {
                Ok(agents) => agents,
                Err(e) => return ToolOutcome::err(e.to_string()),
            }
        };
        // The per-session listing takes no filter, so both predicates are applied
        // here; for the global listing they are already applied in SQL.
        if args.session_id.is_some() {
            agents.retain(|summary| {
                status
                    .as_deref()
                    .is_none_or(|want| summary.agent.status == want)
                    && args
                        .project_id
                        .as_deref()
                        .is_none_or(|id| summary.project_id.as_deref() == Some(id))
            });
        }
        let total = agents.len();
        let rows: Vec<_> = agents.iter().take(limit).map(agent_row).collect();
        ToolOutcome::ok(json!({"total": total, "agents": rows}).to_string())
    }
}

// ---------------------------------------------------------------- model pin

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetModelArgs {
    session_id: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
}

pub struct SetSessionModel(pub AppState);

#[async_trait]
impl Tool for SetSessionModel {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_set_session_model".into(),
            description: "Pin one session - a project session or a control chat - to a provider and model, or clear the pin so it follows the install-wide default again. Give `provider` and `model` (optionally `reasoning_effort`); omit `provider` and `model` to clear. An unknown provider or a model the provider does not offer is refused. Mutating, so it follows the approval policy; it takes effect on that session's next run.".into(),
            parameters: json!({"type":"object","properties":{
                "session_id":{"type":"string"},
                "provider":{"type":"string","description":"Provider profile id from the install's configured profiles"},
                "model":{"type":"string","description":"Model id the provider offers"},
                "reasoning_effort":{"type":"string","enum":["none","minimal","low","medium","high","max"]}
            },"required":["session_id"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: SetModelArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Err(e) = check_text(&args.session_id, "session_id", 128) {
            return ToolOutcome::err(e);
        }
        // No provider and no model is the documented way to clear the pin. An
        // effort on its own would be silently dropped by a clear, so it is
        // refused instead of reported as done.
        let selection = if args.provider.is_none() && args.model.is_none() {
            if args.reasoning_effort.is_some() {
                return ToolOutcome::err(
                    "reasoning_effort only applies to a pinned model: give `provider` and `model` as well, or omit all three to clear the pin",
                );
            }
            None
        } else {
            let selection = SessionModelSelection {
                provider: args.provider.clone().unwrap_or_default(),
                model: args.model.clone().unwrap_or_default(),
                reasoning_effort: args.reasoning_effort.clone(),
            }
            .normalized();
            if let Err(e) =
                crate::settings::validate_session_model(&self.0.settings().snapshot(), &selection)
            {
                return ToolOutcome::err(format!("invalid model selection: {e}"));
            }
            Some(selection)
        };
        if !matches!(
            self.0.store().get_session(&args.session_id).await,
            Ok(Some(_))
        ) {
            return ToolOutcome::err("no such session");
        }
        let title = match &selection {
            Some(selection) => format!(
                "Pin session {} to {}/{}",
                args.session_id, selection.provider, selection.model
            ),
            None => format!("Clear the model pin of session {}", args.session_id),
        };
        let detail = match &selection {
            Some(selection) => format!(
                "The session's next run will use {}/{}{}.",
                selection.provider,
                selection.model,
                selection
                    .reasoning_effort
                    .as_deref()
                    .map(|effort| format!(" with reasoning effort {effort}"))
                    .unwrap_or_default()
            ),
            None => {
                "The session's next run will follow the install-wide default model.".to_string()
            }
        };
        if let Some(err) = gate(
            &ctx,
            title,
            detail,
            json!({"tool":"control_set_session_model","session_id":args.session_id,"model":selection}),
        )
        .await
        {
            return err;
        }
        if let Err(e) = self
            .0
            .store()
            .set_session_model(&args.session_id, selection.as_ref())
            .await
        {
            return ToolOutcome::err(e.to_string());
        }
        self.0.runtime().emit(
            Some(&args.session_id),
            None,
            None,
            Event::SessionModelChanged {
                session_id: args.session_id.clone(),
                model: selection.clone(),
            },
        );
        // The stored row is the answer: what the next run resolves from it is the
        // label, and a label never carries a key or a base URL.
        let session = match self.0.store().get_session(&args.session_id).await {
            Ok(Some(session)) => session,
            Ok(None) => return ToolOutcome::err("no such session"),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        ToolOutcome::ok(
            json!({
                "session_id": session.id,
                "model": session.model,
                "cleared": selection.is_none(),
                "model_label": crate::control::model_label(&self.0, session.model.as_ref()),
            })
            .to_string(),
        )
    }
}

// ---------------------------------------------------------------- control chats

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChatArgs {
    #[serde(default)]
    title: Option<String>,
    /// The chat's opening request. When present without a `title` the chat is
    /// named from it, exactly as `POST /api/control/sessions` does.
    #[serde(default)]
    prompt: Option<String>,
}

pub struct CreateControlChat(pub AppState);

#[async_trait]
impl Tool for CreateControlChat {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_create_control_chat".into(),
            description: "Open another control chat, alongside this one: its own server-owned workspace and its own conversation, with no project. `title` names it, `prompt` is its opening request and names an untitled chat. The new chat does not start running; the user talks to it from the rail. Mutating, so it follows the approval policy.".into(),
            parameters: json!({"type":"object","properties":{
                "title":{"type":"string","description":"Optional name; a named chat is never renamed"},
                "prompt":{"type":"string","description":"Optional opening request; names the chat when no title is given"}
            },"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: CreateChatArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        // Validate before asking: a person should never be shown a prompt to
        // approve that the server is about to reject anyway (a 5 MB body in an
        // approval dialog is not a decision, it is a bug).
        if let Some(title) = args.title.as_deref() {
            if title.trim().is_empty() || title.len() > 200 || title.trim() != title {
                return ToolOutcome::err("title must be nonempty, trimmed and at most 200 bytes");
            }
        }
        if let Some(prompt) = args.prompt.as_deref() {
            if prompt.trim().is_empty()
                || prompt.len() > crate::control::MAX_LAUNCH_PROMPT_BYTES
                || prompt.trim() != prompt
            {
                return ToolOutcome::err(format!(
                    "prompt must be nonempty, trimmed and at most {} bytes",
                    crate::control::MAX_LAUNCH_PROMPT_BYTES
                ));
            }
        }
        if let Some(err) = gate(
            &ctx,
            format!(
                "Open a control chat: {}",
                args.title.as_deref().unwrap_or("untitled")
            ),
            match args.prompt.as_deref() {
                Some(prompt) => format!("Opening request: {}", snippet(prompt)),
                None => "The chat starts empty; its first message names it.".to_string(),
            },
            json!({"tool":"control_create_control_chat","title":args.title,"prompt":args.prompt}),
        )
        .await
        {
            return err;
        }
        match crate::control::new_control_chat(
            &self.0,
            args.title.as_deref(),
            args.prompt.as_deref(),
        )
        .await
        {
            Ok(session) => ToolOutcome::ok(
                json!({
                    "session_id": session.id,
                    "title": session.title,
                    "workspace": session.workspace,
                })
                .to_string(),
            ),
            Err(e) => ToolOutcome::err(e.message),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteChatArgs {
    session_id: String,
}

pub struct DeleteControlChat(pub AppState);

#[async_trait]
impl Tool for DeleteControlChat {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_delete_control_chat".into(),
            description: "Delete one control chat: its transcript, runs and agent rows are gone for good. Only a control chat may be deleted this way - deleting a project session is the user's decision, so ask them and let them do it. The chat's scratch directory stays on disk. An in-flight run is cancelled first. Mutating, so it follows the approval policy. Deleting the chat you are running in is refused.".into(),
            parameters: json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"],"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if let Some(err) = control_only(&self.0, &ctx).await {
            return err;
        }
        let args: DeleteChatArgs = match serde_json::from_value(request.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutcome::err(format!("invalid arguments: {e}")),
        };
        if let Err(e) = check_text(&args.session_id, "session_id", 128) {
            return ToolOutcome::err(e);
        }
        if args.session_id == ctx.approvals.session_id.as_str() {
            return ToolOutcome::err(
                "Refusing to delete the control chat you are running in; ask the user to delete it from the rail.",
            );
        }
        let session = match self.0.store().get_session(&args.session_id).await {
            Ok(Some(session)) => session,
            Ok(None) => return ToolOutcome::err("no such session"),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        // A project session is the user's call, not the control chat's: its work,
        // its project and its workspace belong to them.
        if !matches!(
            self.0.store().is_control_session(&args.session_id).await,
            Ok(true)
        ) {
            return ToolOutcome::err(format!(
                "Only a control chat can be deleted here. Session `{}` belongs to project {}; ask the user to delete it from the session list.",
                session.id,
                session.project_id.as_deref().unwrap_or("(none)")
            ));
        }
        if let Some(err) = gate(
            &ctx,
            format!("Delete control chat {}", session.id),
            format!(
                "Title: {}. Its transcript, runs and agent rows are removed for good; its directory stays on disk.",
                if session.title.trim().is_empty() {
                    "(untitled)"
                } else {
                    session.title.trim()
                }
            ),
            json!({"tool":"control_delete_control_chat","session_id":session.id,"title":session.title,"workspace":session.workspace}),
        )
        .await
        {
            return err;
        }
        // Same order as `DELETE /api/sessions/:id`: stop the run loop before the
        // rows go, so it cannot write to a session that no longer exists.
        self.0.runtime().cancel(&session.id);
        if let Err(e) = self.0.store().delete_session(&session.id).await {
            return ToolOutcome::err(e.to_string());
        }
        self.0.runtime().clear_session_allows(&session.id);
        self.0.runtime().emit(
            Some(&session.id),
            None,
            None,
            Event::SessionDeleted {
                session_id: session.id.clone(),
            },
        );
        ToolOutcome::ok(
            json!({
                "deleted": true,
                "session_id": session.id,
                "workspace_kept": session.workspace,
                "note": "The transcript is gone; the chat's scratch directory was left on disk.",
            })
            .to_string(),
        )
    }
}

pub fn tools(state: AppState) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ListSessions(state.clone())),
        Arc::new(ReadSession(state.clone())),
        Arc::new(ListAgents(state.clone())),
        Arc::new(SetSessionModel(state.clone())),
        Arc::new(CreateControlChat(state.clone())),
        Arc::new(DeleteControlChat(state.clone())),
        Arc::new(SendMessage(state.clone())),
        Arc::new(CancelRun(state.clone())),
        Arc::new(RenameSession(state)),
    ]
}
