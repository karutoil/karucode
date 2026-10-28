//! `GET /api/health`.

use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let config = state.settings().snapshot();
    let effective = config.effective_model();
    let provider_configured =
        !effective.base_url.trim().is_empty() && !effective.model.trim().is_empty();
    let db = state.db_status().await;
    let auth_required = state.auth_token().is_some();
    let projects = state.store().list_projects().await?;

    Ok(Json(json!({
        "status": if db == "ok" { "ok" } else { "degraded" },
        "version": crate::VERSION,
        "uptime_secs": state.uptime_secs(),
        "workspace": state.workspace().root().to_string_lossy(),
        "projects": projects,
        "auth_required": auth_required,
        "provider_configured": provider_configured,
        "db": db,
    })))
}
