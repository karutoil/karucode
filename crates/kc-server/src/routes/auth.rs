//! `/api/auth/*`: sign-in, passkeys, the password, sessions and onboarding.
//!
//! Three of these routes are reachable **without** a credential - `status`,
//! `login` and the two passkey sign-in steps - because a sign-in screen that
//! requires being signed in is not a sign-in screen. Everything else here needs
//! an authenticated request (cookie or bearer token), which is what
//! [`is_public_path`] and the middleware in [`crate::app`] agree on.
//!
//! The cookie is the only state a browser carries: no JWT, no token in
//! `localStorage`. It is `HttpOnly` (script cannot read it), `SameSite=Lax` (a
//! cross-site `POST` does not send it, which is the CSRF defence for the sign-in
//! cookie) and `Secure` whenever the request arrived over TLS.

// @kc-doc: web-auth
// @kc-title: Sign in to the web UI
// @kc-summary: Put a password or a passkey in front of the web UI, choose the address it answers to, and understand what each sign-in method costs.
// @kc-tags: auth, authentication, password, passkey, webauthn, login, cookie, session, access, tunnel, cloudflare, proxy
// @kc-applies-to: route:GET /api/auth/status, route:POST /api/auth/login, route:POST /api/auth/logout, route:GET /api/auth/me, route:POST /api/auth/password, route:DELETE /api/auth/password, route:GET /api/auth/passkeys, route:POST /api/auth/passkeys, route:POST /api/auth/passkeys/options, route:PATCH /api/auth/passkeys/:id, route:DELETE /api/auth/passkeys/:id, route:POST /api/auth/passkey/options, route:POST /api/auth/passkey/login, route:GET /api/auth/sessions, route:DELETE /api/auth/sessions/:id, route:POST /api/auth/sessions/revoke-all, route:GET /api/onboarding, route:POST /api/onboarding/complete, config:[access], config:access.enabled, config:access.public_url, config:access.session_days, config:access.tunnel, config:access.trusted_proxies, env:KARUCODE_ACCESS_ENABLED, env:KARUCODE_PUBLIC_URL, env:KARUCODE_SESSION_DAYS, env:KARUCODE_TUNNEL, env:KARUCODE_TRUSTED_PROXIES, env:KARUCODE_AUTH_TOKEN, ui:AuthScreen, ui:Onboarding, ui:Settings, file:crates/kc-server/src/web_auth.rs, file:crates/kc-server/src/webauthn.rs
//
// # What sign-in is
//
// `KARUCODE_AUTH_TOKEN` protects the API, but a browser cannot put a header on
// the document that runs the app: the token deployment means pasting a secret
// into a form. Web-UI sign-in is the ordinary alternative - a **password** or a
// **passkey** (WebAuthn), exchanged for an `HttpOnly` session cookie. Both work
// alongside the bearer token; neither replaces it.
//
// The sign-in screen replaces the address bar trick: `GET /api/auth/status` is
// public, the app asks it which methods exist, and the browser stays signed in
// for `session_days` (30 by default).
//
// # Turn it on
//
// The onboarding wizard does this in one step, or use Settings -> Access. By API:
//
// ```bash
// # 1. Set a password. This is refused below 10 characters.
// curl -s -X POST localhost:8787/api/auth/password \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"password":"a long passphrase"}'
//
// # 2. (optional) require a second, different name for the tunnel
// curl -s -X PATCH localhost:8787/api/config \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"access":{"public_url":"https://karucode.example.com","tunnel":"cloudflare"}}'
// ```
//
// **Creating the first credential turns sign-in on** (`[access] enabled = true`),
// because setting a password is a statement of intent. The response says so with
// `enabled_now: true`, and the browser that made the request is signed in
// immediately, so the change cannot lock you out. Removing the last credential
// turns it off again.
//
// `[access] enabled = true` with **no** credential is refused with `400`: that
// edit would leave a login screen nobody can pass. The same rule gates a
// non-loopback bind - `--host 0.0.0.0` needs `KARUCODE_AUTH_TOKEN` *or* an
// enabled `[access]` section backed by a real credential.
//
// # The four settings
//
// ```toml
// [access]
// enabled = true                          # KARUCODE_ACCESS_ENABLED=1
// public_url = "https://karucode.example.com"  # KARUCODE_PUBLIC_URL
// session_days = 30                       # KARUCODE_SESSION_DAYS, 1..=365
// tunnel = "cloudflare"                   # KARUCODE_TUNNEL: none|cloudflare|custom
// trusted_proxies = ["10.0.0.0/8"]        # KARUCODE_TRUSTED_PROXIES, comma separated
// ```
//
// * **`public_url`** - the address people type. It is added to the `Host`
//   allowlist (so the proxy's name is served without also setting
//   `KARUCODE_ALLOWED_HOSTS`), it decides `Secure` on the cookie, and it is the
//   origin a passkey is bound to. Scheme, host and optional port only; a path,
//   query or `user@` is a settings error.
// * **`session_days`** - how long a sign-in lasts on one browser. A password
//   change and "sign out everywhere" end every session immediately.
// * **`tunnel`** - which proxy is in front. `none` ignores forwarded headers
//   entirely, `cloudflare` trusts Cloudflare's published proxy ranges in addition
//   to `trusted_proxies`, and `custom` trusts only `trusted_proxies`.
// * **`trusted_proxies`** - IP addresses or CIDR blocks whose
//   `X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host` are believed.
//   **Only the direct peer is checked**: a header from any other address is
//   ignored, because otherwise any client could claim to be `127.0.0.1`. Loopback
//   is always trusted, which is what makes `cloudflared`, a local nginx and a
//   Docker bridge gateway work with no entry at all. `10.0.0.1/8` (host bits set)
//   is refused rather than silently masked.
//
// All five are hot: `PATCH /api/config` applies them on the next request, no
// restart.
//
// # Cloudflare, in one example
//
// ```bash
// # cloudflared tunnel --url http://127.0.0.1:8787
// # then, in the UI: Settings -> Access -> Public address
// #   https://karucode.example.com
// # How is it reached? Cloudflare Tunnel
// ```
//
// `cloudflared` connects *out* and forwards from loopback, so it needs no entry
// in `trusted_proxies`; the Cloudflare preset matters for the orange-cloud proxy
// mode, where Cloudflare connects to the origin from its own addresses.
//
// # What protects what
//
// * The password is hashed with **Argon2id** and verified in constant time. Only
//   the hash is stored, in the SQLite `settings` table (`web_auth.password`) -
//   never in `config.toml`, never in `GET /api/config`.
// * A session cookie is 32 random bytes; the server stores only its **SHA-256**,
//   so a copy of the database cannot be turned into a login.
// * The cookie is `HttpOnly`, `SameSite=Lax` and `Secure` whenever the request
//   arrived over TLS (directly or through a trusted proxy, or because
//   `public_url` is `https`). Mutations still need `X-Karucode-Client: web` and a
//   same-host `Origin`.
// * Eight wrong attempts from one address within 15 minutes lock that address
//   out for 5 minutes (`429 too_many_requests`) - and the lockout applies to the
//   correct password too, or it would only slow a guesser down.
// * Passkeys are **ES256 (P-256) only**. The ceremony is fully checked: the
//   challenge this server minted (single use, 5-minute TTL), the origin, the
//   relying-party id hash, the user-present flag and a signature counter that must
//   not go backwards. The *attestation statement* is not verified - this server
//   makes no claim about which authenticator created a credential, which is what
//   lets Touch ID, Windows Hello, a phone and a YubiKey all work without a policy
//   list. RSA and Ed25519 credentials are refused **at registration** with a
//   message that says so, rather than failing mysteriously at sign-in.
//
// # Gotchas
//
// * A passkey is bound to the host it was registered on. Registering on
//   `http://127.0.0.1:8787` and then signing in through
//   `https://karucode.example.com` cannot work - different relying party - so set
//   `public_url` **before** registering the passkey you plan to use remotely.
//   `GET /api/auth/status` reports the `rp_id` and `origin` the server would use.
// * `localhost` and `127.0.0.1` count as secure contexts for WebAuthn; any other
//   plain-`http://` origin does not, and the browser refuses the ceremony.
// * The bearer token keeps working with `[access]` on. `GET /metrics` is behind
//   whichever credential is configured, so a token deployment can still scrape it.
// * Signing out is `POST /api/auth/logout`; it deletes the session row, so the
//   cookie cannot be replayed even from a copy.
// * `KARUCODE_LOCK_SETTINGS=1` makes `PATCH /api/config` answer `403`, but the
//   password/passkey routes still work: credentials are not configuration.
// * There is no username. One install has one principal; the "sessions" list is
//   there so you can see and end a browser you no longer trust.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_body, parse_required};
use crate::settings::{AccessPatch, ConfigPatch};
use crate::state::AppState;
use crate::web_auth::{
    LoginError, SessionMethod, WebAuth, CHALLENGE_TTL, MAX_PASSWORD_LEN, MIN_PASSWORD_LEN,
    SESSION_COOKIE,
};
use crate::webauthn::{AssertionInput, RegistrationInput};
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kc_core::config::AccessConfig;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;

