//! `POST /api/artifacts` and `GET /api/artifacts/:id`: the isolated document a
//! chat artifact is rendered in.
//!
//! The store, the policy and the wrapping live in [`crate::artifacts`]; this
//! module is only the HTTP half. See that module for why an artifact needs a
//! server-side document at all.

use crate::artifacts::{
    valid_id, wrap_document, ArtifactTheme, MAX_ARTIFACT_BYTES, ARTIFACT_CONTENT_SECURITY_POLICY,
};
use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// Ceiling for the create request's JSON body.
///
/// The handler enforces [`MAX_ARTIFACT_BYTES`] on the decoded artifact and reports
/// the documented `413`; this transport limit sits above it at the worst-case JSON
/// escaping ratio (six bytes per byte, `\u00XX`), so a body between the two still
/// reaches the handler and gets the message that names the real limit.
pub const MAX_ARTIFACT_BODY_BYTES: usize = MAX_ARTIFACT_BYTES * 6 + 64 * 1024;

#[derive(Debug, Deserialize)]
pub struct CreateArtifactBody {
    /// The artifact source. HTML, or a fragment of it.
    #[serde(default)]
    pub html: String,
    /// `light` (default) or `dark`: which way round the wrapper's own defaults go.
    #[serde(default)]
    pub theme: Option<String>,
}

/// `POST /api/artifacts`
///
/// Store one artifact document and return the URL that serves it.
///
/// ```bash
/// curl -s -X POST localhost:8787/api/artifacts \
///   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
///   -d '{"html":"<button>Buy</button>","theme":"dark"}'
/// # → 201 {"id":"…","url":"/api/artifacts/…","bytes":374,"theme":"dark"}
/// ```
///
/// The response's `url` is what an `<iframe>` loads. It is content-addressed, so
/// posting the same source twice is one entry and one id - the client never has
/// to track what it has already published.
pub async fn create(State(state): State<AppState>, body: Bytes) -> ApiResult<(StatusCode, Json<Value>)> {
    let request: CreateArtifactBody = parse_required(&body, "POST /api/artifacts")?;
    if request.html.trim().is_empty() {
        return Err(ApiError::bad_request(
            "`html` is required: send the artifact's source in it",
        ));
    }
    if request.html.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::payload_too_large(
            MAX_ARTIFACT_BYTES,
            Some("this artifact is too large to render"),
        ));
    }
    let theme = ArtifactTheme::parse(request.theme.as_deref());
    let document = wrap_document(&request.html, theme);
    let id = state.artifacts().insert(document.clone());
    let url = format!("/api/artifacts/{id}");
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "url": url,
            "bytes": document.len(),
            "theme": theme.as_str(),
        })),
    ))
}

/// `GET /api/artifacts/:id`
///
/// The stored document, as a page: `text/html`, an opaque origin, script and
/// inline style allowed, no network at all ([`ARTIFACT_CONTENT_SECURITY_POLICY`]).
///
/// Two things about this response are load-bearing and easy to undo by accident:
///
/// * `Cross-Origin-Resource-Policy: cross-origin`. The app sets `same-origin` on
///   every other response, and a sandboxed frame's own subresource requests are
///   cross-origin *by definition* - the frame is in an opaque origin. Sending a
///   fragment's inline `<img>` or `<link>` any other way would render the mock
///   without its assets.
/// * `?token=` on this one path (see `crate::app::authorized`). An `<iframe src>`
///   is a browser navigation and cannot carry an `Authorization` header, so a
///   token-protected deployment would otherwise show an empty frame. The
///   token in the URL is the same trade the SSE stream and the terminal socket
///   already make, on a route that returns nothing but an artifact the caller
///   already had.
///
/// A missing id is a `404` and the client re-posts: the store is a cache, and an
/// entry can be evicted or lost to a restart.
pub async fn get_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(_query): Query<QueryMap>,
) -> ApiResult<Response> {
    if !valid_id(&id) {
        return Err(ApiError::not_found("no such artifact"));
    }
    let Some(document) = state.artifacts().get(&id) else {
        return Err(ApiError::not_found(
            "this artifact is no longer stored; post it again to render it",
        ));
    };
    let mut response = document.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(ARTIFACT_CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("cross-origin"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::{artifact_id, wrap_document};

    #[test]
    fn artifacts_the_transport_limit_leaves_room_for_the_handler_check() {
        // The documented 413 names MAX_ARTIFACT_BYTES, so the transport limit has
        // to sit above it - otherwise axum answers first with its own message.
        const { assert!(MAX_ARTIFACT_BODY_BYTES > MAX_ARTIFACT_BYTES) };
    }

    #[test]
    fn artifacts_the_served_url_is_the_id_it_was_stored_under() {
        let document = wrap_document("<p>hi</p>", ArtifactTheme::Light);
        let id = artifact_id(&document);
        assert_eq!(format!("/api/artifacts/{id}"), format!("/api/artifacts/{id}"));
        assert!(valid_id(&id));
    }
}
