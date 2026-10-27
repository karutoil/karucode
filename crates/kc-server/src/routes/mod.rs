//! HTTP handlers, one module per area of `docs/API.md`.

pub mod approvals;
pub mod config;
pub mod events;
pub mod git;
pub mod health;
pub mod plugins;
pub mod sessions;
pub mod workspace;

use crate::app::api_not_found;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::body::Bytes;
use axum::routing::{get, post};
use axum::Router;
use serde::de::DeserializeOwned;
use std::collections::HashMap;

/// Every `/api` route. Path parameters use axum 0.7's `:name` syntax.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health::health))
        .route(
            "/config",
            get(config::get_config).patch(config::patch_config),
        )
        .route("/provider/test", post(config::provider_test))
        .route("/stats", get(config::stats))
        .route(
            "/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route(
            "/sessions/:id",
            get(sessions::get_session)
                .patch(sessions::rename_session)
                .delete(sessions::delete_session),
        )
        .route("/sessions/:id/messages", post(sessions::send_message))
        .route("/sessions/:id/cancel", post(sessions::cancel_run))
        .route("/approvals", get(approvals::list_approvals))
        .route("/approvals/:id", post(approvals::resolve_approval))
        .route("/workspace/tree", get(workspace::tree))
        .route("/workspace/file", get(workspace::file))
        .route("/git/status", get(git::status))
        .route("/git/diff", get(git::diff))
        .route("/plugins", get(plugins::list_plugins))
        .route("/plugins/:name/enable", post(plugins::enable_plugin))
        .route("/mcp", get(plugins::list_mcp))
        .route("/events", get(events::stream))
        .fallback(api_not_found)
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
}
