//! HTTP handlers, one module per area of `docs/API.md`.

pub mod agents;
pub mod approvals;
pub mod artifacts;
pub mod auth;
pub mod config;
pub mod events;
pub mod git;
pub mod github;
pub mod health;
pub mod info;
pub mod missions;
pub mod models;
pub mod onboarding;
pub mod plugin_install;
pub mod plugins;
pub mod projects;
pub mod rewind;
pub mod sessions;
pub mod skills;
pub mod terminals;
pub mod update;
pub mod workspace;

use crate::app::api_not_found;
use crate::error::{ApiError, ApiResult};
use crate::settings::{authorize_patch, ConfigPatch, ADMIN_TOKEN_HEADER};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;

/// Ceiling for a JSON request body. `MAX_PROMPT_BYTES` (1 MiB, in
/// `routes/sessions.rs`) is the largest documented payload, so the transport
/// limit sits just above it: a body between the two still reaches the handler
/// and gets the documented `400 bad_request` naming the field, instead of a
/// transport-level rejection. axum's own default is 2 MiB, which made that
/// documented 400 unreachable.
pub const MAX_JSON_BODY_BYTES: usize = 1024 * 1024 + 64 * 1024;

/// Ceiling for the raw upload body: the handler's own `MAX_UPLOAD_BYTES` (8 MiB)
/// plus slack, so the handler - not the transport - produces the documented
/// `413 payload_too_large` message that names the limit.
pub const MAX_UPLOAD_BODY_BYTES: usize = workspace::MAX_UPLOAD_BYTES + 64 * 1024;

/// Ceiling for the editor's save body: the handler's own `MAX_WRITE_BYTES`
/// (2 MiB of text) at the worst-case JSON escaping ratio (six bytes per byte,
/// `\u00XX`) plus slack, so the handler - not the transport - produces the
/// documented `413 payload_too_large` that names the limit.
pub const MAX_WRITE_BODY_BYTES: usize = workspace::MAX_WRITE_BYTES * 6 + 64 * 1024;

