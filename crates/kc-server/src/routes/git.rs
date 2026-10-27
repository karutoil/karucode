//! Read-only git endpoints.
//!
//! `kc_tools::git` exposes a rendered text report, but `docs/API.md` needs a
//! structured file list (`path`, `status`, `staged`) and a raw diff, so the
//! server runs `git` itself.
//!
//! Constraints for these read-only endpoints (no approval is involved):
//! * `git` is spawned directly with an argv array (never a shell);
//! * `current_dir` is the canonical workspace root;
//! * repository-local configuration may not execute anything: `core.fsmonitor`
//!   (runs on `status`) and `diff.external` (runs on `diff`) are overridden with
//!   `-c`, `GIT_EXTERNAL_DIFF`/`GIT_PAGER` are pinned, `--no-ext-diff` is passed
//!   to `diff`, and any inherited `GIT_DIR`/`GIT_WORK_TREE` is dropped, so a
//!   cloned repository cannot turn a read-only panel into code execution;
//! * the whole call (spawn + read + wait) is bounded by a 10s timeout and
//!   `kill_on_drop`;
//! * stdout/stderr are drained but kept in memory only up to fixed caps;
//! * `diff` accepts no user data in a flag position: `base` is validated
//!   against `^[A-Za-z0-9._/~^-]{1,120}$`, may not start with `-`, and the path
//!   follows `--`.

use crate::error::{ApiError, ApiResult};
use crate::routes::{query_bool, query_str, QueryMap};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_STDERR_CHARS: usize = 2000;
/// Caps for what is kept in memory; the pipes are still drained so a large
/// repository cannot deadlock the child.
const MAX_GIT_STDOUT: usize = 4 * 1024 * 1024;
const MAX_GIT_STDERR: usize = 64 * 1024;

struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Global `-c` overrides prepended to every invocation.
///
/// * `core.fsmonitor=false` — otherwise `git status` executes a command taken
///   from the repository's own `.git/config`.
/// * `diff.external=` — belt and braces for `git diff`; on its own an *empty*
///   value is parsed as a (failing) command, which is why [`run_git`] callers
///   that diff also pass `--no-ext-diff` (see [`diff`]), the flag that really
///   disables external drivers.
/// * `core.pager=cat` — the server has no terminal, and a repo-local pager would
///   be another command to spawn.
const GIT_HARDENING_ARGS: [&str; 6] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
];

pub async fn status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let root = state.workspace().root().to_path_buf();
    let outcome = run_git(&root, &["status", "--porcelain=v1", "-b"]).await;

    match outcome {
        Ok(output) if output.success => {
            let (branch, files) = parse_status(&output.stdout);
            Ok(Json(json!({
                "is_repo": true,
                "branch": branch,
                "error": Value::Null,
                "files": files,
            })))
        }
        Ok(output) => {
            let stderr = state.redact(&truncate(&output.stderr, MAX_STDERR_CHARS));
            if is_not_a_repo(&output.stderr) {
                Ok(Json(json!({
                    "is_repo": false,
                    "branch": Value::Null,
                    "error": Value::Null,
                    "files": [],
                })))
            } else {
                Ok(Json(json!({
                    "is_repo": false,
                    "branch": Value::Null,
                    "error": if stderr.trim().is_empty() {
                        "git status failed".to_string()
                    } else {
                        stderr
                    },
                    "files": [],
                })))
            }
        }
        Err(error) => {
            // Missing `git` is reported in the payload (the UI shows an empty
            // git panel with the reason), not as a 500.
            Ok(Json(json!({
                "is_repo": false,
                "branch": Value::Null,
                "error": state.redact(&error.message),
                "files": [],
            })))
        }
    }
}

pub async fn diff(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let staged = query_bool(&query, "staged")?;
    let base = query_str(&query, "base");
    if let Some(base) = base {
        validate_ref(base)?;
    }
    let requested_path = query_str(&query, "path");
    let workspace = state.workspace().clone();

    let mut args: Vec<String> = vec!["diff".into(), "--no-ext-diff".into()];
    if staged {
        args.push("--staged".into());
    }
    if let Some(base) = base {
        args.push(base.to_string());
    }
    if let Some(path) = requested_path {
        // Lexical resolution: a deleted file no longer exists but its relative
        // path must still be inside the workspace.
        let absolute = workspace.resolve(path)?;
        let relative = workspace.relative(&absolute);
        if relative != "." {
            args.push("--".into());
            args.push(relative);
        }
    }

    let root = workspace.root().to_path_buf();
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git(&root, &borrowed)
        .await
        .map_err(|e| ApiError::internal(state.redact(&e.message)))?;

    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Err(ApiError::bad_request(
                "the workspace is not a git repository",
            ));
        }
        let stderr = state.redact(&truncate(output.stderr.trim(), MAX_STDERR_CHARS));
        return Err(ApiError::bad_request(if stderr.is_empty() {
            "git diff failed".to_string()
        } else {
            format!("git diff failed: {stderr}")
        }));
    }

    let (diff, truncated) = truncate_bytes(&output.stdout, MAX_DIFF_BYTES);
    Ok(Json(json!({ "diff": diff, "truncated": truncated })))
}

