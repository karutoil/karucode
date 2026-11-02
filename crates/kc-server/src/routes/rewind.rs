//! Rewind routes: `GET .../rewind` previews, `POST .../rewind` performs.
//!
//! Two endpoints rather than one because the dialog has to describe what it will
//! do *before* doing it: which files come back, which are removed, and what the
//! journal cannot reach. Computing that list and then re-deriving it in the POST
//! would let the confirmation and the action disagree, which is exactly the
//! failure a confirmation dialog exists to prevent - so both read the same
//! [`kc_runtime::Runtime::rewind_preview`].
//!
//! # Why the preview is a GET and the action is a POST
//!
//! The preview is a pure read: it hides nothing, restores nothing, and a client
//! may call it on hover. The action hides transcript rows and can write to the
//! workspace, so it is a POST and needs `X-Karucode-Client: web` like every other
//! mutation.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_str, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// What the browser sends to rewind.
///
/// `keep_prompt` is the difference between "edit the prompt I already typed" and
/// "replace it": a rewind that hides the prompt is what a retry and a note need,
/// so the model does not see two user turns in a row.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RewindBody {
    /// The message to rewind to. An assistant message is normalised to the user
    /// prompt that produced it.
    message_id: i64,
    /// Undo the file changes made after that message. `false` keeps them, which
    /// is the safe default: restoring files is the destructive half.
    #[serde(default)]
    revert: bool,
    /// `none` hides the tail and stops (the caller then edits the prompt in the
    /// composer), `retry` hides it and re-sends the original prompt, `note` hides
    /// it and sends `note` instead.
    #[serde(default = "default_mode")]
    mode: String,
    /// The replacement prompt for `mode: "note"`, and an optional override for
    /// `mode: "retry"`.
    #[serde(default)]
    note: Option<String>,
    /// Whether the chosen prompt itself survives. Defaults to `true` for
    /// `mode: "none"` (the user wants to edit it) and `false` otherwise, because
    /// a retry or a note that kept the old prompt would show the model the
    /// original request and its replacement back to back.
    #[serde(default)]
    keep_prompt: Option<bool>,
}

fn default_mode() -> String {
    "none".into()
}

impl RewindBody {
    /// The mode after validation, with the effective `keep_prompt`.
    fn resolve(&self) -> ApiResult<(RewindMode, bool)> {
        let mode = RewindMode::parse(&self.mode)?;
        let keep_prompt = self
            .keep_prompt
            .unwrap_or(mode.defaults_to_keeping_the_prompt());
        if mode == RewindMode::Note && self.note.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ApiError::bad_request(
                "mode `note` requires a non-empty `note`: it is the prompt the agent runs next",
            ));
        }
        Ok((mode, keep_prompt))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RewindMode {
    /// Hide the tail; no new run.
    None,
    /// Hide the tail and re-send the original prompt.
    Retry,
    /// Hide the tail and send the note instead.
    Note,
}

impl RewindMode {
    fn parse(raw: &str) -> ApiResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "none" => Ok(RewindMode::None),
            "retry" => Ok(RewindMode::Retry),
            "note" => Ok(RewindMode::Note),
            other => Err(ApiError::bad_request(format!(
                "`mode` must be `none`, `retry` or `note` (got `{other}`)"
            ))),
        }
    }

    /// Whether the prompt survives a rewind in this mode.
    fn defaults_to_keeping_the_prompt(&self) -> bool {
        matches!(self, RewindMode::None)
    }
}

/// `GET /api/sessions/:id/rewind?message=<id>`
///
/// Describes what a rewind would do without doing any of it. This is what the
/// confirmation dialog renders, and it is safe to call repeatedly.
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    session_or_404(&state, &id).await?;
    let message_id = require_message_id(query_str(&query, "message"))?;
    let (point, preview) = state
        .runtime()
        .rewind_preview(&id, message_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "session_id": id,
        "point": point,
        "preview": preview,
        "running": state.runtime().has_active_run(&id),
    })))
}