/// Every `/api` route. Path parameters use axum 0.7's `:name` syntax.
pub fn api_router() -> Router<AppState> {
    // JSON routes: one explicit body limit for the whole group.
    let json_routes = Router::new()
        .route("/health", get(health::health))
        .route(
            "/control/sessions",
            get(crate::control::list_sessions).post(crate::control::create_session),
        )
        .route("/control/overview", get(crate::control::overview))
        .route(
            "/config",
            get(config::get_config).patch(patch_config_guarded),
        )
        .route("/provider/test", post(config::provider_test))
        // Web-UI sign-in. `status`, `login` and the passkey sign-in steps are
        // reachable without a credential (`auth::is_public_path`); everything
        // else in this group needs one.
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/passkey/options", post(auth::passkey_options))
        .route("/auth/passkey/login", post(auth::passkey_login))
        .route("/auth/me", get(auth::me))
        .route("/auth/logout", post(auth::logout))
        .route(
            "/auth/password",
            post(auth::set_password).delete(auth::delete_password),
        )
        .route(
            "/auth/passkeys",
            get(auth::list_passkeys).post(auth::register_passkey),
        )
        .route("/auth/passkeys/options", post(auth::register_options))
        .route(
            "/auth/passkeys/:id",
            axum::routing::patch(auth::rename_passkey).delete(auth::delete_passkey),
        )
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/revoke-all", post(auth::revoke_all_sessions))
        .route(
            "/auth/sessions/:id",
            axum::routing::delete(auth::revoke_session),
        )
        // Application API keys: named, scoped, expiring bearer credentials.
        // `GET`/`POST` on the collection, then per-key verbs. The handlers apply
        // the admin gate; the auth middleware applies the key scope.
        .route(
            "/auth/api-keys",
            get(auth::list_api_keys).post(auth::create_api_key),
        )
        .route(
            "/auth/api-keys/:id",
            axum::routing::patch(auth::update_api_key).delete(auth::revoke_api_key),
        )
        .route("/auth/api-keys/:id/rotate", post(auth::rotate_api_key))
        .route("/onboarding", get(onboarding::get))
        .route("/onboarding/complete", post(onboarding::complete))
        .route("/models/catalog", get(models::catalog))
        .route("/models/lookup", get(models::lookup))
        .route("/stats", get(config::stats))
        .route(
            "/sessions",
            get(sessions::list_sessions)
                .post(sessions::create_session)
                .delete(sessions::sweep_ephemeral_sessions),
        )
        .route(
            "/sessions/:id",
            get(sessions::get_session)
                .patch(sessions::rename_session)
                .delete(sessions::delete_session),
        )
        .route("/sessions/:id/expire", post(sessions::expire_session))
        .route("/sessions/:id/info", get(info::session_info))
        .route("/sessions/:id/messages", post(sessions::send_message))
        .route("/sessions/:id/queues", get(sessions::list_queues))
        .route(
            "/sessions/:id/queues/:queued_id",
            axum::routing::delete(sessions::recall_queued),
        )
        .route("/sessions/:id/cancel", post(sessions::cancel_run))
        .route(
            "/sessions/:id/rewind",
            get(rewind::preview).post(rewind::rewind),
        )
        .route(
            "/sessions/:id/model",
            axum::routing::patch(sessions::set_session_model),
        )
        .route(
            "/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        .route(
            "/projects/:id",
            axum::routing::patch(projects::rename_project).delete(projects::delete_project),
        )
        .route(
            "/projects/:id/preferences",
            axum::routing::patch(projects::update_project_preferences),
        )
        .route("/projects/:id/favicon", get(projects::project_favicon))
        .route("/missions", get(missions::list).post(missions::create))
        .route("/missions/:id", get(missions::get).delete(missions::delete))
        .route("/missions/:id/sessions", post(missions::start_session))
        .route("/missions/:id/coordinator", post(missions::coordinator))
        .route("/missions/:id/tasks", post(missions::create_task))
        .route("/missions/:id/tasks/:task_id", patch(missions::revise_task))
        .route(
            "/missions/:id/tasks/:task_id/replan",
            post(missions::replan_task),
        )
        .route(
            "/missions/:id/tasks/:task_id/verification",
            post(missions::create_verification_task),
        )
        .route(
            "/missions/:id/tasks/:task_id/retry",
            post(missions::retry_task),
        )
        .route(
            "/missions/:id/artifacts",
            get(missions::list_artifacts).post(missions::publish_artifact),
        )
        .route(
            "/missions/:id/evidence",
            get(missions::list_evidence).post(missions::publish_evidence),
        )
        .route("/missions/:id/start", post(missions::start))
        .route("/missions/:id/pause", post(missions::pause))
        .route("/missions/:id/resume", post(missions::resume))
        .route("/missions/:id/cancel", post(missions::cancel))
        .route("/agents", get(agents::list_agents))
        .route("/agents/:id", get(agents::get_agent))
        // One route for the three control verbs: they take the same path shape,
        // the same body (none) and return the same shape, so three routes would be
        // three copies of the same handler with a different constant in each.
        .route("/agents/:id/:action", post(agents::control_agent))
        .route("/approvals", get(approvals::list_approvals))
        .route("/approvals/:id", post(approvals::resolve_approval))
        .route("/workspace/tree", get(workspace::tree))
        .route("/workspace/file", get(workspace::file))
        .route("/workspace/raw", get(workspace::raw))
        // A wildcard, not a query parameter: a rendered page resolves its own
        // `./style.css` and `../logo.png` against this URL's path, so the path
        // has to be the workspace path. See `workspace::preview`.
        .route("/workspace/preview/*rest", get(workspace::preview))
        .route(
            "/workspace/preview-project/:project/*rest",
            get(workspace::preview_project),
        )
        // Chat artifacts: the isolated document a transcript renders a fenced
        // block in. `POST` publishes the source (registered below with its own
        // body limit), `GET` serves it as a page with its own policy — see
        // `crate::artifacts`.
        .route("/artifacts/:id", get(artifacts::get_artifact))
        .merge(git_routes())
        .merge(github_routes())
        .route(
            "/terminals",
            get(terminals::list_terminals).post(terminals::create_terminal),
        )
        .route(
            "/terminals/:id",
            axum::routing::patch(terminals::rename_terminal).delete(terminals::delete_terminal),
        )
        .route("/terminals/:id/restart", post(terminals::restart_terminal))
        .route("/terminals/:id/socket", get(terminals::terminal_socket))
        .route("/plugins", get(plugins::list_plugins))
        .route("/plugins/reload", post(plugins::reload_plugins))
        // Plugin installs: preview (sync) + a pollable job, admin-gated like
        // `POST /api/update/apply` because a build runs arbitrary code with
        // the server's environment. See routes/plugin_install.rs.
        .route(
            "/plugins/install/preview",
            post(plugin_install::preview_install),
        )
        .route("/plugins/install", post(plugin_install::start_install))
        .route("/plugins/install/:id", get(plugin_install::get_install))
        .route(
            "/plugins/install/:id/cancel",
            post(plugin_install::cancel_install),
        )
        .route("/plugins/:name/enable", post(plugins::enable_plugin))
        .route("/plugins/:name/reload", post(plugins::reload_plugin))
        .route("/plugins/:name/panels/:panel", get(plugins::read_panel))
        .route(
            "/plugins/:name/ui/actions/:action",
            post(plugins::run_ui_action),
        )
        .route("/mcp", get(plugins::list_mcp))
        // MCP servers are read from `[[mcp_servers]]` on demand, so adding,
        // removing or changing one is a reload rather than a restart. The
        // per-name route is the "I rebuilt the server binary" path.
        .route("/mcp/reload", post(plugins::reload_mcp))
        .route("/mcp/:name/reload", post(plugins::reload_mcp_server))
        // Agent skills: reusable instruction packs the agent loads on demand.
        // The list is discovery, not a table (there is no skills table), so
        // `GET` is a live scan of the workspace and the conventional folders.
        // `disabled` and `custom` are edited per skill here rather than through
        // `PATCH /api/config`; each mutation applies the admin gate itself.
        .route(
            "/skills",
            get(skills::list_skills).post(skills::create_skill),
        )
        .route(
            "/skills/:id",
            get(skills::get_skill)
                .patch(skills::patch_skill)
                .delete(skills::delete_skill),
        )
        // Update checking and installing. `GET` is a cached read; the two `POST`s
        // reach the network, and `apply` replaces the running binary (it needs the
        // admin credential when one is configured). See `crate::update`.
        .route("/update", get(update::status))
        .route("/update/check", post(update::check))
        .route("/update/apply", post(update::apply))
        .route("/events", get(events::stream))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(axum::middleware::from_fn(|request, next| {
            json_error_shape_with_limit(request, next, Some(MAX_JSON_BODY_BYTES))
        }));

    // The upload body is the raw file, not JSON, so it gets its own, larger
    // limit; the handler still enforces `MAX_UPLOAD_BYTES` and reports the
    // documented message.
    let upload_routes = Router::new()
        .route("/workspace/upload", post(workspace::upload))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BODY_BYTES))
        .layer(axum::middleware::from_fn(|request, next| {
            json_error_shape_with_limit(request, next, Some(MAX_UPLOAD_BODY_BYTES))
        }));

    // The editor's save body is JSON but can carry a 2 MiB file, so it gets its
    // own limit too. `put` lives here rather than on the GET route above so that
    // a 413 comes from the handler that names `MAX_WRITE_BYTES`.
    let write_routes = Router::new()
        .route("/workspace/file", axum::routing::put(workspace::write_file))
        .layer(DefaultBodyLimit::max(MAX_WRITE_BODY_BYTES))
        .layer(axum::middleware::from_fn(|request, next| {
            json_error_shape_with_limit(request, next, Some(MAX_WRITE_BODY_BYTES))
        }));

    // A chat artifact is up to 1 MiB of HTML with JSON escaping on top, so it
    // needs its own transport limit for the same reason the editor does: the
    // handler's 413 names the real limit, and a transport rejection does not.
    let artifact_routes = Router::new()
        .route("/artifacts", post(artifacts::create))
        .layer(DefaultBodyLimit::max(artifacts::MAX_ARTIFACT_BODY_BYTES))
        .layer(axum::middleware::from_fn(|request, next| {
            json_error_shape_with_limit(request, next, Some(artifacts::MAX_ARTIFACT_BODY_BYTES))
        }));

    json_routes
        .merge(upload_routes)
        .merge(write_routes)
        .merge(artifact_routes)
        .fallback(api_not_found)
        // Outermost, so a transport-level rejection from *any* extractor is
        // rewritten into the documented JSON error shape.
        .layer(axum::middleware::from_fn(json_error_shape))
}