/// `^[A-Za-z0-9._/~^-]{1,120}$` plus the "no leading dash" option-injection
/// rule. Kept deliberately stricter than git's ref grammar: this endpoint only
/// needs ordinary branch/tag/commit refs.
fn validate_ref(base: &str) -> ApiResult<()> {
    if base.is_empty() || base.len() > 120 {
        return Err(ApiError::bad_request(
            "query parameter `base` must be a git ref of 1-120 characters",
        ));
    }
    if base.starts_with('-') {
        return Err(ApiError::bad_request(
            "query parameter `base` may not start with `-`",
        ));
    }
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '~' | '^' | '-');
    if !base.chars().all(allowed) {
        return Err(ApiError::bad_request(
            "query parameter `base` may only contain letters, digits and . _ / ~ ^ -",
        ));
    }
    Ok(())
}

async fn run_git(root: &Path, args: &[&str]) -> Result<GitOutput, ApiError> {
    let mut command = tokio::process::Command::new("git");
    command
        // Repository-local config must not run code on a read-only endpoint.
        .args(GIT_HARDENING_ARGS)
        .args(args)
        // Canonical workspace root; no shell, no `cd`.
        .current_dir(root)
        // Never prompt or page; the server has no terminal.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        // An inherited external diff driver would bypass `core.pager`/`-c`.
        .env("GIT_EXTERNAL_DIFF", "")
        // Read-only endpoints must not take `index.lock` or refresh the index.
        .env("GIT_OPTIONAL_LOCKS", "0")
        // An inherited GIT_DIR/GIT_WORK_TREE points git at a different
        // repository than the workspace the UI is showing.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let root_label = root.display().to_string();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::internal(
                "`git` is not installed or not on PATH; install git to use the git panel",
            ))
        }
        Err(e) => {
            return Err(ApiError::internal(format!(
                "could not run git in {root_label}: {e}"
            )))
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("git stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("git stderr pipe was not created"))?;

    let run = async move {
        let (stdout, stderr, status) = tokio::join!(
            read_bounded(stdout, MAX_GIT_STDOUT),
            read_bounded(stderr, MAX_GIT_STDERR),
            child.wait(),
        );
        let status = status
            .map_err(|e| ApiError::internal(format!("git process error in {root_label}: {e}")))?;
        let (mut stdout, stdout_cut) = stdout.map_err(|e| {
            ApiError::internal(format!("could not read git stdout in {root_label}: {e}"))
        })?;
        let (mut stderr, stderr_cut) = stderr.map_err(|e| {
            ApiError::internal(format!("could not read git stderr in {root_label}: {e}"))
        })?;
        // Drop a possibly partial trailing line so parsers never see half a row.
        if stdout_cut {
            truncate_to_last_line(&mut stdout);
        }
        if stderr_cut {
            truncate_to_last_line(&mut stderr);
        }
        Ok::<_, ApiError>(GitOutput {
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    };

    match tokio::time::timeout(GIT_TIMEOUT, run).await {
        Ok(result) => result,
        // `run` is dropped here; `kill_on_drop` terminates the child.
        Err(_) => Err(ApiError::new(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            format!("git did not finish within {}s", GIT_TIMEOUT.as_secs()),
        )),
    }
}

/// Read a pipe to EOF while retaining at most `limit` bytes. The rest is
/// discarded so the child never blocks on a full pipe.
async fn read_bounded<R>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut out = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0u8; 16 * 1024];
    let mut total = 0usize;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        total += read;
        if out.len() < limit {
            let space = limit - out.len();
            out.extend_from_slice(&chunk[..read.min(space)]);
        }
    }
    Ok((out, total > limit))
}

