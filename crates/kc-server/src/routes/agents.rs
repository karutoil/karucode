//! Subagent inspection: the workspace-wide agent list and one agent's transcript.
//!
//! Subagents used to be visible only as an `agents` array inside the session
//! they were spawned from, which meant "is anything running?" required opening
//! every session. These two routes make the Agents view possible:
//!
//! * `GET /api/agents` lists every agent the server knows about, newest first,
//!   joined with its session title/workspace and filterable by project, session
//!   and status.
//! * `GET /api/agents/:id` returns one agent and the transcript it wrote. Agent
//!   messages are stored in the parent session with `agent_id` set; the session's
//!   own messages are deliberately not part of this response.

use crate::error::{ApiError, ApiResult};
use crate::routes::{message_before, query_str, query_usize, QueryMap, TRANSCRIPT_PAGE_BYTES};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::store::{AgentStatus, MessageScope};
use serde_json::{json, Value};

/// Default tail length of `GET /api/agents/:id` `?messages=`.
const AGENT_MESSAGE_LIMIT: usize = 2000;
/// Upper bound for `?messages=` on one agent's transcript.
const AGENT_MESSAGE_LIMIT_MAX: usize = 50_000;
/// How many status values a `?status=` filter may carry.
const MAX_STATUS_FILTERS: usize = 8;

/// Parse and validate the repeated/comma-separated `?status=` filter.
///
/// Accepts `?status=running,failed` and `?status=running&status=failed` (the
/// query map keeps the last repeated key, so comma-separated is the documented
/// form). An unknown status is a `400` naming the valid set instead of silently
/// returning an empty list.
fn parse_status_filter(query: &QueryMap) -> ApiResult<Vec<String>> {
    let Some(raw) = query_str(query, "status") else {
        return Ok(Vec::new());
    };
    let mut statuses = Vec::new();
    for part in raw.split(',') {
        let status = part.trim();
        if status.is_empty() {
            continue;
        }
        if AgentStatus::parse(status).is_none() {
            return Err(ApiError::bad_request(format!(
                "unknown `status` value `{status}`; expected one of running, paused, completed, failed, cancelled, interrupted"
            )));
        }
        if statuses.len() >= MAX_STATUS_FILTERS {
            return Err(ApiError::bad_request(format!(
                "at most {MAX_STATUS_FILTERS} `status` values are allowed"
            )));
        }
        if !statuses.iter().any(|value: &String| value == status) {
            statuses.push(status.to_string());
        }
    }
    Ok(statuses)
}

/// How many agents `GET /api/agents` returns when the caller does not say.
///
/// The response used to be every agent on the server: 1042 of them, 4.86 MB of
/// JSON and +11.7 MB of resident memory for the first call. A page keeps the
/// worst case near a hundred rows, and `?all=true` is how a script that really
/// wants the fleet asks for it.
const AGENTS_PAGE_DEFAULT: usize = 100;
/// Hard ceiling on one page, `?all=true` included: an unbounded response is what
/// this route was fixed to stop producing, and 2000 rows is already ~10 MB.
const AGENTS_PAGE_MAX: usize = 2000;

/// How many agents this request should return, and where the page starts.
///
/// `?limit=N` (1..=2000) with `?offset=N`, or `?all=true` for the whole fleet up
/// to [`AGENTS_PAGE_MAX`]. `?limit=0` is a 400 rather than a silent default: a
/// caller that computed 0 has a bug, and answering it with 100 rows would hide it.
fn agent_page(query: &QueryMap) -> ApiResult<(usize, usize)> {
    let all = query
        .get("all")
        .map(|value| matches!(value.as_str(), "true" | "1" | "yes"))
        .unwrap_or(false);
    let limit = match query_usize(query, "limit")? {
        Some(0) => {
            return Err(ApiError::bad_request(
                "`limit` must be at least 1; use `all=true` to fetch every agent".to_string(),
            ))
        }
        Some(limit) if limit > AGENTS_PAGE_MAX => {
            return Err(ApiError::bad_request(format!(
                "`limit` must be at most {AGENTS_PAGE_MAX}"
            )))
        }
        Some(limit) => limit,
        None if all => AGENTS_PAGE_MAX,
        None => AGENTS_PAGE_DEFAULT,
    };
    let offset = query_usize(query, "offset")?.unwrap_or(0);
    Ok((limit, offset))
}

