//! Router construction and the security middlewares (auth, CSRF, headers).
//!
//! Layer order (outermost first): security headers -> bearer auth -> CSRF ->
//! API/static routes. That way even a 401/403 carries `nosniff` and the CSP.

use crate::error::ApiError;
use crate::routes;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;

/// The CSP from the task/security docs. `unsafe-inline` is limited to styles;
/// scripts are same-origin files only.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
     connect-src 'self'; frame-ancestors 'none'";

/// Build the complete application router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .nest("/api", routes::api_router())
        .fallback(crate::static_ui::serve_spa)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state)
}

/// Bearer auth. `?token=` is accepted **only** for the SSE endpoint, because a
/// `ReadableStream` fetch can set headers but some clients cannot.
async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.auth_token() else {
        return next.run(request).await;
    };
    if authorized(&request, &expected) {
        next.run(request).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

fn authorized(request: &Request<Body>, expected: &str) -> bool {
    let header_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            if scheme.eq_ignore_ascii_case("bearer") {
                Some(token.trim())
            } else {
                None
            }
        });
    if let Some(token) = header_token {
        if constant_time_eq(token.as_bytes(), expected.as_bytes()) {
            return true;
        }
    }
    if request.uri().path() == "/api/events" {
        if let Some(token) = query_param(request.uri().query(), "token") {
            if constant_time_eq(token.as_bytes(), expected.as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// CSRF defence for cookie-less browsers: mutations need our custom header and a
/// same-host `Origin` when one is present.
async fn csrf_middleware(
    State(_state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let mutating = matches!(
        method,
        Method::POST | Method::PATCH | Method::PUT | Method::DELETE
    );
    if !mutating {
        return next.run(request).await;
    }

    let client_header = request
        .headers()
        .get("x-karucode-client")
        .and_then(|value| value.to_str().ok());
    if client_header != Some("web") {
        return ApiError::forbidden(
            "missing CSRF header: state-changing requests must send `X-Karucode-Client: web`",
        )
        .into_response();
    }

    if let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !origin_matches_host(origin, host) {
            return ApiError::forbidden(
                "cross-origin request rejected: the Origin header does not match the Host header",
            )
            .into_response();
        }
    }

    next.run(request).await
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some((_, rest)) = origin.split_once("://") else {
        return false;
    };
    let origin_host = rest.split('/').next().unwrap_or_default();
    !origin_host.is_empty()
        && !host.trim().is_empty()
        && origin_host.trim().eq_ignore_ascii_case(host.trim())
}

/// Headers applied to every response, including errors. No `Access-Control-*`
/// header is ever emitted: the API is same-origin only.
async fn security_headers(
    State(_state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}

/// Unknown `/api/...` paths answer with the JSON error shape, never the SPA.
pub async fn api_not_found(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> Response {
    ApiError::not_found(format!(
        "no such API endpoint: {} {}",
        uri.path(),
        "(see docs/API.md)"
    ))
    .into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Minimal query-string access for the auth middleware (no `serde_urlencoded`
/// dependency). Values are percent-decoded.
fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    for pair in query?.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(key) == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi as u8) * 16 + lo as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Status helper used by tests and docs.
pub fn is_no_content(status: StatusCode) -> bool {
    status == StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_comparison_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn origin_host_comparison() {
        assert!(origin_matches_host(
            "http://127.0.0.1:8787",
            "127.0.0.1:8787"
        ));
        assert!(origin_matches_host("https://Example.COM", "example.com"));
        assert!(!origin_matches_host("http://evil.com", "127.0.0.1:8787"));
        assert!(!origin_matches_host("null", "127.0.0.1:8787"));
        assert!(!origin_matches_host("http://", "127.0.0.1"));
    }

    #[test]
    fn query_parsing_decodes_tokens() {
        assert_eq!(
            query_param(Some("session=s_1&token=ab%2Bc"), "token").as_deref(),
            Some("ab+c")
        );
        assert_eq!(query_param(Some("session=s_1"), "token"), None);
    }
}
