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
// @kc-applies-to: env:KARUCODE_AUTH_TOKEN, env:KARUCODE_ALLOWED_HOSTS, env:KARUCODE_PUBLIC_URL, env:KARUCODE_TUNNEL, env:KARUCODE_TRUSTED_PROXIES, env:KARUCODE_ADMIN_TOKEN, header:X-Karucode-Client, header:Authorization, header:Cookie, config:auth_token, config:admin_token, config:allow_temp_dir, config:[access], config:access.public_url, config:access.tunnel, config:access.trusted_proxies, config:[updates], config:updates.manifest_url, config:updates.auto_apply, env:KARUCODE_UPDATE_MANIFEST_URL, env:KARUCODE_UPDATE_AUTO_APPLY, route:GET /api/update, route:POST /api/update/apply, route:GET /api/auth/api-keys, route:POST /api/auth/api-keys, route:PATCH /api/auth/api-keys/:id, route:DELETE /api/auth/api-keys/:id, route:POST /api/auth/api-keys/:id/rotate, header:X-Karucode-Admin-Token, doc:SECURITY.md, concept:approvals, concept:workspace containment, concept:sign-in, concept:api key scope, concept:update manifest
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
//   Artifact frames instead receive a random per-artifact `?capability=` from the
//   authenticated publishing POST; the master bearer never enters their URL.
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
// * **Web-UI sign-in** - a password or a passkey (`[access] enabled = true`)
//   exchanges a credential for an `HttpOnly` session cookie, which the app then
//   sends on its own; the bearer token keeps working beside it. Only three paths
//   are public while this is on - `/api/auth/status`, `/api/auth/login` and the
//   two passkey sign-in steps - because a sign-in screen that needs a sign-in
//   cannot exist. `[access] enabled` with no credential is **refused**, and the
//   same rule gates a non-loopback bind, so the two ways to lock yourself out are
//   both blocked at the door. See the `web-auth` topic for the settings, the
//   lockout and what a passkey ceremony actually verifies.
// * **Application API keys** - a named bearer credential for one integration
//   (`kck_…`). It authenticates exactly where the master token does, but it
//   carries `read`, `write` or `admin` scopes that the auth middleware enforces
//   per request, and it can carry an expiry, be rotated (a new secret with the
//   same identity) or be revoked without changing the password or the master
//   token. Only a SHA-256 of it is stored. Creating a key needs sign-in to be on
//   already; managing one needs `X-Karucode-Admin-Token` when that is configured.
//   See the `api-keys` topic.
// * **Transport surface** - the API is same-origin only. No `Access-Control-*`
//   header is ever sent.
// * **CSRF** - `POST`/`PATCH`/`PUT`/`DELETE` require
//   `X-Karucode-Client: web`; a present `Origin` must match `Host`.
// * **DNS rebinding** - on a loopback bind without a token, only `Host:
//   127.0.0.1`, `localhost`, `[::1]` (any port), entries in
//   `KARUCODE_ALLOWED_HOSTS` and the configured `[access] public_url` are
//   served. Everything else gets `403`. The guard is **not** skipped for a
//   password deployment: a cookie, unlike a bearer header, is sent automatically
//   by a browser, so a page whose DNS record is re-pointed at this machine would
//   otherwise carry the session with it. Behind a proxy that rewrites `Host`, an
//   `X-Forwarded-Host` from a **trusted** peer is accepted as a fallback - and it
//   is still checked against the same allowlist, so it cannot widen the guard.
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
// * **Chat artifacts** - an HTML mock from a reply is a document of its own at
//   `GET /api/artifacts/<id>`, served with `sandbox allow-scripts` (an opaque
//   origin: its scripts run, but it cannot read this app's storage, call `/api`
//   or frame the app) and `connect-src 'none'` with no wildcard source, so a
//   prompt-injected `<img src="https://…">` cannot beacon out. Its URL carries
//   only that artifact's random read capability, so even its own inline script
//   cannot learn the master bearer from `location.search`. An SVG artifact is
//   sanitised and drawn in a shadow root; nothing from an artifact is ever
//   inserted into the app's own DOM as markup. See the `artifacts` topic.
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
// # The update manifest is a trust anchor
//
// An update replaces the running binary, so whoever controls the manifest URL
// controls what runs on the machine. Three things bound that:
//
// * **Checking is automatic, installing is not.** A check is one GET of
//   `updates.json`; `auto_apply` is off by default, and even with it on a service
//   account that cannot write `/usr/local/bin/karucode` still needs a privileged
//   `karucode update --apply`. A container install refuses entirely.
// * **The server never trusts the download on its own.** The artifact is verified
//   against the manifest's SHA-256 and size while streaming, then **executed once
//   with `--version`** (cleared environment, 30 s limit) and required to report the
//   build the manifest promised before it is copied anywhere. The copy is compared
//   byte for byte and the swap is an atomic rename that keeps `karucode.old`.
// * **Repointing the manifest is an admin-gated change.** `PATCH /api/config`
//   touching `updates.manifest_url` or `updates.auto_apply` needs
//   `X-Karucode-Admin-Token`, exactly like the approval mode or the provider
//   endpoint, and `POST /api/update/apply` needs it too.
//
// The checksum proves the bytes arrived intact; it does not make the publisher
// trustworthy. Point `updates.manifest_url` at a mirror you control (an air-gapped
// host is the normal reason) and leave `check = false` if you would rather update
// only by hand. `packaging/update.sh` and `packaging/install.sh` verify a release
// tarball the same way, against the published `SHA256SUMS`.
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
// Full reference: `SECURITY.md`.

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
/// cannot phone home. The same mechanism renders PDFs after an authenticated
/// fetch: `frame-src 'self' blob:` permits that managed object URL in an iframe
/// without opening remote frame origins. Route-local raw/artifact sandbox policies
/// still win for those documents themselves.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     connect-src 'self'; frame-src 'self' blob:; frame-ancestors 'none'";

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
/// Three credentials are accepted, in this order: the master bearer token, a
/// web-UI session cookie (or an artifact's read capability), and an application
/// API key. An API key is additionally checked against its scopes: a `read` key
/// cannot mutate, and the credential/shell/config surfaces need `admin`. See
/// [`crate::api_keys`] for the matrix.
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
    let path = request.uri().path().to_string();
    // The SPA shell and its assets are always served: the sign-in screen is part
    // of the shell, and a browser cannot put a credential on `<script src>`.
    if !requires_auth(&path) {
        return next.run(request).await;
    }
    // Signing in cannot require being signed in.
    if crate::routes::auth::is_public_path(&path) {
        return next.run(request).await;
    }
    // Nothing configured: the historical open loopback server.
    if !state.sign_in_required() {
        return next.run(request).await;
    }
    // The master token. It is the one credential with no scopes: it can do
    // everything, exactly as before this module grew API keys.
    if bearer_authorized(&request, state.auth_token().as_deref()) {
        return next.run(request).await;
    }
    // A signed-in browser, or a chat artifact's one-document capability.
    if session_authorized(&state, &request) || artifact_capability_authorized(&state, &request) {
        return next.run(request).await;
    }
    // An application API key. It is checked after the cookie because a stale
    // token in localStorage must not shadow a valid session.
    if let Some(presented) = presented_bearer(&request) {
        if presented.starts_with(crate::api_keys::TOKEN_PREFIX_WITH_SEPARATOR) {
            match state.api_keys().authenticate(&presented) {
                Ok(key) => {
                    let needed = crate::api_keys::required_scope(&path, request.method());
                    if !crate::api_keys::scopes_allow(&key.scopes, needed) {
                        return ApiError::forbidden(format!(
                            "this API key has the {} scope; {} {} requires the {} scope",
                            scope_list(&key.scopes),
                            request.method(),
                            path,
                            needed.as_str(),
                        ))
                        .into_response();
                    }
                    let peer = request
                        .extensions()
                        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                        .map(|axum::extract::ConnectInfo(addr)| addr.ip());
                    state.api_keys().record_use(&key.id, peer).await;
                    return next.run(request).await;
                }
                Err(rejection) if rejection.is_definitive() => {
                    // The secret matched a real key, so naming the reason (expired,
                    // revoked) helps an operator without helping a guesser.
                    tracing::debug!(
                        reason = rejection.message(),
                        "rejected an application API key"
                    );
                    return ApiError::unauthorized_message(rejection.message()).into_response();
                }
                Err(_) => {}
            }
        }
    }
    ApiError::unauthorized().into_response()
}

