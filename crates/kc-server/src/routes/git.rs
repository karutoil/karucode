//! Git state, local mutations, remote sync, and repository setup for the web
//! source-control manager.
//!
//! `kc_tools::git` exposes a rendered text report, but the UI needs structured
//! data (per-file index/worktree codes, commit metadata, branch tracking,
//! remotes, tags, operation state), so the server runs `git` itself and renders
//! JSON. The hardening is *not* duplicated: [`kc_tools::git::SAFE_CONFIG`],
//! [`safe_env`], [`diff_args`], [`literal_pathspec_env`] and the ref/branch
//! validators are the single source of truth shared with the
//! `git_status`/`git_diff` tools and with `kc-tools`' confinement policy, so the
//! HTTP surface cannot drift from the tools.
//!
//! ## Invariants for every endpoint in this module
//!
//! * `git` is spawned directly with an argv array, never through a shell;
//! * `current_dir` is the canonical workspace root; an inherited
//!   `GIT_DIR`/`GIT_WORK_TREE`/`GIT_CONFIG_COUNT` is dropped, so a repository
//!   cannot redirect the command at another checkout or inject configuration;
//! * repository-local configuration may not execute anything on a *read*: the
//!   shared [`SAFE_CONFIG`] covers `core.fsmonitor`, `diff.external` and the
//!   pager, `--no-ext-diff --no-textconv` covers attribute-selected drivers, and
//!   `diff` additionally neutralises every `filter.<driver>.*` the repository
//!   configures (see [`kc_tools::git::filter_env`]);
//! * every path a caller sends is resolved through [`kc_core::paths::Workspace`]
//!   first, so `..`, an absolute path outside the workspace and a symlink escape
//!   are refused with `400` before git sees them; the resulting pathspec is passed
//!   after `--` with `GIT_LITERAL_PATHSPECS=1`, so `a[1].txt` or `*.rs` matching
//!   more than the caller selected is impossible;
//! * every ref, tag, remote or branch name is validated before it sits in a flag
//!   position;
//! * network commands (`fetch`, `pull`, `push`) disable interactive prompts
//!   (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`, `BatchMode=yes` when
//!   `GIT_SSH_COMMAND` is unset) so a missing credential fails quickly rather than
//!   hanging on `/dev/tty`, and never echo URL userinfo credentials;
//! * each invocation is bounded by a timeout with `kill_on_drop`, and stdio is
//!   drained but kept in memory only up to fixed caps.
//!
//! ## What mutations deliberately do *not* neutralise
//!
//! `git add`, `git commit`, `git merge`, `git rebase`, `git push` and
//! `git stash push` are explicit user actions, and they run the repository's own
//! hooks (`pre-commit`, `commit-msg`, `pre-push`, …) and clean/smudge filters
//! exactly like a terminal `git` invocation - a manager that disabled LFS or
//! skipped `pre-commit` would commit something other than what the user asked for.
//! They are therefore *not* routed through [`filter_env`]; the read-only
//! endpoints are.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_body, query_bool, query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::paths::Workspace;
use kc_tools::git::{
    diff_args, filter_env, literal_pathspec_env, safe_env, validate_branch_name,
    validate_revision_arg, SAFE_CONFIG,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as FsPath;
use std::process::Stdio;
use std::time::Duration;

/// Bound for a read-only probe. Nothing here waits on a human.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound for a mutation. A `pre-commit` hook or a commit-signing program is the
/// slow part; both are the user's own configuration, so they get room - but not
/// unbounded room.
const MUTATION_TIMEOUT: Duration = Duration::from_secs(60);
/// Bound for network operations (`fetch`, `pull`, `push`).
const NETWORK_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_DIFF_BYTES: usize = 512 * 1024;
pub(crate) const MAX_STDERR_CHARS: usize = 2000;
/// Caps for what is kept in memory; the pipes are still drained so a large
/// repository cannot deadlock the child.
pub(crate) const MAX_GIT_STDOUT: usize = 4 * 1024 * 1024;
pub(crate) const MAX_GIT_STDERR: usize = 64 * 1024;
/// Bounds on what a single response can carry, so a pathological repository
/// cannot turn one GET into tens of megabytes of JSON.
const MAX_BRANCHES: usize = 300;
const MAX_TAGS: usize = 300;
const MAX_LOG_ENTRIES: usize = 100;
const DEFAULT_LOG_ENTRIES: usize = 30;
const MAX_STASHES: usize = 100;
const MAX_PATHS_PER_REQUEST: usize = 500;
const MAX_COMMIT_MESSAGE_CHARS: usize = 8000;
const MAX_STASH_MESSAGE_CHARS: usize = 400;
const MAX_TAG_MESSAGE_CHARS: usize = 4000;
const MAX_REMOTE_NAME_CHARS: usize = 100;
const MAX_REMOTE_URL_CHARS: usize = 500;
const MAX_SEARCH_CHARS: usize = 200;
/// `%x1f` (unit separator) between fields: it cannot appear in a ref name, an
/// email or a path git prints, and a commit subject that does contain one would
/// have to be deliberately hostile. The subject is always the *last* field, so a
/// stray separator could not shift the columns before it.
const FIELD: &str = "%x1f";

struct GitOutput {
    success: bool,
    /// `None` when the child died from a signal.
    code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
}

/// One parsed entry of `git status --porcelain=v1 -b -z -uall`.
#[derive(Debug, Default)]
struct RepoStatus {
    branch: Option<String>,
    detached: bool,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    files: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Read-only endpoints
// ---------------------------------------------------------------------------

pub async fn status(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    Ok(Json(read_status(&state, &root).await?))
}

/// The status payload, also returned by every mutation so the UI can replace its
/// state from one response instead of racing a second GET.
///
/// Failures are reported *in the payload* (`is_repo: false` plus `error`) rather
/// than as an HTTP error: a non-git workspace is an ordinary state the panel
/// explains, and a missing `git` binary must not look like a broken server.
pub(crate) async fn read_status(state: &AppState, root: &FsPath) -> ApiResult<Value> {
    // `-uall` lists the files inside an untracked directory instead of collapsing
    // it to `dir/`: the manager can then offer a per-file diff, stage and discard
    // for them, which a collapsed entry cannot support.
    let outcome = run_git(
        root,
        &["status", "--porcelain=v1", "-b", "-z", "-uall"],
        &[],
    )
    .await;
    let empty = |error: Option<String>| {
        json!({
            "is_repo": false,
            "branch": Value::Null,
            "detached": false,
            "upstream": Value::Null,
            "ahead": 0,
            "behind": 0,
            "operation": Value::Null,
            "has_head": false,
            "identity": { "name": Value::Null, "email": Value::Null },
            "remotes_count": 0,
            "error": error,
            "files": [],
            "truncated": false,
        })
    };
    match outcome {
        Ok(output) if output.success => {
            let status = parse_status_z(&output.stdout);
            let operation = detect_repo_operation(root).await;
            let has_head = head_exists(root).await.unwrap_or(false);
            let identity = read_git_identity(root).await;
            let remotes_count = count_remotes(root).await;
            Ok(json!({
                "is_repo": true,
                "branch": status.branch,
                "detached": status.detached,
                "upstream": status.upstream,
                "ahead": status.ahead,
                "behind": status.behind,
                "operation": operation,
                "has_head": has_head,
                "identity": {
                    "name": identity.0,
                    "email": identity.1,
                },
                "remotes_count": remotes_count,
                "error": Value::Null,
                "files": status.files,
                // A working tree with hundreds of thousands of changes is
                // reported, not silently cut short.
                "truncated": output.stdout_truncated,
            }))
        }
        Ok(output) => {
            if is_not_a_repo(&output.stderr) {
                Ok(empty(None))
            } else {
                let stderr = state.redact(&truncate(output.stderr.trim(), MAX_STDERR_CHARS));
                Ok(empty(Some(if stderr.is_empty() {
                    "git status failed".to_string()
                } else {
                    stderr
                })))
            }
        }
        Err(error) => Ok(empty(Some(state.redact(&error.message)))),
    }
}

async fn detect_repo_operation(root: &FsPath) -> Option<&'static str> {
    let output = run_git(root, &["rev-parse", "--git-dir"], &[]).await.ok()?;
    if !output.success {
        return None;
    }
    let raw = output.stdout.trim();
    if raw.is_empty() {
        return None;
    }
    let git_dir = {
        let candidate = std::path::PathBuf::from(raw);
        if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        }
    };
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some("rebase")
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some("merge")
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some("cherry-pick")
    } else if git_dir.join("REVERT_HEAD").exists() {
        Some("revert")
    } else {
        None
    }
}

async fn read_git_identity(root: &FsPath) -> (Option<String>, Option<String>) {
    let Ok(output) = run_git(
        root,
        &["config", "--get-regexp", r"^user\.(name|email)$"],
        &[],
    )
    .await
    else {
        return (None, None);
    };
    if !output.success {
        return (None, None);
    }
    let mut name = None;
    let mut email = None;
    for line in output.stdout.lines() {
        if let Some(rest) = line.strip_prefix("user.name ") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                name = Some(trimmed.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("user.email ") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                email = Some(trimmed.to_string());
            }
        }
    }
    (name, email)
}