#[inline(never)]
fn git_routes() -> Router<AppState> {
    let mut r = Router::new();
    r = r.route("/git/status", get(git::status));
    r = r.route("/git/diff", get(git::diff));
    r = r.route("/git/init", post(git::init));
    r = r.route("/git/config", post(git::config_identity));
    r = r.route("/git/branches", get(git::branches).post(git::create_branch));
    r = r.route("/git/branches/rename", post(git::rename_branch));
    r = r.route("/git/branches/delete", post(git::delete_branch));
    r = r.route("/git/checkout", post(git::checkout));
    r = r.route("/git/tags", get(git::tags).post(git::manage_tag));
    r = r.route("/git/remotes", get(git::remotes).post(git::manage_remote));
    r = r.route("/git/fetch", post(git::fetch));
    r = r.route("/git/pull", post(git::pull));
    r = r.route("/git/push", post(git::push));
    r = r.route("/git/merge", post(git::merge));
    r = r.route("/git/rebase", post(git::rebase));
    r = r.route("/git/cherry-pick", post(git::cherry_pick));
    r = r.route("/git/revert", post(git::revert_commit));
    r = r.route("/git/reset", post(git::reset));
    r = r.route("/git/resolve", post(git::resolve_conflict));
    r = r.route("/git/log", get(git::log));
    r = r.route("/git/commits/:sha", get(git::commit_detail));
    r = r.route("/git/stage", post(git::stage));
    r = r.route("/git/unstage", post(git::unstage));
    r = r.route("/git/discard", post(git::discard));
    r = r.route("/git/commit", post(git::commit));
    r = r.route("/git/stash", get(git::stash_list).post(git::stash));
    r = r.route("/git/stash/:index", get(git::stash_detail));
    r
}

