//! `/api/projects` — the workspaces the agent may work in.
//!
//! A project is a directory plus a display name. Creating one canonicalises the
//! path (so two spellings of the same directory collide instead of silently
//! becoming two projects), refuses a path that is not an existing directory,
//! refuses a path outside the configured project roots, and registers it in the
//! in-memory [`crate::state::ProjectRegistry`] that the runtime uses to confine
//! runs.
//!
//! A project's directory becomes the confinement root for every file tool in its
//! sessions, so the path is checked before it is accepted. By default there is
//! **no allowlist**: any existing directory is a valid project, which is what a
//! single-operator install wants - pinning the list to the default workspace
//! made every other checkout ("add /home/me/projects/thing") a `400`. Setting
//! `config.project_roots` (`KARUCODE_PROJECT_ROOTS`, colon-separated) turns that
//! into a real allowlist for a shared or remote-reachable deployment, and `/`
//! is refused either way so a project can never make the whole filesystem the
//! confinement root.
//!
//! Deleting a project refuses while it still holds sessions unless
//! `?force=true`, in which case those sessions (and their history) are deleted
//! with it. That is the only destructive path here and the UI asks explicitly.
//!
//! `GET /api/projects/:id/favicon` serves the project's own `favicon.ico` (see
//! [`kc_tools::favicon`]) so the rail can draw the real icon instead of initials.
//! It is a binary `image/*` response, not JSON: the alternative would be a base64
//! data URL inside `GET /api/projects`, which would grow every list response by
//! a third of the icon's size and force it through the JSON body limit.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_bool, query_str, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
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

    let roots = allowed_project_roots(&state.settings().snapshot());
    let path = resolve_project_path(&body.path, &roots)?;
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

/// `GET /api/projects/:id/favicon?max_bytes=<n>`
///
/// The project's own `favicon.ico`, or an empty `204 No Content` when it has
/// none — which is the common case and is what makes the rail fall back to
/// initials. The search (which files are considered, which are ignored, how deep
/// it goes) lives in [`kc_tools::favicon`].
///
/// An unknown project is still a `404`: that is a caller mistake, unlike a
/// project that simply ships no icon. "Found but too large" is a `204` as well,
/// because the outcome for the rail is identical.
///
/// `max_bytes` lets the rail say how much icon it is willing to draw; the default
/// is also the ceiling, so a caller cannot raise it.
pub async fn project_favicon(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Response> {
    let limit = match query_str(&query, "max_bytes") {
        None => kc_tools::favicon::MAX_ICON_BYTES,
        Some(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                ApiError::bad_request("query parameter `max_bytes` must be a positive integer")
            })?
            .min(kc_tools::favicon::MAX_ICON_BYTES),
    };

    let workspace = state.workspace_for_project(Some(&id))?;
    let icon = tokio::task::spawn_blocking(move || kc_tools::favicon::load(&workspace))
        .await
        .map_err(|e| ApiError::internal(format!("favicon task failed: {e}")))?;
    let Some(icon) = icon.filter(|icon| icon.bytes.len() as u64 <= limit) else {
        // No icon is routine, not an error: a 204 keeps the browser console
        // clean instead of logging a `Failed to load resource`.
        return Ok(crate::app::no_content());
    };

    tracing::debug!(project = %id, path = %icon.path, bytes = icon.bytes.len(), "serving project icon");
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(icon.content_type),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=300"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            // The bytes are sniffed, so a mislabelled file cannot be replayed as
            // HTML even if the content type were ever wrong.
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
            ),
        ],
        icon.bytes,
    )
        .into_response())
}

