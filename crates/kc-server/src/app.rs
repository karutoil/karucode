//! Router construction and the security middlewares (auth, host guard, CSRF,
//! headers).
//!
//! Layer order (outermost first): observability (request id, access log, request
//! counters) -> tracing span -> security headers -> bearer auth -> host guard ->
//! CSRF -> API/static routes. That way even a 401/403 carries `nosniff` and the
//! CSP, and is still logged, counted and correlated by its `X-Request-Id`.
//!
//! Observability is deliberately the outermost layer: a request refused by the
//! auth or `Host` guard is exactly the one an operator needs to see, and a layer
//! placed inside those guards would never run for it.
//!
//! The host guard runs for *every* method (a rebound page can read `GET
//! /api/config` too) and is the DNS-rebinding defence described in [`crate::host`].

// @kc-doc: security
// @kc-title: Understand the security model
// @kc-summary: What karucode protects by default, what it explicitly does not, and the recipe for safe remote access.
// @kc-tags: security, auth, csrf, sandbox, remote, trust
// @kc-applies-to: env:KARUCODE_AUTH_TOKEN, env:KARUCODE_ALLOWED_HOSTS, header:X-Karucode-Client, header:Authorization, config:auth_token, config:allow_temp_dir, doc:docs/SECURITY.md, concept:approvals, concept:workspace containment
//
// # Default posture
//
// Loopback bind (`127.0.0.1:8787`), no token, one user, one machine. File tools are
// confined to the run's project; mutations ask for approval by default. That is
// the whole security story on a laptop.
//
// # What is protected
//
// * **The token** - with `KARUCODE_AUTH_TOKEN` set, every `/api/...` route and
//   `/metrics` require `Authorization: Bearer <token>`; `?token=` is accepted
//   only for the SSE stream and the terminal websocket (a browser can set
//   neither a header on a top-level navigation nor on a websocket handshake).
//   The **SPA shell and its static assets are served unauthenticated**: `GET /`,
//   `/index.html`, `/assets/*`, `/favicon*`, `/brand/*` and any client-side deep
//   link return the built frontend (or the "UI not built" placeholder) with
//   `200`, because a browser cannot attach `Authorization` to the document that
//   would run the app. Those bytes carry no data - the bundle is public in a
//   source checkout - and the app then shows its own auth screen and asks for
//   the token, which is what makes a token-protected deployment usable at all.
//   A missing or wrong token for a non-`/api` path therefore never yields a
//   `401`: it yields the shell. The fallback can only ever serve the frontend,
//   so nothing else is reachable without the token.
// * **Transport surface** - the API is same-origin only. No `Access-Control-*`
//   header is ever sent.
// * **CSRF** - `POST`/`PATCH`/`PUT`/`DELETE` require
//   `X-Karucode-Client: web`; a present `Origin` must match `Host`.
// * **DNS rebinding** - on a loopback bind without a token, only `Host:
//   127.0.0.1`, `localhost`, `[::1]` (any port) and entries in
//   `KARUCODE_ALLOWED_HOSTS` are served. Everything else gets `403`.
// * **Path confinement** - read/write/edit/list/search resolve through the project
//   workspace, plus the system temporary directory (see below); `..`,
//   outside-absolute paths and symlink escapes are refused.
// * **Secrets** - API keys and MCP headers are write-only over the API, redacted in
//   errors and never logged. The data directory is forced to `0700`.
// * **Rendered content** - the UI disables raw HTML in Markdown and sanitises what
//   it renders; the CSP allows no inline scripts. `GET /api/workspace/raw` (the
//   bytes behind an image or PDF preview) is served with a **route-local**
//   `Content-Security-Policy: sandbox; default-src 'none'` plus `nosniff`, so the
//   response is an opaque origin with script disabled: a workspace `.html` file
//   can be hotlinked by `<img src>` or an iframe but cannot execute in the app's
//   origin, cannot read its storage and cannot call `/api`. The shell keeps the
//   app CSP; only the raw route is narrowed.
// * **Plugin panels** - a plugin contributes *data*, never markup. The inspector
//   draws the payload with a first-party component (or a read-only JSON view),
//   so a plugin cannot execute code in the app's origin; the CSP and
//   `frame-ancestors 'none'` keep their meaning, and no iframe, `unsafe-eval` or
//   `unsafe-inline` script exemption is needed.
// * **Response headers** - `nosniff`, `no-referrer`, `X-Frame-Options: DENY`, a
//   restrictive CSP and `Cross-Origin-Resource-Policy: same-origin` on every
//   response, including errors.
//
// # The temp directory is deliberately inside the fence
//
// The file tools may also reach the system temporary directory (`TMPDIR`, else
// `/tmp`). It is a *second root*, not the absence of one: `..` cannot climb out
// of it, a symlink inside it that points elsewhere is refused, and `/etc` stays
// out of reach. Turn it off with `allow_temp_dir = false` (or
// `KARUCODE_ALLOW_TEMP_DIR=0`) for project-only confinement.
//
// This is not a hole, because `/tmp` was never outside the attacker's reach to
// begin with: `run_command` is unsandboxed and runs as the server user, and every
// other process on the machine can already write to `/tmp`. The confinement
// rules exist to stop a *model* wandering out of its project - `~/.ssh`,
// `/etc`, another repository - not to contain code that can already fork a
// shell. Keeping the temp dir usable is what lets builds, screenshots and
// downloads work without an approval prompt per scratch file.
//
// # What is NOT protected - read this
//
// * **Shell commands are not sandboxed.** `run_command` runs `/bin/sh -c` as the
//   server user with that user's filesystem and network access. Approvals are the
//   gate, not isolation.
// * **The packaged unit does not sandbox the filesystem either.** It runs the
//   agent as an unprivileged account under `NoNewPrivileges=yes` - by default the
//   human who ran the installer (`packaging/install.sh` runs the service as the
//   installing account; the dedicated system account is only the fallback when no
//   human can be identified) - and it sets no
//   `ProtectSystem=`/`ProtectHome=`/`ReadWritePaths=`, so the agent can
//   write anywhere that account can - deliberately, so ordinary work (`mv`, build
//   caches, sibling checkouts) does not fail with `EROFS`. That account is the
//   real boundary: give the service its own user and it cannot touch files that
//   user cannot. Add a filesystem sandbox yourself in
//   `karucode.service.d/20-local.conf` if you want one; see
//   `packaging/README.md`.
// * **Plugins and MCP servers are trusted code.** They run as the server user;
//   their declared `permissions` limit which tools the host exposes, not what the
//   process can do. A plugin panel cannot run code in the *browser*, but the
//   plugin process behind it has the server user's privileges.
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
use crate::metrics;
use crate::routes;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower_http::trace::TraceLayer;

