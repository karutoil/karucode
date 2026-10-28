//! `/api/projects` — the workspaces the agent may work in.
//!
//! A project is a directory plus a display name. Creating one canonicalises the
//! path (so two spellings of the same directory collide instead of silently
//! becoming two projects), refuses a path that is not an existing directory, and
//! registers it in the in-memory [`crate::state::ProjectRegistry`] that the
//! runtime uses to confine runs.
//!
//! Deleting a project refuses while it still holds sessions unless
//! `?force=true`, in which case those sessions (and their history) are deleted
//! with it. That is the only destructive path here and the UI asks explicitly.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_bool, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::ids::ProjectId;
use kc_core::paths::Workspace;
use kc_core::store::Project;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Enough for a real multi-repo setup; small enough that the rail stays usable
/// and `GET /api/projects` stays a cheap document.
const MAX_PROJECTS: usize = 64;
const MAX_NAME_CHARS: usize = 80;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    /// Display name; defaults to the directory's basename.
    #[serde(default)]
    name: Option<String>,
    path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    name: String,
}

pub async fn list_projects(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let projects = state.store().list_projects().await?;
    Ok(Json(json!({
        "projects": projects,
        "workspace": state.workspace().root().to_string_lossy(),
    })))
}

pub async fn create_project(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let body: CreateBody = parse_required(&body, "POST /api/projects")?;
    let existing = state.store().list_projects().await?;
    if existing.len() >= MAX_PROJECTS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_PROJECTS} projects are supported"
        )));
    }

    let path = resolve_project_path(&body.path)?;
    let path_text = path.to_string_lossy().to_string();
    if let Some(existing) = existing.iter().find(|project| project.path == path_text) {
        return Err(ApiError::bad_request(format!(
            "`{path_text}` is already the project `{}`",
            existing.name
        )));
    }

    let name = project_name(body.name.as_deref(), &path);
    let id = ProjectId::new();
    let project = state
        .store()
        .create_project(id.as_str(), &name, &path_text)
        .await?;
    // Keep the runtime's view in step with the row. If the directory became
    // unusable between the check and here, undo the row rather than leaving a
    // project that can never resolve.
    if let Err(error) = state.projects().insert(id.as_str(), &path) {
        let _ = state.store().delete_project(id.as_str()).await;
        return Err(ApiError::from(error));
    }
    Ok(Json(json!({ "project": project })))
}

pub async fn rename_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let project = state
        .store()
        .get_project(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no project with id `{id}`")))?;
    let body: RenameBody = parse_required(&body, "PATCH /api/projects/:id")?;
    let name = validate_name(&body.name)?;
    state.store().rename_project(&id, &name).await?;
    Ok(Json(json!({
        "project": Project {
            name,
            updated_at: crate::state::now_ms(),
            ..project
        }
    })))
}

pub async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let force = query_bool(&query, "force")?;
    let project = state
        .store()
        .get_project(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no project with id `{id}`")))?;
    let sessions = state.store().list_sessions_for_project(&id).await?;
    if !sessions.is_empty() && !force {
        return Err(ApiError::bad_request(format!(
            "project `{}` still has {} session(s); pass ?force=true to delete them with it",
            project.name,
            sessions.len()
        )));
    }
    for session in &sessions {
        if state.runtime().is_running(&session.id) {
            return Err(ApiError::bad_request(format!(
                "session `{}` is running; stop it before deleting the project",
                session.id
            )));
        }
    }
    let deleted_sessions = sessions.len();
    for session in &sessions {
        state.store().delete_session(&session.id).await?;
        state.runtime().clear_session_allows(&session.id);
    }
    state.store().delete_project(&id).await?;
    state.projects().remove(&id);
    Ok(Json(json!({
        "deleted": true,
        "deleted_sessions": deleted_sessions,
    })))
}

/// Absolute, existing, canonical directory. The canonical form is what gets
/// stored and what `Workspace` confines tools to, so a symlinked path and its
/// target are the same project.
fn resolve_project_path(raw: &str) -> Result<PathBuf, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("path is required"));
    }
    if trimmed.contains('\0') {
        return Err(ApiError::bad_request("path may not contain NUL bytes"));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(ApiError::bad_request(format!(
            "`{trimmed}` is not an absolute path; use the full path to the project directory"
        )));
    }
    let canonical = std::fs::canonicalize(&path).map_err(|e| {
        ApiError::bad_request(format!(
            "could not use `{trimmed}` as a project directory: {e}"
        ))
    })?;
    if !canonical.is_dir() {
        return Err(ApiError::bad_request(format!(
            "`{}` is a file, not a directory",
            canonical.display()
        )));
    }
    // The same containment rules the tools use (readable, not a symlink escape
    // that lands outside itself, etc.).
    Workspace::new(&canonical).map_err(ApiError::from)?;
    Ok(canonical)
}

fn project_name(raw: Option<&str>, path: &std::path::Path) -> String {
    if let Some(raw) = raw {
        if let Ok(name) = validate_name(raw) {
            return name;
        }
    }
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = base.trim();
    if base.is_empty() {
        "Project".to_string()
    } else {
        base.chars().take(MAX_NAME_CHARS).collect()
    }
}

fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("name may not be empty"));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "name may not contain control characters",
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::bad_request(format!(
            "name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_string())
}