#[inline(never)]
fn github_routes() -> Router<AppState> {
    let mut r = Router::new();
    r = r.route("/git/github/status", get(github::status));
    r = r.route("/git/github/repo", post(github::create_repo));
    r = r.route(
        "/git/github/pulls",
        get(github::list_pulls).post(github::create_pull),
    );
    r = r.route("/git/github/pulls/:number", get(github::get_pull));
    r = r.route("/git/github/pulls/:number/diff", get(github::pull_diff));
    r = r.route(
        "/git/github/pulls/:number/checkout",
        post(github::checkout_pull),
    );
    r = r.route("/git/github/pulls/:number/merge", post(github::merge_pull));
    r = r.route(
        "/git/github/pulls/:number/state",
        post(github::set_pull_state),
    );
    r = r.route(
        "/git/github/pulls/:number/comment",
        post(github::comment_pull),
    );
    r = r.route(
        "/git/github/issues",
        get(github::list_issues).post(github::create_issue),
    );
    r = r.route("/git/github/issues/:number", get(github::get_issue));
    r = r.route(
        "/git/github/issues/:number/state",
        post(github::set_issue_state),
    );
    r = r.route(
        "/git/github/issues/:number/comment",
        post(github::comment_issue),
    );
    r
}

/// `PATCH /api/config`, with the admin gate in front of it.
///
/// The gate lives here rather than in `routes::config` so that the security
/// decision is made in one place, before the patch is applied: a patch that
/// touches `approvals`, `model.base_url`, `model.api_key`, `providers`,
/// `active_provider` or `active_model` needs `X-Karucode-Admin-Token` whenever
/// `config.admin_token` is set. Everything else keeps working with the ordinary
/// bearer token.
async fn patch_config_guarded(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    // Same order as `config::patch_config`: a locked deployment answers 403 even
    // for a malformed body.
    if state.settings().is_locked() {
        return Err(ApiError::forbidden(
            "settings are locked (KARUCODE_LOCK_SETTINGS); change them in the config file or environment",
        ));
    }
    let patch: ConfigPatch = parse_body(&body)?;
    // A patch that turns sign-in on with nothing to sign in with is a lockout;
    // the credential store is only visible here, so the check is here.
    auth::check_access_patch(&state, &patch)?;
    let presented = admin_token_from(&headers);
    let config = state.settings().snapshot();
    let security_fields = authorize_patch(&config, &patch, presented).map_err(ApiError::from)?;
    if !security_fields.is_empty() {
        // Field names only: never the values, and never the credential.
        tracing::info!(
            fields = %security_fields.join(", "),
            "security-relevant settings change authorised"
        );
    }
    config::patch_config(State(state), body).await
}

