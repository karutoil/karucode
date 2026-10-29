//! Router construction and the security middlewares (auth, host guard, CSRF,
//! headers).
//!
//! Layer order (outermost first): security headers -> bearer auth -> host guard
//! -> CSRF -> API/static routes. That way even a 401/403 carries `nosniff` and
//! the CSP.
//!
//! The host guard runs for *every* method (a rebound page can read `GET
//! /api/config` too) and is the DNS-rebinding defence described in [`crate::host`].

// @kc-doc: security
// @kc-title: Understand the security model
// @kc-summary: What karucode protects by default, what it explicitly does not, and the recipe for safe remote access.
// @kc-tags: security, auth, csrf, sandbox, remote, trust
// @kc-applies-to: env:KARUCODE_AUTH_TOKEN, env:KARUCODE_ALLOWED_HOSTS, header:X-Karucode-Client, header:Authorization, config:auth_token, doc:docs/SECURITY.md, concept:approvals, concept:workspace containment
//
// # Default posture
//
// Loopback bind (`127.0.0.1:8787`), no token, one user, one machine. File tools are
// confined to the run's project; mutations ask for approval by default. That is
// the whole security story on a laptop.
//
// # What is protected
//
// * **Transport surface** - the API is same-origin only. No `Access-Control-*`
//   header is ever sent.
// * **CSRF** - `POST`/`PATCH`/`PUT`/`DELETE` require
//   `X-Karucode-Client: web`; a present `Origin` must match `Host`.
// * **DNS rebinding** - on a loopback bind without a token, only `Host:
//   127.0.0.1`, `localhost`, `[::1]` (any port) and entries in
//   `KARUCODE_ALLOWED_HOSTS` are served. Everything else gets `403`.
// * **Path confinement** - read/write/edit/list/search resolve through the project
//   workspace; `..`, outside-absolute paths and symlink escapes are refused.
// * **Secrets** - API keys and MCP headers are write-only over the API, redacted in
//   errors and never logged. The data directory is forced to `0700`.
// * **Rendered content** - the UI disables raw HTML in Markdown and sanitises what
//   it renders; the CSP allows no inline scripts.
// * **Response headers** - `nosniff`, `no-referrer`, `X-Frame-Options: DENY`, a
//   restrictive CSP and `Cross-Origin-Resource-Policy: same-origin` on every
//   response, including errors.
//
// # What is NOT protected - read this
//
// * **Shell commands are not sandboxed.** `run_command` runs `/bin/sh -c` as the
//   server user with that user's filesystem and network access. Approvals are the
//   gate, not isolation.
// * **Plugins and MCP servers are trusted code.** They run as the server user;
//   their declared `permissions` limit which tools the host exposes, not what the
//   process can do.
// * **Subagents share the workspace.** `owned_paths` claims are advisory conflict
//   reporting, not locking.
// * **There is no built-in TLS.** Plain HTTP on loopback is fine; anything else
//   needs a reverse proxy.
//
// # Safe remote access
//
// ```bash
// # 1. a token of at least 16 characters (startup refuses a remote bind without one)
// export KARUCODE_AUTH_TOKEN="$(openssl rand -hex 24)"
// # 2. bind the interface the proxy reaches, and allow the proxy's Host name
// export KARUCODE_HOST=127.0.0.1
// export KARUCODE_ALLOWED_HOSTS="karucode.example.com"
// cargo run --release -p kc-server
// # 3. terminate TLS in nginx/caddy and forward to 127.0.0.1:8787
// ```
//
// `KARUCODE_ALLOWED_HOSTS` is only consulted while the host guard is active (a
// loopback bind without a token); with a token set, auth is the gate. Entries are
// `host`, `host:port`, or `*.example.com` - which matches subdomains but not the
// bare domain, so list that separately.
//
// # Running untrusted work
//
// Put the whole process in a container or VM, keep `approvals.mode =
// "read_only"` for inspection-only sessions, and do not enable `auto` on a
// workspace you have not reviewed. A `read_only` run still executes read-only
// tools; it denies writes, shell and external tools.
//
// Full reference: `docs/SECURITY.md`.

use crate::error::ApiError;
use crate::host::{self, display_host};
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
            host_guard_middleware,
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

/// DNS-rebinding guard: when the process is unauthenticated and bound to
/// loopback, a request may only name an allowlisted `Host`. See [`crate::host`]
/// for the rules and `KARUCODE_ALLOWED_HOSTS`.
async fn host_guard_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !state.enforce_host_allowlist() {
        return next.run(request).await;
    }
    let allowlist = state.host_allowlist();
    // HTTP/1.1 and HTTP/2 put the authority in different places; both are
    // attacker-controlled, so both are validated identically.
    let from_header = request.headers().get(header::HOST).map(|value| {
        value
            .to_str()
            .map_err(|_| "rejected Host header: it is not valid ASCII".to_string())
    });
    let authority = match from_header {
        Some(Ok(value)) => value.to_string(),
        Some(Err(message)) => return ApiError::forbidden(message).into_response(),
        None => match request.uri().authority() {
            Some(authority) => authority.to_string(),
            None => {
                return ApiError::forbidden(format!(
                    "rejected request without a Host header: this server is bound to a loopback \
                     address without KARUCODE_AUTH_TOKEN, so it only accepts Host {}",
                    allowlist.description()
                ))
                .into_response()
            }
        },
    };
    match allowlist.check(&authority) {
        Ok(()) => next.run(request).await,
        Err(message) => {
            tracing::warn!(
                host = %display_host(&authority),
                "rejected request for a Host that is not allowlisted"
            );
            ApiError::forbidden(message).into_response()
        }
    }
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

    // A malformed Origin is rejected, never treated as "no Origin": values with
    // whitespace/control characters and non-absolute URLs cannot be compared.
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let Ok(origin) = origin.to_str() else {
            return ApiError::forbidden(
                "cross-origin request rejected: the Origin header is not valid ASCII",
            )
            .into_response();
        };
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .or_else(|| {
                request
                    .uri()
                    .authority()
                    .map(|authority| authority.as_str())
            })
            .unwrap_or_default();
        if !host::origin_matches_host(origin, host) {
            return ApiError::forbidden(
                "cross-origin request rejected: the Origin header does not match the Host header",
            )
            .into_response();
        }
    }

    next.run(request).await
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
    fn query_parsing_decodes_tokens() {
        assert_eq!(
            query_param(Some("session=s_1&token=ab%2Bc"), "token").as_deref(),
            Some("ab+c")
        );
        assert_eq!(query_param(Some("session=s_1"), "token"), None);
    }
}