async fn count_remotes(root: &FsPath) -> usize {
    let Ok(output) = run_git(root, &["remote"], &[]).await else {
        return 0;
    };
    if !output.success {
        return 0;
    }
    output
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
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
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;

    // Same argv as the `git_diff` tool: `--no-ext-diff --no-textconv` plus the
    // path after `--`. `base` was validated above, so it cannot be a flag.
    let pathspec = match requested_path {
        Some(path) => {
            // Lexical resolution: a deleted file no longer exists but its
            // relative path must still be inside the workspace.
            let absolute = workspace.resolve(path)?;
            let relative = workspace.relative(&absolute);
            if relative == "." {
                None
            } else {
                Some(relative)
            }
        }
        None => None,
    };
    let args = diff_args(staged, base, pathspec.as_deref());

    let root = workspace.root().to_path_buf();
    // An untracked file has no index entry, so `git diff -- <path>` prints
    // nothing at all - the manager would show "no diff" for a file the agent just
    // created. Compare it against the empty file instead (`--no-index`), which is
    // how `git add -N` renders it, and treat the "differences found" exit (1) as
    // success.
    let untracked = match (staged, base, pathspec.as_deref()) {
        (false, None, Some(spec)) => !tracked_in_index(&root, spec).await?,
        _ => false,
    };
    let args = if untracked {
        let mut args: Vec<String> = [
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            "/dev/null",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(spec) = pathspec.as_deref() {
            args.push(spec.to_string());
        }
        args
    } else {
        args
    };

    // `git diff` runs `filter.<driver>.clean`/`.process` for the working-tree
    // side of the patch; neutralise every driver this repository configures.
    let filter_overrides = filter_env(&root)
        .await
        .map_err(|e| ApiError::internal(state.redact(&e.to_string())))?;
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git(&root, &borrowed, &filter_overrides)
        .await
        .map_err(|e| ApiError::internal(state.redact(&e.message)))?;

    if !(output.success || untracked && output.code == Some(1)) {
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

/// `GET /api/git/branches` - local branches, newest commit first.
pub async fn branches(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    Ok(Json(read_branches(&state, &root).await?))
}

async fn read_branches(state: &AppState, root: &FsPath) -> ApiResult<Value> {
    // `%(upstream:track)` is git's own "[ahead 1, behind 2]" summary, so the
    // branch list costs one process instead of one `rev-list` per branch.
    //
    // `for-each-ref` uses its own format language: a raw separator is `%1f`,
    // where the pretty formats (`git log`, `git stash list`) want `%x1f`.
    let format = "--format=%(refname:short)%1f%(objectname:short)%1f%(upstream:short)%1f%(HEAD)%1f%(committerdate:unix)%1f%(upstream:track)%1f%(contents:subject)";
    let output = run_git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            format,
            "refs/heads",
        ],
        &[],
    )
    .await?;
    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Ok(json!({
                "is_repo": false,
                "branch": Value::Null,
                "detached": false,
                "branches": [],
                "truncated": false,
            }));
        }
        return Err(failure(state, "for-each-ref", &output));
    }

    let mut branches = Vec::new();
    for line in output.stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(7, '\u{1f}');
        let name = fields.next().unwrap_or_default().trim().to_string();
        if name.is_empty() {
            continue;
        }
        let commit = fields.next().unwrap_or_default().trim().to_string();
        let upstream = fields.next().unwrap_or_default().trim().to_string();
        let current = fields.next().unwrap_or_default().trim() == "*";
        let timestamp = fields
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let track = fields.next().unwrap_or_default().trim().to_string();
        let subject = fields.next().unwrap_or_default().to_string();
        let (ahead, behind) = parse_track(&track);
        branches.push(json!({
            "name": name,
            "current": current,
            "commit": commit,
            "upstream": if upstream.is_empty() { Value::Null } else { json!(upstream) },
            "ahead": ahead,
            "behind": behind,
            "timestamp": timestamp,
            "subject": subject,
        }));
        if branches.len() >= MAX_BRANCHES {
            break;
        }
    }
    let truncated = output.stdout_truncated || branches.len() >= MAX_BRANCHES;

    // The current branch/HEAD state comes from the same parser the panel uses,
    // so a detached HEAD is reported the same way everywhere.
    let status = run_git(root, &["status", "--porcelain=v1", "-b", "-z"], &[]).await?;
    let parsed = if status.success {
        parse_status_z(&status.stdout)
    } else {
        RepoStatus::default()
    };

    let remote_output = run_git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%1f%(objectname:short)%1f%(committerdate:unix)%1f%(symref)%1f%(contents:subject)",
            "refs/remotes",
        ],
        &[],
    )
    .await?;
    let mut remote_branches = Vec::new();
    if remote_output.success {
        for line in remote_output.stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut fields = line.splitn(5, '\u{1f}');
            let name = fields.next().unwrap_or_default().trim().to_string();
            if name.is_empty() || name.ends_with("/HEAD") {
                continue;
            }
            let commit = fields.next().unwrap_or_default().trim().to_string();
            let timestamp = fields
                .next()
                .unwrap_or_default()
                .trim()
                .parse::<i64>()
                .unwrap_or(0);
            let symref = fields.next().unwrap_or_default().trim().to_string();
            if !symref.is_empty() {
                continue;
            }
            let subject = fields.next().unwrap_or_default().to_string();
            let (remote, branch_name) = match name.split_once('/') {
                Some((r, b)) => (r.to_string(), b.to_string()),
                None => (String::new(), name.clone()),
            };
            remote_branches.push(json!({
                "name": name,
                "remote": remote,
                "branch": branch_name,
                "commit": commit,
                "timestamp": timestamp,
                "subject": subject,
            }));
            if remote_branches.len() >= MAX_BRANCHES {
                break;
            }
        }
    }

    Ok(json!({
        "is_repo": true,
        "branch": parsed.branch,
        "detached": parsed.detached,
        "branches": branches,
        "remote_branches": remote_branches,
        "truncated": truncated,
    }))
}

/// `GET /api/git/log?limit=&path=&branch=&all=&search=` - the commit history.
pub async fn log(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let limit = query_usize(&query, "limit")?
        .unwrap_or(DEFAULT_LOG_ENTRIES)
        .clamp(1, MAX_LOG_ENTRIES);
    let pathspec = match query_str(&query, "path") {
        Some(path) => Some(workspace_pathspec(&workspace, path)?),
        None => None,
    };
    let all = query_bool(&query, "all")?;
    let branch = match query_str(&query, "branch") {
        Some(b) if !b.trim().is_empty() => {
            let trimmed = b.trim();
            validate_ref(trimmed)?;
            Some(trimmed.to_string())
        }
        _ => None,
    };
    let search = match query_str(&query, "search") {
        Some(s) if !s.trim().is_empty() => {
            let trimmed = s.trim();
            if trimmed.chars().count() > MAX_SEARCH_CHARS {
                return Err(ApiError::bad_request(format!(
                    "`search` may be at most {MAX_SEARCH_CHARS} characters"
                )));
            }
            Some(trimmed.to_string())
        }
        _ => None,
    };
    let root = workspace.root().to_path_buf();

    let count = format!("-n{limit}");
    let format =
        format!("--format=%H{FIELD}%h{FIELD}%an{FIELD}%ae{FIELD}%at{FIELD}%P{FIELD}%D{FIELD}%s");
    let mut args: Vec<String> = vec!["log".to_string(), count, format, "--no-color".to_string()];
    if all {
        args.push("--all".to_string());
    } else if let Some(ref b) = branch {
        args.push(b.clone());
    }
    if let Some(ref q) = search {
        args.push("--fixed-strings".to_string());
        args.push("--regexp-ignore-case".to_string());
        args.push(format!("--grep={q}"));
    }
    if let Some(spec) = pathspec {
        args.push("--".to_string());
        args.push(spec);
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git(&root, &borrowed, &[]).await?;
    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Ok(Json(json!({ "is_repo": false, "commits": [] })));
        }
        // A repository with no commits yet is an ordinary empty history, not an
        // error: `git log` refuses to run at all in that state.
        if output
            .stderr
            .to_ascii_lowercase()
            .contains("does not have any commits")
            || output.stderr.contains("unknown revision")
            || output.stderr.contains("ambiguous argument 'HEAD'")
        {
            return Ok(Json(json!({ "is_repo": true, "commits": [] })));
        }
        return Err(failure(&state, "log", &output));
    }

    let mut commits = Vec::new();
    for line in output.stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(8, '\u{1f}');
        let id = fields.next().unwrap_or_default().trim().to_string();
        if id.is_empty() {
            continue;
        }
        let short_id = fields.next().unwrap_or_default().to_string();
        let author = fields.next().unwrap_or_default().to_string();
        let email = fields.next().unwrap_or_default().to_string();
        let timestamp = fields
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let parents: Vec<String> = fields
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let refs = fields.next().unwrap_or_default().to_string();
        let subject = fields.next().unwrap_or_default().to_string();
        commits.push(json!({
            "id": id,
            "short_id": short_id,
            "author": author,
            "email": email,
            "timestamp": timestamp,
            "parents": parents,
            "refs": refs,
            "subject": subject,
        }));
    }
    Ok(Json(json!({
        "is_repo": true,
        "commits": commits,
        "truncated": output.stdout_truncated,
    })))
}

