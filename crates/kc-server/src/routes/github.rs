//! GitHub integration for the source-control manager, driven by the `gh` CLI.
//!
//! Using `gh` delegates GitHub authentication, enterprise host support,
//! pagination and API stability to the user's own `gh auth` session without
//! storing any GitHub token inside karucode. Every command is spawned directly
//! with an explicit argv array (`GH_PROMPT_DISABLED=1`, no shell, bounded output
//! and timeout) and returns structured JSON.

use crate::error::{ApiError, ApiResult};
use crate::routes::git::{
    checked_branch_name, checked_remote_name, mutation_reply, read_remotes, read_status, truncate,
    truncate_bytes, MAX_DIFF_BYTES, MAX_GIT_STDERR, MAX_GIT_STDOUT, MAX_STDERR_CHARS,
};
use crate::routes::{parse_body, query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as FsPath;
use std::process::Stdio;
use std::time::Duration;

const GH_TIMEOUT: Duration = Duration::from_secs(25);
const DEFAULT_GH_LIMIT: usize = 30;
const MAX_GH_LIMIT: usize = 100;
const MAX_TITLE_CHARS: usize = 300;
const MAX_BODY_CHARS: usize = 64_000;
const MAX_REPO_NAME_CHARS: usize = 120;

struct GhOutput {
    installed: bool,
    success: bool,
    stdout: String,
    stderr: String,
}

// ---------------------------------------------------------------------------
// Status & repository creation
// ---------------------------------------------------------------------------

/// `GET /api/git/github/status[?project=]`
///
/// Reports whether `gh` is installed, whether it is authenticated, and (when a
/// GitHub remote is configured) the remote repository metadata. Never fails the
/// panel on missing `gh` or unauthenticated state: those are ordinary states the
/// UI explains.
pub async fn status(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();

    let auth = run_gh(&root, &["auth", "status"]).await?;
    if !auth.installed {
        return Ok(Json(json!({
            "gh_installed": false,
            "authenticated": false,
            "username": Value::Null,
            "remote_slug": Value::Null,
            "repo": Value::Null,
            "error": Value::Null,
        })));
    }

    let combined = format!("{}\n{}", auth.stdout, auth.stderr);
    let authenticated = auth.success;
    let username = if authenticated {
        parse_gh_username(&combined)
    } else {
        None
    };

    let remotes_val = read_remotes(&state, &root).await?;
    let remote_slug = find_github_slug(&remotes_val);

    let mut repo = Value::Null;
    let mut repo_error = Value::Null;
    if authenticated {
        if let Some(slug) = remote_slug.as_deref() {
            let view = run_gh(
                &root,
                &[
                    "repo",
                    "view",
                    slug,
                    "--json",
                    "nameWithOwner,name,owner,url,sshUrl,defaultBranchRef,isPrivate,description,stargazerCount,forkCount",
                ],
            )
            .await?;
            if view.success {
                repo = serde_json::from_str::<Value>(&view.stdout).unwrap_or(Value::Null);
            } else {
                let err = state.redact(&truncate(view.stderr.trim(), MAX_STDERR_CHARS));
                if !err.is_empty() {
                    repo_error = json!(err);
                }
            }
        }
    }

    Ok(Json(json!({
        "gh_installed": true,
        "authenticated": authenticated,
        "username": username,
        "remote_slug": remote_slug,
        "repo": repo,
        "error": repo_error,
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRepoBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// `private` (default), `public`, or `internal`.
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    push: bool,
}

/// `POST /api/git/github/repo` - create a GitHub repository via `gh repo create`.
pub async fn create_repo(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CreateRepoBody = parse_body(&body)?;
    let name = checked_repo_name(&body.name)?;
    let visibility = match body
        .visibility
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("private")
        .to_ascii_lowercase()
        .as_str()
    {
        "private" => "--private",
        "public" => "--public",
        "internal" => "--internal",
        other => {
            return Err(ApiError::bad_request(format!(
                "`visibility` must be private, public, or internal (got `{other}`)"
            )))
        }
    };
    let remote = body
        .remote
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(checked_remote_name)
        .transpose()?
        .unwrap_or_else(|| "origin".to_string());

    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();

    let status_val = read_status(&state, &root).await?;
    if !status_val["is_repo"].as_bool().unwrap_or(false) {
        return Err(ApiError::bad_request(
            "initialize a local git repository before creating a GitHub remote",
        ));
    }
    let has_head = status_val["has_head"].as_bool().unwrap_or(false);

    let mut args = vec![
        "repo".to_string(),
        "create".to_string(),
        name.clone(),
        visibility.to_string(),
        "--source=.".to_string(),
        format!("--remote={remote}"),
    ];
    if let Some(desc) = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if desc.chars().count() > 500 {
            return Err(ApiError::bad_request(
                "`description` may be at most 500 characters",
            ));
        }
        args.push("--description".to_string());
        args.push(desc.to_string());
    }
    if body.push && has_head {
        args.push("--push".to_string());
    }

    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_gh_required(&state, &root, &borrowed, "repo create").await?;
    let url = out
        .stdout
        .lines()
        .find(|line| line.trim().starts_with("http"))
        .map(|s| s.trim().to_string());

    let fresh_status = read_status(&state, &root).await?;
    let remotes_val = read_remotes(&state, &root).await?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("created GitHub repository `{name}`"),
        "url": url,
        "status": fresh_status,
        "commit": Value::Null,
        "remotes": remotes_val["remotes"],
    })))
}

