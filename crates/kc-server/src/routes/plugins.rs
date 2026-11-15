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
    Ok(Json(json!({
        "plugins": plugin_values(&state, plugins).await?,
    })))
}

/// One plugin's JSON, plus the tool-output budget that actually applies to its
/// results.
///
/// The number is resolved by the server, not the host, because it depends on the
/// live `limits` (the global default and the ceiling a grant may reach) as well
/// as on the operator's recorded decision. It is the same number the runtime
/// uses when it truncates, so what the operator sees here is what the model gets.
async fn plugin_values(
    state: &AppState,
    plugins: Vec<kc_core::plugin::PluginDiagnostics>,
) -> ApiResult<Vec<Value>> {
    let budgets = state.plugin_tool_output();
    let mut out = Vec::with_capacity(plugins.len());
    for diagnostics in plugins {
        let mut value = serde_json::to_value(&diagnostics)
            .map_err(|e| ApiError::internal(format!("could not serialize plugin: {e}")))?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "tool_output".into(),
                budgets.summary(&diagnostics.name).await,
            );
        }
        out.push(value);
    }
    Ok(out)
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
        "plugins": plugin_values(&state, diagnostics).await?,
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

// ---------------------------------------------------------------------------
// Per-plugin tool-output budgets
//
// `limits.max_tool_output_bytes` is the default for every plugin. One plugin can
// be given its own budget - higher or lower - and only the operator can give it:
// the agent files a request, the operator answers, and the answer is written to
// the database with who, when and why. See `crate::plugin_tool_output`.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolOutputRequestBody {
    /// The exact byte value being asked for.
    bytes: usize,
    /// Why this plugin needs more (or less) than the global default. Shown to the
    /// operator, stored with the answer, and part of the audit trail.
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolOutputDecisionBody {
    /// `grant` / `granted` / `allow` or `refuse` / `refused` / `deny`.
    decision: String,
    /// The granted value. Defaults to what was asked for; the operator may name a
    /// different number, which is how a grant narrows a request.
    #[serde(default)]
    bytes: Option<usize>,
    #[serde(default)]
    reason: String,
    /// Which surface answered. `web` from the browser, `cli-tty` from the CLI
    /// verb (which refuses to run without a terminal), `api` otherwise.
    #[serde(default)]
    channel: Option<String>,
}

/// `GET /api/plugins/:name/tool-output` — the budget in force, the answer on
/// record, a pending ask and the whole audit trail.
pub async fn get_tool_output(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let view = state.plugin_tool_output().view(&name).await?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        ApiError::internal(format!("could not serialize the tool-output record: {e}"))
    })?))
}

/// `POST /api/plugins/:name/tool-output/request` — file the ask.
///
/// This is the agent's half: it names the plugin, the exact bytes and the
/// reason, and grants **nothing**. Until the operator answers, the plugin keeps
/// the global default. A plugin whose budget the operator already answered
/// cannot be asked again here; only they can reopen it.
pub async fn request_tool_output(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: ToolOutputRequestBody =
        parse_required(&body, "POST /api/plugins/:name/tool-output/request")?;
    let view = state
        .plugin_tool_output()
        .request(&name, body.bytes, &body.reason)
        .await?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        ApiError::internal(format!("could not serialize the tool-output record: {e}"))
    })?))
}

/// `POST /api/plugins/:name/tool-output/decision` — the operator's answer.
///
/// The one path that creates a grant. It takes the same admin gate as
/// `POST /api/plugins/install` whenever this install has an admin credential, so
/// a shell cannot answer it on a hardened server; on a default loopback install
/// there is no credential to require, exactly as `PATCH /api/config` behaves.
pub async fn decide_tool_output(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    authorize_decision(&state, &headers)?;
    let body: ToolOutputDecisionBody =
        parse_required(&body, "POST /api/plugins/:name/tool-output/decision")?;
    let decision =
        kc_core::plugin::tool_output::Decision::parse(&body.decision).ok_or_else(|| {
            ApiError::bad_request(format!(
                "`decision` must be `grant` or `refuse`, not `{}`",
                body.decision
            ))
        })?;
    let channel = decision_channel(&headers, body.channel.as_deref());
    let view = state
        .plugin_tool_output()
        .decide(&name, decision, body.bytes, &body.reason, channel)
        .await?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        ApiError::internal(format!("could not serialize the tool-output record: {e}"))
    })?))
}

/// `DELETE /api/plugins/:name/tool-output` — forget the answer, so the operator
/// can be asked again (or asked something different). Operator-only, like the
/// decision itself.
pub async fn clear_tool_output(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Json<Value>> {
    authorize_decision(&state, &headers)?;
    let channel = decision_channel(&headers, None);
    let view = state.plugin_tool_output().clear(&name, channel).await?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        ApiError::internal(format!("could not serialize the tool-output record: {e}"))
    })?))
}

/// The admin gate, exactly like the install routes: recording a decision is a
/// security decision, so it uses the same credential an install would.
fn authorize_decision(state: &AppState, headers: &axum::http::HeaderMap) -> ApiResult<()> {
    let config = state.settings().snapshot();
    let presented = headers
        .get(crate::settings::ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    crate::settings::authorize_admin(&config, presented, "a plugin tool-output decision")
        .map_err(ApiError::from)
}

/// Which surface answered, for the audit record. Only the two operator surfaces
/// are named; anything else is the API, which the record says plainly.
fn decision_channel(headers: &axum::http::HeaderMap, declared: Option<&str>) -> &'static str {
    use crate::plugin_tool_output::{CHANNEL_API, CHANNEL_CLI, CHANNEL_TIMEOUT, CHANNEL_WEB};
    if declared.is_some_and(|value| value.trim() == CHANNEL_CLI) {
        return CHANNEL_CLI;
    }
    // The wait running out is not the operator answering, and the service forces
    // it to a refusal whatever the body says.
    if declared.is_some_and(|value| value.trim() == CHANNEL_TIMEOUT) {
        return CHANNEL_TIMEOUT;
    }
    let client = headers
        .get("x-karucode-client")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if client.trim() == CHANNEL_WEB || declared.is_some_and(|value| value.trim() == CHANNEL_WEB) {
        return CHANNEL_WEB;
    }
    CHANNEL_API
}