/// Absolute, existing, canonical directory. The canonical form is what gets
/// stored and what `Workspace` confines tools to, so a symlinked path and its
/// target are the same project.
///
/// `roots` is the allowlist and is **empty by default**, which means "no
/// allowlist": any existing directory below the filesystem root is accepted.
/// When `config.project_roots` is set, the path must be inside one of them. Both
/// sides are canonicalised before the comparison, and the comparison is
/// `Path::starts_with`, which is component-wise - `/srv/app-evil` is not inside
/// `/srv/app`, and no string prefix trick can fake it.
fn resolve_project_path(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, ApiError> {
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
    // Refuse the filesystem root explicitly: it is technically "inside" nothing
    // useful and would make every file tool able to read the whole disk.
    if canonical.parent().is_none() {
        return Err(ApiError::bad_request(format!(
            "`{}` is the filesystem root; a project must be a directory below it",
            canonical.display()
        )));
    }
    ensure_inside_roots(&canonical, roots)?;
    // The same containment rules the tools use (readable, not a symlink escape
    // that lands outside itself, etc.).
    Workspace::new(&canonical).map_err(ApiError::from)?;
    Ok(canonical)
}

/// The allowlist for [`resolve_project_path`]: `config.project_roots` as
/// configured, and **empty** when it is not set - empty means "no allowlist",
/// not "the default workspace only". Restricting project creation to the default
/// workspace was a limiting factor with no upside on a single-operator install:
/// the obvious second project (`/home/me/projects/other-repo`) is a sibling of
/// the workspace, so every attempt failed with "outside the allowed project
/// roots" until the operator edited the environment. Roots are canonicalised so
/// a symlinked or non-normalised entry still matches; an entry that cannot be
/// canonicalised (it does not exist) is kept as written and simply never
/// matches, which fails closed.
fn allowed_project_roots(config: &kc_core::config::Config) -> Vec<PathBuf> {
    config
        .project_roots
        .iter()
        .map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
        .collect()
}

/// Component-wise containment check. `Path::starts_with` compares whole path
/// components, so `/srv/app-evil` is correctly *not* inside `/srv/app`. An empty
/// `roots` is the default "no allowlist" case and accepts everything the caller
/// has already validated (absolute, existing, not `/`).
fn ensure_inside_roots(canonical: &std::path::Path, roots: &[PathBuf]) -> Result<(), ApiError> {
    if roots.is_empty() || roots.iter().any(|root| canonical.starts_with(root)) {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "`{}` is outside the allowed project roots ({}); set KARUCODE_PROJECT_ROOTS \
         (colon-separated) or `project_roots` in the config file to allow it, or unset \
         it to allow any directory",
        canonical.display(),
        describe_roots(roots)
    )))
}

