//! Plugin and MCP introspection. Enabling, reloading or rescanning a plugin also
//! reloads the runtime's tool registry so the change is visible to the very next
//! run.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_str, QueryMap};
use crate::state::{mcp_summary, AppState};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::plugin::ToolCallContext;
use kc_plugin_host::PluginPanelsHandle;
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
    Ok(Json(json!({ "plugins": plugin_values(plugins)? })))
}

fn plugin_values(plugins: Vec<kc_core::plugin::PluginDiagnostics>) -> ApiResult<Vec<Value>> {
    plugins
        .into_iter()
        .map(|diagnostics| {
            serde_json::to_value(diagnostics)
                .map_err(|e| ApiError::internal(format!("could not serialize plugin: {e}")))
        })
        .collect()
}

/// `POST /api/plugins/reload` — re-read every plugin source and reconcile the
/// running processes with it.
///
/// This is the no-restart path for installing a new plugin, upgrading one whose
/// binary was rebuilt in place, and removing one: names that are new are started,
/// names that disappeared are stopped and dropped, and a plugin whose config or
/// files changed is replaced. Plugins that did not change are left running
/// untouched, so this is safe to call on a hunch (and the watcher calls it on a
/// timer).
///
/// The body is the report (`added`, `removed`, `restarted`, `unchanged`) plus the
/// live diagnostics, so a caller does not need a second request.
pub async fn reload_plugins(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (report, diagnostics) = state.reload_plugins().await;
    Ok(Json(json!({
        "report": report,
        "plugins": plugin_values(diagnostics)?,
    })))
}

/// `POST /api/plugins/:name/reload` — replace one plugin's process.
///
/// The "I rebuilt it" / "I edited its manifest" path. A runtime disable is
/// preserved: reloading a disabled plugin rebuilds its slot without starting it.
pub async fn reload_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let diagnostics = state.reload_plugin(&name).await.map_err(map_plugin_error)?;
    serde_json::to_value(diagnostics)
        .map(Json)
        .map_err(|e| ApiError::internal(format!("could not serialize plugin: {e}")))
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
            .map_err(map_plugin_error)?
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

/// `GET /api/plugins/:name/panels/:panel[?project=&session=]`
///
/// The plugin answers `panels/read` with opaque JSON; this handler wraps it in
/// `{title, data, version, fetched_ms}` and never interprets `data`. The
/// frontend renders it with a component registered for that panel, or as
/// read-only JSON when none is — so a plugin still cannot run code in the app's
/// origin.
///
/// `version` is a cheap cache key: it changes whenever the payload could have
/// changed (it is the response's own timestamp), so a poller can skip a redraw.
pub async fn read_panel(
    State(state): State<AppState>,
    Path((name, panel)): Path<(String, String)>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let session_id = query_str(&query, "session");
    let project_id = query_str(&query, "project");

    // Resolve the workspace the call belongs to exactly like the file routes: an
    // unknown project is a 404 before the plugin is even consulted.
    let workspace = state.workspace_for_project(project_id)?;
    // An unknown session is a caller mistake, not an empty panel.
    if let Some(session) = session_id {
        if state.store().get_session(session).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "no session with id `{session}`"
            )));
        }
    }

    let ctx = ToolCallContext {
        session_id: session_id.unwrap_or_default().to_string(),
        project: project_id.map(str::to_string),
        workspace: workspace.root().to_string_lossy().into_owned(),
        ..Default::default()
    };

    // Take a cheap handle and drop the host mutex: a panel read waits on another
    // process (up to the call timeout), and holding the lock would stall every
    // other `/api` request that touches plugins.
    let panels: PluginPanelsHandle = {
        let host = state.plugins().lock().await;
        host.panels_handle()
    };
    let payload = panels
        .panel(&name, &panel, &ctx)
        .await
        .map_err(map_plugin_error)?;

    let fetched_ms = crate::state::now_ms();
    Ok(Json(json!({
        "title": payload.title,
        "data": payload.data,
        "version": fetched_ms,
        "fetched_ms": fetched_ms,
    })))
}

/// Map a plugin failure onto the documented HTTP error, exactly like
/// [`enable_plugin`] does: a wrong name is a 404, a plugin that is not usable is
/// a 400, anything else is a plugin-side failure.
fn map_plugin_error(error: kc_core::Error) -> ApiError {
    match error {
        kc_core::Error::NotFound(message) => ApiError::not_found(message),
        kc_core::Error::Config(message) => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}
