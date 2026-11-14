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
use kc_plugin_host::PluginUiHandle;
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

/// `POST /api/plugins/:name/ui/actions/:action[?project=&session=]`
///
/// Run one UI action a plugin declared. The action id — not the contribution id —
/// names the handler, because two buttons may share one. The host refuses an
/// action no contribution declares, so a stale page cannot make an installed
/// plugin execute something it no longer offers.
///
/// The response is the plugin's own answer, already validated by the host:
///
/// ```json
/// {"ok": true, "message": "Checked 3 files", "refresh": ["todo"], "open_url": null}
/// ```
///
/// `refresh` names **panels** of this plugin the UI should re-read. `open_url` is
/// only ever an absolute `http`/`https` URL — the host drops anything else, and
/// the frontend checks again before opening a tab.
///
/// A UI action is not a tool call: it runs because a person clicked a control the
/// plugin itself contributed, so it does not go through the agent's approval
/// policy. It also does not feed the model, change the tool registry, or carry
/// markup — see `plugins` for the whole contract.
pub async fn run_ui_action(
    State(state): State<AppState>,
    Path((name, action)): Path<(String, String)>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let session_id = query_str(&query, "session");
    let project_id = query_str(&query, "project");

    // Same scoping as a panel read: an unknown project is a 404 before the plugin
    // is consulted, and an unknown session is a caller mistake rather than an
    // empty answer.
    let workspace = state.workspace_for_project(project_id)?;
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

    // Drop the host mutex before waiting on the plugin process.
    let ui: PluginUiHandle = {
        let host = state.plugins().lock().await;
        host.ui_handle()
    };
    let result = ui
        .action(&name, &action, &ctx)
        .await
        .map_err(map_plugin_error)?;
    Ok(Json(json!(result)))
}

pub async fn list_mcp(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let servers = {
        let mcp = state.mcp().lock().await;
        mcp.statuses().iter().map(mcp_summary).collect::<Vec<_>>()
    };
    Ok(Json(json!({ "servers": servers })))
}

/// `POST /api/mcp/reload` — re-read `[[mcp_servers]]` and reconcile the running
/// servers with it.
///
/// This is the no-restart path for adding, removing or changing an MCP server:
/// new names are started, names that disappeared are stopped and dropped, and an
/// entry whose fields changed is closed and started again. Servers that did not
/// change are left running untouched, so this is safe to call on a hunch (and the
/// watcher calls it on a timer).
///
/// The body is the report (`added`, `removed`, `restarted`, `retried`,
/// `unchanged`) plus the live statuses, so a caller does not need a second
/// request.
pub async fn reload_mcp(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let report = state.reload_mcp().await;
    let servers = {
        let mcp = state.mcp().lock().await;
        mcp.statuses().iter().map(mcp_summary).collect::<Vec<_>>()
    };
    Ok(Json(json!({ "report": report, "servers": servers })))
}

/// `POST /api/mcp/:name/reload` — replace one server's process/connection.
///
/// The "I rebuilt the server binary" / "I edited its entry" path. A name that is
/// configured but not yet running is started, so this can also be the first
/// start; an unknown name is `404`.
pub async fn reload_mcp_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let status = state
        .reload_mcp_server(&name)
        .await
        .map_err(map_mcp_error)?;
    serde_json::to_value(status)
        .map(Json)
        .map_err(|e| ApiError::internal(format!("could not serialize MCP server: {e}")))
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

/// Map an MCP failure onto the documented HTTP error: a name that is not
/// configured is a 404, a bad entry is a 400, anything else is a server-side
/// failure.
fn map_mcp_error(error: kc_core::Error) -> ApiError {
    match error {
        kc_core::Error::NotFound(message) => ApiError::not_found(message),
        kc_core::Error::Config(message) => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}
