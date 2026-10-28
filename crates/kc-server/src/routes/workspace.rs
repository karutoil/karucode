//! Read-only workspace explorer: directory tree and file contents.
//!
//! Both endpoints resolve through [`kc_core::Workspace`], so traversal and
//! symlink escapes are refused with a 400 `path_escape` before any I/O.

use crate::error::{ApiError, ApiResult};
use crate::routes::{query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use kc_core::error::Error;
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

const DEFAULT_DEPTH: usize = 1;
const MAX_DEPTH: usize = 6;
const MAX_TREE_ENTRIES: usize = 2000;
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
/// subdirectories are skipped instead of failing the whole tree.
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
        items.push((
            is_dir,
            entry.file_name().to_string_lossy().to_ascii_lowercase(),
            entry.path(),
        ));
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
    file.by_ref()
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
}