/// Paths under `/api` that do not require a credential.
///
/// Kept as one function so the router and the middleware cannot disagree: a route
/// added to the router without being listed here is protected (fail closed), and
/// one listed here that does not exist is a 404, not a hole.
pub fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/api/auth/status"
            | "/api/auth/login"
            | "/api/auth/passkey/options"
            | "/api/auth/passkey/login"
    )
}

/// The client address, when the transport supplies one.
fn peer_addr(connect: &Option<ConnectInfo<SocketAddr>>) -> Option<std::net::IpAddr> {
    connect.as_ref().map(|ConnectInfo(addr)| addr.ip())
}

/// A stable key for the failure counter: the client address, else a single
/// shared bucket (a server with no connect info still gets a lockout, it is just
/// shared between clients).
fn failure_key(state: &AppState, headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> String {
    match state.proxy_trust().client(peer, headers).ip {
        Some(ip) => ip.to_string(),
        None => "unknown".to_string(),
    }
}

/// Whether **this** request reached the browser over TLS.
///
/// Two sources, and the distinction matters for the cookie: a trusted proxy's
/// `X-Forwarded-Proto` describes a request that really was HTTPS, while
/// `public_url = https://…` only says that the deployment *has* a TLS address.
/// Believing the configured URL for every request marks the cookie `Secure` on a
/// plain-`http://localhost` sign-in too, which a browser is free to drop - so the
/// configured URL only counts when the request is actually addressed to it.
fn client_reads_https(
    state: &AppState,
    access: &AccessConfig,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
) -> bool {
    if state.proxy_trust().client(peer, headers).https {
        return true;
    }
    match (access.public_url_is_https(), access.public_authority()) {
        (true, Some(public)) => request_host(state, headers, peer)
            .map(|host| host.eq_ignore_ascii_case(&public))
            .unwrap_or(false),
        _ => false,
    }
}

/// The `Set-Cookie` value for a new session.
fn session_cookie(token: &str, max_age_secs: u64, secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// The `Set-Cookie` value that deletes the cookie.
fn clear_cookie(secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn with_cookie(mut response: Response, cookie: String) -> Response {
    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response
                .headers_mut()
                .append(header::SET_COOKIE, value);
            response
        }
        Err(_) => response,
    }
}

/// The session cookie value from a request, if present.
pub fn session_cookie_from(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        for part in text.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix(SESSION_COOKIE) {
                if let Some(token) = rest.strip_prefix('=') {
                    let token = token.trim();
                    if !token.is_empty() {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }
    None
}

/// The request's own origin, from the `Origin` header or `scheme://host`.
fn request_origin(state: &AppState, headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> Option<String> {
    let access = state.access();
    let https = client_reads_https(state, &access, headers, peer);
    let host = request_host(state, headers, peer)?;
    // A browser sends `Origin` on every cross-origin request and on same-origin
    // POSTs; when it is present and names the same host, it is the authority on
    // its own scheme.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if let Some((scheme, rest)) = origin.split_once("://") {
            let origin_host = rest.split('/').next().unwrap_or_default();
            if strip_port(origin_host).eq_ignore_ascii_case(&strip_port(&host)) {
                return Some(format!("{}://{}", scheme.to_ascii_lowercase(), origin_host));
            }
        }
    }
    Some(format!(
        "{}://{}",
        if https { "https" } else { "http" },
        host
    ))
}

/// The `host[:port]` this request arrived for.
fn request_host(state: &AppState, headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> Option<String> {
    if let Some(value) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_ascii_lowercase());
        }
    }
    if let Some(authority) = state.proxy_trust().forwarded_authority(peer, headers) {
        return Some(match authority.port {
            Some(port) => format!("{}:{port}", authority.host),
            None => authority.host,
        });
    }
    state.public_authority()
}

