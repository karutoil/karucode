//! `/api/onboarding`: the first-run wizard's state and its finish line.
//!
//! The wizard is a **frontend** flow - provider, appearance, approvals and access
//! are each saved through the API they already own (`PATCH /api/config`,
//! `POST /api/auth/...`). This module exists for the two questions the client
//! cannot answer on its own:
//!
//! * *should the wizard run at all?* - a fresh install has no provider, no
//!   project and no credential; an install that has been in use for a month must
//!   not be greeted with "welcome, let's get you set up";
//! * *has it been finished?* - so "Skip" is remembered, and a reload does not
//!   put the user back on step one.
//!
//! The marker lives in the `settings` table (`web_auth.onboarding`), which is why
//! it survives a restart without a schema change.

use crate::error::ApiResult;
use crate::routes::auth::access_summary;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// `GET /api/onboarding` - what is missing, and whether to show the wizard.
///
/// `needed` is deliberately conservative: it is true only while the install still
/// looks new. A configured provider, an existing session or a recorded completion
/// each switch it off, so an upgrade never interrupts an established workspace.
pub async fn get(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let web_auth = state.web_auth();
    let completed = web_auth.onboarding_completed().await;
    let config = state.settings().snapshot();
    // "Configured" means an endpoint credential exists: a default model name and
    // the shipped base URL are not a decision the user made. A keyless local
    // gateway therefore sees the wizard once - which is the point of the wizard -
    // and never again after it is finished.
    let provider_configured =
        config.model.api_key.is_some() || config.providers.iter().any(|profile| profile.api_key.is_some());
    let sessions = state.store().list_sessions().await.unwrap_or_default();
    let has_sessions = !sessions.is_empty();
    let needed = !completed && !provider_configured && !has_sessions;
    Ok(Json(json!({
        "needed": needed,
        "completed": completed,
        "provider_configured": provider_configured,
        "has_sessions": has_sessions,
        "project_count": state.store().list_projects().await.map(|p| p.len()).unwrap_or(0),
        "has_credential": web_auth.has_credential(),
        "access": access_summary(&state).await,
        "workspace": state.workspace_root_display(),
        "version": crate::VERSION,
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteBody {
    /// The user pressed "Skip" rather than finishing the wizard. Recorded so the
    /// UI can mention later that setup is still incomplete without nagging.
    #[serde(default)]
    pub skipped: bool,
}

/// `POST /api/onboarding/complete` - record that the wizard is done (or skipped).
pub async fn complete(
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CompleteBody = crate::routes::parse_body(&body)?;
    state
        .web_auth()
        .complete_onboarding()
        .await
        .map_err(crate::error::ApiError::from)?;
    tracing::info!(skipped = body.skipped, "onboarding finished");
    Ok(Json(json!({
        "ok": true,
        "completed": true,
        "skipped": body.skipped,
    })))
}
