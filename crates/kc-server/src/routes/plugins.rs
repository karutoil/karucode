//! Plugin and MCP introspection. Enabling a plugin also reloads the runtime's
//! tool registry so the change is visible to the very next run.

use crate::error::{ApiError, ApiResult};
use crate::routes::parse_required;
use crate::state::{mcp_summary, AppState};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnableBody {
    #[serde(default)]
    enabled: Option<bool>,
}

pub async fn list_plugins(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let plugins = {
        let host = state.plugins().lock().await;
        host.diagnostics()
    };
    let values: Vec<Value> = plugins
        .iter()
        .map(|diagnostics| {
            serde_json::to_value(diagnostics)
                .map_err(|e| ApiError::internal(format!("could not serialize plugin: {e}")))
        })
        .collect::<ApiResult<_>>()?;
    Ok(Json(json!({ "plugins": values })))
}

pub async fn enable_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: EnableBody = parse_required(&body, "POST /api/plugins/:name/enable")?;
    let enabled = body
        .enabled
        .ok_or_else(|| ApiError::bad_request("enabled is required (true or false)"))?;

    let diagnostics = {
        let mut host = state.plugins().lock().await;
        host.set_enabled(&name, enabled)
            .await
            .map_err(|error| match error {
                kc_core::Error::NotFound(message) => ApiError::not_found(message),
                kc_core::Error::Config(message) => ApiError::bad_request(message),
                other => ApiError::from(other),
            })?
    };

    // The registry the model sees must match the host immediately.
    state.reload_tools().await;

    Ok(Json(serde_json::to_value(diagnostics).map_err(|e| {
        ApiError::internal(format!("could not serialize plugin: {e}"))
    })?))
}

pub async fn list_mcp(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let servers = {
        let mcp = state.mcp().lock().await;
        mcp.statuses().iter().map(mcp_summary).collect::<Vec<_>>()
    };
    Ok(Json(json!({ "servers": servers })))
}