fn strip_port(host: &str) -> String {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some((inner, _)) = rest.split_once(']') {
            return format!("[{inner}]");
        }
    }
    match host.rsplit_once(':') {
        Some((head, port)) if port.chars().all(|c| c.is_ascii_digit()) => head.to_string(),
        _ => host.to_string(),
    }
}

/// Every origin acceptable for a WebAuthn ceremony on this request.
fn ceremony_origins(state: &AppState, headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> Vec<String> {
    let mut origins = Vec::new();
    if let Some(origin) = request_origin(state, headers, peer) {
        origins.push(origin);
    }
    // The configured public URL is a second acceptable origin: a passkey
    // registered through the tunnel is used through the tunnel, and the browser
    // may be opened on either name.
    if let Some(public) = state.access().expected_origin() {
        if !origins.iter().any(|origin| origin.eq_ignore_ascii_case(&public)) {
            origins.push(public);
        }
    }
    origins
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// `GET /api/auth/status` - what the sign-in screen needs to draw itself.
pub async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> ApiResult<Json<Value>> {
    let peer = peer_addr(&connect);
    let access = state.access();
    let web_auth = state.web_auth();
    let signed_in = session_cookie_from(&headers)
        .and_then(|token| web_auth.session(&token))
        .map(|session| session.method.as_str().to_string());
    Ok(Json(json!({
        "enabled": access.enabled,
        "sign_in_required": state.sign_in_required(),
        "password_set": web_auth.password_set(),
        "passkey_count": web_auth.passkey_count(),
        // No credential yet: the wizard has to offer "create a password", not
        // "sign in".
        "needs_setup": !web_auth.has_credential(),
        "signed_in": signed_in.is_some(),
        "signed_in_with": signed_in,
        "token_required": state.auth_token().is_some(),
        "session_days": access.session_lifetime_days(),
        "public_url": access.public_url,
        "tunnel": access.tunnel.as_str(),
        "trusted_proxies": access.trusted_proxies,
        "cloudflare_proxies": access.tunnel == kc_core::config::TunnelKind::Cloudflare,
        // What a passkey on this request would be scoped to, so the UI can warn
        // when the browser is on a different host than the public URL.
        "origin": request_origin(&state, &headers, peer),
        "rp_id": request_host(&state, &headers, peer).map(|host| strip_port(&host)),
        "min_password_len": MIN_PASSWORD_LEN,
        "max_password_len": MAX_PASSWORD_LEN,
        "passkey_challenge_ttl_secs": CHALLENGE_TTL.as_secs(),
    })))
}

/// `GET /api/auth/me` - who this request is, for the signed-in UI.
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let session = session_cookie_from(&headers).and_then(|token| state.web_auth().session(&token));
    Ok(Json(match session {
        Some(session) => json!({
            "signed_in": true,
            "principal": session.method.as_str(),
            "label": session.label,
            "expires_at": session.expires_at,
            "token": true,
        }),
        None => json!({
            "signed_in": false,
            "principal": if state.auth_token().is_some() { "token" } else { "none" },
            "label": Value::Null,
            "expires_at": Value::Null,
            "token": state.auth_token().is_some(),
        }),
    }))
}

// ---------------------------------------------------------------------------
// Sign in
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginBody {
    pub password: String,
}

/// `POST /api/auth/login` - password sign-in.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let peer = peer_addr(&connect);
    let body: LoginBody = parse_body(&body)?;
    let web_auth = state.web_auth();
    let key = failure_key(&state, &headers, peer);
    if let Some(secs) = web_auth.locked_out_for(&key) {
        return Err(ApiError::too_many_requests(format!(
            "too many failed sign-in attempts; try again in {secs} seconds"
        )));
    }
    if !web_auth.password_set() {
        return Err(ApiError::bad_request(
            "no password is configured on this server; set one in the Access settings, or use \
             the bearer token",
        ));
    }
    if !web_auth.verify_password(&body.password).await {
        let locked = web_auth.record_failure(&key);
        return Err(if locked {
            ApiError::too_many_requests(format!(
                "too many failed sign-in attempts; try again in {} seconds",
                crate::web_auth::LOCKOUT_SECS
            ))
        } else {
            ApiError::unauthorized_message("that password is not correct")
        });
    }
    web_auth.clear_failures(&key);
    sign_in_response(&state, &headers, peer, SessionMethod::Password, "password").await
}

