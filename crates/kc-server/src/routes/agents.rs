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
use crate::routes::{query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::store::AgentStatus;
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
                "unknown `status` value `{status}`; expected one of running, completed, failed, cancelled, interrupted"
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

pub async fn list_agents(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let project = query_str(&query, "project");
    let session = query_str(&query, "session");
    let statuses = parse_status_filter(&query)?;
    let mut agents = state.store().list_agents_all(project).await?;
    if let Some(session) = session {
        agents.retain(|summary| summary.agent.session_id == session);
    }
    if !statuses.is_empty() {
        agents.retain(|summary| {
            statuses
                .iter()
                .any(|status| status == &summary.agent.status)
        });
    }
    let running = agents
        .iter()
        .filter(|summary| summary.agent.status == AgentStatus::Running.as_str())
        .count();
    Ok(Json(json!({
        "agents": agents,
        "running": running,
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
    let mut messages = state.store().list_agent_messages(&id, limit + 1).await?;
    let messages_truncated = messages.len() > limit;
    if messages_truncated {
        // `list_agent_messages` returns the tail; drop the overflowing oldest row.
        let excess = messages.len() - limit;
        messages.drain(..excess);
    }
    Ok(Json(json!({
        "agent": agent,
        "messages": messages,
        "messages_truncated": messages_truncated,
        "messages_limit": limit,
    })))
}
