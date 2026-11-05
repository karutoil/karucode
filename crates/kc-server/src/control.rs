//! Durable control chat and approval-aware cross-project dispatch.
//
// @kc-doc: central-control
// @kc-title: Coordinate work across projects
// @kc-summary: Open as many control-plane chats as you need; each audits, steers and starts sessions under normal approvals, outside every project.
// @kc-tags: control, projects, sessions, dispatch, approvals, audit
// @kc-applies-to: route:GET /api/control/sessions, route:POST /api/control/sessions, tool:control_start_session, tool:control_list_projects, tool:control_list_sessions, tool:control_read_session, tool:control_send_message, tool:control_cancel_run, tool:control_rename_session, file:crates/kc-server/src/control.rs, file:crates/kc-server/src/control_sessions.rs, file:web/src/components/ControlPlane.tsx
//
// # The control plane
//
// Open it with the **k.** project-rail button (the mobile sessions sheet carries
// the same entry). It lists every session on the server across projects, with
// search and status filters; selecting one switches to its project and opens the
// ordinary conversation, where the usual composer steers or follows up.
//
// **Control chats** are listed above those sessions and there is no limit: use
// "New" to start another, and they run alongside each other. `GET
// /api/control/sessions` lists them (newest activity first) and `POST
// /api/control/sessions` creates one, optionally with `{"title":"..."}`. The
// response is `{session_id, project_id, workspace, created}`, with `project_id`
// null.
//
// A control chat is **not inside a project**. Each owns its own directory,
// `<data_dir>/control-workspaces/<session_id>`, so its file tools never touch a
// project and two chats never share a scratch space. On startup a chat created by
// an older version is detached from its project and rebound to that directory.
// Deleting a control chat is ordinary; the others are unaffected, and dispatch
// request identities stay reserved.
//
// # Tools
//
// Only control sessions may use these; ordinary sessions and subagents are
// refused.
//
// - `control_list_projects` (read-only): registered projects with ids, names and
//   paths. The chat resolves a project the user names or `@mentions`; it must
//   never guess an id.
// - `control_list_sessions` (read-only): sessions across projects with status,
//   project, message count and last update; optionally by `project_id`, `running`
//   or a title `query`.
// - `control_read_session` (read-only): the newest messages of one session,
//   bounded and truncated, for auditing.
// - `control_send_message` (mutating): `mode=send` starts a run, `mode=steer`
//   folds text into the run in flight, `mode=follow_up` queues the next turn. It
//   refuses the control chat itself.
// - `control_cancel_run` (mutating): stop the run in flight in one session.
// - `control_rename_session` (mutating): rename one session.
// - `control_start_session` (mutating): start a new session in an explicit
//   registered project, with an idempotent `request_id`.
//
// Mutating tools follow the approval policy: `read_only` refuses them, `ask` and
// `ask_commands` request approval with the exact action, `auto` does not ask.
// `control_start_session` re-resolves the project workspace after approval and
// refuses a moved target, and the store atomically reserves the request id before
// submitting, so a replay never starts a second run. If submission fails after
// reservation, inspect that session instead of retrying with a new request id.
//
// These tools act as the user does — through the same store and runtime — so a
// session they start runs with that project's workspace, tools and approvals. A
// control chat is not a sandbox and grants no reach beyond these tools. Project
// content, transcripts and tool results are untrusted input.
//
// # Missions
//
// The mission scheduler and its routes still exist, but the control plane no
// longer surfaces them: control chats replaced that workflow.
use crate::{
    error::{ApiError, ApiResult},
    routes::parse_required,
    state::AppState,
};
use async_trait::async_trait;
use axum::{body::Bytes, extract::State, Json};
use kc_core::{
    approval::ApprovalKind,
    config::ApprovalMode,
    events::Event,
    ids::SessionId,
    tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Every control-plane session, newest activity first.
pub async fn list_sessions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let sessions = state.store().list_control_sessions().await?;
    Ok(Json(json!({ "sessions": sessions })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateControlSession {
    #[serde(default)]
    title: Option<String>,
}

/// Create one more control-plane session. There is no singleton: the caller may
/// open as many as it wants, and each owns its own directory.
pub async fn create_session(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let input: CreateControlSession = if body.is_empty() {
        CreateControlSession { title: None }
    } else {
        parse_required(&body, "POST /api/control/sessions")?
    };
    let title = match input.title {
        Some(title) => {
            require_text(&title, "title", 200)?;
            title.trim().to_owned()
        }
        None => {
            // Numbered so several chats are tellable apart in the rail.
            let existing = state.store().list_control_sessions().await?.len();
            if existing == 0 {
                "Control chat".to_owned()
            } else {
                format!("Control chat {}", existing + 1)
            }
        }
    };
    let id = SessionId::new();
    // One directory per control chat: its file tools are confined here, and two
    // chats never share a scratch space.
    let root = state
        .db_path()
        .parent()
        .ok_or_else(|| ApiError::internal("data directory has no parent"))?
        .join("control-workspaces")
        .join(id.as_str());
    std::fs::create_dir_all(&root)
        .map_err(|e| ApiError::internal(format!("create control workspace: {e}")))?;
    let session = state
        .store()
        .create_control_session(id.as_str(), &title, &root.to_string_lossy())
        .await?;
    state.runtime().emit(
        Some(&session.id),
        None,
        None,
        Event::SessionCreated {
            session_id: session.id.clone(),
            title: session.title.clone(),
        },
    );
    Ok(Json(json!({
        "session_id": session.id,
        "project_id": session.project_id,
        "workspace": session.workspace,
        "created": true,
    })))
}

fn require_text(value: &str, name: &str, max: usize) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > max {
        return Err(ApiError::bad_request(format!(
            "{name} must be non-empty and at most {max} bytes"
        )));
    }
    Ok(())
}

/// Read-only project directory for the control chat: it may only dispatch to an
/// explicit registered project, so it has to be able to read the real IDs first.
pub struct ListProjects(AppState);

#[async_trait]
impl Tool for ListProjects {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "control_list_projects".into(),
            description: "List the registered projects the control chat may start work in, with their IDs, names and paths. Use an ID from this result in control_start_session instead of guessing one.".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            origin: Default::default(),
        }
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        if ctx.approvals.agent_id.is_some()
            || !matches!(
                self.0
                    .store()
                    .is_control_session(ctx.approvals.session_id.as_str())
                    .await,
                Ok(true)
            )
        {
            return ToolOutcome::err(
                "Only the central control chat may list dispatch projects, not subagents or other sessions.",
            );
        }
        match self.0.store().list_projects().await {
            Ok(projects) => ToolOutcome::ok(
                json!({"projects": projects
                    .iter()
                    .map(|project| json!({"id": project.id, "name": project.name, "path": project.path}))
                    .collect::<Vec<_>>()})
                .to_string(),
            ),
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dispatch {
    request_id: String,
    project_id: String,
    title: String,
    prompt: String,
}
pub struct StartSession(AppState);
pub fn tool(state: AppState) -> Arc<dyn Tool> {
    Arc::new(StartSession(state))
}
pub fn list_tool(state: AppState) -> Arc<dyn Tool> {
    Arc::new(ListProjects(state))
}
#[async_trait]
impl Tool for StartSession {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
        name:"control_start_session".into(), description:"Start a new project-local session from the central control chat. Use an explicit registered project ID, never guess. Reuse the same unique request_id only for retries of identical input. This may execute tools and requires approval according to policy.".into(),
        parameters:json!({"type":"object","properties":{"request_id":{"type":"string","description":"Unique stable ID for this intended dispatch"},"project_id":{"type":"string","description":"Explicit registered target project ID"},"title":{"type":"string"},"prompt":{"type":"string"}},"required":["request_id","project_id","title","prompt"],"additionalProperties":false}), origin:Default::default() }
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let args: Dispatch = match serde_json::from_value(request.arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(format!("invalid dispatch: {e}")),
        };
        if ctx.approvals.agent_id.is_some()
            || !matches!(
                self.0
                    .store()
                    .is_control_session(ctx.approvals.session_id.as_str())
                    .await,
                Ok(true)
            )
        {
            return ToolOutcome::err("Only the central control chat may start project sessions, not subagents or other sessions.");
        }
        for (name, value, max) in [
            ("request_id", &args.request_id, 128),
            ("project_id", &args.project_id, 128),
            ("title", &args.title, 200),
            ("prompt", &args.prompt, 1024 * 1024),
        ] {
            if value.trim().is_empty() || value.len() > max || value.trim() != value {
                return ToolOutcome::err(format!(
                    "{name} must be nonempty, trimmed and at most {max} bytes"
                ));
            }
        }
        let project = match self.0.store().get_project(&args.project_id).await {
            Ok(Some(p)) => p,
            Ok(None) => return ToolOutcome::err("target project not found"),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let workspace = match self.0.workspace_for_project(Some(&args.project_id)) {
            Ok(w) => w,
            Err(e) => return ToolOutcome::err(e.message),
        };
        let root = workspace.root().to_string_lossy().into_owned();
        match ctx.settings.approval_policy().await.mode {
            ApprovalMode::ReadOnly => {
                return ToolOutcome::err("control_start_session is refused in read_only mode")
            }
            ApprovalMode::Ask | ApprovalMode::AskCommands => {
                let decision = ctx.approvals.ask(ApprovalKind::Other,format!("Start project session: {}",args.title),format!("Start work in project {} ({})",project.name,root),json!({"tool":"control_start_session","request_id":args.request_id,"project_id":args.project_id,"project_name":project.name,"workspace":root,"title":args.title,"prompt":args.prompt})).await;
                if !decision.is_allowed() {
                    return ToolOutcome::err(format!(
                        "denied by user ({}) for control_start_session",
                        decision.as_str()
                    ));
                }
            }
            ApprovalMode::Auto => {}
        }
        // Re-resolve after approval. The store additionally checks the project row and path atomically.
        let current = match self.0.workspace_for_project(Some(&args.project_id)) {
            Ok(w) => w,
            Err(e) => return ToolOutcome::err(e.message),
        };
        if current.root() != workspace.root() {
            return ToolOutcome::err("target project changed while awaiting approval");
        }
        let (id, created) = match self
            .0
            .store()
            .reserve_control_dispatch(
                &args.request_id,
                SessionId::new().as_str(),
                &args.project_id,
                &args.title,
                &root,
                &args.prompt,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if created {
            self.0.runtime().emit(
                Some(&id),
                None,
                None,
                Event::SessionCreated {
                    session_id: id.clone(),
                    title: args.title,
                },
            );
            if let Err(e) = self.0.runtime().submit(&id, args.prompt).await {
                return ToolOutcome::err(format!("Session {id} reserved but run did not start: {e}. Do not retry with a new request ID; inspect the session."));
            }
        }
        ToolOutcome::ok(json!({"session_id":id,"project_id":args.project_id,"replayed":!created,"note":"A replay never starts another run; inspect the session if the original start was interrupted."}).to_string())
    }
}