/// Mint the session, set the cookie and answer.
async fn sign_in_response(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    method: SessionMethod,
    label: &str,
) -> ApiResult<Response> {
    let access = state.access();
    let lifetime = access.session_lifetime_days() * 24 * 3600;
    let (token, session) = state
        .web_auth()
        .create_session(method, label, lifetime)
        .await
        .map_err(ApiError::from)?;
    let secure = client_reads_https(state, &access, headers, peer);
    tracing::info!(
        method = session.method.as_str(),
        expires_at = session.expires_at,
        "web UI sign-in"
    );
    let response = Json(json!({
        "ok": true,
        "method": session.method.as_str(),
        "label": session.label,
        "expires_at": session.expires_at,
        "session_days": access.session_lifetime_days(),
    }))
    .into_response();
    Ok(with_cookie(response, session_cookie(&token, lifetime, secure)))
}

/// `POST /api/auth/logout` - sign this browser out.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> ApiResult<Response> {
    let secure = client_reads_https(&state, &state.access(), &headers, peer_addr(&connect));
    if let Some(token) = session_cookie_from(&headers) {
        let _ = state.web_auth().revoke_session(&token).await;
    }
    Ok(with_cookie(Json(json!({"ok": true})).into_response(), clear_cookie(secure)))
}

// ---------------------------------------------------------------------------
// Passkeys: sign in
// ---------------------------------------------------------------------------