/// `GET /api/git/commits/:sha` - one commit with its file stat and its patch.
pub async fn commit_detail(
    State(state): State<AppState>,
    Path(sha): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let sha = validate_commit_sha(&sha)?;
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();

    let format = format!(
        "--format=%H{FIELD}%h{FIELD}%an{FIELD}%ae{FIELD}%at{FIELD}%P{FIELD}%D{FIELD}%s{FIELD}%b"
    );
    let meta = run_git(
        &root,
        &["show", "-s", &format, "--no-color", sha.as_str(), "--"],
        &[],
    )
    .await?;
    if !meta.success {
        if is_not_a_repo(&meta.stderr) {
            return Err(ApiError::bad_request(
                "the workspace is not a git repository",
            ));
        }
        // An unknown object is the caller's mistake, not a server fault.
        return Err(ApiError::not_found(format!(
            "no commit `{sha}` in this repository"
        )));
    }
    let line = meta.stdout.lines().next().unwrap_or_default();
    let mut fields = line.splitn(9, '\u{1f}');
    let id = fields.next().unwrap_or_default().trim().to_string();
    let short_id = fields.next().unwrap_or_default().to_string();
    let author = fields.next().unwrap_or_default().to_string();
    let email = fields.next().unwrap_or_default().to_string();
    let timestamp = fields
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    let parents: Vec<String> = fields
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let refs = fields.next().unwrap_or_default().to_string();
    let subject = fields.next().unwrap_or_default().to_string();
    let body = fields.next().unwrap_or_default().trim_end().to_string();

    // `--numstat` gives machine-readable per-file counts (and a `-`/`-` pair for
    // a binary file) instead of the human `--stat` graph.
    let stat = run_git(
        &root,
        &[
            "show",
            "--numstat",
            "--format=",
            "--no-color",
            sha.as_str(),
            "--",
        ],
        &[],
    )
    .await?;
    if !stat.success {
        return Err(failure(&state, "show --numstat", &stat));
    }
    let mut files = Vec::new();
    for line in stat.stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(additions) = parts.next() else {
            continue;
        };
        let Some(deletions) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else { continue };
        files.push(json!({
            "path": path,
            "additions": additions.parse::<u64>().ok(),
            "deletions": deletions.parse::<u64>().ok(),
            // `null` counts are how git reports a binary file.
            "binary": additions == "-" || deletions == "-",
        }));
    }

    let patch = run_git(
        &root,
        &[
            "show",
            "--patch",
            "--format=",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            sha.as_str(),
            "--",
        ],
        &[],
    )
    .await?;
    if !patch.success {
        return Err(failure(&state, "show", &patch));
    }
    let (diff, truncated) = truncate_bytes(&patch.stdout, MAX_DIFF_BYTES);

    Ok(Json(json!({
        "id": id,
        "short_id": short_id,
        "author": author,
        "email": email,
        "timestamp": timestamp,
        "parents": parents,
        "refs": refs,
        "subject": subject,
        "body": body,
        "files": files,
        "diff": diff,
        "truncated": truncated,
    })))
}

/// `GET /api/git/stash` - the stash stack, newest first.
pub async fn stash_list(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    Ok(Json(read_stashes(&state, &root).await?))
}

async fn read_stashes(state: &AppState, root: &FsPath) -> ApiResult<Value> {
    let format = format!("--format=%gd{FIELD}%H{FIELD}%at{FIELD}%gs");
    let output = run_git(root, &["stash", "list", &format], &[]).await?;
    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Ok(json!({ "is_repo": false, "stashes": [] }));
        }
        return Err(failure(state, "stash list", &output));
    }
    let mut stashes = Vec::new();
    for line in output.stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(4, '\u{1f}');
        let reference = fields.next().unwrap_or_default().trim().to_string();
        if reference.is_empty() {
            continue;
        }
        let commit = fields.next().unwrap_or_default().to_string();
        let timestamp = fields
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let subject = fields.next().unwrap_or_default().to_string();
        stashes.push(json!({
            "ref": reference,
            "index": stash_index_of(&reference),
            "commit": commit,
            "timestamp": timestamp,
            "subject": subject,
        }));
        if stashes.len() >= MAX_STASHES {
            break;
        }
    }
    Ok(json!({
        "is_repo": true,
        "stashes": stashes,
        "truncated": output.stdout_truncated,
    }))
}

/// `GET /api/git/stash/:index` - one stash's file stat and unified diff.
pub async fn stash_detail(
    State(state): State<AppState>,
    Path(index): Path<u32>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    if index >= MAX_STASHES as u32 {
        return Err(ApiError::bad_request(format!(
            "`index` must be below {MAX_STASHES}"
        )));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let reference = format!("stash@{{{index}}}");

    let stat = run_git(
        &root,
        &[
            "stash",
            "show",
            "--numstat",
            "--include-untracked",
            "--no-color",
            &reference,
        ],
        &[],
    )
    .await?;
    if !stat.success {
        if is_not_a_repo(&stat.stderr) {
            return Err(ApiError::bad_request(
                "the workspace is not a git repository",
            ));
        }
        return Err(ApiError::not_found(format!(
            "no stash entry `{reference}` in this repository"
        )));
    }
    let mut files = Vec::new();
    for line in stat.stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(additions) = parts.next() else {
            continue;
        };
        let Some(deletions) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else { continue };
        files.push(json!({
            "path": path,
            "additions": additions.parse::<u64>().ok(),
            "deletions": deletions.parse::<u64>().ok(),
            "binary": additions == "-" || deletions == "-",
        }));
    }

    let patch = run_git(
        &root,
        &[
            "stash",
            "show",
            "-p",
            "--include-untracked",
            "--no-color",
            &reference,
        ],
        &[],
    )
    .await?;
    if !patch.success {
        return Err(failure(&state, "stash show", &patch));
    }
    let (diff, truncated) = truncate_bytes(&patch.stdout, MAX_DIFF_BYTES);
    Ok(Json(json!({
        "ref": reference,
        "index": index,
        "files": files,
        "diff": diff,
        "truncated": truncated,
    })))
}

/// `GET /api/git/tags` - repository tags, newest first.
pub async fn tags(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    Ok(Json(read_tags(&state, &root).await?))
}

async fn read_tags(state: &AppState, root: &FsPath) -> ApiResult<Value> {
    let format = "--format=%(refname:short)%1f%(objecttype)%1f%(*objectname:short)%1f%(objectname:short)%1f%(creatordate:unix)%1f%(contents:subject)";
    let output = run_git(
        root,
        &["for-each-ref", "--sort=-creatordate", format, "refs/tags"],
        &[],
    )
    .await?;
    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Ok(json!({ "is_repo": false, "tags": [], "truncated": false }));
        }
        return Err(failure(state, "for-each-ref refs/tags", &output));
    }
    let mut tags = Vec::new();
    for line in output.stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(6, '\u{1f}');
        let name = fields.next().unwrap_or_default().trim().to_string();
        if name.is_empty() {
            continue;
        }
        let obj_type = fields.next().unwrap_or_default().trim();
        let target_short = fields.next().unwrap_or_default().trim();
        let obj_short = fields.next().unwrap_or_default().trim();
        let timestamp = fields
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let subject = fields.next().unwrap_or_default().to_string();
        let annotated = obj_type == "tag";
        let commit = if target_short.is_empty() {
            obj_short.to_string()
        } else {
            target_short.to_string()
        };
        tags.push(json!({
            "name": name,
            "commit": commit,
            "annotated": annotated,
            "timestamp": timestamp,
            "subject": subject,
        }));
        if tags.len() >= MAX_TAGS {
            break;
        }
    }
    let truncated = output.stdout_truncated || tags.len() >= MAX_TAGS;
    Ok(json!({
        "is_repo": true,
        "tags": tags,
        "truncated": truncated,
    }))
}

/// `GET /api/git/remotes` - configured git remotes with sanitized URLs and GitHub slug info.
pub async fn remotes(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    Ok(Json(read_remotes(&state, &root).await?))
}

pub(crate) async fn read_remotes(state: &AppState, root: &FsPath) -> ApiResult<Value> {
    let output = run_git(root, &["remote", "-v"], &[]).await?;
    if !output.success {
        if is_not_a_repo(&output.stderr) {
            return Ok(json!({ "is_repo": false, "remotes": [] }));
        }
        return Err(failure(state, "remote -v", &output));
    }
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();
    for line in output.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once('\t') else {
            continue;
        };
        let name = name.trim().to_string();
        let (url, kind) = if let Some(u) = rest.strip_suffix(" (fetch)") {
            (u.trim(), "fetch")
        } else if let Some(u) = rest.strip_suffix(" (push)") {
            (u.trim(), "push")
        } else {
            (rest.trim(), "fetch")
        };
        if !map.contains_key(&name) {
            order.push(name.clone());
        }
        let entry = map.entry(name).or_insert((None, None));
        let clean = state.redact(&sanitize_remote_url(url));
        if kind == "push" {
            entry.1 = Some(clean);
        } else {
            entry.0 = Some(clean);
        }
    }
    let mut list = Vec::with_capacity(order.len());
    for name in order {
        let (fetch_url, push_url) = map.remove(&name).unwrap_or((None, None));
        let slug = fetch_url
            .as_deref()
            .or(push_url.as_deref())
            .and_then(parse_github_slug);
        list.push(json!({
            "name": name,
            "fetch_url": fetch_url,
            "push_url": push_url,
            "github": slug.map(|(owner, repo)| json!({ "owner": owner, "repo": repo })),
        }));
    }
    Ok(json!({
        "is_repo": true,
        "remotes": list,
    }))
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Body of `POST /api/git/stage` and `POST /api/git/unstage`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsBody {
    /// Workspace-relative paths (or absolute ones inside the workspace). A path
    /// that resolves outside the workspace is refused before git runs.
    #[serde(default)]
    paths: Vec<String>,
    /// Act on the whole workspace instead of naming paths.
    #[serde(default)]
    all: bool,
}

/// Body of `POST /api/git/discard`.
///
/// Discarding is the one destructive verb here, so it needs `confirm: true` in
/// addition to the paths: an accidental click is cheap to avoid and impossible to
/// undo (`git restore` overwrites the working tree, and an untracked file is
/// deleted from disk).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscardBody {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    all: bool,
    /// `false` (default) reverts the working tree to the index - the "Changes"
    /// group. `true` reverts both the index and the working tree to `HEAD` - the
    /// "Staged" group.
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    confirm: bool,
}

/// Body of `POST /api/git/commit`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitBody {
    /// The message. May be multi-line; `git commit -m` keeps it verbatim. Empty is
    /// refused unless `amend` reuses the previous message.
    #[serde(default)]
    message: String,
    /// Rewrite the previous commit instead of adding one.
    #[serde(default)]
    amend: bool,
    /// Stage every change first (`git add -A`), for "commit all".
    #[serde(default)]
    stage_all: bool,
}

/// Body of `POST /api/git/branches`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBranchBody {
    name: String,
    /// Start point; defaults to `HEAD`.
    #[serde(default)]
    start: Option<String>,
    /// Switch to the new branch after creating it (`git switch -c`).
    #[serde(default)]
    checkout: bool,
}

/// Body of `POST /api/git/checkout`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutBody {
    name: String,
    /// Check out a remote-tracking branch (`git switch --track <name>`).
    #[serde(default)]
    track: bool,
    /// Detach HEAD at a commit/tag (`git switch --detach <name>`).
    #[serde(default)]
    detach: bool,
}