/// The CSP from the task/security docs. `unsafe-inline` is limited to styles;
/// scripts are same-origin files only.
///
/// `img-src` additionally allows `blob:`, which the project rail needs: an icon
/// is fetched with the bearer token (an `<img src>` cannot send `Authorization`)
/// and handed to the DOM as an object URL. A `blob:` URL inherits the creating
/// document's origin, so it can only carry same-origin content — it is not a way
/// to load or leak anything from another host, which is what `img-src` exists to
/// prevent. Remote images stay blocked, so a prompt-injected Markdown image still
/// cannot phone home.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     connect-src 'self'; frame-ancestors 'none'";

/// Build the complete application router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // `GET /metrics` (Prometheus text). Registered here rather than in
        // `routes::api_router` because it is not part of the documented `/api`
        // surface: it is an operator endpoint, and it must not be reachable
        // through the SPA fallback. It sits *inside* the auth layer, so a
        // deployment with `KARUCODE_AUTH_TOKEN` set needs the bearer token to
        // scrape it - metrics name routes and counts, which is not something to
        // hand to an unauthenticated caller on a non-loopback bind.
        .route("/metrics", axum::routing::get(metrics::metrics))
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
        // One span per request, carrying the id `observability` minted. The
        // span is what a tracing backend groups the handler's own events under;
        // the access-log line and the counters live in the layer below.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(metrics::request_span)
                .on_response(metrics::record_response),
        )
        // Outermost: mint the request id, log one line, count the request and
        // echo `X-Request-Id`. Outside the auth and `Host` guards so a 401/403
        // is logged and counted too.
        .layer(axum::middleware::from_fn(metrics::observability))
        .with_state(state)
}

