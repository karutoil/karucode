//! Session CRUD, message submission and cancellation.

use crate::error::{ApiError, ApiResult};
use crate::routes::parse_required;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kc_core::events::Event;
use kc_core::ids::SessionId;
use kc_core::store::Session;
use serde::Deserialize;
use serde_json::{json, Value};

/// Bounded prompt size: the UI sends text, not files. 1 MiB is far above any
/// real prompt and keeps a single request from exhausting memory.
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const MAX_TITLE_CHARS: usize = 200;
/// History returned by `GET /api/sessions/:id` (tail, most recent last).
const MESSAGE_LIMIT: usize = 5000;
const RUN_LIMIT: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleBody {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    #[serde(default)]
    content: Option<String>,
}

pub async fn list_sessions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let sessions = state.store().list_sessions().await?;
    Ok(Json(json!({ "sessions": sessions })))
}

pub async fn create_session(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let body: TitleBody = crate::routes::parse_body(&body)?;
    let title = normalize_title(body.title)?;
    let id = SessionId::new();
    let session = state
        .store()
        .create_session(
            id.as_str(),
            &title,
            &state.workspace().root().to_string_lossy(),
        )
        .await?;
    // Runtime-stamped so the SSE `seq` stays monotonic for all clients.
    state.runtime().emit(
        Some(session.id.as_str()),
        None,
        None,
        Event::SessionCreated {
            session_id: session.id.clone(),
            title: session.title.clone(),
        },
    );
    Ok(Json(
        serde_json::to_value(session).map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &id).await?;
    let store = state.store();
    let messages = store.list_messages(&session.id, MESSAGE_LIMIT).await?;
    let runs = store.list_runs(&session.id, RUN_LIMIT).await?;
    let agents = store.list_agents(&session.id).await?;
    let approvals = store.list_pending_approvals(Some(&session.id)).await?;
    let claims = store.list_claims(&session.id).await?;
    let running = state.runtime().is_running(&session.id);
    Ok(Json(json!({
        "session": session,
        "messages": messages,
        "runs": runs,
        "agents": agents,
        "approvals": approvals,
        "claims": claims,
        "running": running,
    })))
}

pub async fn rename_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let body: TitleBody = parse_required(&body, "PATCH /api/sessions/:id")?;
    let Some(raw) = body.title else {
        return Err(ApiError::bad_request("title is required"));
    };
    let title = normalize_title(Some(raw))?;
    state.store().rename_session(&id, &title).await?;
    let session = session_or_404(&state, &id).await?;
    state.runtime().emit(
        Some(&id),
        None,
        None,
        Event::SessionRenamed {
            session_id: id.clone(),
            title: session.title.clone(),
        },
    );
    Ok(Json(
        serde_json::to_value(session).map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    // Stop any in-flight work before dropping its rows so the run loop cannot
    // resurrect a deleted session.
    state.runtime().cancel(&id);
    state.store().delete_session(&id).await?;
    state.runtime().clear_session_allows(&id);
    state.runtime().emit(
        Some(&id),
        None,
        None,
        Event::SessionDeleted {
            session_id: id.clone(),
        },
    );
    Ok(Json(json!({ "deleted": true })))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let _ = session_or_404(&state, &id).await?;
    let body: MessageBody = parse_required(&body, "POST /api/sessions/:id/messages")?;
    let content = body
        .content
        .ok_or_else(|| ApiError::bad_request("content is required"))?;
    if content.trim().is_empty() {
        return Err(ApiError::bad_request("content may not be empty"));
    }
    if content.len() > MAX_PROMPT_BYTES {
        return Err(ApiError::bad_request(format!(
            "content is too large ({} bytes, maximum {MAX_PROMPT_BYTES})",
            content.len()
        )));
    }
    if content.contains('\0') {
        return Err(ApiError::bad_request("content may not contain NUL bytes"));
    }

    // Cheap up-front check gives the documented 409 instead of the runtime's
    // generic "already active" bad_request. `submit` still re-checks under its
    // lock, so the race window cannot start two runs.
    if state.runtime().is_running(&id) {
        return Err(ApiError::conflict(
            "a run is already active for this session; wait for it to finish or cancel it",
        ));
    }
    let handle = state
        .runtime()
        .submit(&id, content)
        .await
        .map_err(|error| match error {
            kc_core::Error::BadRequest(message) if message.contains("already active") => {
                ApiError::conflict(message)
            }
            other => ApiError::from(other),
        })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "run_id": handle.run_id,
            "session_id": handle.session_id,
        })),
    )
        .into_response())
}

pub async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let cancelled = state.runtime().cancel(&id);
    Ok(Json(json!({ "cancelled": cancelled })))
}

async fn session_or_404(state: &AppState, id: &str) -> ApiResult<Session> {
    state
        .store()
        .get_session(id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session `{id}` does not exist")))
}

fn normalize_title(raw: Option<String>) -> ApiResult<String> {
    let title = raw.unwrap_or_default();
    let title = title.trim();
    if title.is_empty() {
        return Ok("New session".to_string());
    }
    if title.contains('\0') {
        return Err(ApiError::bad_request("title may not contain NUL bytes"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(ApiError::bad_request(format!(
            "title is too long ({} characters, maximum {MAX_TITLE_CHARS})",
            title.chars().count()
        )));
    }
    Ok(title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_normalized_and_bounded() {
        assert_eq!(normalize_title(None).unwrap(), "New session");
        assert_eq!(normalize_title(Some("  hi  ".into())).unwrap(), "hi");
        assert!(normalize_title(Some("x".repeat(201))).is_err());
        assert!(normalize_title(Some("a\0b".into())).is_err());
    }
}