/// Body of `POST /api/git/branches/rename`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBranchBody {
    old_name: String,
    new_name: String,
}

/// Body of `POST /api/git/init`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitBody {
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    initial_commit: bool,
    #[serde(default)]
    commit_message: Option<String>,
    #[serde(default)]
    gitignore_template: Option<String>,
}

/// Body of `POST /api/git/config`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigIdentityBody {
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    user_email: Option<String>,
}

/// Body of `POST /api/git/remotes`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteBody {
    /// `add`, `set_url`, `remove`, or `rename`.
    action: String,
    name: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
}

/// Body of `POST /api/git/fetch`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    prune: bool,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    tags: bool,
}

/// Body of `POST /api/git/pull`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PullBody {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    rebase: bool,
}

/// Body of `POST /api/git/push`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PushBody {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    set_upstream: bool,
    #[serde(default)]
    force_with_lease: bool,
    #[serde(default)]
    tags: bool,
    #[serde(default)]
    confirm: bool,
}

/// Body of `POST /api/git/merge`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeBody {
    #[serde(default)]
    branch: Option<String>,
    /// `merge` (default), `continue`, or `abort`.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    no_ff: bool,
    #[serde(default)]
    squash: bool,
}

/// Body of `POST /api/git/rebase`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RebaseBody {
    #[serde(default)]
    branch: Option<String>,
    /// `rebase` (default), `continue`, `abort`, or `skip`.
    #[serde(default)]
    action: Option<String>,
}

/// Body of `POST /api/git/cherry-pick`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CherryPickBody {
    #[serde(default)]
    sha: Option<String>,
    /// `pick` (default), `continue`, `abort`, or `skip`.
    #[serde(default)]
    action: Option<String>,
}

/// Body of `POST /api/git/revert`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevertBody {
    sha: String,
}

/// Body of `POST /api/git/reset`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetBody {
    target: String,
    /// `soft`, `mixed` (default), or `hard`.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    confirm: bool,
}

/// Body of `POST /api/git/resolve`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    path: String,
    /// `ours` or `theirs`.
    strategy: String,
}

/// Body of `POST /api/git/tags`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagBody {
    /// `create`, `delete`, or `push`.
    action: String,
    name: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    remote: Option<String>,
}

/// Body of `POST /api/git/branches/delete`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBranchBody {
    name: String,
    /// Delete even when unmerged (`git branch -D`). Needs `confirm`.
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

/// Body of `POST /api/git/stash`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StashBody {
    /// `push`, `pop`, `apply` or `drop`.
    action: String,
    /// Which `stash@{N}` to pop/apply/drop. Defaults to 0 (the newest).
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    message: Option<String>,
    /// Include untracked files in a `push` (`git stash push -u`).
    #[serde(default)]
    include_untracked: bool,
    /// Required to `drop`: it deletes the stash entry for good.
    #[serde(default)]
    confirm: bool,
}

pub async fn stage(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: PathsBody = parse_body(&body)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let specs = select_pathspecs(&workspace, &body.paths, body.all)?;
    let mut args: Vec<String> = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
    args.extend(specs);
    let root = workspace.root().to_path_buf();
    run_mutation(&state, &root, &args, "add").await?;
    mutation_reply(&state, &root, None, Value::Null).await
}

pub async fn unstage(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: PathsBody = parse_body(&body)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let specs = select_pathspecs(&workspace, &body.paths, body.all)?;
    let root = workspace.root().to_path_buf();
    // `git restore --staged` needs a HEAD to restore *from*; in a repository whose
    // first commit has not happened yet there is nothing to restore, so the entry
    // is removed from the index instead. `--ignore-unmatch` keeps a mixed path list
    // (some staged, some not) from failing the whole request.
    let mut args: Vec<String> = if head_exists(&root).await? {
        vec![
            "restore".to_string(),
            "--staged".to_string(),
            "--".to_string(),
        ]
    } else {
        vec![
            "rm".to_string(),
            "--cached".to_string(),
            "-r".to_string(),
            "--quiet".to_string(),
            "--ignore-unmatch".to_string(),
            "--".to_string(),
        ]
    };
    args.extend(specs);
    run_mutation(&state, &root, &args, "restore --staged").await?;
    mutation_reply(&state, &root, None, Value::Null).await
}

pub async fn discard(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: DiscardBody = parse_body(&body)?;
    if !body.confirm {
        return Err(ApiError::bad_request(
            "discarding changes is destructive; send `confirm: true` to proceed",
        ));
    }
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let specs = select_pathspecs(&workspace, &body.paths, body.all)?;
    let root = workspace.root().to_path_buf();

    if body.staged {
        if !head_exists(&root).await? {
            return Err(ApiError::bad_request(
                "there is no commit to restore staged changes from yet; unstage them instead",
            ));
        }
        // Discard the staged side *and* the working-tree side, which is what
        // "discard all changes in this file" means to the person clicking it.
        let mut args: Vec<String> = ["restore", "--source=HEAD", "--staged", "--worktree", "--"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.extend(specs);
        run_mutation(&state, &root, &args, "restore").await?;
        return mutation_reply(&state, &root, None, Value::Null).await;
    }

    // The untracked half cannot be reverted by git: there is no index entry and no
    // HEAD blob to restore, so the file is deleted. That is what the user asked for
    // ("discard this new file"), and it only ever deletes a regular file that
    // resolves *inside* the workspace.
    let live = read_status(&state, &root).await?;
    let untracked: Vec<String> = live
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter(|file| file["untracked"].as_bool().unwrap_or(false))
                .filter_map(|file| file["path"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let (untracked_specs, tracked_specs): (Vec<String>, Vec<String>) = specs
        .into_iter()
        .partition(|spec| untracked.iter().any(|path| path == spec));

    let mut deleted = 0usize;
    for spec in &untracked_specs {
        let absolute = workspace.resolve(spec)?;
        let meta = std::fs::symlink_metadata(&absolute).map_err(|e| {
            ApiError::internal(format!("could not inspect {}: {e}", absolute.display()))
        })?;
        if !meta.file_type().is_file() {
            return Err(ApiError::bad_request(format!(
                "`{spec}` is not a regular file; remove it from the Files tab"
            )));
        }
        std::fs::remove_file(&absolute).map_err(|e| {
            ApiError::internal(format!("could not delete {}: {e}", absolute.display()))
        })?;
        deleted += 1;
    }

    if !tracked_specs.is_empty() {
        let mut args: Vec<String> = vec!["restore".to_string(), "--".to_string()];
        args.extend(tracked_specs);
        run_mutation(&state, &root, &args, "restore").await?;
    }

    let message = (deleted > 0).then(|| {
        format!(
            "deleted {deleted} untracked file{}",
            if deleted == 1 { "" } else { "s" }
        )
    });
    mutation_reply(&state, &root, message, Value::Null).await
}

pub async fn commit(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CommitBody = parse_body(&body)?;
    let message = body.message.trim_end().to_string();
    if !body.amend && message.trim().is_empty() {
        return Err(ApiError::bad_request(
            "a commit needs a message; the message was empty",
        ));
    }
    if message.chars().count() > MAX_COMMIT_MESSAGE_CHARS {
        return Err(ApiError::bad_request(format!(
            "the commit message is longer than {MAX_COMMIT_MESSAGE_CHARS} characters"
        )));
    }
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    if body.stage_all {
        run_mutation(
            &state,
            &root,
            &["add", "-A", "--", "."].map(str::to_string),
            "add",
        )
        .await?;
    }

    let mut args: Vec<String> = vec!["commit".to_string()];
    if body.amend {
        args.push("--amend".to_string());
        if message.trim().is_empty() {
            // Amend that keeps the previous message: `-m ""` would replace it.
            args.push("--no-edit".to_string());
        }
    }
    if !message.trim().is_empty() {
        args.push("-m".to_string());
        args.push(message);
    }
    args.push("--".to_string());
    run_mutation(&state, &root, &args, "commit").await?;

    let head = run_git(
        &root,
        &[
            "log",
            "-1",
            &format!("--format=%H{FIELD}%h{FIELD}%s"),
            "--no-color",
        ],
        &[],
    )
    .await?;
    let mut fields = head
        .stdout
        .lines()
        .next()
        .unwrap_or_default()
        .splitn(3, '\u{1f}');
    let commit = json!({
        "id": fields.next().unwrap_or_default().trim(),
        "short_id": fields.next().unwrap_or_default(),
        "subject": fields.next().unwrap_or_default(),
    });
    mutation_reply(&state, &root, None, commit).await
}

pub async fn create_branch(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CreateBranchBody = parse_body(&body)?;
    let name = checked_branch_name(&body.name)?;
    let start = match body.start.as_deref().map(str::trim) {
        Some(start) if !start.is_empty() => {
            validate_ref(start)?;
            Some(start.to_string())
        }
        _ => None,
    };
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let mut args: Vec<String> = if body.checkout {
        vec!["switch".to_string(), "-c".to_string(), name.clone()]
    } else {
        vec!["branch".to_string(), name.clone()]
    };
    if let Some(start) = start {
        args.push("--".to_string());
        args.push(start);
    }
    run_mutation(&state, &root, &args, "branch").await?;
    let message = if body.checkout {
        Some(format!("switched to new branch {name}"))
    } else {
        Some(format!("created branch {name}"))
    };
    mutation_reply(&state, &root, message, Value::Null).await
}

pub async fn checkout(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CheckoutBody = parse_body(&body)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();
    let (args, msg) = if body.detach {
        let target = body.name.trim();
        validate_ref(target)?;
        (
            vec![
                "switch".to_string(),
                "--detach".to_string(),
                target.to_string(),
            ],
            format!("detached HEAD at {target}"),
        )
    } else if body.track {
        let remote_ref = body.name.trim();
        validate_ref(remote_ref)?;
        if !remote_ref.contains('/') {
            return Err(ApiError::bad_request(
                "tracking checkout expects a remote branch like `origin/feat`",
            ));
        }
        (
            vec![
                "switch".to_string(),
                "--track".to_string(),
                remote_ref.to_string(),
            ],
            format!("checked out tracking branch for {remote_ref}"),
        )
    } else {
        let name = checked_branch_name(&body.name)?;
        // `git switch` (not `checkout`) so a name and a path can never be confused;
        // a dirty tree that would be overwritten fails with git's own message, which
        // the caller shows verbatim instead of a guess.
        (
            vec!["switch".to_string(), name.clone()],
            format!("switched to {name}"),
        )
    };
    run_mutation(&state, &root, &args, "switch").await?;
    mutation_reply(&state, &root, Some(msg), Value::Null).await
}

pub async fn rename_branch(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: RenameBranchBody = parse_body(&body)?;
    let old_name = checked_branch_name(&body.old_name)?;
    let new_name = checked_branch_name(&body.new_name)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();
    run_mutation(
        &state,
        &root,
        &[
            "branch".to_string(),
            "-m".to_string(),
            "--".to_string(),
            old_name.clone(),
            new_name.clone(),
        ],
        "branch -m",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("renamed {old_name} to {new_name}")),
        Value::Null,
    )
    .await
}

pub async fn delete_branch(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: DeleteBranchBody = parse_body(&body)?;
    let name = checked_branch_name(&body.name)?;
    if body.force && !body.confirm {
        return Err(ApiError::bad_request(
            "deleting an unmerged branch loses its commits; send `confirm: true` to force it",
        ));
    }
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();
    // `-d` refuses an unmerged branch, which is exactly the guard the UI wants;
    // `-D` is the explicit "yes, really" path behind `force` + `confirm`.
    let flag = if body.force { "-D" } else { "-d" };
    run_mutation(
        &state,
        &root,
        &["branch".to_string(), flag.to_string(), name.clone()],
        "branch",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("deleted branch {name}")),
        Value::Null,
    )
    .await
}

pub async fn stash(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: StashBody = parse_body(&body)?;
    let action = body.action.trim().to_ascii_lowercase();
    let index = body.index.unwrap_or(0);
    if index >= MAX_STASHES as u32 {
        return Err(ApiError::bad_request(format!(
            "`index` must be below {MAX_STASHES}"
        )));
    }
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();
    let reference = format!("stash@{{{index}}}");

    let (args, message) = match action.as_str() {
        "push" => {
            let message = body.message.as_deref().map(str::trim).unwrap_or("");
            if message.chars().count() > MAX_STASH_MESSAGE_CHARS {
                return Err(ApiError::bad_request(format!(
                    "a stash message may be at most {MAX_STASH_MESSAGE_CHARS} characters"
                )));
            }
            let mut args: Vec<String> = vec!["stash".to_string(), "push".to_string()];
            if body.include_untracked {
                args.push("-u".to_string());
            }
            if !message.is_empty() {
                args.push("-m".to_string());
                args.push(message.to_string());
            }
            args.push("--".to_string());
            args.push(".".to_string());
            (args, Some("stashed the working tree".to_string()))
        }
        "pop" | "apply" => (
            vec![
                "stash".to_string(),
                action.clone(),
                "--quiet".to_string(),
                reference.clone(),
            ],
            Some(format!("{action}ed {reference}")),
        ),
        "drop" => {
            if !body.confirm {
                return Err(ApiError::bad_request(
                    "dropping a stash deletes it; send `confirm: true` to proceed",
                ));
            }
            (
                vec![
                    "stash".to_string(),
                    "drop".to_string(),
                    "--quiet".to_string(),
                    reference.clone(),
                ],
                Some(format!("dropped {reference}")),
            )
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of push, pop, apply, drop (got `{other}`)"
            )))
        }
    };

    run_mutation(&state, &root, &args, "stash").await?;
    // The message for pop/apply/drop names the entry, which is otherwise only in
    // the git output; dropping it would make the action invisible in the UI.
    let message = message.map(|m| m.replace("applyed", "applied").replace("poped", "popped"));
    mutation_reply(&state, &root, message, Value::Null).await
}

