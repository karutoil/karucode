//! Workspace explorer (directory tree, file contents) and attachment uploads.
//!
//! The read endpoints and the upload both resolve through
//! [`kc_core::Workspace`], so traversal and symlink escapes are refused with a
//! 400 `path_escape` before any I/O.

use crate::error::{ApiError, ApiResult};
use crate::routes::{query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use kc_core::error::Error;
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Where composer attachments are written, relative to the project root. The
/// directory lives *inside* the workspace on purpose: the file tools are
/// confined to the workspace, so this is the only place the agent can read an
/// uploaded file from.
pub const UPLOAD_ROOT: &str = ".karucode/attachments";

/// Hard ceiling for one upload. The body is buffered in memory, so this is also
/// the worst-case allocation for a single request.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENT_NAME_BYTES: usize = 160;
const MAX_SESSION_ID_BYTES: usize = 64;
const MAX_CONTENT_TYPE_BYTES: usize = 128;
/// `name`, `name-1`, … `name-N`; exhausting this is a 409 rather than a hang.
const COLLISION_ATTEMPTS: u32 = 1000;

const DEFAULT_DEPTH: usize = 1;
const MAX_DEPTH: usize = 6;
const MAX_TREE_ENTRIES: usize = 2000;
/// Directories that are never useful in a UI listing: build output, dependency
/// trees and VCS internals. They are skipped *before* the entry budget is spent,
/// so a project is not truncated down to `.git` objects and `target/` artifacts.
/// Naming one explicitly (`path: ".git"`) still lists it, because the filter
/// only applies while descending.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".next",
    ".turbo",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    "coverage",
    ".idea",
];
/// Hard ceiling for a single file response, independent of the configurable
/// read limit, so a huge file cannot exhaust memory.
const HARD_READ_LIMIT: usize = 16 * 1024 * 1024;

pub async fn tree(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let path = query_str(&query, "path").unwrap_or(".").to_string();
    let depth = query_usize(&query, "depth")?
        .unwrap_or(DEFAULT_DEPTH)
        .clamp(DEFAULT_DEPTH, MAX_DEPTH);
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;

    let value = tokio::task::spawn_blocking(move || tree_blocking(&workspace, &path, depth))
        .await
        .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;
    Ok(Json(value))
}

fn tree_blocking(workspace: &Workspace, path: &str, depth: usize) -> Result<Value, Error> {
    let root = workspace.resolve_existing(path)?;
    if !root.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{path}` is not a directory; use /api/workspace/file for a file"
        )));
    }
    let mut entries = Vec::new();
    walk(workspace, &root, depth, &mut entries);
    Ok(json!({
        "path": workspace.relative(&root),
        "entries": entries,
    }))
}

/// Depth-first listing, directories first, bounded in count. Unreadable
/// subdirectories are skipped instead of failing the whole tree, and
/// [`IGNORED_DIRS`] never enters the listing at all.
fn walk(workspace: &Workspace, dir: &Path, depth: usize, out: &mut Vec<Value>) {
    if depth == 0 || out.len() >= MAX_TREE_ENTRIES {
        return;
    }
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(e) => {
            tracing::debug!(path = %dir.display(), "skipping unreadable directory: {e}");
            return;
        }
    };
    let mut items: Vec<(bool, String, std::path::PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        // Do not follow symlinked directories: they could point outside the
        // workspace and recursion could loop.
        let is_dir = file_type.is_dir();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if is_dir && IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }
        items.push((is_dir, name, entry.path()));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    for (is_dir, _name, full) in items {
        if out.len() >= MAX_TREE_ENTRIES {
            return;
        }
        let name = full
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = if is_dir {
            0
        } else {
            std::fs::symlink_metadata(&full)
                .map(|m| m.len())
                .unwrap_or(0)
        };
        out.push(json!({
            "name": name,
            "path": workspace.relative(&full),
            "kind": if is_dir { "dir" } else { "file" },
            "size": size,
        }));
        if is_dir {
            walk(workspace, &full, depth - 1, out);
        }
    }
}

pub async fn file(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let path = query_str(&query, "path")
        .ok_or_else(|| ApiError::bad_request("query parameter `path` is required"))?
        .to_string();
    // Reads are confined to the same project the tree/git endpoints use, so a
    // file opened from a project's tree is read from that project.
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let limit = state
        .settings()
        .limits()
        .await
        .max_file_read_bytes
        .clamp(1024, HARD_READ_LIMIT);

    let value = tokio::task::spawn_blocking(move || file_blocking(&workspace, &path, limit))
        .await
        .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;
    Ok(Json(value))
}

fn file_blocking(workspace: &Workspace, path: &str, limit: usize) -> Result<Value, Error> {
    let abs = workspace.resolve_existing(path)?;
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(abs.clone(), e))?;
    if meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{path}` is a directory; use /api/workspace/tree"
        )));
    }
    let size = meta.len();
    let mut file = std::fs::File::open(&abs).map_err(|e| Error::io(abs.clone(), e))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io(abs.clone(), e))?;
    let truncated = bytes.len() > limit || size > limit as u64;
    bytes.truncate(limit);

    let binary = is_binary(&bytes);
    let content = if binary {
        String::new()
    } else {
        match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                // Not valid UTF-8: report it as binary rather than mangling it.
                return Ok(json!({
                    "path": workspace.relative(&abs),
                    "content": "",
                    "size": size,
                    "truncated": truncated,
                    "binary": true,
                    "language": Value::Null,
                }));
            }
        }
    };

    Ok(json!({
        "path": workspace.relative(&abs),
        "content": content,
        "size": size,
        "truncated": truncated,
        "binary": binary,
        "language": language_for(&abs),
    }))
}