/// `POST /api/auth/passkey/options` - start a passkey sign-in.
pub async fn passkey_options(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> ApiResult<Json<Value>> {
    let peer = peer_addr(&connect);
    let key = failure_key(&state, &headers, peer);
    if let Some(secs) = state.web_auth().locked_out_for(&key) {
        return Err(ApiError::too_many_requests(format!(
            "too many failed sign-in attempts; try again in {secs} seconds"
        )));
    }
    let origins = ceremony_origins(&state, &headers, peer);
    if origins.is_empty() {
        return Err(ApiError::bad_request(
            "cannot determine this server's address; set the public URL in the Access settings",
        ));
    }
    let rp_id = rp_id_from(&origins);
    let (challenge_id, challenge) = state
        .web_auth()
        .mint_challenge(crate::web_auth::ChallengeKind::Login);
    if challenge.is_empty() {
        return Err(ApiError::internal(
            "the server could not generate a challenge; see the server log",
        ));
    }
    Ok(Json(json!({
        "challenge_id": challenge_id,
        "challenge": challenge,
        "rp_id": rp_id,
        "timeout_ms": CHALLENGE_TTL.as_millis() as u64,
        // Credential ids only: a browser needs them to offer the right passkey,
        // and they are opaque handles rather than secrets. Names stay here.
        "allow": state.web_auth().passkeys().into_iter().map(|passkey| json!({
            "id": passkey.id,
            "type": "public-key",
        })).collect::<Vec<_>>(),
    })))
}

/// `POST /api/auth/passkey/login` - finish a passkey sign-in.
pub async fn passkey_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let peer = peer_addr(&connect);
    let key = failure_key(&state, &headers, peer);
    if let Some(secs) = state.web_auth().locked_out_for(&key) {
        return Err(ApiError::too_many_requests(format!(
            "too many failed sign-in attempts; try again in {secs} seconds"
        )));
    }
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        challenge_id: String,
        #[serde(default)]
        credential_id: String,
        #[serde(default)]
        client_data_json: String,
        #[serde(default)]
        authenticator_data: String,
        #[serde(default)]
        signature: String,
    }
    let body: Body = parse_required(&body, "passkey sign-in")?;
    let input = AssertionInput {
        credential_id: body.credential_id,
        client_data_json: body.client_data_json,
        authenticator_data: body.authenticator_data,
        signature: body.signature,
    };
    let origins = ceremony_origins(&state, &headers, peer);
    let passkey = state
        .web_auth()
        .verify_assertion(&body.challenge_id, &input, &origins)
        .map_err(|error| login_error(&state, &key, error))?;
    let _ = state
        .web_auth()
        .record_assertion_use(&passkey.id, passkey.counter)
        .await;
    state.web_auth().clear_failures(&key);
    sign_in_response(&state, &headers, peer, SessionMethod::Passkey, &passkey.name).await
}