pub async fn init(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: InitBody = if body.is_empty() {
        InitBody::default()
    } else {
        parse_body(&body)?
    };
    let branch = match body.default_branch.as_deref().map(str::trim) {
        Some(b) if !b.is_empty() => checked_branch_name(b)?,
        _ => "main".to_string(),
    };
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    if root.join(".git").exists() {
        return Err(ApiError::bad_request(
            "this workspace is already a git repository",
        ));
    }

    run_mutation(
        &state,
        &root,
        &["init".to_string(), "-b".to_string(), branch.clone()],
        "init",
    )
    .await?;

    if let Some(template) = body.gitignore_template.as_deref().map(str::trim) {
        if !template.is_empty() && template != "none" {
            let content = gitignore_for_template(template).ok_or_else(|| {
                ApiError::bad_request(format!("unknown .gitignore template `{template}`"))
            })?;
            let target = root.join(".gitignore");
            if !target.exists() {
                std::fs::write(&target, content)
                    .map_err(|e| ApiError::internal(format!("could not write .gitignore: {e}")))?;
            }
        }
    }

    let mut commit_info = Value::Null;
    if body.initial_commit {
        let msg = body
            .commit_message
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("chore: initial commit")
            .to_string();
        if msg.chars().count() > MAX_COMMIT_MESSAGE_CHARS {
            return Err(ApiError::bad_request(format!(
                "the commit message is longer than {MAX_COMMIT_MESSAGE_CHARS} characters"
            )));
        }
        run_mutation(
            &state,
            &root,
            &[
                "add".to_string(),
                "-A".to_string(),
                "--".to_string(),
                ".".to_string(),
            ],
            "add",
        )
        .await?;
        run_mutation(
            &state,
            &root,
            &[
                "commit".to_string(),
                "--allow-empty".to_string(),
                "-m".to_string(),
                msg,
            ],
            "commit",
        )
        .await?;
        if let Ok(head) = run_git(
            &root,
            &[
                "log",
                "-1",
                &format!("--format=%H{FIELD}%h{FIELD}%s"),
                "--no-color",
            ],
            &[],
        )
        .await
        {
            let mut fields = head
                .stdout
                .lines()
                .next()
                .unwrap_or_default()
                .splitn(3, '\u{1f}');
            commit_info = json!({
                "id": fields.next().unwrap_or_default().trim(),
                "short_id": fields.next().unwrap_or_default(),
                "subject": fields.next().unwrap_or_default(),
            });
        }
    }

    mutation_reply(
        &state,
        &root,
        Some(format!("initialized git repository on `{branch}`")),
        commit_info,
    )
    .await
}

pub async fn config_identity(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: ConfigIdentityBody = parse_body(&body)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let name = body
        .user_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let email = body
        .user_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if name.is_none() && email.is_none() {
        return Err(ApiError::bad_request(
            "provide `user_name` and/or `user_email`",
        ));
    }
    for (label, val) in [("user_name", name), ("user_email", email)] {
        if let Some(v) = val {
            if v.chars().count() > 200 || v.contains('\0') || v.contains('\n') || v.contains('\r') {
                return Err(ApiError::bad_request(format!(
                    "`{label}` must be a single line of at most 200 characters"
                )));
            }
        }
    }
    if let Some(name) = name {
        run_mutation(
            &state,
            &root,
            &[
                "config".to_string(),
                "user.name".to_string(),
                name.to_string(),
            ],
            "config user.name",
        )
        .await?;
    }
    if let Some(email) = email {
        run_mutation(
            &state,
            &root,
            &[
                "config".to_string(),
                "user.email".to_string(),
                email.to_string(),
            ],
            "config user.email",
        )
        .await?;
    }
    mutation_reply(
        &state,
        &root,
        Some("updated repository git identity".to_string()),
        Value::Null,
    )
    .await
}

pub async fn manage_remote(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: RemoteBody = parse_body(&body)?;
    let action = body.action.trim().to_ascii_lowercase();
    let name = checked_remote_name(&body.name)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let message = match action.as_str() {
        "add" => {
            let url = checked_remote_url(body.url.as_deref().unwrap_or(""))?;
            run_mutation(
                &state,
                &root,
                &[
                    "remote".to_string(),
                    "add".to_string(),
                    "--".to_string(),
                    name.clone(),
                    url,
                ],
                "remote add",
            )
            .await?;
            format!("added remote `{name}`")
        }
        "set_url" => {
            let url = checked_remote_url(body.url.as_deref().unwrap_or(""))?;
            run_mutation(
                &state,
                &root,
                &[
                    "remote".to_string(),
                    "set-url".to_string(),
                    "--".to_string(),
                    name.clone(),
                    url,
                ],
                "remote set-url",
            )
            .await?;
            format!("updated URL for remote `{name}`")
        }
        "remove" => {
            run_mutation(
                &state,
                &root,
                &[
                    "remote".to_string(),
                    "remove".to_string(),
                    "--".to_string(),
                    name.clone(),
                ],
                "remote remove",
            )
            .await?;
            format!("removed remote `{name}`")
        }
        "rename" => {
            let new_name = checked_remote_name(body.new_name.as_deref().unwrap_or(""))?;
            run_mutation(
                &state,
                &root,
                &[
                    "remote".to_string(),
                    "rename".to_string(),
                    "--".to_string(),
                    name.clone(),
                    new_name.clone(),
                ],
                "remote rename",
            )
            .await?;
            format!("renamed remote `{name}` to `{new_name}`")
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of add, set_url, remove, rename (got `{other}`)"
            )))
        }
    };

    let status = read_status(&state, &root).await?;
    let remotes_payload = read_remotes(&state, &root).await?;
    Ok(Json(json!({
        "ok": true,
        "message": message,
        "status": status,
        "commit": Value::Null,
        "remotes": remotes_payload["remotes"],
    })))
}

