//! Durable control chat and approval-aware cross-project dispatch.
//
// @kc-doc: central-control
// @kc-title: Coordinate work across projects
// @kc-summary: Open as many control-plane chats as you need; each audits, steers and starts sessions under normal approvals, outside every project.
// @kc-tags: control, projects, sessions, dispatch, approvals, audit
// @kc-applies-to: route:GET /api/control/sessions, route:POST /api/control/sessions, route:GET /api/control/overview, tool:control_start_session, tool:control_list_projects, tool:control_list_sessions, tool:control_list_agents, tool:control_read_session, tool:control_send_message, tool:control_cancel_run, tool:control_rename_session, tool:control_set_session_model, tool:control_create_control_chat, tool:control_delete_control_chat, file:crates/kc-server/src/control.rs, file:crates/kc-server/src/control_sessions.rs, file:web/src/components/ControlPlane.tsx
//
// # The control plane
//
// Open it with the **k.** project-rail button (the mobile sessions sheet carries
// the same entry). It lists every session on the server across projects, with
// search and status filters; selecting one switches to its project and opens the
// ordinary conversation, where the usual composer steers or follows up.
//
// **Control chats** are listed above those sessions and there is no limit: use
// "New" to start another, and they run alongside each other.
//
// `POST /api/control/sessions` creates one and answers
// `{session_id, project_id, workspace, title, created}`, with `project_id` null.
// The body is optional:
//
// ```bash
// curl -X POST localhost:8787/api/control/sessions \
//     -H 'content-type: application/json' -H 'x-karucode-client: web' \
//     -d '{"prompt":"audit the open pull requests"}'
// ```
//
// - `title` names the chat. A chat you name is never renamed.
// - `prompt` (at most 32 KiB) is the chat's opening request, and **only** names
//   it: it is not stored as a message and does not start a run. Without a `title`
//   a chat created with a prompt is named from it - deterministically at once, and
//   by a model as well when `[titles]` is on (the default; see `session-titles`).
//   The response returns before that model call finishes.
// - With neither, the chat is created **untitled** and its first message names it
//   (`POST /api/sessions/:id/messages`), so the rail rarely shows an empty row.
// - A `title` you pass always wins: the prompt does not rename a named chat, even
//   when the name happens to be one the framework also uses as a placeholder.
//
// `GET /api/control/sessions` lists them, newest activity first. Each row is the
// session summary - `id`, `title`, `workspace`, `project_id` (null), `model`,
// `created_at`, `updated_at`, `message_count`, `last_run_status`, `running` and
// `control: true` - plus `agents: {live, total}` (live is running *or* paused),
// `pending_approvals` (unanswered approval questions on that chat's run) and
// `model_label`, the model the chat would use right now: its own pin, else the
// install-wide default. `model_label` is a model id, never a key or a base URL.
//
// # The overview
//
// `GET /api/control/overview` is the whole plane in one read-only call, for a
// dashboard that would otherwise make five requests:
//
// ```json
// { "generated_at": 1730000000000,
//   "sessions": { "total": 4, "running": 1, "failed": 0, "control": 2 },
//   "agents": { "live": 1, "paused": 0, "failed": 0, "total": 3 },
//   "approvals": { "pending": 1 },
//   "projects": [ { "id": "p_...", "name": "karucode", "path": "/home/me/karucode",
//                   "sessions": 2, "running": 1 } ],
//   "control": [ /* the rows of GET /api/control/sessions */ ],
//   "running": [ /* SessionSummary, at most 12 */ ],
//   "attention": [ /* SessionSummary, at most 12 */ ],
//   "waiting": [ /* SessionSummary, at most 12 */ ],
//   "recent": [ /* SessionSummary, not running, at most 12 */ ],
//   "live_agents": [ /* AgentSummary with session_title, workspace, project_id */ ] }
// ```
//
// - Control chats appear only in `control`; the other lists are project sessions.
// - `sessions.total` counts **project sessions only** (control chats are counted
//   in `sessions.control`), so the two are separate totals rather than parts of
//   one. `sessions.running` is likewise project sessions.
// - `attention` is a session whose **latest run** ended `failed`, `interrupted`
//   or `limit_reached`. `cancelled` is something you asked for and `completed`
//   needs nothing, so neither is listed. `sessions.failed` counts every such
//   session, not the first twelve.
// - `waiting` is every project session with an **unanswered approval**, and
//   `approvals.pending` counts the questions. An approval means the run is
//   stopped until a person answers, so these are the most actionable rows on the
//   plane; each control chat carries its own `pending_approvals` count.
// - `live_agents` is running or paused subagents, most recently active first, at
//   most 12, and each row carries the session it belongs to so a click can open it.
//
// A control chat is **not inside a project**. Each owns its own directory,
// `<data_dir>/control-workspaces/<session_id>`, so its file tools never touch a
// project and two chats never share a scratch space. On startup a chat created by
// an older version is detached from its project and rebound to that directory.
// Deleting a control chat is ordinary; the others are unaffected, and dispatch
// request identities stay reserved. Deleting it removes its transcript, its runs
// and its agent rows and leaves its directory on disk: the store is not the
// owner of files a tool wrote into that scratch space.
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
// - `control_list_agents` (read-only): the subagents of any session - including a
//   control chat's own - with status, model, iterations, latest `note` and
//   `progress`. Pass `session_id` for one session, or narrow the whole server by
//   `project_id` and `status`. This is how the chat audits delegated work.
// - `control_read_session` (read-only): the newest messages of one session,
//   bounded and truncated, for auditing.
// - `control_send_message` (mutating): `mode=send` starts a run, `mode=steer`
//   folds text into the run in flight, `mode=follow_up` queues the next turn. It
//   refuses the control chat itself.
// - `control_cancel_run` (mutating): stop the run in flight in one session.
// - `control_rename_session` (mutating): rename one session.
// - `control_set_session_model` (mutating): pin one session to `provider` +
//   `model`, optionally with a `reasoning_effort`, or clear the pin by omitting
//   them so the session follows the install-wide default. An unknown provider or
//   a model the profile does not offer is refused; the pin takes effect on that
//   session's next run.
// - `control_create_control_chat` (mutating): open another control chat, with its
//   own workspace and its own conversation, exactly like `New` in the rail. It
//   does not run anything: the user talks to the new chat.
// - `control_delete_control_chat` (mutating): delete one **control chat**. A
//   project session is refused - deleting it is the user's decision - and the
//   chat's scratch directory is left on disk. Deleting the chat the tool is
//   running in is refused too.
// - `control_start_session` (mutating): start a new session in an explicit
//   registered project, with an idempotent `request_id`. `title` is optional:
//   omit it and the new session is named from its `prompt` (and by a model when
//   `[titles]` is on), exactly like a chat opened with a prompt. Pass a title only
//   when the user named the work, because a named session is never renamed.
//   `provider` + `model` (and optionally `reasoning_effort`) pin the new session
//   to that model, so "start this in project X on model Y" is one call.
//
// A replay of the same `request_id` never retitles the session, never starts a
// second run and never re-pins its model: only the call that created it names it
// and sets its model, so a choice the user has changed since is left alone.
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
    config::{ApprovalMode, SessionModelSelection},
    events::Event,
    ids::SessionId,
    store::{AgentStatus, AgentSummary, Session, SessionSummary},
    tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Every control-plane session, newest activity first, each with the counts and