// ---------------------------------------------------------------------------
// Pull Requests
// ---------------------------------------------------------------------------

/// `GET /api/git/github/pulls[?project=&state=open|closed|merged|all&limit=30]`
pub async fn list_pulls(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let pr_state = match query_str(&query, "state").unwrap_or("open") {
        s @ ("open" | "closed" | "merged" | "all") => s,
        other => {
            return Err(ApiError::bad_request(format!(
                "`state` must be open, closed, merged, or all (got `{other}`)"
            )))
        }
    };
    let limit = query_usize(&query, "limit")?
        .unwrap_or(DEFAULT_GH_LIMIT)
        .clamp(1, MAX_GH_LIMIT)
        .to_string();

    let fields = "number,title,state,isDraft,author,headRefName,baseRefName,url,createdAt,updatedAt,labels,reviewDecision,mergeable,additions,deletions,changedFiles,statusCheckRollup,body";
    let out = run_gh_required(
        &state,
        &root,
        &[
            "pr", "list", "--state", pr_state, "--limit", &limit, "--json", fields,
        ],
        "pr list",
    )
    .await?;
    let pulls: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| ApiError::internal(format!("could not parse gh pr list JSON: {e}")))?;
    Ok(Json(json!({ "pulls": pulls })))
}

/// `GET /api/git/github/pulls/:number[?project=]`
pub async fn get_pull(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    let fields = "number,title,body,state,isDraft,author,headRefName,baseRefName,url,createdAt,updatedAt,labels,reviewDecision,mergeable,additions,deletions,changedFiles,commits,comments,reviews,statusCheckRollup,files";
    let out = run_gh_required(
        &state,
        &root,
        &["pr", "view", &num, "--json", fields],
        "pr view",
    )
    .await?;
    let pull: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| ApiError::internal(format!("could not parse gh pr view JSON: {e}")))?;
    Ok(Json(json!({ "pull": pull })))
}

