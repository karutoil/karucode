//! Control-chat tools over the whole server: audit, guide and start sessions.
//!
//! Every tool here is restricted to the single control session and every mutating
//! one passes through the ordinary approval policy (or is refused under
//! `read_only`). They act through the same store and runtime a user acts through;
//! none of them reads or writes project files directly.

use async_trait::async_trait;
use kc_core::{
    approval::ApprovalKind,
    store::StoredMessage,
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

pub fn tools(state: AppState) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ListSessions(state.clone())),
        Arc::new(ReadSession(state.clone())),
        Arc::new(SendMessage(state.clone())),
        Arc::new(CancelRun(state.clone())),
        Arc::new(RenameSession(state)),
    ]
}