/// The admin credential from the request headers, if any. A header that is not
/// valid ASCII is treated as absent (and therefore as a failed check) rather
/// than as a match.
fn admin_token_from(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
}

/// Rewrite a non-JSON error response (axum's plain-text `413` from
/// `DefaultBodyLimit`, and any other transport rejection) into the documented
/// `{"error":{"code","message"}}` shape. Responses that are already JSON are
/// passed through untouched.
async fn json_error_shape(request: axum::extract::Request, next: Next) -> Response {
    json_error_shape_with_limit(request, next, None).await
}

/// Rewrite a transport failure, carrying the route's actual body limit when its
/// sub-router supplied one. The outer fallback has no such knowledge and must not
/// claim that the generic JSON limit rejected an 8 MiB upload or a 2 MiB save.
async fn json_error_shape_with_limit(
    request: axum::extract::Request,
    next: Next,
    body_limit: Option<usize>,
) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with("application/json"))
        .unwrap_or(false);
    if is_json {
        return response;
    }
    let error = if status == StatusCode::PAYLOAD_TOO_LARGE {
        match body_limit {
            Some(limit) => ApiError::payload_too_large(limit, None),
            None => ApiError::new(
                status,
                "payload_too_large",
                "the request body exceeds this endpoint's configured size limit",
            ),
        }
    } else {
        ApiError::new(
            status,
            "bad_request",
            status.canonical_reason().unwrap_or("request rejected"),
        )
    };
    error.into_response()
}

/// Parse an optional JSON body. An empty body is accepted and yields `Default`,
/// which is what the frontend sends for `POST /api/provider/test` and
/// `/cancel` (it posts `{}`).
pub fn parse_body<T: DeserializeOwned + Default>(body: &Bytes) -> ApiResult<T> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON request body: {e}")))
}

/// Parse a required JSON object body.
pub fn parse_required<T: DeserializeOwned>(body: &Bytes, what: &str) -> ApiResult<T> {
    if body.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{what} requires a JSON request body"
        )));
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON request body: {e}")))
}

/// Query parameters as strings; repeated keys keep the last value.
pub type QueryMap = HashMap<String, String>;

pub fn query_str<'a>(query: &'a QueryMap, name: &str) -> Option<&'a str> {
    query
        .get(name)
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

pub fn query_usize(query: &QueryMap, name: &str) -> ApiResult<Option<usize>> {
    match query_str(query, name) {
        None => Ok(None),
        Some(raw) => raw.parse::<usize>().map(Some).map_err(|_| {
            ApiError::bad_request(format!(
                "query parameter `{name}` must be a positive integer"
            ))
        }),
    }
}

/// Exclusive message cursor shared by transcript endpoints.
pub fn message_before(query: &QueryMap) -> ApiResult<Option<i64>> {
    match query_str(query, "before") {
        None => Ok(None),
        Some(raw) => raw
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .map(Some)
            .ok_or_else(|| {
                ApiError::bad_request("query parameter `before` must be a positive message id")
            }),
    }
}

/// Approximate stored payload budget; one oversized message is returned alone.
pub const TRANSCRIPT_PAGE_BYTES: usize = 1024 * 1024;

