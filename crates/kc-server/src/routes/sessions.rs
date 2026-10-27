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
/// Title `POST /api/sessions` assigns when the caller supplies no title. A
/// session still carrying it is treated as untitled.
const DEFAULT_SESSION_TITLE: &str = "New session";
/// Auto-derived titles are cut here (plus `…`) so the session rail stays
/// readable at narrow widths.
const MAX_AUTO_TITLE_CHARS: usize = 60;
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
    let session = session_or_404(&state, &id).await?;
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
    maybe_auto_title(&state, &session, &content).await;
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

/// Give an untitled session a title derived from its first user message.
///
/// Runs before `runtime.submit` so the rail shows a real title as soon as the
/// run starts. Cosmetic only: any failure is logged and the request proceeds.
async fn maybe_auto_title(state: &AppState, session: &Session, content: &str) {
    if !is_placeholder_title(&session.title) {
        return;
    }
    let Some(title) = derive_title(content) else {
        return;
    };
    match state.store().rename_session(&session.id, &title).await {
        Ok(()) => {
            state.runtime().emit(
                Some(&session.id),
                None,
                None,
                Event::SessionRenamed {
                    session_id: session.id.clone(),
                    title,
                },
            );
        }
        Err(error) => tracing::warn!(
            session = %session.id,
            error = %error,
            "could not auto-title session"
        ),
    }
}

/// True while the session still carries the title it was created with.
///
/// The store default is the empty string (rows created before titles existed or
/// outside the API) and `POST /api/sessions` fills in [`DEFAULT_SESSION_TITLE`].
/// A user who explicitly renames into one of those two values gets auto-titled
/// once more; the schema keeps no `renamed` provenance bit, so that case is
/// indistinguishable from an untouched session.
fn is_placeholder_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == DEFAULT_SESSION_TITLE
}

/// Derive a rail-friendly title from a prompt.
///
/// 1. trim the edges;
/// 2. drop one trivial leading marker (a leading `/` command sigil or a
///    `[tool:...]` prefix) so synthetic prompts do not title the session
///    `[tool:...]`;
/// 3. collapse every whitespace run (newlines included) to one space;
/// 4. keep at most [`MAX_AUTO_TITLE_CHARS`] characters on a char boundary and
///    append `…` when text was dropped (so at most 61 characters total).
///
/// Returns `None` when nothing printable is left, which leaves the title alone.
fn derive_title(content: &str) -> Option<String> {
    let text = collapse_whitespace(strip_leading_marker(content.trim()));
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= MAX_AUTO_TITLE_CHARS {
        return Some(text);
    }
    let mut title: String = text.chars().take(MAX_AUTO_TITLE_CHARS).collect();
    let kept = title.trim_end().len();
    title.truncate(kept);
    title.push('…');
    Some(title)
}

/// Remove a leading `/` command sigil or `[tool:...]` prefix. Anything more
/// elaborate (quoting, escapes) is not worth guessing at.
fn strip_leading_marker(text: &str) -> &str {
    let mut rest = text;
    if let Some(after) = rest.strip_prefix('/') {
        rest = after.trim_start();
    }
    if let Some(after) = rest.strip_prefix("[tool:") {
        if let Some(end) = after.find(']') {
            rest = after[end + 1..].trim_start();
        }
    }
    let rest = rest.trim();
    // A prompt that was nothing but the marker keeps its original text.
    if rest.is_empty() {
        text
    } else {
        rest
    }
}

/// Replace every run of whitespace (spaces, tabs, newlines, ...) with a single
/// space and trim the edges.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        assert_eq!(normalize_title(None).unwrap(), DEFAULT_SESSION_TITLE);
        assert_eq!(normalize_title(Some("  hi  ".into())).unwrap(), "hi");
        assert!(normalize_title(Some("x".repeat(201))).is_err());
        assert!(normalize_title(Some("a\0b".into())).is_err());
    }

    #[test]
    fn placeholder_titles_are_detected() {
        assert!(is_placeholder_title(""));
        assert!(is_placeholder_title("   "));
        assert!(is_placeholder_title(DEFAULT_SESSION_TITLE));
        assert!(is_placeholder_title(" New session "));
        assert!(!is_placeholder_title("New session!"));
        assert!(!is_placeholder_title("fix the build"));
    }

    #[test]
    fn titles_are_derived_from_content() {
        assert_eq!(
            derive_title("  fix   the\n\nbuild\tplease  ").unwrap(),
            "fix the build please"
        );
        assert_eq!(derive_title("/clear").unwrap(), "clear");
        assert_eq!(
            derive_title("[tool:read] summarise this").unwrap(),
            "summarise this"
        );
        assert_eq!(derive_title("   \n\t "), None);
        assert_eq!(
            derive_title("é".repeat(61).as_str())
                .unwrap()
                .chars()
                .count(),
            61
        );
        // Exactly at the limit: no ellipsis, no truncation.
        assert_eq!(derive_title(&"a".repeat(60)).unwrap(), "a".repeat(60));
        let long = derive_title(&"é".repeat(200)).unwrap();
        assert_eq!(long.chars().count(), 61);
        assert!(long.ends_with('…'));
        assert_eq!(long.chars().filter(|c| *c == 'é').count(), 60);
    }
}
