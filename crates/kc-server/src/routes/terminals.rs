//! `GET/POST /api/terminals`, `PATCH/DELETE /api/terminals/:id`,
//! `POST /api/terminals/:id/restart` and the attach websocket.
//!
//! The behaviour, the limits and the environment the shell gets are documented
//! on [`crate::terminals`]; this module is the HTTP surface over it.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_body, query_usize, QueryMap};
use crate::state::AppState;
use crate::terminals::{self, Terminal};
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use kc_core::store::TerminalSession;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Body of `POST /api/terminals`. Every field is optional: an empty body starts
/// a shell in the default workspace with a derived title.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    /// Tab label. Empty means the derived one ("karucode-1").
    #[serde(default)]
    title: Option<String>,
    /// Directory to start in. Must resolve inside a workspace root; the default
    /// is the project's directory, then the default workspace.
    #[serde(default)]
    cwd: Option<String>,
    /// Project to start in, and to record on the row as a label.
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
}

/// Body of `PATCH /api/terminals/:id`.
///
/// `title` is a three-way field, so it uses the standard
/// `Option<Option<T>>` + `double_option` spelling: **absent** means "leave the
/// name alone", `null`/`""` means "back to the derived default", and a string is
/// a rename. A plain `Option<String>` cannot tell the first two apart, and
/// collapsing them would make a `PATCH` that only touches something else rename
/// the tab.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    /// New label. `null` or an empty string restores the derived default.
    #[serde(default, deserialize_with = "double_option")]
    title: Option<Option<String>>,
}

fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

pub async fn list_terminals(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(terminals::list(&state).await?))
}

pub async fn create_terminal(
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CreateBody = parse_body(&body)?;
    let settings = state.settings().snapshot().terminals;
    if !settings.enabled {
        return Err(ApiError::forbidden(
            "the terminal is disabled on this server (terminals.enabled, or KARUCODE_TERMINALS=0)",
        ));
    }

    let store = state.store();
    // The cap is checked against the *rows*, not the live processes: closing a
    // shell must not free a slot that a restart would immediately take back, and
    // a terminal whose process was reaped still owns its tab.
    let existing = store.list_terminals().await?;
    let budget = settings.terminal_budget();
    if existing.len() >= budget {
        return Err(ApiError::bad_request(format!(
            "this install already has {budget} terminals, which is the `terminals.max_terminals` \
             limit; close one before creating another"
        )));
    }

    // Resolve the directory. A terminal is not confined the way a file tool is,
    // but the directory it *starts* in must still be one the project registry
    // knows, so `{"cwd":"/etc"}` cannot be used to place a tab outside every
    // workspace.
    let (cwd, project_id) = resolve_start_dir(&state, body.project.as_deref(), body.cwd.as_deref())?;

    let id = kc_core::ids::TerminalId::new().as_str().to_string();
    let position = existing
        .iter()
        .map(|row| row.position)
        .max()
        .unwrap_or(0)
        + 1;
    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| TerminalSession::default_title(&cwd, position));
    let row = TerminalSession {
        id: id.clone(),
        title,
        cwd: cwd.clone(),
        project_id: project_id.clone(),
        position,
        created_at: 0,
        updated_at: 0,
        last_used_at: 0,
    };
    let created = store.create_terminal(&row).await?;

    // Start the shell eagerly so a create that cannot spawn (a bad `shell`, a
    // deleted directory) fails here with a clear message instead of as a broken
    // websocket the first time the tab is opened.
    let cols = body.cols.unwrap_or(80);
    let rows = body.rows.unwrap_or(24);
    let terminal = match Terminal::spawn(&created.id, &created, &settings, cols, rows) {
        Ok(terminal) => terminal,
        Err(error) => {
            // The row is rolled back: a terminal that can never start is not a
            // tab worth keeping.
            let _ = store.delete_terminal(&created.id).await;
            return Err(ApiError::from(error));
        }
    };
    state.terminals().insert(Arc::clone(&terminal));
    Ok(Json(json!({
        "terminal": terminals::terminal_json_value(&created, Some(&terminal)),
    })))
}

pub async fn rename_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: RenameBody = parse_body(&body)?;
    let store = state.store();
    if store.get_terminal(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("no terminal with id `{id}`")));
    }
    // `{"title": null}` and `{"title": ""}` both mean "back to the default",
    // which is the documented way to undo a rename.
    let title = match &body.title {
        Some(Some(value)) if !value.trim().is_empty() => Some(value.trim()),
        Some(_) => None,
        // No `title` key at all is a no-op, not a reset.
        None => {
            let row = store.get_terminal(&id).await?.expect("checked above");
            return Ok(Json(json!({
                "terminal": terminals::terminal_json_value(
                    &row,
                    state.terminals().get(&id).as_deref(),
                ),
            })));
        }
    };
    store.rename_terminal(&id, title).await?;
    let row = store.get_terminal(&id).await?.expect("just renamed");
    Ok(Json(json!({
        "terminal": terminals::terminal_json_value(&row, state.terminals().get(&id).as_deref()),
    })))
}

pub async fn delete_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let store = state.store();
    if store.get_terminal(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("no terminal with id `{id}`")));
    }
    // Kill the shell before dropping the row, so a delete can never leave a
    // process with no tab pointing at it.
    if let Some(terminal) = state.terminals().remove(&id) {
        terminal.shutdown();
    }
    store.delete_terminal(&id).await?;
    Ok(Json(json!({ "deleted": id })))
}