pub async fn fetch(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: FetchBody = if body.is_empty() {
        FetchBody::default()
    } else {
        parse_body(&body)?
    };
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let mut args = vec!["fetch".to_string()];
    if body.prune {
        args.push("--prune".to_string());
    }
    if body.tags {
        args.push("--tags".to_string());
    }
    if body.all {
        args.push("--all".to_string());
    } else if let Some(remote) = body
        .remote
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let name = checked_remote_name(remote)?;
        args.push("--".to_string());
        args.push(name);
    }

    run_network_mutation(&state, &root, &args, "fetch").await?;
    mutation_reply(
        &state,
        &root,
        Some("fetched updates".to_string()),
        Value::Null,
    )
    .await
}

pub async fn pull(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: PullBody = if body.is_empty() {
        PullBody::default()
    } else {
        parse_body(&body)?
    };
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let mut args = vec!["pull".to_string()];
    if body.rebase {
        args.push("--rebase".to_string());
    }
    if let Some(remote) = body
        .remote
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let remote = checked_remote_name(remote)?;
        args.push("--".to_string());
        args.push(remote);
        if let Some(branch) = body
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            args.push(checked_branch_name(branch)?);
        }
    } else if body.branch.as_deref().is_some_and(|b| !b.trim().is_empty()) {
        return Err(ApiError::bad_request(
            "`branch` requires `remote` to be specified",
        ));
    }

    run_network_mutation(&state, &root, &args, "pull").await?;
    mutation_reply(
        &state,
        &root,
        Some("pulled latest changes".to_string()),
        Value::Null,
    )
    .await
}

pub async fn push(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: PushBody = if body.is_empty() {
        PushBody::default()
    } else {
        parse_body(&body)?
    };
    if body.force_with_lease && !body.confirm {
        return Err(ApiError::bad_request(
            "force-pushing rewrites remote history; send `confirm: true` to proceed",
        ));
    }
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let mut args = vec!["push".to_string()];
    if body.set_upstream {
        args.push("-u".to_string());
    }
    if body.force_with_lease {
        args.push("--force-with-lease".to_string());
    }
    if body.tags {
        args.push("--tags".to_string());
    }
    let remote = body
        .remote
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(checked_remote_name)
        .transpose()?;
    let branch = body
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(checked_branch_name)
        .transpose()?;

    if let Some(r) = remote {
        args.push("--".to_string());
        args.push(r);
        if let Some(b) = branch {
            args.push(b);
        }
    } else if let Some(b) = branch {
        args.push("--".to_string());
        args.push("origin".to_string());
        args.push(b);
    } else if body.set_upstream {
        // Determine current branch to push `-u origin <branch>` when no upstream exists.
        let status = read_status(&state, &root).await?;
        let current = status["branch"]
            .as_str()
            .ok_or_else(|| ApiError::bad_request("cannot set upstream on a detached HEAD"))?;
        args.push("--".to_string());
        args.push("origin".to_string());
        args.push(current.to_string());
    }

    run_network_mutation(&state, &root, &args, "push").await?;
    mutation_reply(
        &state,
        &root,
        Some("pushed to remote".to_string()),
        Value::Null,
    )
    .await
}

pub async fn merge(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: MergeBody = parse_body(&body)?;
    let action = body
        .action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("merge")
        .to_ascii_lowercase();
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let (args, msg, extra_env) = match action.as_str() {
        "merge" => {
            let target = body
                .branch
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ApiError::bad_request("`branch` is required to merge"))?;
            validate_ref(target)?;
            let mut args = vec!["merge".to_string(), "--no-edit".to_string()];
            if body.no_ff {
                args.push("--no-ff".to_string());
            }
            if body.squash {
                args.push("--squash".to_string());
            }
            args.push("--".to_string());
            args.push(target.to_string());
            (args, format!("merged `{target}`"), Vec::new())
        }
        "continue" => (
            vec!["merge".to_string(), "--continue".to_string()],
            "completed merge".to_string(),
            vec![("GIT_EDITOR".to_string(), "true".to_string())],
        ),
        "abort" => (
            vec!["merge".to_string(), "--abort".to_string()],
            "aborted merge".to_string(),
            Vec::new(),
        ),
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of merge, continue, abort (got `{other}`)"
            )))
        }
    };

    run_mutation_with_env(&state, &root, &args, "merge", &extra_env).await?;
    mutation_reply(&state, &root, Some(msg), Value::Null).await
}

pub async fn rebase(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: RebaseBody = parse_body(&body)?;
    let action = body
        .action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("rebase")
        .to_ascii_lowercase();
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let (args, msg) = match action.as_str() {
        "rebase" => {
            let target = body
                .branch
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ApiError::bad_request("`branch` is required to rebase"))?;
            validate_ref(target)?;
            (
                vec!["rebase".to_string(), "--".to_string(), target.to_string()],
                format!("rebased onto `{target}`"),
            )
        }
        "continue" => (
            vec!["rebase".to_string(), "--continue".to_string()],
            "continued rebase".to_string(),
        ),
        "abort" => (
            vec!["rebase".to_string(), "--abort".to_string()],
            "aborted rebase".to_string(),
        ),
        "skip" => (
            vec!["rebase".to_string(), "--skip".to_string()],
            "skipped commit in rebase".to_string(),
        ),
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of rebase, continue, abort, skip (got `{other}`)"
            )))
        }
    };

    run_mutation_with_env(
        &state,
        &root,
        &args,
        "rebase",
        &[("GIT_EDITOR".to_string(), "true".to_string())],
    )
    .await?;
    mutation_reply(&state, &root, Some(msg), Value::Null).await
}

pub async fn cherry_pick(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CherryPickBody = parse_body(&body)?;
    let action = body
        .action
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("pick")
        .to_ascii_lowercase();
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let (args, msg, extra_env) = match action.as_str() {
        "pick" => {
            let sha = validate_commit_sha(body.sha.as_deref().unwrap_or(""))?;
            (
                vec!["cherry-pick".to_string(), sha.clone()],
                format!("cherry-picked `{sha}`"),
                Vec::new(),
            )
        }
        "continue" => (
            vec!["cherry-pick".to_string(), "--continue".to_string()],
            "continued cherry-pick".to_string(),
            vec![("GIT_EDITOR".to_string(), "true".to_string())],
        ),
        "abort" => (
            vec!["cherry-pick".to_string(), "--abort".to_string()],
            "aborted cherry-pick".to_string(),
            Vec::new(),
        ),
        "skip" => (
            vec!["cherry-pick".to_string(), "--skip".to_string()],
            "skipped commit in cherry-pick".to_string(),
            Vec::new(),
        ),
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of pick, continue, abort, skip (got `{other}`)"
            )))
        }
    };

    run_mutation_with_env(&state, &root, &args, "cherry-pick", &extra_env).await?;
    mutation_reply(&state, &root, Some(msg), Value::Null).await
}

pub async fn revert_commit(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: RevertBody = parse_body(&body)?;
    let sha = validate_commit_sha(&body.sha)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    run_mutation(
        &state,
        &root,
        &["revert".to_string(), "--no-edit".to_string(), sha.clone()],
        "revert",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("reverted `{sha}`")),
        Value::Null,
    )
    .await
}

pub async fn reset(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: ResetBody = parse_body(&body)?;
    let target = body.target.trim();
    validate_ref(target)?;
    let mode = body
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("mixed")
        .to_ascii_lowercase();
    let flag = match mode.as_str() {
        "soft" => "--soft",
        "mixed" => "--mixed",
        "hard" => {
            if !body.confirm {
                return Err(ApiError::bad_request(
                    "hard reset discards uncommitted changes; send `confirm: true` to proceed",
                ));
            }
            "--hard"
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "`mode` must be soft, mixed, or hard (got `{other}`)"
            )))
        }
    };
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    run_mutation(
        &state,
        &root,
        &["reset".to_string(), flag.to_string(), target.to_string()],
        "reset",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("reset ({mode}) to `{target}`")),
        Value::Null,
    )
    .await
}

pub async fn resolve_conflict(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: ResolveBody = parse_body(&body)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let spec = workspace_pathspec(&workspace, &body.path)?;
    let flag = match body.strategy.trim().to_ascii_lowercase().as_str() {
        "ours" => "--ours",
        "theirs" => "--theirs",
        other => {
            return Err(ApiError::bad_request(format!(
                "`strategy` must be `ours` or `theirs` (got `{other}`)"
            )))
        }
    };
    let root = workspace.root().to_path_buf();
    run_mutation(
        &state,
        &root,
        &[
            "checkout".to_string(),
            flag.to_string(),
            "--".to_string(),
            spec.clone(),
        ],
        "checkout",
    )
    .await?;
    run_mutation(
        &state,
        &root,
        &["add".to_string(), "--".to_string(), spec.clone()],
        "add",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("resolved `{spec}` using {}", body.strategy.trim())),
        Value::Null,
    )
    .await
}

pub async fn manage_tag(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: TagBody = parse_body(&body)?;
    let action = body.action.trim().to_ascii_lowercase();
    let name = checked_tag_name(&body.name)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();

    let msg = match action.as_str() {
        "create" => {
            let mut args = vec!["tag".to_string()];
            if let Some(m) = body
                .message
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if m.chars().count() > MAX_TAG_MESSAGE_CHARS {
                    return Err(ApiError::bad_request(format!(
                        "a tag message may be at most {MAX_TAG_MESSAGE_CHARS} characters"
                    )));
                }
                args.push("-a".to_string());
                args.push(name.clone());
                args.push("-m".to_string());
                args.push(m.to_string());
            } else {
                args.push(name.clone());
            }
            if let Some(target) = body
                .target
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                validate_ref(target)?;
                args.push(target.to_string());
            }
            run_mutation(&state, &root, &args, "tag").await?;
            format!("created tag `{name}`")
        }
        "delete" => {
            run_mutation(
                &state,
                &root,
                &["tag".to_string(), "-d".to_string(), name.clone()],
                "tag -d",
            )
            .await?;
            format!("deleted tag `{name}`")
        }
        "push" => {
            let remote = body
                .remote
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(checked_remote_name)
                .transpose()?
                .unwrap_or_else(|| "origin".to_string());
            run_network_mutation(
                &state,
                &root,
                &[
                    "push".to_string(),
                    "--".to_string(),
                    remote,
                    format!("refs/tags/{name}"),
                ],
                "push tag",
            )
            .await?;
            format!("pushed tag `{name}`")
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be one of create, delete, push (got `{other}`)"
            )))
        }
    };

    mutation_reply(&state, &root, Some(msg), Value::Null).await
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The success envelope every mutation returns.
///
/// `status` is a fresh read, so the caller's model of the working tree is never
/// stale after an action it just took. `commit` carries the new revision for
/// `POST /api/git/commit` and `null` everywhere else.
pub(crate) async fn mutation_reply(
    state: &AppState,
    root: &FsPath,
    message: Option<String>,
    commit: Value,
) -> ApiResult<Json<Value>> {
    let status = read_status(state, root).await?;
    Ok(Json(json!({
        "ok": true,
        "message": message,
        "status": status,
        "commit": commit,
    })))
}