/// `POST /api/sessions/:id/rewind`
///
/// Hides the transcript from `message_id` onwards, optionally reverts the file
/// changes made after it, and - for `retry` and `note` - starts the run that
/// replaces the hidden turn.
pub async fn rewind(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    session_or_404(&state, &id).await?;
    let body: RewindBody = parse_required(&body, "POST /api/sessions/:id/rewind")?;
    let (mode, keep_prompt) = body.resolve()?;

    // A run in flight would keep writing after the rewind; the runtime refuses,
    // but saying so here keeps the error shape consistent with `cancel`.
    if state.runtime().is_running(&id) {
        return Err(ApiError::conflict(
            "a run is already active for this session; cancel it before rewinding",
        ));
    }

    let result = match mode {
        RewindMode::None => state
            .runtime()
            .rewind(&id, body.message_id, body.revert, keep_prompt)
            .await
            .map_err(ApiError::from)?,
        RewindMode::Retry | RewindMode::Note => {
            // A retry sends the prompt that is being replaced; the preview knows
            // it, and reading it here (rather than trusting the client to echo it
            // back) is what keeps the retried prompt identical to the original.
            let (point, _) = state
                .runtime()
                .rewind_preview(&id, body.message_id)
                .await
                .map_err(ApiError::from)?;
            let prompt = match mode {
                RewindMode::Note => body.note.clone().unwrap_or_default(),
                _ => body.note.clone().unwrap_or_else(|| point.prompt.clone()),
            };
            if prompt.trim().is_empty() {
                return Err(ApiError::bad_request(
                    "there is no prompt to send: the original message is empty and no `note` was given",
                ));
            }
            state
                .runtime()
                .rewind_and_run(&id, body.message_id, body.revert, prompt)
                .await
                .map_err(ApiError::from)?
        }
    };

    // The transcript changed for every client, not only the one that asked.
    state.runtime().emit(
        Some(&id),
        None,
        None,
        kc_core::events::Event::SessionRewound {
            session_id: id.clone(),
            message_id: body.message_id,
            messages_removed: result.point.messages_removed,
        },
    );

    let undone = result
        .outcomes
        .iter()
        .filter(|outcome| outcome.is_clean())
        .count();
    let failed = result
        .outcomes
        .iter()
        .filter(|outcome| !outcome.is_clean())
        .count();

    Ok((
        StatusCode::OK,
        Json(json!({
            "session_id": id,
            "point": result.point,
            "preview": result.preview,
            "outcomes": result.outcomes,
            "run_id": result.run_id,
            "mode": mode.as_str(),
            "files_restored": undone,
            "files_skipped": failed,
        })),
    )
        .into_response())
}

impl RewindMode {
    fn as_str(&self) -> &'static str {
        match self {
            RewindMode::None => "none",
            RewindMode::Retry => "retry",
            RewindMode::Note => "note",
        }
    }
}

fn require_message_id(raw: Option<&str>) -> ApiResult<i64> {
    match raw {
        Some(value) => value.parse::<i64>().map_err(|_| {
            ApiError::bad_request(format!(
                "query parameter `message` must be a message id (got `{value}`)"
            ))
        }),
        None => Err(ApiError::bad_request(
            "query parameter `message` is required: it names the message to rewind to",
        )),
    }
}

async fn session_or_404(state: &AppState, id: &str) -> ApiResult<()> {
    match state.store().get_session(id).await? {
        Some(_) => Ok(()),
        None => Err(ApiError::not_found(format!(
            "session `{id}` does not exist"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(raw: Value) -> RewindBody {
        serde_json::from_value(raw).expect("body")
    }

    #[test]
    fn keep_prompt_defaults_per_mode() {
        // Editing the prompt you already typed: it survives, the composer takes
        // over from it.
        let (mode, keep) = body(json!({"message_id": 1})).resolve().unwrap();
        assert_eq!(mode, RewindMode::None);
        assert!(keep, "a plain rewind keeps the prompt for editing");

        // A retry re-sends it, so keeping the old row would duplicate it.
        let (mode, keep) = body(json!({"message_id": 1, "mode": "retry"}))
            .resolve()
            .unwrap();
        assert_eq!(mode, RewindMode::Retry);
        assert!(!keep, "a retry replaces the prompt");

        let (_, keep) = body(json!({"message_id": 1, "mode": "note", "note": "do it differently"}))
            .resolve()
            .unwrap();
        assert!(!keep, "a note replaces the prompt");
    }

    #[test]
    fn an_explicit_keep_prompt_wins() {
        let (_, keep) = body(json!({"message_id": 1, "mode": "retry", "keep_prompt": true}))
            .resolve()
            .unwrap();
        assert!(keep);
        let (_, keep) = body(json!({"message_id": 1, "keep_prompt": false}))
            .resolve()
            .unwrap();
        assert!(!keep);
    }

    #[test]
    fn a_note_mode_needs_a_note() {
        let err = body(json!({"message_id": 1, "mode": "note"}))
            .resolve()
            .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("non-empty `note`"), "{}", err.message);

        let err = body(json!({"message_id": 1, "mode": "note", "note": "   "}))
            .resolve()
            .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn an_unknown_mode_is_a_bad_request_that_names_the_options() {
        let err = body(json!({"message_id": 1, "mode": "undo"}))
            .resolve()
            .unwrap_err();
        assert!(
            err.message.contains("`none`, `retry` or `note`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn revert_defaults_to_off() {
        let parsed = body(json!({"message_id": 7}));
        assert!(!parsed.revert, "restoring files must be opted into");
    }

    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        // A typo like `reverts` silently doing nothing would be worse than a 400.
        assert!(
            serde_json::from_value::<RewindBody>(json!({"message_id": 1, "reverts": true}))
                .is_err()
        );
    }

    #[test]
    fn the_message_id_is_required_and_numeric() {
        assert!(require_message_id(None).is_err());
        assert!(require_message_id(Some("abc")).is_err());
        assert_eq!(require_message_id(Some("42")).unwrap(), 42);
    }
}