/// Kill the shell and start a new one in the same directory, keeping the id,
/// the title and the tab position.
pub async fn restart_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let store = state.store();
    let row = store
        .get_terminal(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no terminal with id `{id}`")))?;
    let settings = state.settings().snapshot().terminals;
    if !settings.enabled {
        return Err(ApiError::forbidden(
            "the terminal is disabled on this server (terminals.enabled, or KARUCODE_TERMINALS=0)",
        ));
    }
    if let Some(previous) = state.terminals().remove(&id) {
        previous.shutdown();
    }
    let cols = query_usize(&query, "cols")?
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(80);
    let rows = query_usize(&query, "rows")?
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(24);
    let terminal = Terminal::spawn(&row.id, &row, &settings, cols, rows)?;
    state.terminals().insert(Arc::clone(&terminal));
    let _ = store.touch_terminal(&id).await;
    Ok(Json(json!({
        "terminal": terminals::terminal_json_value(&row, Some(&terminal)),
    })))
}

pub async fn terminal_socket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    terminals::socket(State(state), Path(id), Query(query), headers, ws).await
}

/// Resolve the directory a new terminal starts in.
///
/// The rules, in order: a `cwd` that resolves inside a workspace root wins; then
/// the project's directory; then the default workspace. Anything else is a
/// `404`/`400` naming what was asked for, because a shell started in a
/// half-resolved directory is worse than a create that failed.
fn resolve_start_dir(
    state: &AppState,
    project: Option<&str>,
    cwd: Option<&str>,
) -> ApiResult<(String, Option<String>)> {
    let workspace = state.workspace_for_project(project)?;
    let project_id = project.map(str::to_string).or_else(|| {
        // No explicit project: record the one that owns the workspace the shell
        // starts in, so the tab strip can group by project later.
        state
            .projects()
            .roots()
            .into_iter()
            .find(|(_, root)| *root == workspace.root())
            .map(|(id, _)| id)
    });

    let Some(cwd) = cwd else {
        return Ok((workspace.root().to_string_lossy().into_owned(), project_id));
    };
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return Ok((workspace.root().to_string_lossy().into_owned(), project_id));
    }
    let resolved = workspace
        .resolve_existing(trimmed)
        .map_err(|error| ApiError::from(error))?;
    if !resolved.is_dir() {
        return Err(ApiError::bad_request(format!(
            "`cwd` must name a directory, and `{trimmed}` is not one"
        )));
    }
    Ok((resolved.to_string_lossy().into_owned(), project_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminals::{origin_verdict, OriginVerdict, TerminalRegistry};
    use kc_core::config::TerminalConfig;

    #[test]
    fn a_create_body_defaults_to_the_workspace() {
        let body: CreateBody = serde_json::from_str("{}").unwrap();
        assert!(body.title.is_none());
        assert!(body.cwd.is_none());
        assert!(body.project.is_none());
        // Deny-unknown-fields keeps a typo from silently doing nothing.
        assert!(serde_json::from_str::<CreateBody>(r#"{"titel":"x"}"#).is_err());
    }

    #[test]
    fn a_title_reset_accepts_null_and_empty() {
        let null: RenameBody = serde_json::from_str(r#"{"title":null}"#).unwrap();
        assert_eq!(null.title, Some(None));
        let empty: RenameBody = serde_json::from_str(r#"{"title":""}"#).unwrap();
        assert_eq!(empty.title, Some(Some(String::new())));
        // No key at all is a no-op, which is different from a reset.
        let absent: RenameBody = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.title, None);
    }

    #[test]
    fn terminal_settings_default_to_enabled_with_a_budget() {
        let settings = TerminalConfig::default();
        assert!(settings.enabled);
        assert_eq!(settings.terminal_budget(), 16);
        assert_eq!(settings.replay_capacity(), 256 * 1024);
        // `/bin/sh` at worst, `$SHELL` when set.
        let program = settings.shell_program();
        assert!(!program.is_empty());
    }

    /// A websocket handshake is not covered by the CSRF middleware, so the
    /// `Origin` check is the only thing standing between a hostile page and a
    /// shell. A same-host origin passes; anything else is refused.
    #[test]
    fn the_origin_check_refuses_a_cross_site_handshake() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:8787".parse().unwrap());
        // Same origin: the browser sends the host it dialled.
        headers.insert("origin", "http://127.0.0.1:8787".parse().unwrap());
        assert!(matches!(origin_verdict(&headers), OriginVerdict::SameOrigin));
        // A different host is refused outright.
        headers.insert("origin", "https://evil.example".parse().unwrap());
        assert!(matches!(origin_verdict(&headers), OriginVerdict::Mismatch(..)));
        // A different port is a different origin too.
        headers.insert("origin", "http://127.0.0.1:9999".parse().unwrap());
        assert!(matches!(origin_verdict(&headers), OriginVerdict::Mismatch(..)));
        // A non-browser client sends no `Origin`; the host guard covers it.
        headers.remove("origin");
        assert!(matches!(origin_verdict(&headers), OriginVerdict::NotABrowser));
        // A malformed `Origin` is refused, never treated as absent.
        headers.insert("origin", "not a url".parse().unwrap());
        assert!(matches!(origin_verdict(&headers), OriginVerdict::Mismatch(..)));
    }

    /// The registry starts empty: a restart is a fresh process table, and that
    /// is exactly why the durable half lives in SQLite.
    #[test]
    fn a_new_registry_has_no_live_shells() {
        let registry = TerminalRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.get("t_anything").is_none());
        assert!(registry.reap_idle(60).is_empty());
    }
}