/// `GET /api/git/github/pulls/:number/diff[?project=]`
pub async fn pull_diff(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    let out = run_gh_required(&state, &root, &["pr", "diff", &num], "pr diff").await?;
    let (diff, truncated) = truncate_bytes(&out.stdout, MAX_DIFF_BYTES);
    Ok(Json(json!({ "diff": diff, "truncated": truncated })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePullBody {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    draft: bool,
}

/// `POST /api/git/github/pulls` - create a pull request via `gh pr create`.
pub async fn create_pull(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CreatePullBody = parse_body(&body)?;
    let title = checked_title(&body.title)?;
    let pr_body = checked_body(body.body.as_deref())?;
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();

    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title,
        "--body".to_string(),
        pr_body,
    ];
    if let Some(base) = body
        .base
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("--base".to_string());
        args.push(checked_branch_name(base)?);
    }
    if let Some(head) = body
        .head
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("--head".to_string());
        args.push(checked_branch_name(head)?);
    }
    if body.draft {
        args.push("--draft".to_string());
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_gh_required(&state, &root, &borrowed, "pr create").await?;
    let url = out
        .stdout
        .lines()
        .find(|l| l.trim().starts_with("http"))
        .map(|s| s.trim().to_string());
    Ok(Json(json!({
        "ok": true,
        "message": "created pull request",
        "url": url,
    })))
}

/// `POST /api/git/github/pulls/:number/checkout`
pub async fn checkout_pull(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    run_gh_required(&state, &root, &["pr", "checkout", &num], "pr checkout").await?;
    mutation_reply(
        &state,
        &root,
        Some(format!("checked out PR #{number}")),
        Value::Null,
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergePullBody {
    /// `merge` (default), `squash`, or `rebase`.
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    delete_branch: bool,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/git/github/pulls/:number/merge`
pub async fn merge_pull(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let body: MergePullBody = parse_body(&body)?;
    if !body.confirm {
        return Err(ApiError::bad_request(
            "merging a pull request modifies the remote base branch; send `confirm: true` to proceed",
        ));
    }
    let method_flag = match body
        .method
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("merge")
        .to_ascii_lowercase()
        .as_str()
    {
        "merge" => "--merge",
        "squash" => "--squash",
        "rebase" => "--rebase",
        other => {
            return Err(ApiError::bad_request(format!(
                "`method` must be merge, squash, or rebase (got `{other}`)"
            )))
        }
    };
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    let mut args = vec!["pr", "merge", &num, method_flag];
    if body.delete_branch {
        args.push("--delete-branch");
    }
    run_gh_required(&state, &root, &args, "pr merge").await?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("merged PR #{number}"),
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateActionBody {
    /// `close` or `reopen`.
    action: String,
}

/// `POST /api/git/github/pulls/:number/state`
pub async fn set_pull_state(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let body: StateActionBody = parse_body(&body)?;
    let verb = match body.action.trim().to_ascii_lowercase().as_str() {
        "close" => "close",
        "reopen" => "reopen",
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be close or reopen (got `{other}`)"
            )))
        }
    };
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    run_gh_required(&state, &root, &["pr", verb, &num], "pr state").await?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("{verb}d PR #{number}").replace("closedd", "closed"),
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentBody {
    body: String,
}

/// `POST /api/git/github/pulls/:number/comment`
pub async fn comment_pull(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("PR number must be >= 1"));
    }
    let body: CommentBody = parse_body(&body)?;
    let text = body.body.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("comment `body` may not be empty"));
    }
    if text.chars().count() > MAX_BODY_CHARS {
        return Err(ApiError::bad_request(format!(
            "comment `body` may be at most {MAX_BODY_CHARS} characters"
        )));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    run_gh_required(
        &state,
        &root,
        &["pr", "comment", &num, "--body", text],
        "pr comment",
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("commented on PR #{number}"),
    })))
}

// ---------------------------------------------------------------------------
// Issues
// ---------------------------------------------------------------------------

/// `GET /api/git/github/issues[?project=&state=open|closed|all&limit=30]`
pub async fn list_issues(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let issue_state = match query_str(&query, "state").unwrap_or("open") {
        s @ ("open" | "closed" | "all") => s,
        other => {
            return Err(ApiError::bad_request(format!(
                "`state` must be open, closed, or all (got `{other}`)"
            )))
        }
    };
    let limit = query_usize(&query, "limit")?
        .unwrap_or(DEFAULT_GH_LIMIT)
        .clamp(1, MAX_GH_LIMIT)
        .to_string();

    let fields = "number,title,state,author,url,createdAt,updatedAt,labels,assignees,comments,body";
    let out = run_gh_required(
        &state,
        &root,
        &[
            "issue",
            "list",
            "--state",
            issue_state,
            "--limit",
            &limit,
            "--json",
            fields,
        ],
        "issue list",
    )
    .await?;
    let issues: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| ApiError::internal(format!("could not parse gh issue list JSON: {e}")))?;
    Ok(Json(json!({ "issues": issues })))
}

/// `GET /api/git/github/issues/:number[?project=]`
pub async fn get_issue(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("issue number must be >= 1"));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    let fields = "number,title,body,state,author,url,createdAt,updatedAt,labels,assignees,comments";
    let out = run_gh_required(
        &state,
        &root,
        &["issue", "view", &num, "--json", fields],
        "issue view",
    )
    .await?;
    let issue: Value = serde_json::from_str(&out.stdout)
        .map_err(|e| ApiError::internal(format!("could not parse gh issue view JSON: {e}")))?;
    Ok(Json(json!({ "issue": issue })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIssueBody {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

/// `POST /api/git/github/issues` - create an issue via `gh issue create`.
pub async fn create_issue(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: CreateIssueBody = parse_body(&body)?;
    let title = checked_title(&body.title)?;
    let issue_body = checked_body(body.body.as_deref())?;
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();

    let mut args = vec![
        "issue".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title,
        "--body".to_string(),
        issue_body,
    ];
    for label in body.labels.iter().take(20) {
        let trimmed = label.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('-') && trimmed.len() <= 80 {
            args.push("--label".to_string());
            args.push(trimmed.to_string());
        }
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_gh_required(&state, &root, &borrowed, "issue create").await?;
    let url = out
        .stdout
        .lines()
        .find(|l| l.trim().starts_with("http"))
        .map(|s| s.trim().to_string());
    Ok(Json(json!({
        "ok": true,
        "message": "created issue",
        "url": url,
    })))
}

/// `POST /api/git/github/issues/:number/state`
pub async fn set_issue_state(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("issue number must be >= 1"));
    }
    let body: StateActionBody = parse_body(&body)?;
    let verb = match body.action.trim().to_ascii_lowercase().as_str() {
        "close" => "close",
        "reopen" => "reopen",
        other => {
            return Err(ApiError::bad_request(format!(
                "`action` must be close or reopen (got `{other}`)"
            )))
        }
    };
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    run_gh_required(&state, &root, &["issue", verb, &num], "issue state").await?;
    Ok(Json(json!({
        "ok": true,
        "message": if verb == "close" {
            format!("closed issue #{number}")
        } else {
            format!("reopened issue #{number}")
        },
    })))
}

/// `POST /api/git/github/issues/:number/comment`
pub async fn comment_issue(
    State(state): State<AppState>,
    Path(number): Path<u64>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if number == 0 {
        return Err(ApiError::bad_request("issue number must be >= 1"));
    }
    let body: CommentBody = parse_body(&body)?;
    let text = body.body.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("comment `body` may not be empty"));
    }
    if text.chars().count() > MAX_BODY_CHARS {
        return Err(ApiError::bad_request(format!(
            "comment `body` may be at most {MAX_BODY_CHARS} characters"
        )));
    }
    let root = state
        .workspace_for_project(query_str(&query, "project"))?
        .root()
        .to_path_buf();
    let num = number.to_string();
    run_gh_required(
        &state,
        &root,
        &["issue", "comment", &num, "--body", text],
        "issue comment",
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("commented on issue #{number}"),
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn checked_repo_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_REPO_NAME_CHARS {
        return Err(ApiError::bad_request(format!(
            "repository name must be 1-{MAX_REPO_NAME_CHARS} characters"
        )));
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') || name.contains("..")
    {
        return Err(ApiError::bad_request("invalid GitHub repository name"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    {
        return Err(ApiError::bad_request(
            "repository name may only contain letters, digits, `.`, `_`, `-`, or `/`",
        ));
    }
    if name.matches('/').count() > 1 {
        return Err(ApiError::bad_request(
            "repository name may contain at most one `/` (`owner/repo`)",
        ));
    }
    Ok(name.to_string())
}

fn checked_title(raw: &str) -> ApiResult<String> {
    let title = raw.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request("`title` may not be empty"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(ApiError::bad_request(format!(
            "`title` may be at most {MAX_TITLE_CHARS} characters"
        )));
    }
    if title.contains('\0') || title.contains('\n') || title.contains('\r') {
        return Err(ApiError::bad_request("`title` must be a single line"));
    }
    Ok(title.to_string())
}

fn checked_body(raw: Option<&str>) -> ApiResult<String> {
    let body = raw.unwrap_or("").trim().to_string();
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(ApiError::bad_request(format!(
            "`body` may be at most {MAX_BODY_CHARS} characters"
        )));
    }
    Ok(body)
}

fn find_github_slug(remotes_val: &Value) -> Option<String> {
    let remotes = remotes_val.get("remotes")?.as_array()?;
    // Prefer `origin` when multiple remotes exist.
    let mut fallback: Option<String> = None;
    for remote in remotes {
        let name = remote["name"].as_str().unwrap_or("");
        if let (Some(owner), Some(repo)) = (
            remote["github"]["owner"].as_str(),
            remote["github"]["repo"].as_str(),
        ) {
            let slug = format!("{owner}/{repo}");
            if name == "origin" {
                return Some(slug);
            }
            if fallback.is_none() {
                fallback = Some(slug);
            }
        }
    }
    fallback
}

/// Extract the active GitHub username from `gh auth status` output.
pub(crate) fn parse_gh_username(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(after) = line.split("account ").nth(1) {
            let token = after.split_whitespace().next()?.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

async fn run_gh_required(
    state: &AppState,
    root: &FsPath,
    args: &[&str],
    action: &str,
) -> ApiResult<GhOutput> {
    let out = run_gh(root, args).await?;
    if !out.installed {
        return Err(ApiError::bad_request(
            "`gh` (GitHub CLI) is not installed on this server",
        ));
    }
    if !out.success {
        let stderr = state.redact(&truncate(out.stderr.trim(), MAX_STDERR_CHARS));
        return Err(ApiError::bad_request(if stderr.is_empty() {
            format!("gh {action} failed")
        } else {
            format!("gh {action} failed: {stderr}")
        }));
    }
    Ok(out)
}

async fn run_gh(root: &FsPath, args: &[&str]) -> ApiResult<GhOutput> {
    let mut command = tokio::process::Command::new("gh");
    command
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GhOutput {
                installed: false,
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
        Err(e) => return Err(ApiError::internal(format!("could not run gh: {e}"))),
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("gh stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("gh stderr pipe was not created"))?;

    let run = async move {
        let (stdout, stderr, status) = tokio::join!(
            crate::routes::git::read_bounded(stdout, MAX_GIT_STDOUT),
            crate::routes::git::read_bounded(stderr, MAX_GIT_STDERR),
            child.wait(),
        );
        let status = status.map_err(|e| ApiError::internal(format!("gh process error: {e}")))?;
        let (stdout, _) =
            stdout.map_err(|e| ApiError::internal(format!("could not read gh stdout: {e}")))?;
        let (stderr, _) =
            stderr.map_err(|e| ApiError::internal(format!("could not read gh stderr: {e}")))?;
        Ok::<_, ApiError>(GhOutput {
            installed: true,
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    };

    match tokio::time::timeout(GH_TIMEOUT, run).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::new(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            format!("gh did not finish within {}s", GH_TIMEOUT.as_secs()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gh_auth_status_username() {
        let sample = "github.com\n  ✓ Logged in to github.com account karutoil (/home/u/.config/gh/hosts.yml)\n  - Active account: true\n";
        assert_eq!(parse_gh_username(sample).as_deref(), Some("karutoil"));
        assert_eq!(parse_gh_username("not logged in"), None);
    }

    #[test]
    fn validates_repo_names_and_finds_origin_slug() {
        assert!(checked_repo_name("karucode").is_ok());
        assert!(checked_repo_name("karutoil/karucode").is_ok());
        assert!(checked_repo_name("-evil").is_err());
        assert!(checked_repo_name("../escape").is_err());
        assert!(checked_repo_name("a/b/c").is_err());

        let remotes = json!({
            "remotes": [
                { "name": "upstream", "github": { "owner": "up", "repo": "proj" } },
                { "name": "origin", "github": { "owner": "karutoil", "repo": "karucode" } }
            ]
        });
        assert_eq!(
            find_github_slug(&remotes).as_deref(),
            Some("karutoil/karucode")
        );
    }
}