fn login_error(state: &AppState, key: &str, error: LoginError) -> ApiError {
    match error {
        LoginError::NotConfigured => ApiError::bad_request(
            "no passkey is registered on this server; register one in the Access settings",
        ),
        LoginError::LockedOut(secs) => ApiError::too_many_requests(format!(
            "too many failed sign-in attempts; try again in {secs} seconds"
        )),
        LoginError::BadRequest(message) => ApiError::bad_request(message),
        LoginError::Rejected(message) => {
            let locked = state.web_auth().record_failure(key);
            if locked {
                ApiError::too_many_requests("too many failed sign-in attempts; try again later")
            } else {
                ApiError::unauthorized_message(message)
            }
        }
    }
}

fn rp_id_from(origins: &[String]) -> String {
    origins
        .first()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, rest)| strip_port(rest.split('/').next().unwrap_or_default()))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

// ---------------------------------------------------------------------------
// Password
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordBody {
    pub password: String,
    /// The current password, required when one is already set.
    #[serde(default)]
    pub current_password: Option<String>,
}

/// `POST /api/auth/password` - set or change the password.
pub async fn set_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let body: PasswordBody = parse_required(&body, "setting a password")?;
    let web_auth = state.web_auth();
    if web_auth.password_set() {
        // Changing a password is a credentialed action: knowing the old one is
        // what stops a hijacked session from locking the owner out.
        let current = body.current_password.clone().unwrap_or_default();
        if !web_auth.verify_password(&current).await {
            return Err(ApiError::forbidden(
                "the current password is required to change it",
            ));
        }
    }
    web_auth.set_password(&body.password).await.map_err(|error| {
        ApiError::bad_request(state.redact(&error.to_string()))
    })?;
    // Creating the first credential turns sign-in on: someone who sets a password
    // wants a password. The value is reported so the UI can say so.
    let enabled_now = ensure_access_enabled(&state).await?;
    tracing::info!(enabled_now, "web-UI password set");
    let peer = peer_addr(&connect);
    // Sign this browser in so the change cannot lock the person out.
    let mut response = sign_in_response(&state, &headers, peer, SessionMethod::Password, "password").await?;
    let payload = json!({
        "ok": true,
        "enabled": true,
        "enabled_now": enabled_now,
        "session_days": state.access().session_lifetime_days(),
    });
    *response.body_mut() = Body::from(payload.to_string());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

/// `DELETE /api/auth/password` - remove the password.
pub async fn delete_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        #[serde(default)]
        current_password: Option<String>,
    }
    let body: Body = parse_body(&body)?;
    let web_auth = state.web_auth();
    if !web_auth.password_set() {
        return Ok(Json(json!({"ok": true, "password_set": false})));
    }
    if !web_auth
        .verify_password(body.current_password.as_deref().unwrap_or_default())
        .await
    {
        return Err(ApiError::forbidden(
            "the current password is required to remove it",
        ));
    }
    web_auth.clear_password().await.map_err(ApiError::from)?;
    // Signing out every browser is the safe half of removing a credential.
    let _ = session_cookie_from(&headers);
    let remaining = web_auth.passkey_count();
    if remaining == 0 {
        disable_access(&state).await?;
    }
    Ok(Json(json!({
        "ok": true,
        "password_set": false,
        "passkey_count": remaining,
        "enabled": state.access().enabled,
    })))
}