/// Bearer auth. `?token=` is accepted **only** for the SSE endpoint, because a
/// `ReadableStream` fetch can set headers but some clients cannot.
///
/// Only `/api/...` and `/metrics` are gated. Everything else is the SPA shell and
/// its static assets, which are served unauthenticated on purpose: a browser
/// cannot attach an `Authorization` header to a top-level navigation or an
/// `<script src>`, so gating them would leave a token-protected deployment with
/// no usable UI at all - the bundle would 401 and `AuthScreen` (which asks for
/// the token *inside* the app) could never render. The shell carries no data, so
/// this is the standard SPA-with-token split: the app loads, then authenticates.
///
/// A wrong or missing token on a non-API path therefore never produces a `401`
/// JSON body; it produces the shell. [`requires_auth`] is what decides, by path
/// shape, so the fallback's behaviour and the middleware's decision cannot drift
/// apart: `static_ui::serve_spa` only ever answers with a file from the web
/// directory/embedded bundle or the placeholder page - it never lists a
/// directory and cannot be walked out of (`static_ui::normalize` rejects `..`) -
/// so an unauthenticated caller gets exactly the assets and nothing else.
async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.auth_token() else {
        return next.run(request).await;
    };
    if !requires_auth(request.uri().path()) || authorized(&request, &expected) {
        next.run(request).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

/// Whether a request path is behind the bearer token.
///
/// True for `/api` and everything below it, and for `/metrics` (the operator
/// endpoint registered next to the API in [`build_router`]). False for the SPA
/// shell, its assets and client-side deep links - see [`auth_middleware`] for
/// why. `/apifoo` is *not* an API path: it is a client-side route, and the
/// fallback serves the shell for it.
///
/// The path is compared after collapsing repeated slashes and `.`/`..` segments,
/// so a request written as `//api/config` or `/x/../api/config` is gated too even
/// though the router would send it to the SPA fallback: the decision fails closed
/// even if a proxy or a future layer normalises the path before matching it.
fn requires_auth(path: &str) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let normalized = parts.join("/");
    normalized == "metrics" || normalized == "api" || normalized.starts_with("api/")
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
    // Two endpoints accept `?token=`: the SSE stream (a `ReadableStream` fetch
    // cannot always set headers) and the terminal websocket (a browser never can
    // on a handshake). Both are same-origin only, and the websocket additionally
    // checks `Origin` against `Host`.
    let path = request.uri().path();
    if path == "/api/events" || (path.starts_with("/api/terminals/") && path.ends_with("/socket")) {
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
    apply_security_headers(response.headers_mut());
    response
}

/// The body of [`security_headers`], as a function of the headers alone.
///
/// Split out so the "a handler may only narrow" rule can be tested directly: the
/// middleware needs a whole `AppState`, which a unit test cannot cheaply build,
/// and this rule is exactly what a regression here would break.
fn apply_security_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    // A route may narrow the CSP for its own response, and then that value wins:
    // `GET /api/workspace/raw` and `GET /api/projects/:id/favicon` both serve
    // bytes whose content type came from a file name, and both answer with
    // `sandbox; default-src 'none'` (plus `sandbox`) so the document is an opaque
    // origin with no script. Overwriting it here would silently restore
    // `default-src 'self'` - which *allows* same-origin script - and turn a
    // workspace `.html` file back into a same-origin XSS. The rule is therefore
    // "a handler may only narrow": the overrides in the tree all do.
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        );
    }
    // Same "a handler may only narrow" rule as the CSP above. Almost every
    // response keeps the strict `same-origin`, so no workspace or API byte is
    // loadable by a foreign page. The one deliberate widening is
    // `GET /api/workspace/preview/*`, which must serve a document's own
    // subresources to a *sandboxed* (opaque-origin) frame: CORP `same-origin`
    // makes the browser refuse every one of them, so the preview rendered its
    // HTML but none of its styles, images or scripts (measured in Chromium).
    // Without this `contains_key` guard the assignment below silently reverted
    // that route's header, which is exactly how the bug survived a first pass.
    if !headers.contains_key(header::HeaderName::from_static(
        "cross-origin-resource-policy",
    )) {
        headers.insert(
            header::HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        );
    }
    // A document that carries its own `sandbox` CSP must be embeddable by *our*
    // own pages, which is the whole point of it: the file preview puts a PDF or an
    // image from `/api/workspace/raw` in an <iframe>/<img>, and `X-Frame-Options:
    // DENY` on the response blocks the iframe (verified in Chromium: the PDF pane
    // renders nothing with DENY, and correctly with no XFO). That header is
    // redundant there anyway - the sandboxed document is an opaque origin with
    // `default-src 'none'`, so it cannot be a clickjacking surface. Every page we
    // serve ourselves still carries both DENY and `frame-ancestors 'none'`.
    let sandboxed = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|policy| policy.contains("sandbox"));
    if !sandboxed {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
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

/// The "nothing to send" answer for a route whose absence is routine (a project
/// with no favicon), rather than a `404`.
///
/// A `404` is correct semantically, but every browser logs a 4xx as
/// `Failed to load resource` in the console, so a normal, expected absence looks
/// like a broken deployment. `204 No Content` says the same thing without the
/// noise and without inventing a body. It is deliberately *not* used where a
/// missing thing is an error: every other route still answers with the
/// documented `404 not_found`.
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
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

    /// The SPA shell is public, the API is not. Anything that could *become* an
    /// API path after slashes or dot segments are collapsed is treated as one, so
    /// the gate fails closed rather than relying on the router's normalisation.
    #[test]
    fn only_the_api_and_metrics_require_the_token() {
        for gated in [
            "/api",
            "/api/",
            "/api/config",
            "/api/terminals/t_1/socket",
            "/metrics",
            "/metrics/",
            "//api/config",
            "/x/../api/config",
            "/./api/health",
        ] {
            assert!(requires_auth(gated), "{gated} must require the token");
        }
        for open in [
            "/",
            "/index.html",
            "/assets/index-1234abcd.js",
            "/favicon.ico",
            "/brand/logo.svg",
            "/sessions/s_123",
            "/apifoo",
            "/metricsx",
        ] {
            assert!(!requires_auth(open), "{open} must stay open");
        }
    }

    #[test]
    fn query_parsing_decodes_tokens() {
        assert_eq!(
            query_param(Some("session=s_1&token=ab%2Bc"), "token").as_deref(),
            Some("ab+c")
        );
        assert_eq!(query_param(Some("session=s_1"), "token"), None);
    }

    /// A handler may narrow the CSP and CORP; the middleware must not overwrite
    /// either. The preview route depends on both: it answers its own
    /// `sandbox allow-scripts` policy, and it widens CORP to `cross-origin` so a
    /// sandboxed (opaque-origin) page may load its sibling CSS and images.
    /// Without the `contains_key` guards the middleware restored `same-origin`
    /// and the preview rendered its HTML but none of its assets.
    #[test]
    fn a_handler_may_narrow_the_csp_and_corp_without_being_overwritten() {
        // A sandboxed document that already answered its own headers.
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox allow-scripts; default-src 'none'"),
        );
        headers.insert(
            header::HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("cross-origin"),
        );
        apply_security_headers(&mut headers);

        assert_eq!(
            headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "sandbox allow-scripts; default-src 'none'"
        );
        assert_eq!(
            headers
                .get(header::HeaderName::from_static(
                    "cross-origin-resource-policy"
                ))
                .unwrap(),
            "cross-origin"
        );
        // A sandboxed document is embeddable by the viewer's own iframe; adding
        // DENY would blank that pane (verified in Chromium for the PDF preview).
        assert!(headers.get(header::X_FRAME_OPTIONS).is_none());

        // Every ordinary response still gets the strict pair.
        let mut plain = axum::http::HeaderMap::new();
        apply_security_headers(&mut plain);
        assert_eq!(
            plain.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            CONTENT_SECURITY_POLICY
        );
        assert_eq!(
            plain
                .get(header::HeaderName::from_static(
                    "cross-origin-resource-policy"
                ))
                .unwrap(),
            "same-origin"
        );
        assert_eq!(plain.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(
            plain.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
    }
}