/// Resolve the caller's paths into git pathspecs.
///
/// `all` is the explicit whole-workspace form (`.`, which is inside the workspace
/// by construction); otherwise every path is resolved through [`Workspace`] and,
/// because git is run with `GIT_LITERAL_PATHSPECS=1`, a resolved path can only
/// ever match the file the caller named.
fn select_pathspecs(workspace: &Workspace, paths: &[String], all: bool) -> ApiResult<Vec<String>> {
    if all {
        return Ok(vec![".".to_string()]);
    }
    if paths.is_empty() {
        return Err(ApiError::bad_request(
            "send `paths` naming at least one file, or `all: true` for the whole workspace",
        ));
    }
    if paths.len() > MAX_PATHS_PER_REQUEST {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_PATHS_PER_REQUEST} paths can be sent at once; use `all: true`"
        )));
    }
    let mut specs: Vec<String> = Vec::with_capacity(paths.len());
    for raw in paths {
        let spec = workspace_pathspec(workspace, raw)?;
        if !specs.contains(&spec) {
            specs.push(spec);
        }
    }
    Ok(specs)
}

/// One path -> one workspace-relative pathspec. Refuses the workspace root itself
/// (that is `all: true`) and anything [`Workspace::resolve`] rejects.
fn workspace_pathspec(workspace: &Workspace, raw: &str) -> ApiResult<String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(ApiError::bad_request("a path may not be empty"));
    }
    if path.contains('\0') {
        return Err(ApiError::bad_request("a path may not contain a NUL byte"));
    }
    let absolute = workspace.resolve(path)?;
    let relative = workspace.relative(&absolute);
    if relative == "." {
        return Err(ApiError::bad_request(
            "`.` is the whole workspace; send `all: true` instead of naming it",
        ));
    }
    Ok(relative)
}

/// Refuse a branch name that git would read as an option or reject anyway, so the
/// caller gets the rule instead of git's syntax error.
pub(crate) fn checked_branch_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    validate_branch_name(name).map_err(|reason| {
        ApiError::bad_request(format!("invalid branch name `{name}`: {reason}"))
    })?;
    Ok(name.to_string())
}

pub(crate) fn checked_remote_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_REMOTE_NAME_CHARS {
        return Err(ApiError::bad_request(format!(
            "remote name must be 1-{MAX_REMOTE_NAME_CHARS} characters"
        )));
    }
    if name.starts_with('-') {
        return Err(ApiError::bad_request("remote name may not start with `-`"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ApiError::bad_request(
            "remote name may only contain letters, digits, `.`, `_`, or `-`",
        ));
    }
    Ok(name.to_string())
}

fn checked_remote_url(raw: &str) -> ApiResult<String> {
    let url = raw.trim();
    if url.is_empty() || url.chars().count() > MAX_REMOTE_URL_CHARS {
        return Err(ApiError::bad_request(format!(
            "remote URL must be 1-{MAX_REMOTE_URL_CHARS} characters"
        )));
    }
    if url.starts_with('-') {
        return Err(ApiError::bad_request("remote URL may not start with `-`"));
    }
    if url
        .chars()
        .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
    {
        return Err(ApiError::bad_request(
            "remote URL may not contain whitespace or control characters",
        ));
    }
    Ok(url.to_string())
}

fn checked_tag_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    validate_branch_name(name)
        .map_err(|reason| ApiError::bad_request(format!("invalid tag name `{name}`: {reason}")))?;
    Ok(name.to_string())
}

/// Strip embedded `user:pass@` credentials from `http(s)://` URLs while leaving
/// SSH SCP syntax like `git@github.com:owner/repo.git` intact.
pub(crate) fn sanitize_remote_url(url: &str) -> String {
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            if let Some((authority, path)) = rest.split_once('/') {
                if let Some((_userinfo, host)) = authority.rsplit_once('@') {
                    return format!("{scheme}{host}/{path}");
                }
            } else if let Some((_userinfo, host)) = rest.rsplit_once('@') {
                return format!("{scheme}{host}");
            }
        }
    }
    url.to_string()
}

/// Parse `owner/repo` when the URL targets `github.com`.
pub(crate) fn parse_github_slug(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = if let Some(after) = trimmed.strip_prefix("git@github.com:") {
        after
    } else {
        trimmed
            .strip_prefix("https://github.com/")
            .or_else(|| trimmed.strip_prefix("http://github.com/"))
            .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))
            .or_else(|| trimmed.strip_prefix("git://github.com/"))?
    };
    let clean = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = clean.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

fn gitignore_for_template(template: &str) -> Option<&'static str> {
    match template.to_ascii_lowercase().as_str() {
        "rust" => Some("/target/\nCargo.lock\n.DS_Store\n"),
        "node" => Some("node_modules/\ndist/\n.env\n.env.local\n.DS_Store\n"),
        "python" => Some("__pycache__/\n*.py[cod]\n.venv/\nvenv/\n.env\n.DS_Store\n"),
        "go" => Some("bin/\n*.exe\n*.test\n*.out\n.env\n.DS_Store\n"),
        "general" => Some(".env\n.env.local\n.DS_Store\n*.log\n"),
        _ => None,
    }
}

/// `404`-worthy when the path parameter is not a commit id at all; the hex
/// grammar also keeps `<sha>` from ever being an option.
fn validate_commit_sha(sha: &str) -> ApiResult<String> {
    let sha = sha.trim();
    if sha.len() < 4 || sha.len() > 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "`sha` must be a hexadecimal commit id of 4-64 characters",
        ));
    }
    Ok(sha.to_string())
}

/// The `base=` grammar, delegated to the shared tools crate so the tool and the
/// endpoint cannot accept different refs.
pub(crate) fn validate_ref(base: &str) -> ApiResult<()> {
    validate_revision_arg(base)
        .map_err(|reason| ApiError::bad_request(format!("query parameter `base`: {reason}")))
}

async fn head_exists(root: &FsPath) -> ApiResult<bool> {
    let output = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"], &[]).await?;
    Ok(output.success)
}

/// Whether `pathspec` has an index entry. `git ls-files --error-unmatch` exits 1
/// for an untracked path, which is the answer, not a failure.
async fn tracked_in_index(root: &FsPath, pathspec: &str) -> ApiResult<bool> {
    let output = run_git(root, &["ls-files", "--error-unmatch", "--", pathspec], &[]).await?;
    Ok(output.success)
}

/// Run a mutating git command and turn a non-zero exit into the documented `400`
/// with git's own message (a rejected commit, a dirty checkout, an unmerged
/// branch - all of them are the caller's request being refused, not a server
/// fault).
async fn run_mutation(
    state: &AppState,
    root: &FsPath,
    args: &[String],
    action: &str,
) -> ApiResult<()> {
    run_mutation_with_env(state, root, args, action, &[]).await
}

async fn run_mutation_with_env(
    state: &AppState,
    root: &FsPath,
    args: &[String],
    action: &str,
    extra_env: &[(String, String)],
) -> ApiResult<()> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git_with_timeout(root, &borrowed, extra_env, MUTATION_TIMEOUT).await?;
    if output.success {
        return Ok(());
    }
    Err(failure(state, action, &output))
}

async fn run_network_mutation(
    state: &AppState,
    root: &FsPath,
    args: &[String],
    action: &str,
) -> ApiResult<()> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut env = vec![("GCM_INTERACTIVE".to_string(), "never".to_string())];
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        env.push((
            "GIT_SSH_COMMAND".to_string(),
            "ssh -o BatchMode=yes".to_string(),
        ));
    }
    let output = run_git_with_timeout(root, &borrowed, &env, NETWORK_TIMEOUT).await?;
    if output.success {
        return Ok(());
    }
    Err(failure(state, action, &output))
}

fn failure(state: &AppState, action: &str, output: &GitOutput) -> ApiError {
    if is_not_a_repo(&output.stderr) {
        return ApiError::bad_request("the workspace is not a git repository");
    }
    let stderr = state.redact(&truncate(output.stderr.trim(), MAX_STDERR_CHARS));
    ApiError::bad_request(if stderr.is_empty() {
        format!("git {action} failed")
    } else {
        format!("git {action} failed: {stderr}")
    })
}

async fn run_git(
    root: &FsPath,
    args: &[&str],
    config_overrides: &[(String, String)],
) -> Result<GitOutput, ApiError> {
    run_git_with_timeout(root, args, config_overrides, GIT_TIMEOUT).await
}