pub async fn list_agents(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let project = query_str(&query, "project");
    let session = query_str(&query, "session");
    let statuses = parse_status_filter(&query)?;
    let (limit, offset) = agent_page(&query)?;
    let store = state.store();
    let agents = store
        .list_agents_page(project, session, &statuses, limit, offset)
        .await?;
    // `running` is "alive and spending", which includes paused: a paused agent
    // still holds a subagent slot, so a caller that used this count to decide
    // whether it had capacity would be wrong if paused were excluded. The exact
    // split is one `status` per row, which the client already has.
    //
    // Counted over the whole filtered set, not over the page: `total` is what lets
    // a client say "100 of 1042", and a truncation must not report "3 running" for
    // a server with 30.
    let counts = store
        .agent_status_counts(project, session, &statuses)
        .await?;
    Ok(Json(json!({
        "agents": agents,
        "running": counts.live,
        "paused": counts.paused,
        "total": counts.total,
        // The runtime's own view, which covers both scopes: the store list above
        // can be filtered, while `live` is "loops that exist right now".
        "live": state.runtime().live_agent_count(
            session.unwrap_or_default()
        ),
    })))
}

pub async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let agent = state
        .store()
        .get_agent(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent {id}")))?;
    let limit = query_usize(&query, "messages")?.unwrap_or(AGENT_MESSAGE_LIMIT);
    let limit = limit.clamp(1, AGENT_MESSAGE_LIMIT_MAX);
    let page = state
        .store()
        .list_message_page(
            &agent.agent.session_id,
            MessageScope::Agent(&id),
            message_before(&query)?,
            limit,
            TRANSCRIPT_PAGE_BYTES,
        )
        .await?;
    let messages_truncated = page.has_more;
    let messages = page.messages;
    let messages_before = messages_truncated
        .then(|| messages.first().map(|m| m.id))
        .flatten();
    Ok(Json(json!({
        "agent": agent,
        "messages": messages,
        "messages_truncated": messages_truncated,
        "messages_limit": limit,
        "messages_before": messages_before,
    })))
}

/// `POST /api/agents/:id/{pause,resume,cancel}`
///
/// The human's control surface for a live subagent. Every one of the three is the
/// same operation from the server's point of view - ask the runtime to change what
/// a running loop is doing, then report what it is now - so one handler covers
/// them and an unknown verb is a 400 naming the three that exist.
///
/// A pause takes effect at the agent's next iteration boundary: the turn in flight
/// finishes, because that is the only point where stopping costs no work. A cancel
/// fires the agent's token at once and lets its tools unwind, so the response says
/// `cancelled` while the transcript catches up a moment later.
///
/// An unknown or already-settled agent is a 404 rather than a silent success: a
/// browser that raced the finish has to be told, or its pause button looks broken
/// forever.
pub async fn control_agent(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let Some(action) = kc_runtime::AgentAction::parse(&action) else {
        return Err(ApiError::bad_request(format!(
            "unknown agent action `{action}`; expected pause, resume or cancel"
        )));
    };
    let status = state.runtime().control_agent(&id, action).await?;
    Ok(Json(json!({
        "agent_id": id,
        "action": match action {
            kc_runtime::AgentAction::Pause => "pause",
            kc_runtime::AgentAction::Resume => "resume",
            kc_runtime::AgentAction::Cancel => "cancel",
        },
        "status": status.as_str(),
    })))
}