fn truncate_to_last_line(buffer: &mut Vec<u8>) {
    if let Some(position) = buffer.iter().rposition(|byte| *byte == b'\n') {
        buffer.truncate(position + 1);
    } else {
        buffer.clear();
    }
}

fn is_not_a_repo(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("not a git repository")
}

/// Parse `git status --porcelain=v1 -b` output.
fn parse_status(stdout: &str) -> (Option<String>, Vec<Value>) {
    let mut branch = None;
    let mut files = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            branch = parse_branch(rest);
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let index = line.as_bytes()[0] as char;
        let worktree = line.as_bytes()[1] as char;
        let path_part = line.get(3..).unwrap_or("").trim();
        if path_part.is_empty() {
            continue;
        }
        // Renames/copies are rendered as `old -> new`; the new path is current.
        let path = match path_part.rsplit_once(" -> ") {
            Some((_old, new)) => unquote(new.trim()),
            None => unquote(path_part),
        };
        let code = [index, worktree];
        let status = if code.iter().all(|c| *c == '?') {
            "??".to_string()
        } else {
            code.iter().filter(|c| **c != ' ').collect::<String>()
        };
        let staged = index != ' ' && index != '?';
        files.push(json!({
            "path": path,
            "status": if status.is_empty() { " ".to_string() } else { status },
            "staged": staged,
        }));
    }
    (branch, files)
}

fn parse_branch(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() || rest.starts_with("HEAD (no branch)") {
        return None;
    }
    if let Some(name) = rest.strip_prefix("No commits yet on ") {
        return Some(name.trim().to_string());
    }
    let name = rest.split(['.', ' ']).next().unwrap_or("").trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').replace("\\\"", "\"")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Byte-bounded truncation that never splits a UTF-8 character.
fn truncate_bytes(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_status_parses_branch_and_flags() {
        let text =
            "## main...origin/main\n M src/x.rs\nM  Cargo.toml\n?? new.txt\nR  old.rs -> new.rs\n";
        let (branch, files) = parse_status(text);
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(files.len(), 4);
        assert_eq!(files[0]["path"], "src/x.rs");
        assert_eq!(files[0]["status"], "M");
        assert_eq!(files[0]["staged"], false);
        assert_eq!(files[1]["staged"], true);
        assert_eq!(files[2]["status"], "??");
        assert_eq!(files[3]["path"], "new.rs");
    }

    #[test]
    fn branch_parsing_handles_edge_cases() {
        assert_eq!(parse_branch("HEAD (no branch)"), None);
        assert_eq!(
            parse_branch("No commits yet on main"),
            Some("main".to_string())
        );
        assert_eq!(parse_branch("feat/x"), Some("feat/x".to_string()));
    }

    #[test]
    fn git_ref_validation_blocks_options() {
        assert!(validate_ref("main").is_ok());
        assert!(validate_ref("HEAD~2").is_ok());
        assert!(validate_ref("origin/main").is_ok());
        assert!(validate_ref("v1.2.3").is_ok());
        assert!(validate_ref("--upload-pack=evil").is_err());
        assert!(validate_ref("-n").is_err());
        assert!(validate_ref("main; rm -rf /").is_err());
        assert!(validate_ref("HEAD@{1}").is_err());
        assert!(validate_ref(&"a".repeat(121)).is_err());
        assert!(validate_ref("").is_err());
    }

    #[tokio::test]
    async fn bounded_read_keeps_the_limit_and_drains() {
        let data = vec![b'a'; 1024 * 1024];
        let (kept, cut) = read_bounded(std::io::Cursor::new(data), 1024)
            .await
            .expect("read");
        assert!(cut);
        assert_eq!(kept.len(), 1024);

        let (kept, cut) = read_bounded(std::io::Cursor::new(b"short".to_vec()), 1024)
            .await
            .expect("read");
        assert!(!cut);
        assert_eq!(kept, b"short");
    }

    #[test]
    fn partial_trailing_lines_are_dropped() {
        let mut buffer = b"## main\n M a.rs\n M b".to_vec();
        truncate_to_last_line(&mut buffer);
        assert_eq!(buffer, b"## main\n M a.rs\n");
        let mut buffer = b"partial".to_vec();
        truncate_to_last_line(&mut buffer);
        assert!(buffer.is_empty());
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let (out, cut) = truncate_bytes("héllo", 2);
        assert!(cut);
        assert_eq!(out, "h");
        let (out, cut) = truncate_bytes("hi", 10);
        assert!(!cut);
        assert_eq!(out, "hi");
    }
}