/// The scopes of a key, for a refusal message (`read+write`).
fn scope_list(scopes: &[crate::api_keys::Scope]) -> String {
    scopes
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

/// The bearer value a request presents, from `Authorization` or - only on the
/// two endpoints that cannot send a header - `?token=`.
fn presented_bearer(request: &Request<Body>) -> Option<String> {
    let header_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            if scheme.eq_ignore_ascii_case("bearer") {
                Some(token.trim().to_string())
            } else {
                None
            }
        });
    if header_token.is_some() {
        return header_token;
    }
    // Only two endpoints accept the master `?token=`: the SSE stream (a
    // `ReadableStream` fetch cannot always set headers) and the terminal websocket
    // (a browser never can on a handshake). An artifact frame never receives this
    // exemption: its inline JavaScript is untrusted and can read location.search.
    // Its separately checked per-artifact capability is handled separately.
    let path = request.uri().path();
    if path == "/api/events" || (path.starts_with("/api/terminals/") && path.ends_with("/socket")) {
        return query_param(request.uri().query(), "token");
    }
    None
}

/// Whether the request carries the configured bearer token (header, or `?token=`
/// on the two endpoints that cannot send one).
fn bearer_authorized(request: &Request<Body>, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    presented_bearer(request)
        .is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()))
}