/// `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]`
///
/// The body is the raw file (`Content-Type` is that of the file, or omitted);
/// JSON and `multipart/form-data` bodies are rejected because storing them
/// verbatim is never what a client means. The file lands in
/// `<project>/.karucode/attachments/<session>/`, so it stays readable by the
/// workspace-confined file tools. An existing name is never overwritten: the
/// response reports the deduplicated name it actually used.
pub async fn upload(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let session_id = query_str(&query, "session")
        .ok_or_else(|| {
            ApiError::bad_request(
                "query parameter `session` is required (the session the file belongs to)",
            )
        })?
        .to_string();
    validate_session_id(&session_id)?;

    let name = query_str(&query, "name")
        .ok_or_else(|| {
            ApiError::bad_request(
                "query parameter `name` is required (the file name, without directories)",
            )
        })?
        .to_string();
    let name = validate_attachment_name(&name)?;
    let content_type = attachment_content_type(&headers)?;

    // Unknown sessions cannot own files; the session also decides which project
    // the upload belongs to when `project` is omitted.
    let session = state
        .store()
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` does not exist")))?;
    let project_id = query_str(&query, "project")
        .map(str::to_string)
        .or_else(|| session.project_id.clone());
    let workspace = state.workspace_for_project(project_id.as_deref())?;

    // An honest `Content-Length` is rejected before a byte is buffered. A
    // missing or lying one is caught by the bounded read below.
    if let Some(declared) = content_length(&headers)? {
        if declared > MAX_UPLOAD_BYTES as u64 {
            return Err(payload_too_large(declared));
        }
    }
    let bytes = match axum::body::to_bytes(body, MAX_UPLOAD_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) if is_length_limit(&err) => {
            return Err(payload_too_large(MAX_UPLOAD_BYTES as u64 + 1));
        }
        Err(err) => {
            return Err(ApiError::bad_request(format!(
                "could not read the request body: {err}"
            )))
        }
    };
    if bytes.is_empty() {
        return Err(ApiError::bad_request(
            "the request body is empty; send the file bytes as the raw request body",
        ));
    }

    let root = workspace.root().to_string_lossy().to_string();
    let size = bytes.len() as u64;
    let stored =
        tokio::task::spawn_blocking(move || store_upload(&workspace, &session_id, &name, &bytes))
            .await
            .map_err(|e| ApiError::internal(format!("upload task failed: {e}")))??;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "session": stored.session,
            "name": stored.name,
            "path": stored.path,
            "size": size,
            "content_type": content_type.unwrap_or_else(|| "application/octet-stream".into()),
            "workspace": root,
            "deduplicated": stored.deduplicated,
        })),
    ))
}

#[derive(Debug)]
struct StoredUpload {
    session: String,
    name: String,
    /// Workspace-relative, so it can be fed straight back to
    /// `/api/workspace/file` and to `@`-mentions.
    path: String,
    deduplicated: bool,
}

/// Blocking half of [`upload`]: create the attachment directory and write the
/// file with `create_new`, so two concurrent uploads of the same name cannot
/// overwrite each other (the loser retries with the next candidate name).
fn store_upload(
    workspace: &Workspace,
    session_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<StoredUpload, ApiError> {
    let dir_rel = format!("{UPLOAD_ROOT}/{session_id}");
    let dir = workspace.resolve_for_write(&dir_rel)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;
    // Re-check the *canonical* directory: a symlinked `.karucode` component is
    // refused by `resolve_for_write`, but the check is repeated here so the file
    // cannot be written through a link created in the meantime.
    let canon_dir = std::fs::canonicalize(&dir)
        .map_err(|e| ApiError::internal(format!("cannot open {}: {e}", dir.display())))?;
    if !canon_dir.starts_with(workspace.root()) || !canon_dir.is_dir() {
        return Err(Error::PathEscape {
            path: canon_dir,
            root: workspace.root().to_path_buf(),
        }
        .into());
    }
    restrict_dir(&canon_dir);

    for attempt in 0..COLLISION_ATTEMPTS {
        let candidate = candidate_name(name, attempt);
        let target: PathBuf = canon_dir.join(&candidate);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|e| {
                    // Do not leave a truncated attachment behind.
                    let _ = std::fs::remove_file(&target);
                    ApiError::internal(format!("cannot write {}: {e}", target.display()))
                })?;
                drop(file);
                restrict_file(&target);
                return Ok(StoredUpload {
                    session: session_id.to_string(),
                    name: candidate,
                    path: workspace.relative(&target),
                    deduplicated: attempt > 0,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // A directory or a dangling symlink already sitting on the name is a
            // collision too; never write through either.
            Err(_) if std::fs::symlink_metadata(&target).is_ok() => continue,
            Err(e) => {
                return Err(ApiError::internal(format!(
                    "cannot write {}: {e}",
                    target.display()
                )))
            }
        }
    }
    Err(ApiError::conflict(format!(
        "too many attachments already named `{name}` in {UPLOAD_ROOT}/{session_id}"
    )))
}

/// `name`, then `stem-1.ext`, `stem-2.ext`, … An extensionless or dotfile name
/// (`.env`) is suffixed as a whole.
fn candidate_name(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            format!("{}-{attempt}{}", &name[..dot], &name[dot..])
        }
        _ => format!("{name}-{attempt}"),
    }
}

/// A file name is a single path segment: no directories, no traversal, no
/// control characters. Everything else (dots, spaces, unicode) is allowed.
fn validate_attachment_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("`name` may not be empty"));
    }
    if name.len() > MAX_ATTACHMENT_NAME_BYTES {
        return Err(ApiError::bad_request(format!(
            "`name` is too long ({} bytes, maximum {MAX_ATTACHMENT_NAME_BYTES})",
            name.len()
        )));
    }
    if name == "." || name == ".." {
        return Err(ApiError::bad_request(
            "`name` must be a file name, not a directory reference",
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ApiError::bad_request(
            "`name` must not contain a directory separator; attachments are named, not pathed",
        ));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(ApiError::bad_request(
            "`name` must not contain control characters",
        ));
    }
    Ok(name.to_string())
}

/// Session ids reach the filesystem as a directory name, so only the alphabet
/// the ids actually use is accepted (`s_<hex>`).
fn validate_session_id(id: &str) -> ApiResult<()> {
    if id.is_empty() || id.len() > MAX_SESSION_ID_BYTES {
        return Err(ApiError::bad_request(format!(
            "`session` must be 1–{MAX_SESSION_ID_BYTES} characters"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "`session` may only contain letters, digits, `_` and `-`",
        ));
    }
    Ok(())
}

fn attachment_content_type(headers: &HeaderMap) -> ApiResult<Option<String>> {
    let Some(raw) = headers.get(header::CONTENT_TYPE) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| ApiError::bad_request("`Content-Type` must be ASCII"))?;
    // Parameters (`; charset=…`) belong to the file, not to the MIME type we
    // echo back.
    let essence = raw.split(';').next().unwrap_or("").trim();
    if essence.is_empty() {
        return Ok(None);
    }
    if essence.len() > MAX_CONTENT_TYPE_BYTES
        || !essence.is_ascii()
        || essence.chars().any(|c| c.is_control())
    {
        return Err(ApiError::bad_request(
            "`Content-Type` must be a short ASCII MIME type",
        ));
    }
    if essence.eq_ignore_ascii_case("multipart/form-data")
        || essence.eq_ignore_ascii_case("application/x-www-form-urlencoded")
    {
        return Err(ApiError::bad_request(
            "send the file bytes as the raw request body; form encodings are not unpacked",
        ));
    }
    if !essence.contains('/') {
        return Err(ApiError::bad_request(
            "`Content-Type` must be a MIME type such as `text/plain` or `application/pdf`",
        ));
    }
    Ok(Some(essence.to_ascii_lowercase()))
}

fn content_length(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    let Some(raw) = headers.get(header::CONTENT_LENGTH) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| ApiError::bad_request("`Content-Length` must be ASCII"))?;
    raw.trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|_| ApiError::bad_request("`Content-Length` must be a non-negative integer"))
}

fn payload_too_large(declared: u64) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        format!("the upload is {declared} bytes; the maximum is {MAX_UPLOAD_BYTES} bytes"),
    )
}

/// `axum::body::to_bytes` reports a tripped limit as `http-body-util`'s
/// `LengthLimitError`, which this crate cannot name (axum only exposes it as a
/// boxed error), so match its message; any other failure is a broken body.
fn is_length_limit(err: &axum::Error) -> bool {
    use std::error::Error as StdError;
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(err) = current {
        if err.to_string().contains("length limit") {
            return true;
        }
        current = err.source();
    }
    false
}

/// Attachments can contain secrets; keep the directory private. Best effort -
/// a filesystem without Unix modes (or a pre-existing user directory) must not
/// fail the upload.
fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!(path = %dir.display(), "could not restrict attachment directory: {e}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

fn restrict_file(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(path = %file.display(), "could not restrict attachment file: {e}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

fn language_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "jsonc" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" | "zsh" => "shell",
        "fish" => "shell",
        "ps1" => "powershell",
        "sql" => "sql",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "less" => "less",
        "xml" => "xml",
        "yaml" | "yml" => "yaml",
        "ini" | "cfg" | "conf" => "ini",
        "dockerfile" => "dockerfile",
        "lua" => "lua",
        "swift" => "swift",
        "zig" => "zig",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "scala" => "scala",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (std::path::PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!(
            "kc-server-ws-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# hi\n").unwrap();
        let ws = Workspace::new(&dir).unwrap();
        (dir, ws)
    }

    #[test]
    fn tree_skips_noise_directories() {
        let (dir, ws) = workspace();
        for path in [
            ".git/objects",
            "node_modules/pkg",
            "target/debug",
            "dist/assets",
            ".github/workflows",
        ] {
            std::fs::create_dir_all(dir.join(path)).unwrap();
        }
        std::fs::write(dir.join(".git/HEAD"), "ref: main").unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(dir.join("target/debug/bin"), "x").unwrap();
        std::fs::write(dir.join("dist/assets/app.js"), "x").unwrap();
        std::fs::write(dir.join(".github/workflows/ci.yml"), "on: push").unwrap();

        let value = tree_blocking(&ws, ".", 4).unwrap();
        let paths: Vec<String> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap().to_string())
            .collect();

        for ignored in [".git", "node_modules", "target", "dist"] {
            assert!(
                !paths
                    .iter()
                    .any(|path| path == ignored || path.starts_with(&format!("{ignored}/"))),
                "{ignored} must not be listed: {paths:?}"
            );
        }
        // Dot-directories that hold real project files are still listed.
        assert!(paths.contains(&".github/workflows/ci.yml".to_string()));
        assert!(paths.contains(&"src/main.rs".to_string()));

        // Asking for an ignored directory explicitly still works.
        let value = tree_blocking(&ws, ".git", 1).unwrap();
        let explicit: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect();
        assert!(explicit.contains(&".git/HEAD"), "{explicit:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tree_lists_directories_first() {
        let (dir, ws) = workspace();
        let value = tree_blocking(&ws, ".", 2).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries[0]["kind"], "dir");
        assert!(entries.iter().any(|e| e["path"] == "src/main.rs"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn file_reports_content_and_language() {
        let (dir, ws) = workspace();
        let value = file_blocking(&ws, "src/main.rs", 1024).unwrap();
        assert_eq!(value["content"], "fn main() {}\n");
        assert_eq!(value["language"], "rust");
        assert_eq!(value["binary"], false);
        assert_eq!(value["truncated"], false);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn file_truncates_and_detects_binary() {
        let (dir, ws) = workspace();
        let value = file_blocking(&ws, "README.md", 1024).unwrap();
        assert!(!value["binary"].as_bool().unwrap());

        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let value = file_blocking(&ws, "blob.bin", 1024).unwrap();
        assert_eq!(value["binary"], true);
        assert_eq!(value["content"], "");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn traversal_is_refused() {
        let (dir, ws) = workspace();
        let err = file_blocking(&ws, "../../etc/passwd", 1024).unwrap_err();
        assert!(matches!(err, Error::PathEscape { .. }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn collision_names_add_a_counter_before_the_extension() {
        assert_eq!(candidate_name("report.pdf", 0), "report.pdf");
        assert_eq!(candidate_name("report.pdf", 2), "report-2.pdf");
        assert_eq!(candidate_name("archive.tar.gz", 1), "archive.tar-1.gz");
        assert_eq!(candidate_name("README", 1), "README-1");
        assert_eq!(candidate_name(".env", 1), ".env-1");
    }

    #[test]
    fn attachment_names_are_a_single_segment() {
        assert_eq!(
            validate_attachment_name("  notes.txt ").unwrap(),
            "notes.txt"
        );
        assert!(validate_attachment_name("dir/notes.txt").is_err());
        assert!(validate_attachment_name("..\\notes.txt").is_err());
        assert!(validate_attachment_name("..").is_err());
        assert!(validate_attachment_name(".").is_err());
        assert!(validate_attachment_name("").is_err());
        assert!(validate_attachment_name("a\0b").is_err());
        assert!(validate_attachment_name(&"a".repeat(MAX_ATTACHMENT_NAME_BYTES + 1)).is_err());
        // Unicode and spaces are ordinary file names, not attacks.
        assert!(validate_attachment_name("Résumé final.pdf").is_ok());
    }

    #[test]
    fn session_ids_cannot_escape_the_upload_directory() {
        assert!(validate_session_id("s_ab12cd34").is_ok());
        for bad in ["", "../x", "s_ab/12", "s ab", "s_ab\\12", "."] {
            assert!(validate_session_id(bad).is_err(), "{bad} must be refused");
        }
        assert!(validate_session_id(&"s".repeat(MAX_SESSION_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn store_upload_writes_under_the_session_directory() {
        let (dir, ws) = workspace();
        let stored = store_upload(&ws, "s_test", "notes.txt", b"hello").unwrap();
        assert_eq!(stored.path, ".karucode/attachments/s_test/notes.txt");
        assert!(!stored.deduplicated);
        assert_eq!(
            std::fs::read(dir.join(".karucode/attachments/s_test/notes.txt")).unwrap(),
            b"hello"
        );

        // A second upload of the same name must not overwrite the first.
        let again = store_upload(&ws, "s_test", "notes.txt", b"second").unwrap();
        assert_eq!(again.name, "notes-1.txt");
        assert!(again.deduplicated);
        assert_eq!(
            std::fs::read(dir.join(".karucode/attachments/s_test/notes.txt")).unwrap(),
            b"hello"
        );

        // A directory already occupying the name is a collision, not a crash.
        let squatted = dir.join(".karucode/attachments/s_dir");
        std::fs::create_dir_all(squatted.join("taken.txt")).unwrap();
        let stored = store_upload(&ws, "s_dir", "taken.txt", b"data").unwrap();
        assert_eq!(stored.name, "taken-1.txt");
        assert!(stored.deduplicated);

        // Attachments can hold secrets, so they are not world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(dir.join(".karucode/attachments/s_test/notes.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            let dir_mode = std::fs::metadata(dir.join(".karucode/attachments/s_test"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn store_upload_refuses_a_symlinked_attachment_root() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!(
            "kc-server-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(dir.join(".karucode")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(UPLOAD_ROOT)).unwrap();

        let err = store_upload(&ws, "s_test", "notes.txt", b"hello").unwrap_err();
        assert_eq!(err.code, "path_escape");
        assert!(!outside.join("s_test").exists());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(outside).ok();
    }
}