/// Spawn `git` with the shared read-only hardening.
///
/// `literal_pathspec_env` is applied to every invocation: for the mutations the
/// argv is the whole point (a pathspec here is always a path the caller named),
/// and for the reads it is inert because no argv in this module contains a
/// pathspec.
async fn run_git_with_timeout(
    root: &FsPath,
    args: &[&str],
    config_overrides: &[(String, String)],
    timeout: Duration,
) -> Result<GitOutput, ApiError> {
    let mut command = tokio::process::Command::new("git");
    command
        // Repository-local config must not run code on a read-only endpoint.
        // Shared with the `git_status`/`git_diff` tools so the two cannot drift.
        .args(SAFE_CONFIG)
        .args(args)
        // Canonical workspace root; no shell, no `cd`.
        .current_dir(root);
    // Never prompt or page, drop system config/attributes, pin the external diff
    // driver, and keep the index untouched.
    for (key, value) in safe_env() {
        command.env(key, value);
    }
    for (key, value) in literal_pathspec_env() {
        command.env(key, value);
    }
    // A mutation has to write the index; the read-only profile's
    // `GIT_OPTIONAL_LOCKS=0` would leave a stale stat cache behind for the next
    // status.
    command.env("GIT_OPTIONAL_LOCKS", "1");
    // An inherited GIT_DIR/GIT_WORK_TREE points git at a different repository
    // than the workspace the UI is showing; an inherited GIT_CONFIG_COUNT would
    // inject configuration the caller did not ask for.
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CONFIG_COUNT");
    // Per-repository neutralisation of `filter.<driver>.*` (see `filter_env`).
    for (key, value) in config_overrides {
        command.env(key, value);
    }
    command
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
            code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            stdout_truncated: stdout_cut,
        })
    };

    match tokio::time::timeout(timeout, run).await {
        Ok(result) => result,
        // `run` is dropped here; `kill_on_drop` terminates the child.
        Err(_) => Err(ApiError::new(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            format!("git did not finish within {}s", timeout.as_secs()),
        )),
    }
}

/// Read a pipe to EOF while retaining at most `limit` bytes. The rest is
/// discarded so the child never blocks on a full pipe.
pub(crate) async fn read_bounded<R>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)>
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

/// Interpret `%(upstream:track)` (`[ahead 1, behind 2]`, `[gone]`, `""`).
fn parse_track(track: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in track.trim_matches(['[', ']']).split(',') {
        let part = part.trim();
        if let Some(count) = part.strip_prefix("ahead ") {
            ahead = count.trim().parse().unwrap_or(0);
        }
        if let Some(count) = part.strip_prefix("behind ") {
            behind = count.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

/// `stash@{3}` -> `3`.
fn stash_index_of(reference: &str) -> u32 {
    reference
        .trim()
        .trim_start_matches("stash@{")
        .trim_end_matches('}')
        .parse()
        .unwrap_or(0)
}

/// Parse `git status --porcelain=v1 -b -z -uall`.
///
/// Each record is `<XY> <path>`; with `-z` a rename or copy is followed by its
/// source as its own NUL-separated record (where the non-`-z` form prints
/// `old -> new`). Paths arrive unquoted, so a name with `"` or a newline cannot be
/// mis-parsed.
fn parse_status_z(stdout: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut records = stdout.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if let Some(rest) = record.strip_prefix("## ") {
            let (branch, detached, upstream, ahead, behind) = parse_branch_header(rest);
            status.branch = branch;
            status.detached = detached;
            status.upstream = upstream;
            status.ahead = ahead;
            status.behind = behind;
            continue;
        }
        if record.len() < 4 {
            continue;
        }
        let index = record.as_bytes()[0] as char;
        let worktree = record.as_bytes()[1] as char;
        let path = record.get(3..).unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        // Consume the source path of a rename/copy so it cannot be read as the
        // next entry - even though the UI shows only the current name.
        let orig_path = if matches!(index, 'R' | 'C') {
            records.next().map(str::to_string)
        } else {
            None
        };
        let untracked = index == '?' && worktree == '?';
        let conflict = matches!(
            (index, worktree),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        );
        let code = [index, worktree];
        let label: String = if untracked {
            "??".to_string()
        } else {
            code.iter().filter(|c| **c != ' ').collect()
        };
        status.files.push(json!({
            "path": path,
            "orig_path": orig_path,
            "index": index.to_string(),
            "worktree": worktree.to_string(),
            "status": if label.is_empty() { " ".to_string() } else { label },
            "staged": index != ' ' && index != '?',
            "unstaged": untracked || worktree != ' ',
            "untracked": untracked,
            "conflict": conflict,
        }));
    }
    status
}

/// `## main...origin/main [ahead 1, behind 2]`, `## HEAD (no branch)`,
/// `## No commits yet on main`.
fn parse_branch_header(rest: &str) -> (Option<String>, bool, Option<String>, u32, u32) {
    let rest = rest.trim();
    if let Some(name) = rest.strip_prefix("No commits yet on ") {
        return (Some(name.trim().to_string()), false, None, 0, 0);
    }
    if rest.starts_with("HEAD (no branch)") || rest == "HEAD" {
        return (None, true, None, 0, 0);
    }
    let (head, track) = match rest.rfind(" [") {
        Some(index) => (
            &rest[..index],
            Some(rest[index + 2..].trim_end_matches(']')),
        ),
        None => (rest, None),
    };
    let (name, upstream) = match head.split_once("...") {
        Some((name, upstream)) => (name, Some(upstream)),
        None => (head, None),
    };
    let (ahead, behind) = parse_track(track.unwrap_or(""));
    let name = name.trim().split(' ').next().unwrap_or("").trim();
    (
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
        false,
        upstream
            .map(str::trim)
            .filter(|upstream| !upstream.is_empty())
            .map(str::to_string),
        ahead,
        behind,
    )
}

pub(crate) fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Byte-bounded truncation that never splits a UTF-8 character.
pub(crate) fn truncate_bytes(value: &str, max_bytes: usize) -> (String, bool) {
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
        // `-z`: NUL between records, and a rename's source as its own record.
        let text = "## main...origin/main\u{0} M src/x.rs\u{0}M  Cargo.toml\u{0}?? new.txt\u{0}R  new.rs\u{0}old.rs\u{0}";
        let status = parse_status_z(text);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.files.len(), 4);
        assert_eq!(status.files[0]["path"], "src/x.rs");
        assert_eq!(status.files[0]["status"], "M");
        assert_eq!(status.files[0]["staged"], false);
        assert_eq!(status.files[0]["unstaged"], true);
        assert_eq!(status.files[1]["staged"], true);
        assert_eq!(status.files[1]["unstaged"], false);
        assert_eq!(status.files[2]["status"], "??");
        assert_eq!(status.files[2]["untracked"], true);
        assert_eq!(status.files[3]["path"], "new.rs");
        assert_eq!(status.files[3]["orig_path"], "old.rs");
    }

    #[test]
    fn a_file_can_be_staged_and_modified_at_once() {
        let status = parse_status_z("## main\u{0}MM both.txt\u{0}AA conflict.txt\u{0}");
        assert_eq!(status.files[0]["staged"], true);
        assert_eq!(status.files[0]["unstaged"], true);
        assert_eq!(status.files[0]["status"], "MM");
        assert_eq!(status.files[1]["conflict"], true);
    }

    #[test]
    fn branch_header_parsing_covers_tracking_and_edge_cases() {
        let (branch, detached, upstream, ahead, behind) =
            parse_branch_header("main...origin/main [ahead 1, behind 2]");
        assert_eq!(branch.as_deref(), Some("main"));
        assert!(!detached);
        assert_eq!(upstream.as_deref(), Some("origin/main"));
        assert_eq!((ahead, behind), (1, 2));

        let (branch, detached, upstream, _, _) = parse_branch_header("HEAD (no branch)");
        assert_eq!(branch, None);
        assert!(detached);
        assert_eq!(upstream, None);

        let (branch, _, upstream, _, _) = parse_branch_header("No commits yet on main");
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(upstream, None);

        let (branch, _, upstream, _, _) = parse_branch_header("feat/x...origin/feat/x");
        assert_eq!(branch.as_deref(), Some("feat/x"));
        assert_eq!(upstream.as_deref(), Some("origin/feat/x"));

        let (branch, _, upstream, _, _) = parse_branch_header("main");
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(upstream, None);
    }

    #[test]
    fn tracking_counts_and_stash_refs_are_parsed() {
        assert_eq!(parse_track("[ahead 3]"), (3, 0));
        assert_eq!(parse_track("[behind 4]"), (0, 4));
        assert_eq!(parse_track("[gone]"), (0, 0));
        assert_eq!(parse_track(""), (0, 0));
        assert_eq!(stash_index_of("stash@{7}"), 7);
        assert_eq!(stash_index_of("stash@{0}"), 0);
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

    #[test]
    fn branch_names_and_commit_ids_are_validated() {
        assert!(checked_branch_name("feat/x-1").is_ok());
        assert!(checked_branch_name("-d").is_err());
        assert!(checked_branch_name("a b").is_err());
        assert!(checked_branch_name("a..b").is_err());

        assert!(validate_commit_sha("deadbeef").is_ok());
        assert!(validate_commit_sha("HEAD").is_err());
        assert!(validate_commit_sha("--output=/tmp/x").is_err());
        assert!(validate_commit_sha("abc").is_err());
        assert!(validate_commit_sha(&"a".repeat(65)).is_err());
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

    #[test]
    fn remote_validation_and_github_slug_parsing() {
        assert!(checked_remote_name("origin").is_ok());
        assert!(checked_remote_name("upstream-2.0").is_ok());
        assert!(checked_remote_name("-flag").is_err());
        assert!(checked_remote_name("bad name").is_err());
        assert!(checked_remote_url("https://github.com/karutoil/karucode.git").is_ok());
        assert!(checked_remote_url("--upload-pack=evil").is_err());
        assert!(checked_remote_url("https://evil.com/a b").is_err());

        assert_eq!(
            sanitize_remote_url("https://x-access-token:ghp_secret@github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(
            sanitize_remote_url("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
        assert_eq!(
            parse_github_slug("https://github.com/karutoil/karucode.git"),
            Some(("karutoil".into(), "karucode".into()))
        );
        assert_eq!(
            parse_github_slug("git@github.com:karutoil/karucode.git"),
            Some(("karutoil".into(), "karucode".into()))
        );
        assert_eq!(parse_github_slug("https://gitlab.com/a/b.git"), None);
    }
}