/// Whether the request carries a live web-UI session cookie.
fn session_authorized(state: &AppState, request: &Request<Body>) -> bool {
    let Some(token) = crate::routes::auth::session_cookie_from(request.headers()) else {
        return false;
    };
    state.web_auth().session(&token).is_some()
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
    let normalized = normalize_path(path);
    normalized == "/metrics" || normalized == "/api" || normalized.starts_with("/api/")
}

/// Collapse repeated slashes and `.`/`..` segments into one canonical path with
/// a leading slash.
///
/// Both the auth gate and the API-key scope check use it, so a request written
/// as `//api/config` or `/x/../api/terminals` cannot be gated as one thing and
/// scoped as another. It never fails: `..` past the root is dropped, which is
/// what a router would do too.
pub(crate) fn normalize_path(path: &str) -> String {
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
    format!("/{}", parts.join("/"))
}

/// A frame URL carries a random read grant for one artifact, never the master
/// bearer. The handler repeats this check so the capability remains mandatory
/// even when the deployment has no master token configured.
fn artifact_capability_authorized(state: &AppState, request: &Request<Body>) -> bool {
    let Some(id) = request.uri().path().strip_prefix("/api/artifacts/") else {
        return false;
    };
    if id.contains('/') || !crate::artifacts::valid_id(id) {
        return false;
    }
    let Some(capability) = query_param(request.uri().query(), "capability") else {
        return false;
    };
    state.artifacts().capability_matches(id, &capability)
}

/// DNS-rebinding guard: while the server has no bearer token and is bound to
/// loopback, a request may only name an allowlisted `Host`. See [`crate::host`]
/// for the rules, `KARUCODE_ALLOWED_HOSTS` and `[access] public_url`.
///
/// A **web-UI session cookie is why this guard is not skipped for password
/// deployments.** With a bearer token the header cannot be guessed by a
/// rebinding page, so the guard is unnecessary; a cookie, on the other hand, is
/// sent automatically by the browser - including to a name that just resolved to
/// this machine. So the guard stays on, and the public URL an operator configured
/// (plus `KARUCODE_ALLOWED_HOSTS`) is what it accepts.
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
                     address without a bearer token, so it only accepts Host {}",
                    allowlist.description()
                ))
                .into_response()
            }
        },
    };
    if host_is_allowed(&state, &authority) {
        return next.run(request).await;
    }
    // A proxy that rewrites `Host` to an internal name (a Docker service name, a
    // compose network alias) puts the public name in `X-Forwarded-Host`. That
    // header is only readable from a trusted peer, and the value still has to be
    // allowlisted, so the fallback cannot widen the guard.
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|axum::extract::ConnectInfo(addr)| addr.ip());
    if let Some(forwarded) = state
        .proxy_trust()
        .forwarded_authority(peer, request.headers())
    {
        let candidate = match forwarded.port {
            Some(port) => format!("{}:{port}", forwarded.host),
            None => forwarded.host,
        };
        if host_is_allowed(&state, &candidate) {
            return next.run(request).await;
        }
    }
    tracing::warn!(
        host = %display_host(&authority),
        "rejected request for a Host that is not allowlisted"
    );
    ApiError::forbidden(allowlist.check(&authority).unwrap_err()).into_response()
}

/// Whether `host[:port]` is an accepted name for this server.
///
/// The configured public URL is accepted in addition to the `Host` allowlist: it
/// is the one name an operator typed themselves, and it is what a tunnel forwards.
fn host_is_allowed(state: &AppState, raw: &str) -> bool {
    let Some(authority) = host::parse_authority(raw) else {
        return false;
    };
    if state.host_allowlist().allows(&authority) {
        return true;
    }
    match state.public_authority() {
        Some(public) => match host::parse_authority(&public) {
            Some(expected) => {
                expected.host == authority.host
                    && (expected.port.is_none() || expected.port == authority.port)
            }
            None => false,
        },
        None => false,
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
        let csp = plain
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        let frame_src = csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("frame-src"))
            .expect("application response has an explicit frame-src");
        assert_eq!(frame_src, "frame-src 'self' blob:");
    }
}