/// `GET /api/auth/passkeys` - the registered passkeys.
pub async fn list_passkeys(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({
        "passkeys": state.web_auth().passkeys(),
        "password_set": state.web_auth().password_set(),
    })))
}

// ---------------------------------------------------------------------------
// Passkeys: registration
// ---------------------------------------------------------------------------

/// `POST /api/auth/passkeys/options` - start registering a passkey.
pub async fn register_options(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> ApiResult<Json<Value>> {
    let peer = peer_addr(&connect);
    let origins = ceremony_origins(&state, &headers, peer);
    if origins.is_empty() {
        return Err(ApiError::bad_request(
            "cannot determine this server's address; set the public URL in the Access settings",
        ));
    }
    let rp_id = rp_id_from(&origins);
    let (challenge_id, challenge) = state
        .web_auth()
        .mint_challenge(crate::web_auth::ChallengeKind::Register);
    if challenge.is_empty() {
        return Err(ApiError::internal(
            "the server could not generate a challenge; see the server log",
        ));
    }
    // A stable per-install user handle: 16 bytes derived from the relying party,
    // which is what an authenticator stores next to the credential.
    let user_id = crate::webauthn::encode_b64url(&{
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"karucode-web-ui");
        hasher.update(rp_id.as_bytes());
        hasher.finalize()[..16].to_vec()
    });
    Ok(Json(json!({
        "challenge_id": challenge_id,
        "challenge": challenge,
        "rp": { "id": rp_id, "name": "karucode" },
        "user": {
            "id": user_id,
            "name": "karucode",
            "displayName": "karucode",
        },
        "timeout_ms": CHALLENGE_TTL.as_millis() as u64,
        "exclude": state.web_auth().passkeys().into_iter().map(|passkey| json!({
            "id": passkey.id,
            "type": "public-key",
        })).collect::<Vec<_>>(),
    })))
}

/// `POST /api/auth/passkeys` - finish registering a passkey.
pub async fn register_passkey(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        challenge_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        credential_id: String,
        #[serde(default)]
        client_data_json: String,
        #[serde(default)]
        attestation_object: String,
    }
    let body: Body = parse_required(&body, "registering a passkey")?;
    let input = RegistrationInput {
        credential_id: body.credential_id,
        client_data_json: body.client_data_json,
        attestation_object: body.attestation_object,
    };
    let origins = ceremony_origins(&state, &headers, peer_addr(&connect));
    let passkey = state
        .web_auth()
        .register_passkey(&body.challenge_id, &body.name, &input, &origins)
        .await
        .map_err(|error| login_error(&state, "", error))?;
    let enabled_now = ensure_access_enabled(&state).await?;
    tracing::info!(passkey = %passkey.name, "web-UI passkey registered");
    Ok(Json(json!({
        "passkey": passkey,
        "enabled": true,
        "enabled_now": enabled_now,
    })))
}

/// `PATCH /api/auth/passkeys/:id` - rename a passkey.
pub async fn rename_passkey(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        name: String,
    }
    let body: Body = parse_required(&body, "renaming a passkey")?;
    let passkey = state
        .web_auth()
        .rename_passkey(&id, &body.name)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "passkey": passkey })))
}

