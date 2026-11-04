//! Git state and git *mutations* for the web source-control manager.
//!
//! `kc_tools::git` exposes a rendered text report, but the UI needs structured
//! data (per-file index/worktree codes, commit metadata, branch tracking), so the
//! server runs `git` itself and renders JSON. The hardening is *not* duplicated:
//! [`kc_tools::git::SAFE_CONFIG`], [`safe_env`], [`diff_args`],
//! [`literal_pathspec_env`] and the ref/branch validators are the single source of
//! truth shared with the `git_status`/`git_diff` tools and with `kc-tools`'
//! confinement policy, so the HTTP surface cannot drift from the tools.
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
//! * every ref or branch name is validated by
//!   [`kc_tools::git::validate_revision_arg`]/[`validate_branch_name`] before it
//!   sits in a flag position;
//! * each invocation is bounded by a timeout with `kill_on_drop`, and stdio is
//!   drained but kept in memory only up to fixed caps.
//!
//! ## What mutations deliberately do *not* neutralise
//!
//! `git add`, `git commit` and `git stash push` are explicit user actions, and they
//! run the repository's own hooks (`pre-commit`, `commit-msg`, …) and clean/smudge
//! filters exactly like a terminal `git` invocation - a manager that disabled LFS
//! or skipped `pre-commit` would commit something other than what the user asked
//! for. They are therefore *not* routed through [`filter_env`]; the read-only
//! endpoints are. Nothing here reaches the network: fetch/pull/push stay in the
//! terminal, where credentials and prompts already live.

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
const MAX_DIFF_BYTES: usize = 512 * 1024;
const MAX_STDERR_CHARS: usize = 2000;
/// Caps for what is kept in memory; the pipes are still drained so a large
/// repository cannot deadlock the child.
const MAX_GIT_STDOUT: usize = 4 * 1024 * 1024;
const MAX_GIT_STDERR: usize = 64 * 1024;
/// Bounds on what a single response can carry, so a pathological repository
/// cannot turn one GET into tens of megabytes of JSON.
const MAX_BRANCHES: usize = 300;
const MAX_LOG_ENTRIES: usize = 100;
const DEFAULT_LOG_ENTRIES: usize = 30;
const MAX_STASHES: usize = 100;
const MAX_PATHS_PER_REQUEST: usize = 500;
const MAX_COMMIT_MESSAGE_CHARS: usize = 8000;
const MAX_STASH_MESSAGE_CHARS: usize = 400;
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
async fn read_status(state: &AppState, root: &FsPath) -> ApiResult<Value> {
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
            "error": error,
            "files": [],
            "truncated": false,
        })
    };
    match outcome {
        Ok(output) if output.success => {
            let status = parse_status_z(&output.stdout);
            Ok(json!({
                "is_repo": true,
                "branch": status.branch,
                "detached": status.detached,
                "upstream": status.upstream,
                "ahead": status.ahead,
                "behind": status.behind,
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

    Ok(json!({
        "is_repo": true,
        "branch": parsed.branch,
        "detached": parsed.detached,
        "branches": branches,
        "truncated": truncated,
    }))
}

/// `GET /api/git/log?limit=&path=` - the commit history, optionally for one file.
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
    let root = workspace.root().to_path_buf();

    let count = format!("-n{limit}");
    let format =
        format!("--format=%H{FIELD}%h{FIELD}%an{FIELD}%ae{FIELD}%at{FIELD}%P{FIELD}%D{FIELD}%s");
    let mut args: Vec<&str> = vec!["log", &count, &format, "--no-color"];
    let pathspec_ref = pathspec.as_deref();
    if let Some(spec) = pathspec_ref {
        args.push("--");
        args.push(spec);
    }
    let output = run_git(&root, &args, &[]).await?;
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
    let name = checked_branch_name(&body.name)?;
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let root = workspace.root().to_path_buf();
    // `git switch` (not `checkout`) so a name and a path can never be confused;
    // a dirty tree that would be overwritten fails with git's own message, which
    // the caller shows verbatim instead of a guess.
    run_mutation(
        &state,
        &root,
        &["switch".to_string(), name.clone()],
        "switch",
    )
    .await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("switched to {name}")),
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

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The success envelope every mutation returns.
///
/// `status` is a fresh read, so the caller's model of the working tree is never
/// stale after an action it just took. `commit` carries the new revision for
/// `POST /api/git/commit` and `null` everywhere else.
async fn mutation_reply(
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
fn checked_branch_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    validate_branch_name(name).map_err(|reason| {
        ApiError::bad_request(format!("invalid branch name `{name}`: {reason}"))
    })?;
    Ok(name.to_string())
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
fn validate_ref(base: &str) -> ApiResult<()> {
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
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_git_with_timeout(root, &borrowed, &[], MUTATION_TIMEOUT).await?;
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
}