fn describe_roots(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return "none configured".to_string();
    }
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory tree: `root/workspace` and `root/outside`.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "kc-projects-test-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(root.join("workspace")).expect("workspace dir");
            std::fs::create_dir_all(root.join("outside")).expect("outside dir");
            Self { root }
        }

        fn workspace(&self) -> PathBuf {
            self.root
                .join("workspace")
                .canonicalize()
                .expect("canonical")
        }

        fn outside(&self) -> PathBuf {
            self.root.join("outside").canonicalize().expect("canonical")
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn config_with_roots(
        workspace: &std::path::Path,
        roots: Vec<PathBuf>,
    ) -> kc_core::config::Config {
        kc_core::config::Config {
            workspace_root: workspace.to_path_buf(),
            project_roots: roots,
            ..kc_core::config::Config::default()
        }
    }

    #[test]
    fn the_filesystem_root_is_refused() {
        // Even with no allowlist: `/` as a project would make every file tool
        // able to read the whole disk.
        let tree = TempTree::new();
        let roots = allowed_project_roots(&config_with_roots(&tree.workspace(), Vec::new()));
        assert!(roots.is_empty(), "no roots configured means no allowlist");
        let error = resolve_project_path("/", &roots).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("filesystem root"),
            "unhelpful message: {}",
            error.message
        );
    }

    #[test]
    fn no_configured_roots_means_every_directory_is_allowed() {
        // The regression this guards: with `KARUCODE_PROJECT_ROOTS` unset the
        // allowlist used to be "the default workspace only", so the obvious
        // second project - a sibling of the workspace - was a 400.
        let tree = TempTree::new();
        let config = config_with_roots(&tree.workspace(), Vec::new());
        let roots = allowed_project_roots(&config);
        assert!(roots.is_empty(), "{roots:?}");
        let outside = tree.outside().to_string_lossy().to_string();
        let resolved = resolve_project_path(&outside, &roots)
            .expect("without an allowlist any existing directory is a project");
        assert_eq!(resolved, tree.outside());
    }

    #[test]
    fn a_path_inside_the_workspace_is_allowed() {
        let tree = TempTree::new();
        let roots = allowed_project_roots(&config_with_roots(&tree.workspace(), Vec::new()));
        let inside = tree.workspace().to_string_lossy().to_string();
        let resolved = resolve_project_path(&inside, &roots).expect("workspace is allowed");
        assert_eq!(resolved, tree.workspace());
    }

    #[test]
    fn a_subdirectory_of_the_workspace_is_allowed() {
        let tree = TempTree::new();
        let nested = tree.workspace().join("project-two");
        std::fs::create_dir_all(&nested).expect("nested dir");
        let roots = allowed_project_roots(&config_with_roots(&tree.workspace(), Vec::new()));
        let resolved = resolve_project_path(&nested.to_string_lossy(), &roots)
            .expect("a subdirectory of the workspace is allowed");
        assert_eq!(resolved, nested.canonicalize().unwrap());
    }

    #[test]
    fn a_path_outside_a_configured_root_is_refused() {
        let tree = TempTree::new();
        // Setting the workspace as the only root turns the allowlist back on.
        let config = config_with_roots(&tree.workspace(), vec![tree.workspace()]);
        let roots = allowed_project_roots(&config);
        let outside = tree.outside().to_string_lossy().to_string();
        let error = resolve_project_path(&outside, &roots).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.message.contains("outside the allowed project roots"),
            "unhelpful message: {}",
            error.message
        );
        // The message must say how to widen - or remove - the allowlist.
        assert!(error.message.contains("KARUCODE_PROJECT_ROOTS"));
    }

    #[test]
    fn a_path_inside_a_configured_root_is_allowed() {
        let tree = TempTree::new();
        let config = config_with_roots(&tree.workspace(), vec![tree.outside()]);
        let roots = allowed_project_roots(&config);
        let outside = tree.outside().to_string_lossy().to_string();
        let resolved = resolve_project_path(&outside, &roots).expect("configured root is allowed");
        assert_eq!(resolved, tree.outside());
    }

    #[test]
    fn a_sibling_with_a_shared_name_prefix_is_not_inside() {
        // `Path::starts_with` is component-wise: `/tmp/x/app-evil` must not be
        // accepted just because it shares a string prefix with `/tmp/x/app`.
        let tree = TempTree::new();
        let evil = tree.root.join("workspace-evil");
        std::fs::create_dir_all(&evil).expect("evil dir");
        let config = config_with_roots(&tree.workspace(), vec![tree.workspace()]);
        let roots = allowed_project_roots(&config);
        let error = resolve_project_path(&evil.to_string_lossy(), &roots).unwrap_err();
        assert!(error.message.contains("outside the allowed project roots"));
    }

    #[test]
    fn a_configured_root_narrows_the_allowlist() {
        let tree = TempTree::new();
        let config = config_with_roots(&tree.workspace(), vec![tree.outside()]);
        let roots = allowed_project_roots(&config);
        // The workspace is not implicitly allowed once roots are set...
        let workspace = tree.workspace().to_string_lossy().to_string();
        assert!(resolve_project_path(&workspace, &roots).is_err());
        // ...but the configured root is, and `/` still is not.
        assert!(resolve_project_path(&tree.outside().to_string_lossy(), &roots).is_ok());
        assert!(resolve_project_path("/", &roots).is_err());
    }

    #[test]
    fn an_unusable_path_is_refused_with_or_without_an_allowlist() {
        let tree = TempTree::new();
        for roots in [
            Vec::new(),
            allowed_project_roots(&config_with_roots(
                &tree.workspace(),
                vec![tree.workspace()],
            )),
        ] {
            // Relative.
            let error = resolve_project_path("relative/dir", &roots).unwrap_err();
            assert!(error.message.contains("absolute"), "{}", error.message);
            // Missing.
            let missing = tree.root.join("not-here");
            let error = resolve_project_path(&missing.to_string_lossy(), &roots).unwrap_err();
            assert!(error.message.contains("could not use"), "{}", error.message);
            // A file, not a directory.
            let file = tree.root.join("file.txt");
            std::fs::write(&file, "x").expect("file");
            let error = resolve_project_path(&file.to_string_lossy(), &roots).unwrap_err();
            assert!(
                error.message.contains("not a directory"),
                "{}",
                error.message
            );
            // NUL bytes.
            let error = resolve_project_path("/tmp/a\0b", &roots).unwrap_err();
            assert!(error.message.contains("NUL"), "{}", error.message);
        }
    }
}