/// `DELETE /api/auth/passkeys/:id` - remove a passkey.
pub async fn delete_passkey(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<Value>> {
    let web_auth = state.web_auth();
    web_auth.remove_passkey(&id).await.map_err(ApiError::from)?;
    let remaining = web_auth.passkey_count();
    // Removing the last credential must not leave sign-in switched on with
    // nothing to sign in with.
    if remaining == 0 && !web_auth.password_set() {
        disable_access(&state).await?;
    }
    Ok(Json(json!({
        "ok": true,
        "passkey_count": remaining,
        "enabled": state.access().enabled,
    })))
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// `GET /api/auth/sessions` - the browsers that are signed in.
pub async fn list_sessions(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let current = session_cookie_from(&headers).and_then(|token| state.web_auth().session(&token));
    let current_id = current.map(|session| session.token_hash.chars().take(12).collect::<String>());
    Ok(Json(json!({
        "sessions": state.web_auth().sessions(),
        "current": current_id,
    })))
}

/// `DELETE /api/auth/sessions/:id` - sign one browser out.
pub async fn revoke_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<Value>> {
    let removed = state
        .web_auth()
        .revoke_session_by_id(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": removed, "removed": removed })))
}

/// `POST /api/auth/sessions/revoke-all` - sign every browser out, including this
/// one.
pub async fn revoke_all_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> ApiResult<Response> {
    let removed = state
        .web_auth()
        .revoke_all_sessions()
        .await
        .map_err(ApiError::from)?;
    let secure = client_reads_https(&state, &state.access(), &headers, peer_addr(&connect));
    tracing::info!(removed, "every web-UI session was signed out");
    let response = Json(json!({ "ok": true, "removed": removed })).into_response();
    Ok(with_cookie(response, clear_cookie(secure)))
}

// ---------------------------------------------------------------------------
// Policy helpers
// ---------------------------------------------------------------------------

/// Turn sign-in on. Returns whether this call changed it.
///
/// Called after the first credential is created: setting a password is a
/// statement of intent, and leaving `[access] enabled = false` afterwards would
/// silently ignore it.
async fn ensure_access_enabled(state: &AppState) -> ApiResult<bool> {
    if state.access().enabled {
        return Ok(false);
    }
    state
        .settings()
        .update(&ConfigPatch {
            access: Some(AccessPatch {
                enabled: Some(true),
                ..AccessPatch::default()
            }),
            ..ConfigPatch::default()
        })
        .await
        .map_err(ApiError::from)?;
    Ok(true)
}

/// Turn sign-in off, for when the last credential was removed.
async fn disable_access(state: &AppState) -> ApiResult<()> {
    if !state.access().enabled {
        return Ok(());
    }
    state
        .settings()
        .update(&ConfigPatch {
            access: Some(AccessPatch {
                enabled: Some(false),
                ..AccessPatch::default()
            }),
            ..ConfigPatch::default()
        })
        .await
        .map_err(ApiError::from)?;
    tracing::warn!(
        "sign-in was switched off because the last password/passkey was removed; the web UI is \
         open to anyone who can reach the port again"
    );
    Ok(())
}

/// Reject a settings patch that would switch sign-in on with no credential.
///
/// This is the lockout guard: `[access] enabled = true` with an empty credential
/// store means the login screen can never be passed. The check lives at the API
/// boundary because only the server knows the credential store.
pub fn check_access_patch(state: &AppState, patch: &ConfigPatch) -> ApiResult<()> {
    let Some(access) = &patch.access else {
        return Ok(());
    };
    let enabling = access.enabled == Some(true) && !state.access().enabled;
    if !enabling {
        return Ok(());
    }
    if state.web_auth().has_credential() || state.auth_token().is_some() {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "cannot require sign-in before a password or passkey exists: that would lock you out of \
         the web UI. Set a password first, then turn sign-in on",
    ))
}

/// The sign-in screen's "manage" payload: everything the Access tab reads.
pub async fn access_summary(state: &AppState) -> Value {
    let access = state.access();
    let web_auth: &WebAuth = state.web_auth();
    json!({
        "enabled": access.enabled,
        "password_set": web_auth.password_set(),
        "passkeys": web_auth.passkeys(),
        "sessions": web_auth.sessions(),
        "public_url": access.public_url,
        "session_days": access.session_lifetime_days(),
        "tunnel": access.tunnel.as_str(),
        "trusted_proxies": access.trusted_proxies,
        "trusts_forwarded_headers": access.trusts_forwarded_headers(),
        "cloudflare_ranges": access.tunnel == kc_core::config::TunnelKind::Cloudflare,
        "token_required": state.auth_token().is_some(),
        "settings_locked": state.settings().is_locked(),
    })
}