/// the model label the rail draws next to it.
pub async fn list_sessions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let sessions = state.store().list_control_sessions().await?;
    let (pending, _) = pending_counts(&state).await?;
    let counts = agent_counts(&state).await?;
    let chats = control_chat_summaries(&state, sessions, &pending, &counts).await?;
    Ok(Json(json!({ "sessions": chats })))
}

/// Create one control-plane session. There is no singleton: the caller may open
/// as many as it wants, and each owns its own directory.
///
/// Without a `title` the chat carries the empty title, which is what "untitled"
/// means, and the first message names it (`POST /api/sessions/:id/messages`).
/// With a `prompt` the chat is named from it immediately, deterministically, and
/// - when `[titles]` is on - by a model as well.
pub async fn create_session(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let input: CreateControlSession = if body.is_empty() {
        CreateControlSession {
            title: None,
            prompt: None,
        }
    } else {
        parse_required(&body, "POST /api/control/sessions")?
    };
    let session = new_control_chat(&state, input.title.as_deref(), input.prompt.as_deref()).await?;
    Ok(Json(json!({
        "session_id": session.id,
        "project_id": session.project_id,
        "workspace": session.workspace,
        "title": session.title,
        "created": true,
    })))
}

/// Create one control chat: the row, its own directory, the `SessionCreated`
/// event, and - when a prompt is given and no title is - its name.
///
/// One helper for both callers, so `POST /api/control/sessions` and the
/// `control_create_control_chat` tool can never drift on the id shape, the
/// workspace rule or the naming rule. Both inputs are optional; a chat created
/// with neither is created *untitled* and its first message names it.
pub(crate) async fn new_control_chat(
    state: &AppState,
    title: Option<&str>,
    prompt: Option<&str>,
) -> ApiResult<Session> {
    let prompt = match prompt {
        Some(prompt) => {
            let prompt = prompt.trim();
            if prompt.is_empty() || prompt.len() > MAX_LAUNCH_PROMPT_BYTES {
                return Err(ApiError::bad_request(format!(
                    "prompt must be non-empty and at most {MAX_LAUNCH_PROMPT_BYTES} bytes"
                )));
            }
            Some(prompt.to_owned())
        }
        None => None,
    };
    let title = match title {
        Some(title) => {
            require_text(title, "title", 200)?;
            Some(title.trim().to_owned())
        }
        // Untitled by design: the first message (or the prompt below) names it,
        // and a numbered default would make every chat look deliberately named.
        None => None,
    };
    let id = SessionId::new();
    let root = workspace_for_control_chat(state, id.as_str())?;
    let session = state
        .store()
        .create_control_session(
            id.as_str(),
            title.as_deref().unwrap_or(""),
            &root.to_string_lossy(),
        )
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
    // Only an *unnamed* chat is named from its prompt. A title the caller typed is
    // a decision: `is_untitled` also treats the framework's `New session`
    // placeholder as untitled, so naming after a title would overwrite a chat
    // somebody named exactly that - which the docs promise never happens.
    if let (None, Some(prompt)) = (title.as_deref(), prompt.as_deref()) {
        crate::titles::name_untitled_session(state, &session.id, prompt).await;
    }
    Ok(state
        .store()
        .get_session(&session.id)
        .await?
        .unwrap_or(session))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateControlSession {
    #[serde(default)]
    title: Option<String>,
    /// The chat's opening request. When present and no `title` is given, the
    /// chat is named from it; without either, it stays untitled until its first
    /// message names it.
    #[serde(default)]
    prompt: Option<String>,
}

fn workspace_for_control_chat(state: &AppState, id: &str) -> ApiResult<std::path::PathBuf> {
    // One directory per control chat: its file tools are confined here, and two
    // chats never share a scratch space.
    let root = state
        .db_path()
        .parent()
        .ok_or_else(|| ApiError::internal("data directory has no parent"))?
        .join("control-workspaces")
        .join(id);
    std::fs::create_dir_all(&root)
        .map_err(|e| ApiError::internal(format!("create control workspace: {e}")))?;
    Ok(root)
}

/// How many rows each activity list of [`overview`] carries.
const OVERVIEW_LIMIT: usize = 12;
/// A launching prompt is a first message, not a transcript.
///
/// Public because `control_create_control_chat` validates its own arguments
/// against the same bound before asking for approval.
pub(crate) const MAX_LAUNCH_PROMPT_BYTES: usize = 32 * 1024;
/// Run outcomes that mean "a person should look at this".
///
/// The store's own vocabulary: `cancelled` is something a human asked for and
/// `completed` needs nothing, so neither belongs in an attention list.
const ATTENTION_STATUSES: [&str; 3] = ["failed", "interrupted", "limit_reached"];

/// One control chat as the rail needs it: the session row plus what the chat is
/// running and which model answers for it.
///
/// `control` is always `true` (see [`SessionSummary::control`]), so a client can
/// keep one row type for every session in the plane.
#[derive(serde::Serialize)]
struct ControlChatSummary {
    #[serde(flatten)]
    session: SessionSummary,
    /// Subagent counts. `live` is running *or* paused: a paused agent still
    /// holds its slot, so it is not finished work.
    agents: AgentCounts,
    /// Model questions parked on this chat's run. A chat with one is *stalled on
    /// a person*, which is the one thing a plane must never hide.
    pending_approvals: usize,
    /// The model this chat resolves to right now - the chat's own pin if it has
    /// one, otherwise the install-wide default. `None` only when no model is
    /// configured at all. Never carries a key or a base URL.
    model_label: Option<String>,
}

#[derive(serde::Serialize, Clone, Copy, Default)]
struct AgentCounts {
    live: usize,
    total: usize,
}

/// The whole control plane in one call: counts, projects, control chats and the
/// three session lists the overview panel draws, plus the live subagents.
///
/// Read-only and a fixed number of store calls - five reads plus one grouped
/// count, never one query per chat - so this stays cheap enough for the panel to
/// poll every few seconds.
#[derive(serde::Serialize)]
pub struct ControlOverview {
    /// When this snapshot was taken, in epoch milliseconds.
    generated_at: i64,
    sessions: SessionCounts,
    agents: AgentTotals,
    /// Approval questions parked on a run right now, across the whole server.
    approvals: ApprovalTotals,
    projects: Vec<ProjectRow>,
    control: Vec<ControlChatSummary>,
    running: Vec<SessionSummary>,
    attention: Vec<SessionSummary>,
    /// Project sessions stalled on an unanswered approval, newest activity first.
    waiting: Vec<SessionSummary>,
    recent: Vec<SessionSummary>,
    live_agents: Vec<AgentSummary>,
}

#[derive(serde::Serialize)]
struct ApprovalTotals {
    /// How many questions are parked. Each one is a run that cannot progress.
    pending: usize,
}

#[derive(serde::Serialize)]
struct SessionCounts {
    total: usize,
    running: usize,
    failed: usize,
    /// Control chats, so `total - control` is the project-session count.
    control: usize,
}

#[derive(serde::Serialize)]
struct AgentTotals {
    live: usize,
    paused: usize,
    failed: usize,
    total: usize,
}

#[derive(serde::Serialize)]
struct ProjectRow {
    id: String,
    name: String,
    path: String,
    sessions: usize,
    running: usize,
}

/// Resolve the display label of the model a selection would use.
///
/// Only the id and the human name: a label is what the rail shows, and a
/// provider credential must never reach a browser.
pub(crate) fn model_label(
    state: &AppState,
    selection: Option<&SessionModelSelection>,
) -> Option<String> {
    let model = crate::settings::resolve_session_model(&state.settings().snapshot(), selection);
    let id = model.model.trim();
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

fn is_live_agent(summary: &AgentSummary) -> bool {
    AgentStatus::parse(&summary.agent.status).is_some_and(|status| status.is_live())
}

fn is_paused_agent(summary: &AgentSummary) -> bool {
    summary.agent.status == AgentStatus::Paused.as_str()
}

fn is_failed_agent(summary: &AgentSummary) -> bool {
    summary.agent.status == AgentStatus::Failed.as_str()
}

fn needs_attention(session: &SessionSummary) -> bool {
    session
        .last_run_status
        .as_deref()
        .is_some_and(|status| ATTENTION_STATUSES.contains(&status))
}

async fn control_chat_summaries(
    state: &AppState,
    sessions: Vec<SessionSummary>,
    pending: &HashMap<String, usize>,
    agent_counts: &HashMap<String, (usize, usize)>,
) -> ApiResult<Vec<ControlChatSummary>> {
    let mut chats = Vec::with_capacity(sessions.len());
    for session in sessions {
        let (live, total) = agent_counts.get(&session.id).copied().unwrap_or((0, 0));
        chats.push(ControlChatSummary {
            model_label: model_label(state, session.model.as_ref()),
            pending_approvals: pending.get(&session.id).copied().unwrap_or(0),
            agents: AgentCounts { live, total },
            session,
        });
    }
    Ok(chats)
}

/// Group `(session_id, live, total)` rows the store returns into a lookup.
///
/// `None` is fine: a session with no subagents simply has no row, and every
/// caller treats a missing entry as `0 / 0`.
async fn agent_counts(state: &AppState) -> ApiResult<HashMap<String, (usize, usize)>> {
    Ok(state
        .store()
        .agent_counts_by_session()
        .await?
        .into_iter()
        .map(|(id, live, total)| (id, (live, total)))
        .collect())
}

/// Pending approval counts per session, and how many questions are parked.
///
/// One query for the whole server: an approval means a run is *stopped* until a
/// person answers, so both the per-chat badge and the overview's total come from
/// the same read and can never disagree.
async fn pending_counts(
    state: &AppState,
) -> ApiResult<(HashMap<String, usize>, usize)> {
    let pending = state.store().list_pending_approvals(None).await?;
    let total = pending.len();
    let mut by_session: HashMap<String, usize> = HashMap::new();
    for approval in &pending {
        *by_session.entry(approval.session_id.clone()).or_default() += 1;
    }
    Ok((by_session, total))
}

/// `GET /api/control/overview` - the control plane's one-shot snapshot.
pub async fn overview(State(state): State<AppState>) -> ApiResult<Json<ControlOverview>> {
    let store = state.store();
    let sessions = store.list_sessions().await?;
    let control_sessions = store.list_control_sessions().await?;
    let projects = store.list_projects().await?;
    let agents = store.list_agents_all(None).await?;
    let (pending_by_session, pending_total) = pending_counts(&state).await?;

    let mut running: Vec<SessionSummary> = Vec::new();
    let mut attention: Vec<SessionSummary> = Vec::new();
    let mut recent: Vec<SessionSummary> = Vec::new();
    let mut waiting: Vec<SessionSummary> = Vec::new();
    let mut project_sessions = 0usize;
    let mut project_running = 0usize;
    for session in sessions.iter().filter(|session| !session.control) {
        project_sessions += 1;
        if session.running {
            project_running += 1;
            running.push(session.clone());
        } else {
            recent.push(session.clone());
        }
        if needs_attention(session) {
            attention.push(session.clone());
        }
        if pending_by_session.contains_key(&session.id) {
            waiting.push(session.clone());
        }
    }
    // Counted before the lists are truncated: `sessions.failed` is a total, and a
    // truncated list must not turn 30 failing sessions into 12.
    let attention_total = attention.len();
    // Sorted by `updated_at` already; the calls above only drop rows.
    running.truncate(OVERVIEW_LIMIT);
    recent.truncate(OVERVIEW_LIMIT);
    attention.truncate(OVERVIEW_LIMIT);
    waiting.truncate(OVERVIEW_LIMIT);

    // Per-project counts, from the same one pass: a session whose project was
    // deleted simply has no row to land in.
    let mut per_project: HashMap<&str, (usize, usize)> = HashMap::new();
    for session in sessions.iter().filter(|session| !session.control) {
        if let Some(project_id) = session.project_id.as_deref() {
            let counts = per_project.entry(project_id).or_default();
            counts.0 += 1;
            if session.running {
                counts.1 += 1;
            }
        }
    }
    let rows: Vec<ProjectRow> = projects
        .iter()
        .map(|project| {
            let (sessions, running) = per_project
                .get(project.id.as_str())
                .copied()
                .unwrap_or_default();
            ProjectRow {
                id: project.id.clone(),
                name: project.name.clone(),
                path: project.path.clone(),
                sessions,
                running,
            }
        })
        .collect();

    let mut live_agents: Vec<AgentSummary> = agents
        .iter()
        .filter(|summary| is_live_agent(summary))
        .cloned()
        .collect();
    // `list_agents_all` orders by creation; a supervisor cares about activity.
    live_agents.sort_by(|a, b| {
        b.agent
            .updated_at
            .cmp(&a.agent.updated_at)
            .then_with(|| a.agent.id.cmp(&b.agent.id))
    });
    live_agents.truncate(OVERVIEW_LIMIT);

    let counts = agent_counts(&state).await?;
    let control =
        control_chat_summaries(&state, control_sessions, &pending_by_session, &counts).await?;

    Ok(Json(ControlOverview {
        generated_at: crate::state::now_ms(),
        sessions: SessionCounts {
            total: project_sessions,
            running: project_running,
            // The latest-run outcome is per session, not a stored counter: a
            // session that failed and was retried successfully is not failed.
            failed: attention_total,
            control: control.len(),
        },
        agents: AgentTotals {
            live: agents
                .iter()
                .filter(|summary| is_live_agent(summary))
                .count(),
            paused: agents
                .iter()
                .filter(|summary| is_paused_agent(summary))
                .count(),
            failed: agents
                .iter()
                .filter(|summary| is_failed_agent(summary))
                .count(),
            total: agents.len(),
        },
        approvals: ApprovalTotals {
            pending: pending_total,
        },
        projects: rows,
        control,
        running,
        attention,
        waiting,
        recent,
        live_agents,
    }))
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
    /// Optional. Without one the session is created untitled and named from
    /// `prompt`, exactly as a control chat opened with a prompt is.
    #[serde(default)]
    title: Option<String>,
    prompt: String,
    /// Optional model pin for the new session: `provider` + `model`, optionally
    /// with a `reasoning_effort`. All three absent leaves the child on the
    /// install-wide default.
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
}

impl Dispatch {
    /// The pin this dispatch asks for, or `None` for the install-wide default.
    ///
    /// A half-filled pair is left to validation: an empty provider or model is
    /// what [`SessionModelSelection::validate_shape`] rejects, so the caller gets
    /// the field-level message instead of a silently ignored model.
    fn model_pin(&self) -> Option<SessionModelSelection> {
        if self.provider.is_none() && self.model.is_none() && self.reasoning_effort.is_none() {
            return None;
        }
        Some(
            SessionModelSelection {
                provider: self.provider.clone().unwrap_or_default(),
                model: self.model.clone().unwrap_or_default(),
                reasoning_effort: self.reasoning_effort.clone(),
            }
            .normalized(),
        )
    }
}
pub struct StartSession(AppState);

/// Name a freshly dispatched session from its prompt, off the caller's path.
///
/// The derived title is stored before this returns to the tool, so one message
/// later the child session is already named; the model-written title follows on
/// a spawned task and never delays the dispatch.
fn name_from_prompt(state: AppState, session_id: String, prompt: String) {
    tokio::spawn(async move {
        let Some(derived) =
            crate::titles::derive_untitled_title(&state, &session_id, &prompt).await
        else {
            return;
        };
        if !state.settings().snapshot().titles.enabled {
            return;
        }
        // `maybe_auto_title` re-checks that the derived title is still the
        // session's, so a rename that landed in between wins.
        crate::titles::maybe_auto_title(&state, &session_id, &derived, &prompt).await;
    });
}
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
        name:"control_start_session".into(), description:"Start a new project-local session from the central control chat. Use an explicit registered project ID, never guess. Reuse the same unique request_id only for retries of identical input. Omit `title` to let the new session be named from its prompt (and by a model when titles are on); pass one only when the user named it. `provider` + `model` (and optionally `reasoning_effort`) pin the new session to that model, so \"start this in project X on model Y\" is one call; the pin is applied only when the call actually creates the session, never on a replay, so a model the user changed afterwards is not overwritten. This may execute tools and requires approval according to policy.".into(),
        parameters:json!({"type":"object","properties":{"request_id":{"type":"string","description":"Unique stable ID for this intended dispatch"},"project_id":{"type":"string","description":"Explicit registered target project ID"},"title":{"type":"string","description":"Optional session title. Omit to name the session from its prompt."},"prompt":{"type":"string","description":"The opening request the new session runs"},"provider":{"type":"string","description":"Optional provider profile id to pin the new session to; requires `model`"},"model":{"type":"string","description":"Optional model id to pin the new session to; requires `provider`"},"reasoning_effort":{"type":"string","enum":["none","minimal","low","medium","high","max"],"description":"Optional reasoning effort for the pinned model"}},"required":["request_id","project_id","prompt"],"additionalProperties":false}), origin:Default::default() }
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
            ("prompt", &args.prompt, 1024 * 1024),
        ] {
            if value.trim().is_empty() || value.len() > max || value.trim() != value {
                return ToolOutcome::err(format!(
                    "{name} must be nonempty, trimmed and at most {max} bytes"
                ));
            }
        }
        let title = match &args.title {
            Some(title) => {
                if title.trim().is_empty() || title.len() > 200 || title.trim() != title {
                    return ToolOutcome::err(
                        "title must be nonempty, trimmed and at most 200 bytes when given; omit it to name the session from its prompt",
                    );
                }
                Some(title.as_str())
            }
            None => None,
        };
        let approval_title = title.unwrap_or("New session");
        // Validate the pin before approval: a model the profiles do not offer is
        // refused here, not after the user has granted permission for it.
        let pin = args.model_pin();
        if let Some(pin) = &pin {
            if let Err(e) =
                crate::settings::validate_session_model(&self.0.settings().snapshot(), pin)
            {
                return ToolOutcome::err(format!("invalid model selection: {e}"));
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
                let decision = ctx.approvals.ask(ApprovalKind::Other,format!("Start project session: {approval_title}"),format!("Start work in project {} ({})",project.name,root),json!({"tool":"control_start_session","request_id":args.request_id,"project_id":args.project_id,"project_name":project.name,"workspace":root,"title":title,"prompt":args.prompt,"model":pin})).await;
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
        // No title means "name it from the prompt": the row starts untitled, so
        // `name_untitled_session` can improve it below. A replay never reaches
        // that call, so it can neither retitle nor restart the session.
        let reserved_title = title.unwrap_or("");
        let (id, created) = match self
            .0
            .store()
            .reserve_control_dispatch(
                &args.request_id,
                SessionId::new().as_str(),
                &args.project_id,
                reserved_title,
                &root,
                &args.prompt,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if created {
            // `SessionCreated` carries the reserved title; a session named from
            // its prompt then announces the rename, like any other rename. The
            // prompt itself is never echoed here - it is not a title.
            self.0.runtime().emit(
                Some(&id),
                None,
                None,
                Event::SessionCreated {
                    session_id: id.clone(),
                    title: reserved_title.to_string(),
                },
            );
            if title.is_none() {
                // The agent turn that called this tool must not wait on the
                // titling provider: keep the derived title now, let the model
                // improve it in the background (`titles.enabled` decides).
                name_from_prompt(self.0.clone(), id.clone(), args.prompt.clone());
            }
            // Only the call that created the session pins it. A replay must not
            // overwrite a model the user has changed since.
            if let Some(pin) = &pin {
                if let Err(e) = self.0.store().set_session_model(&id, Some(pin)).await {
                    return ToolOutcome::err(format!("Session {id} reserved but its model pin was not applied: {e}. Do not retry with a new request ID; inspect the session."));
                }
                self.0.runtime().emit(
                    Some(&id),
                    None,
                    None,
                    Event::SessionModelChanged {
                        session_id: id.clone(),
                        model: Some(pin.clone()),
                    },
                );
            }
            if let Err(e) = self.0.runtime().submit(&id, args.prompt).await {
                return ToolOutcome::err(format!("Session {id} reserved but run did not start: {e}. Do not retry with a new request ID; inspect the session (control_read_session), and re-send with control_send_message if it has no messages."));
            }
        }
        // A replay is only reassuring if the work is actually moving. The reserve
        // and the submit are two steps, so a crash or a failed submit in between
        // leaves a session with no run; a replay must say that rather than
        // answer "already started".
        let (run_started, stored_model) = match self.0.store().get_session(&id).await {
            Ok(Some(session)) => (
                !self
                    .0
                    .store()
                    .list_runs(&id, 1)
                    .await
                    .unwrap_or_default()
                    .is_empty(),
                session.model,
            ),
            // Gone (the user deleted it): the replay is pointing at nothing.
            Ok(None) => (false, None),
            Err(_) => (true, None),
        };
        let note = if run_started {
            "A replay never starts another run and never re-pins the session; inspect it if the original start was interrupted."
        } else {
            "No run has ever started in that session: the original dispatch reserved it but did not submit. Do not retry with a new request ID - send control_send_message to it instead."
        };
        ToolOutcome::ok(json!({"session_id":id,"project_id":args.project_id,"model":stored_model,"requested_model":pin,"run_started":run_started,"replayed":!created,"note":note}).to_string())
    }
}