pub fn query_bool(query: &QueryMap, name: &str) -> ApiResult<bool> {
    match query_str(query, name) {
        None => Ok(false),
        Some(raw) => match raw.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => Err(ApiError::bad_request(format!(
                "query parameter `{name}` must be true or false"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body_uses_defaults() {
        #[derive(serde::Deserialize, Default)]
        struct Body {
            #[serde(default)]
            value: Option<String>,
        }
        let parsed: Body = parse_body(&Bytes::new()).unwrap();
        assert!(parsed.value.is_none());
    }

    #[test]
    fn malformed_json_is_a_clear_bad_request() {
        let err = parse_body::<serde_json::Value>(&Bytes::from_static(b"{")).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.message.contains("JSON"));
    }

    #[test]
    fn query_helpers_validate_numbers() {
        let mut query = QueryMap::new();
        query.insert("depth".into(), "x".into());
        assert!(query_usize(&query, "depth").is_err());
        query.insert("depth".into(), "3".into());
        assert_eq!(query_usize(&query, "depth").unwrap(), Some(3));
        query.insert("staged".into(), "yes".into());
        assert!(query_bool(&query, "staged").unwrap());
    }

    #[test]
    fn the_json_limit_leaves_room_for_the_documented_prompt_error() {
        // `MAX_PROMPT_BYTES` is 1 MiB and its documented answer is a 400 naming
        // the field. The transport limit must sit above it, or that 400 is
        // unreachable and the client sees a 413 instead.
        const { assert!(MAX_JSON_BODY_BYTES > 1024 * 1024) };
        // ...and below axum's 2 MiB default, which is what we are replacing.
        const { assert!(MAX_JSON_BODY_BYTES < 2 * 1024 * 1024) };
    }

    #[test]
    fn the_upload_limit_leaves_room_for_the_handlers_own_check() {
        // The handler reports the documented 413 itself, so the transport limit
        // must be the larger of the two.
        const { assert!(MAX_UPLOAD_BODY_BYTES > workspace::MAX_UPLOAD_BYTES) };
    }

    /// A body over the limit must come back as the documented JSON error, not
    /// axum's plain-text 413.
    #[tokio::test]
    async fn an_oversized_body_is_a_json_payload_too_large() {
        use tower::ServiceExt;

        // A minimal router with the same limit + rewrite layer as `api_router`,
        // so this exercises the real rejection path without a full `AppState`.
        let app: Router = Router::new()
            .route(
                "/echo",
                post(|body: Bytes| async move { body.len().to_string() }),
            )
            .layer(DefaultBodyLimit::max(1024))
            .layer(axum::middleware::from_fn(json_error_shape));

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(vec![b'x'; 4096]))
            .unwrap();
        let response = app.oneshot(request).await.expect("router call");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "413 body was not JSON ({e}): {}",
                String::from_utf8_lossy(&body)
            )
        });
        assert_eq!(
            value["error"]["code"],
            serde_json::json!("payload_too_large")
        );
        assert!(value["error"]["message"].is_string());
        let message = value["error"]["message"].as_str().unwrap();
        assert!(
            !message.contains(&MAX_JSON_BODY_BYTES.to_string()),
            "an unknown transport limit must not claim the generic JSON cap: {message}"
        );
    }

    #[tokio::test]
    async fn a_route_transport_rejection_reports_its_actual_limit() {
        use tower::ServiceExt;

        let app: Router = Router::new()
            .route(
                "/echo",
                post(|body: Bytes| async move { body.len().to_string() }),
            )
            .layer(DefaultBodyLimit::max(4096))
            .layer(axum::middleware::from_fn(|request, next| {
                json_error_shape_with_limit(request, next, Some(4096))
            }));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .body(axum::body::Body::from(vec![b'x'; 8192]))
                    .unwrap(),
            )
            .await
            .expect("router call");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let message = value["error"]["message"].as_str().unwrap();
        assert!(message.contains("4096"), "{message}");
        assert!(
            !message.contains(&MAX_JSON_BODY_BYTES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn the_admin_credential_is_read_from_its_own_header() {
        let mut headers = HeaderMap::new();
        assert_eq!(admin_token_from(&headers), None);
        headers.insert(ADMIN_TOKEN_HEADER, "s3cret".parse().unwrap());
        assert_eq!(admin_token_from(&headers), Some("s3cret"));
        // The ordinary bearer token is not the admin credential.
        let mut bearer = HeaderMap::new();
        bearer.insert(header::AUTHORIZATION, "Bearer s3cret".parse().unwrap());
        assert_eq!(admin_token_from(&bearer), None);
    }

    /// A JSON error from a handler must pass through untouched.
    #[tokio::test]
    async fn an_existing_json_error_is_not_rewritten() {
        use tower::ServiceExt;

        let app: Router = Router::new()
            .route(
                "/boom",
                get(|| async { ApiError::not_found("no such thing") }),
            )
            .layer(axum::middleware::from_fn(json_error_shape));

        let request = axum::http::Request::builder()
            .uri("/boom")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.expect("router call");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["code"], serde_json::json!("not_found"));
        assert_eq!(
            value["error"]["message"],
            serde_json::json!("no such thing")
        );
    }
}
