// @kc-doc: api-keys
// @kc-title: Issue API keys for applications
// @kc-summary: Mint scoped, expiring bearer keys for scripts and integrations, then rotate or revoke them without touching the web password.
// @kc-tags: api keys, authentication, bearer, token, integration, scopes, expiry, rotate, revoke, security, automation
// @kc-applies-to: route:GET /api/auth/api-keys, route:POST /api/auth/api-keys, route:PATCH /api/auth/api-keys/:id, route:DELETE /api/auth/api-keys/:id, route:POST /api/auth/api-keys/:id/rotate, route:GET /api/auth/me, route:GET /api/auth/status, header:Authorization, header:X-Karucode-Client, header:X-Karucode-Admin-Token, env:KARUCODE_ADMIN_TOKEN, config:admin_token, config:[access], config:access.enabled, ui:Settings, ui:AccessSettings, file:crates/kc-server/src/api_keys.rs, file:crates/kc-server/src/app.rs, doc:docs/API.md, doc:docs/SECURITY.md
//
// # What an API key is
//
// An API key is a **named bearer credential for one application**. It works
// wherever `KARUCODE_AUTH_TOKEN` works, but it is not the master token:
//
// * it has a **name** (`ci-runner`, `slack-bot`) so the key list says who holds it;
// * it carries **scopes** - `read`, `write` or `admin` - and the server refuses
//   the surface outside them with `403 permission_denied`;
// * it **expires** (by default it does not; ask for a lifetime when you create it);
// * it can be **rotated** (same key name and scopes, a new secret) or **revoked**
//   without changing the web password, the passkeys or `KARUCODE_AUTH_TOKEN`.
//
// Only a SHA-256 of the key is stored, so a copy of the database cannot be turned
// back into a credential. The full token is shown **once**, in the response that
// created (or rotated) it.
//
// # Before you can create one
//
// Keys only make a server *more* closed, so karucode refuses to mint one while the
// server is open: sign-in must already be required, either by
// `KARUCODE_AUTH_TOKEN` or by `[access] enabled = true` with a password or a
// passkey. Creating a key on an open server would be security theatre, because
// the key would not be needed to reach anything.
//
// # Create, use, revoke
//
// ```bash
// # Create. The full token is in `.token` and is never shown again.
// curl -s -X POST localhost:8787/api/auth/api-keys \
//   -H "Authorization: Bearer $KARUCODE_AUTH_TOKEN" \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"name":"ci-runner","scopes":["read","write"],"expires_in_days":30}'
//
// # Drive the API with it. Mutations also need the CSRF header.
// curl -s localhost:8787/api/sessions \
//   -H "Authorization: Bearer kck_..."
// curl -s -X POST localhost:8787/api/sessions/abc/messages \
//   -H "Authorization: Bearer kck_..." \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"content":"summarise today’s failures"}'
//
// # Rotation returns a new secret for the same key; the old one stops working
// # on the next request. Revocation is immediate.
// curl -s -X POST localhost:8787/api/auth/api-keys/<id>/rotate \
//   -H "Authorization: Bearer $KARUCODE_AUTH_TOKEN" -H 'X-Karucode-Client: web'
// curl -s -X DELETE localhost:8787/api/auth/api-keys/<id> \
//   -H "Authorization: Bearer $KARUCODE_AUTH_TOKEN" -H 'X-Karucode-Client: web'
// ```
//
// `POST` answers `{ "api_key": {...}, "token": "kck_...", "warning": ... }`.
// `GET /api/auth/api-keys` lists every retained key with its `status`
// (`active`, `expired`, `revoked`), `prefix`, last four characters, scopes,
// creation time, expiry, last use and request count.
//
// `PATCH /api/auth/api-keys/<id>` renames a key or changes its deadline:
// `{"name":"…"}`, `{"expires_in_days":90}`, or `{"clear_expiry":true}`. A key
// that has already expired or been revoked cannot be edited or rotated - mint a
// new one, because reviving a dead credential is exactly the mistake the dead
// state exists to prevent.
//
// # Scopes
//
// | Scope | What it allows |
// | --- | --- |
// | `read` | `GET`/`HEAD` on `/api` and `/metrics` |
// | `write` | the above **plus** `POST`/`PATCH`/`PUT`/`DELETE` on ordinary routes: sessions, messages, projects, files, git, missions, approvals-listing, artifacts |
// | `admin` | everything, including the surfaces below |
//
// `write` implies `read`; `admin` implies both. The default when `scopes` is
// omitted is `["read","write"]` - an integration can do its job, but cannot
// manage credentials. `admin` must be asked for explicitly.
//
// These routes always need `admin`, even for a `GET`:
//
// * `/api/auth/api-keys…` - key management itself;
// * `/api/auth/password`, `/api/auth/passkeys…` and revoking sessions - changing
//   how a human signs in;
// * mutating `/api/config`, `/api/plugins…`, `/api/update/apply` and
//   `/api/provider/test` - changing how the agent behaves or which binary runs;
// * every `/api/terminals…` route - a terminal is a shell without an approval
//   prompt;
// * `POST /api/approvals/:id` - answering an approval is the human gate, and a
//   request-only key must not be able to click "allow" on its own.
//
// A `write` key can still start runs, edit workspace files and commit; that is
// what an integration is for. Use `read` for dashboards and monitors.
//
// # Expiry and retention
//
// `expires_in_days` (1–3650) or `expires_at` (Unix seconds, also at most 3650
// days away) sets the deadline; giving both is a `400`. Omitting both creates a
// key that never expires, and the response says so in `warning`.
//
// An expired key is refused with `401` on its next request; the expiry message
// says it expired rather than pretending the key is unknown. Revoked and expired
// rows stay in the list for 30 days so an operator can see what happened, then
// they are dropped at startup. The list is capped at `max_keys` (100 live keys),
// and `POST` answers `400` beyond it.
//
// # Admin token, and the two credentials
//
// With `KARUCODE_ADMIN_TOKEN` configured, **managing** keys (create, rotate,
// update, revoke) needs `X-Karucode-Admin-Token` as well. Listing is an ordinary
// authenticated read. The browser does not store the admin token; Settings ->
// Access has a field for it, and a terminal can use `curl`. This is deliberate:
// without it, anyone holding the ordinary bearer token could mint a long-lived
// key that survives a token rotation.
//
// An `admin`-scoped API key does **not** satisfy that admin gate, and it does not
// turn `KARUCODE_LOCK_SETTINGS` off. The admin token remains a separate
// human-held credential.
//
// # Gotchas
//
// * Mutations still need `X-Karucode-Client: web` (the CSRF rule applies to the
//   API key too), and the API has no CORS: an integration calls it server-side,
//   not from a browser page.
// * `GET /api/events` and `GET /api/terminals/:id/socket` are the only routes
//   that accept `?token=<key>` as well as the header. The terminal socket needs
//   the `admin` scope; the event stream needs `read`.
// * `GET /api/auth/me` identifies the caller: `principal: "api_key"`, the key's
//   `label`, its `expires_at` and its `scopes`.
// * Last use is written to the database at most once a minute per server, so a
//   burst of requests updates the timestamp without a write per request.
// * Keys never appear in `config.toml` or `GET /api/config`, and no log line
//   contains the secret. Treat the token like a password: HTTPS (a reverse proxy)
//   or loopback only.
// * `POST` is not idempotent and the token exists only in its answer. A create
//   that times out (or whose connection drops) is therefore ambiguous: the key
//   may exist on the server while its secret is gone for good. The web UI
//   (Settings -> Access) never silently re-sends in that case — it re-reads the
//   key list so a minted key is visible, and tells the reader to **rotate** that
//   key (which mints a new secret) rather than create a second one. A script
//   hitting the same ambiguity should do the same: list, then rotate the row
//   that appeared, instead of POSTing `create` twice.
//
// Full reference: `docs/API.md` (API keys), `docs/SECURITY.md` (credentials).

use axum::http::Method;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use kc_core::error::{Error, Result};
use kc_core::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Settings-table key holding the JSON key list.
pub const SETTING_KEY: &str = "api_keys.registry";
/// The token's fixed prefix, so a key is recognisable on sight.
pub const TOKEN_PREFIX: &str = "kck";
/// The prefix plus its separator: the cheap first test for "is this a key?".
pub const TOKEN_PREFIX_WITH_SEPARATOR: &str = "kck_";
/// Characters in the public id (hex-encoded 8 random bytes).
pub const ID_CHARS: usize = 16;
/// Bytes of entropy in a key's secret.
pub const SECRET_BYTES: usize = 32;
/// Longest bearer value considered at all, so a pathological header is cheap.
pub const MAX_TOKEN_LEN: usize = 128;
/// Live keys one install may hold. Revoked and expired rows do not count.
pub const MAX_KEYS: usize = 100;
/// Longest accepted key name.
pub const MAX_NAME_CHARS: usize = 64;
/// Longest lifetime a key may be given, in days.
pub const MAX_LIFETIME_DAYS: i64 = 3_650;
/// How often a last-used change is written to the database.
pub const LAST_USED_PERSIST_SECS: i64 = 60;
/// How long a dead key's row is kept so the list can explain what happened.
pub const RETENTION_SECS: i64 = 30 * 24 * 3600;

const DAY_SECS: i64 = 86_400;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hash_token(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()))
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

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out)
        .map_err(|error| Error::Internal(format!("the OS random source failed: {error}")))?;
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// A fresh secret, base64url so it is safe in a header and in a URL's
/// `?token=`.
fn mint_secret() -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<SECRET_BYTES>()?))
}

/// What a key may do. `write` implies `read`; `admin` implies both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Scope::Read => 0,
            Scope::Write => 1,
            Scope::Admin => 2,
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Scope::Read),
            "write" => Some(Scope::Write),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }
}

/// Whether `scopes` contains something that grants `need`.
pub fn scopes_allow(scopes: &[Scope], need: Scope) -> bool {
    match need {
        Scope::Read => scopes
            .iter()
            .any(|scope| matches!(scope, Scope::Read | Scope::Write | Scope::Admin)),
        Scope::Write => scopes
            .iter()
            .any(|scope| matches!(scope, Scope::Write | Scope::Admin)),
        Scope::Admin => scopes.contains(&Scope::Admin),
    }
}

/// The scope one request needs, from its normalized path and method.
///
/// The caller passes the path through [`crate::app::normalize_path`]-compatible
/// rules (the middleware does), so `//api/terminals` and `/x/../api/config`
/// cannot map to a weaker scope than their real target.
pub fn required_scope(path: &str, method: &Method) -> Scope {
    let normalized = crate::app::normalize_path(path);
    let path = normalized.as_str();
    if admin_only(path, method) {
        return Scope::Admin;
    }
    match *method {
        Method::GET | Method::HEAD => Scope::Read,
        _ => Scope::Write,
    }
}

/// The routes whose weakest acceptable credential is an admin-scoped key.
fn admin_only(path: &str, method: &Method) -> bool {
    // Key management and everything that changes how a person signs in.
    if path.starts_with("/api/auth/api-keys")
        || path.starts_with("/api/auth/password")
        || path.starts_with("/api/auth/passkeys")
    {
        return true;
    }
    if path == "/api/auth/sessions/revoke-all" {
        return true;
    }
    if path.starts_with("/api/auth/sessions/") && *method == Method::DELETE {
        return true;
    }
    // A shell without an approval prompt, and the answers to approvals.
    if path.starts_with("/api/terminals") {
        return true;
    }
    if path.starts_with("/api/approvals/") && *method == Method::POST {
        return true;
    }
    // Changing how the agent behaves, which model endpoint it talks to, which
    // plugins run, or which binary is installed.
    if path == "/api/config" && *method != Method::GET {
        return true;
    }
    if path == "/api/provider/test" {
        return true;
    }
    if path.starts_with("/api/plugins") && *method != Method::GET {
        return true;
    }
    if path == "/api/update/apply" {
        return true;
    }
    false
}

/// One API key as stored. The raw token is never part of this struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKey {
    /// Public id, embedded in the token so a lookup does not scan every hash.
    pub id: String,
    pub name: String,
    /// The display prefix (`kck_<id>`), never the secret.
    #[serde(default)]
    pub prefix: String,
    /// The last four characters of the secret, for recognising a key by eye.
    #[serde(default)]
    pub last4: String,
    /// Base64url SHA-256 of the full token.
    pub token_hash: String,
    pub scopes: Vec<Scope>,
    pub created_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub revoked_at: Option<i64>,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub last_used_ip: Option<String>,
    #[serde(default)]
    pub request_count: u64,
}

/// The browser-facing view: metadata only, no hash, no secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub last4: String,
    pub scopes: Vec<Scope>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub last_used_ip: Option<String>,
    pub request_count: u64,
    /// `active`, `expired` or `revoked`.
    pub status: String,
}

/// What the auth middleware needs after a key is proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyIdentity {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub expires_at: Option<i64>,
}

/// Why a presented key was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRejection {
    /// Not a well-formed key, or no key with that id and secret.
    Unknown,
    Revoked,
    Expired,
}

impl KeyRejection {
    /// Whether the presented secret did match a real key, so the refusal can name
    /// the reason without helping a guesser.
    pub fn is_definitive(&self) -> bool {
        !matches!(self, KeyRejection::Unknown)
    }

    pub fn message(&self) -> &'static str {
        match self {
            KeyRejection::Unknown => "the API key is not valid",
            KeyRejection::Revoked => "this API key was revoked; create a new one",
            KeyRejection::Expired => "this API key has expired; create a new one",
        }
    }
}

/// `POST /api/auth/api-keys` body.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateKeyRequest {
    pub name: String,
    /// Omitted means `["read","write"]`; never implies `admin`.
    pub scopes: Option<Vec<String>>,
    /// Convenience lifetime, 1..=3650.
    pub expires_in_days: Option<i64>,
    /// Absolute Unix-second deadline, in the future and at most 3650 days away.
    pub expires_at: Option<i64>,
}

/// `PATCH /api/auth/api-keys/:id` body. Every field is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateKeyRequest {
    pub name: Option<String>,
    pub expires_in_days: Option<i64>,
    /// Remove the expiry entirely. Mutually exclusive with `expires_in_days`.
    pub clear_expiry: Option<bool>,
}

/// The stored set of keys, mirrored in memory and persisted as one JSON row.
///
/// The settings table is used deliberately, like the web-UI credentials: a key
/// hash must not be copied into `config.toml`, must not appear in
/// `GET /api/config`, and must survive a restart.
pub struct ApiKeyStore {
    store: Arc<dyn Store>,
    keys: RwLock<Vec<ApiKey>>,
    /// Serialises read-modify-write persistence.
    write_lock: tokio::sync::Mutex<()>,
    /// Unix seconds of the last last-used flush, so a burst of requests does not
    /// write the database per request.
    last_persist_secs: AtomicI64,
}

impl ApiKeyStore {
    /// Load the keys, dropping rows that are both dead and past retention.
    ///
    /// An unreadable row is ignored with a warning rather than aborting startup:
    /// a corrupt key list must not take the server down, and the keys it held are
    /// unusable anyway. No key means "no application credential", never "open
    /// server" - whether the API needs a credential is decided by the token and
    /// `[access]`, not by this list.
    pub async fn load(store: Arc<dyn Store>) -> Result<Arc<Self>> {
        let mut keys = Vec::new();
        // Whether retention removed something, so the smaller list can be
        // written back instead of leaving dead rows in the JSON forever.
        let mut pruned = false;
        let rows = store
            .all_settings()
            .await
            .map_err(|e| Error::Store(format!("could not read the API-key settings: {e}")))?;
        for (key, value) in rows {
            if key != SETTING_KEY {
                continue;
            }
            match serde_json::from_str::<Vec<ApiKey>>(&value) {
                Ok(parsed) => {
                    let now = now_secs();
                    let before = parsed.len();
                    keys = parsed
                        .into_iter()
                        .filter(|key| {
                            !key.id.is_empty()
                                && !key.token_hash.is_empty()
                                && !dead_past_retention(key, now)
                        })
                        .collect();
                    pruned |= keys.len() != before;
                }
                Err(error) => tracing::warn!(
                    key = SETTING_KEY,
                    "ignoring an unreadable API-key list: {error}"
                ),
            }
        }
        if pruned {
            // Best effort: a read-only database must not stop the server, and
            // the in-memory list is already the pruned one either way.
            match serde_json::to_string(&keys) {
                Ok(value) => {
                    if let Err(error) = store.set_setting(SETTING_KEY, &value).await {
                        tracing::warn!(%error, "could not persist the pruned API-key list");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "could not encode the pruned API-key list");
                }
            }
        }
        Ok(Arc::new(Self {
            store,
            keys: RwLock::new(keys),
            write_lock: tokio::sync::Mutex::new(()),
            last_persist_secs: AtomicI64::new(0),
        }))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<ApiKey>> {
        self.keys.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Vec<ApiKey>> {
        self.keys.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Every retained key, newest first.
    pub fn list(&self) -> Vec<ApiKeyView> {
        let now = now_secs();
        let mut views: Vec<ApiKeyView> = self.read().iter().map(|key| key.view(now)).collect();
        views.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        views
    }

    /// Keys that would authenticate right now.
    pub fn live_count(&self) -> usize {
        let now = now_secs();
        self.read()
            .iter()
            .filter(|key| key.status(now) == KeyStatus::Active)
            .count()
    }

    /// Whether anything at all is retained (for the startup warning).
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Mint a key. The returned token is the only copy of the secret.
    pub async fn create(&self, request: CreateKeyRequest) -> Result<(ApiKeyView, String)> {
        let now = now_secs();
        let name = validate_name(&request.name)?;
        let scopes = parse_scopes(request.scopes.as_deref())?;
        let expires_at = normalize_expiry(request.expires_in_days, request.expires_at, now)?;

        let _guard = self.write_lock.lock().await;
        let current = self.read().clone();
        if current
            .iter()
            .filter(|key| key.status(now) == KeyStatus::Active)
            .count()
            >= MAX_KEYS
        {
            return Err(Error::BadRequest(format!(
                "this server already has {MAX_KEYS} live API keys; revoke one before creating another"
            )));
        }

        let (id, secret, token) = self.mint_token(&current)?;
        let key = ApiKey {
            prefix: format!("{TOKEN_PREFIX}_{id}"),
            last4: last_four(&secret),
            id,
            name,
            token_hash: hash_token(&token),
            scopes,
            created_at: now,
            expires_at,
            revoked_at: None,
            last_used_at: None,
            last_used_ip: None,
            request_count: 0,
        };
        let view = key.view(now);
        let mut next = current;
        next.push(key);
        self.persist(&next).await?;
        self.write().clone_from(&next);
        Ok((view, token))
    }

    /// Replace a key's name and/or expiry. A revoked or already-expired key is
    /// refused: extending a dead credential would resurrect it silently.
    pub async fn update(&self, id: &str, request: UpdateKeyRequest) -> Result<ApiKeyView> {
        let now = now_secs();
        let clear_expiry = request.clear_expiry.unwrap_or(false);
        if clear_expiry && request.expires_in_days.is_some() {
            return Err(Error::BadRequest(
                "`clear_expiry` and `expires_in_days` cannot be sent together".into(),
            ));
        }
        let name = match request.name.as_deref() {
            Some(name) => Some(validate_name(name)?),
            None => None,
        };
        let expiry = if clear_expiry {
            Some(None)
        } else {
            match request.expires_in_days {
                Some(days) => Some(normalize_expiry(Some(days), None, now)?),
                None => None,
            }
        };
        if name.is_none() && expiry.is_none() {
            return Err(Error::BadRequest(
                "nothing to change: send `name`, `expires_in_days` or `clear_expiry`".into(),
            ));
        }

        let _guard = self.write_lock.lock().await;
        let mut next = self.read().clone();
        let Some(key) = next.iter_mut().find(|key| key.id == id) else {
            return Err(Error::NotFound(format!("no API key with id `{id}`")));
        };
        match key.status(now) {
            KeyStatus::Active => {}
            KeyStatus::Revoked => {
                return Err(Error::BadRequest(
                    "this API key is revoked; create a new one instead".into(),
                ))
            }
            KeyStatus::Expired => {
                return Err(Error::BadRequest(
                    "this API key has expired; create a new one instead".into(),
                ))
            }
        }
        if let Some(name) = name {
            key.name = name;
        }
        if let Some(expiry) = expiry {
            key.expires_at = expiry;
        }
        let view = key.view(now);
        self.persist(&next).await?;
        self.write().clone_from(&next);
        Ok(view)
    }

    /// Revoke a key, or remove its row entirely when `purge` is set.
    pub async fn revoke(&self, id: &str, purge: bool) -> Result<()> {
        let now = now_secs();
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().clone();
        let Some(index) = next.iter().position(|key| key.id == id) else {
            return Err(Error::NotFound(format!("no API key with id `{id}`")));
        };
        if purge {
            next.remove(index);
        } else {
            let key = &mut next[index];
            if key.revoked_at.is_none() {
                key.revoked_at = Some(now);
            }
        }
        self.persist(&next).await?;
        self.write().clone_from(&next);
        Ok(())
    }

    /// Replace the secret, keeping the id, name, scopes and expiry.
    pub async fn rotate(&self, id: &str) -> Result<(ApiKeyView, String)> {
        let now = now_secs();
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().clone();
        let Some(index) = next.iter().position(|key| key.id == id) else {
            return Err(Error::NotFound(format!("no API key with id `{id}`")));
        };
        if next[index].status(now) != KeyStatus::Active {
            return Err(Error::BadRequest(
                "only an active API key can be rotated; create a new one".into(),
            ));
        }
        let secret = mint_secret()?;
        let token = format!("{TOKEN_PREFIX}_{id}_{secret}");
        {
            let key = &mut next[index];
            key.token_hash = hash_token(&token);
            key.last4 = last_four(&secret);
            key.last_used_at = None;
            key.last_used_ip = None;
            key.request_count = 0;
        }
        let view = next[index].view(now);
        self.persist(&next).await?;
        self.write().clone_from(&next);
        Ok((view, token))
    }

    /// Prove a presented bearer value. Never mutates: the caller records the use.
    pub fn authenticate(&self, raw: &str) -> std::result::Result<ApiKeyIdentity, KeyRejection> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > MAX_TOKEN_LEN {
            return Err(KeyRejection::Unknown);
        }
        let Some(id) = parse_token_id(raw) else {
            return Err(KeyRejection::Unknown);
        };
        let hash = hash_token(raw);
        let keys = self.read();
        let Some(key) = keys.iter().find(|key| key.id == id) else {
            return Err(KeyRejection::Unknown);
        };
        if !constant_time_eq(key.token_hash.as_bytes(), hash.as_bytes()) {
            return Err(KeyRejection::Unknown);
        }
        let now = now_secs();
        match key.status(now) {
            KeyStatus::Revoked => Err(KeyRejection::Revoked),
            KeyStatus::Expired => Err(KeyRejection::Expired),
            KeyStatus::Active => Ok(ApiKeyIdentity {
                id: key.id.clone(),
                name: key.name.clone(),
                scopes: key.scopes.clone(),
                expires_at: key.expires_at,
            }),
        }
    }

    /// Note that a key was just used, flushing the timestamp at most once a minute.
    ///
    /// A failure to persist is logged, never returned: the request has already
    /// been authorised, and a bookkeeping write must not fail it.
    pub async fn record_use(&self, id: &str, ip: Option<IpAddr>) {
        let now = now_secs();
        let due = {
            let mut keys = self.write();
            let Some(key) = keys.iter_mut().find(|key| key.id == id) else {
                return;
            };
            key.last_used_at = Some(now);
            if let Some(ip) = ip {
                key.last_used_ip = Some(ip.to_string());
            }
            key.request_count = key.request_count.saturating_add(1);
            now.saturating_sub(self.last_persist_secs.load(Ordering::Relaxed))
                >= LAST_USED_PERSIST_SECS
        };
        if !due {
            return;
        }
        let _guard = self.write_lock.lock().await;
        let now = now_secs();
        if now.saturating_sub(self.last_persist_secs.load(Ordering::Relaxed))
            < LAST_USED_PERSIST_SECS
        {
            return;
        }
        self.last_persist_secs.store(now, Ordering::Relaxed);
        if let Err(error) = self.persist_current().await {
            tracing::warn!(%error, "could not persist API-key usage");
        }
    }

    fn mint_token(&self, existing: &[ApiKey]) -> Result<(String, String, String)> {
        for _ in 0..8 {
            let id = hex(&random_bytes::<8>()?);
            if existing.iter().any(|key| key.id == id) {
                continue;
            }
            let secret = mint_secret()?;
            let token = format!("{TOKEN_PREFIX}_{id}_{secret}");
            return Ok((id, secret, token));
        }
        Err(Error::Internal(
            "could not mint a unique API-key id; try again".into(),
        ))
    }

    async fn persist_current(&self) -> Result<()> {
        let next = self.read().clone();
        self.persist(&next).await
    }

    async fn persist(&self, keys: &[ApiKey]) -> Result<()> {
        let value = serde_json::to_string(keys)
            .map_err(|e| Error::Store(format!("could not encode the API-key list: {e}")))?;
        self.store
            .set_setting(SETTING_KEY, &value)
            .await
            .map_err(|e| Error::Store(format!("could not save the API-key list: {e}")))?;
        self.last_persist_secs.store(now_secs(), Ordering::Relaxed);
        Ok(())
    }
}

fn last_four(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

fn validate_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("an API key needs a name".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::BadRequest(format!(
            "the key name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(|ch| ch.is_control()) {
        return Err(Error::BadRequest(
            "the key name cannot contain control characters".into(),
        ));
    }
    Ok(name.to_string())
}

fn parse_scopes(raw: Option<&[String]>) -> Result<Vec<Scope>> {
    let Some(raw) = raw else {
        return Ok(vec![Scope::Read, Scope::Write]);
    };
    if raw.is_empty() {
        return Err(Error::BadRequest(
            "`scopes` cannot be empty; omit it for read+write, or list read/write/admin".into(),
        ));
    }
    let mut scopes = Vec::new();
    for entry in raw {
        let Some(scope) = Scope::parse(entry) else {
            return Err(Error::BadRequest(format!(
                "unknown scope `{entry}`; use read, write or admin"
            )));
        };
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.sort_by_key(|scope| scope.rank());
    Ok(scopes)
}

fn normalize_expiry(
    expires_in_days: Option<i64>,
    expires_at: Option<i64>,
    now: i64,
) -> Result<Option<i64>> {
    if expires_in_days.is_some() && expires_at.is_some() {
        return Err(Error::BadRequest(
            "give either `expires_in_days` or `expires_at`, not both".into(),
        ));
    }
    if let Some(days) = expires_in_days {
        if !(1..=MAX_LIFETIME_DAYS).contains(&days) {
            return Err(Error::BadRequest(format!(
                "`expires_in_days` must be between 1 and {MAX_LIFETIME_DAYS}"
            )));
        }
        return Ok(Some(now + days * DAY_SECS));
    }
    if let Some(at) = expires_at {
        if at <= now {
            return Err(Error::BadRequest(
                "`expires_at` must be a Unix-second time in the future".into(),
            ));
        }
        let latest = now + MAX_LIFETIME_DAYS * DAY_SECS;
        if at > latest {
            return Err(Error::BadRequest(format!(
                "`expires_at` is more than {MAX_LIFETIME_DAYS} days away"
            )));
        }
        return Ok(Some(at));
    }
    Ok(None)
}

/// The id portion of a well-formed token, if it is one.
fn parse_token_id(raw: &str) -> Option<&str> {
    if !raw.starts_with(TOKEN_PREFIX_WITH_SEPARATOR) {
        return None;
    }
    let mut parts = raw.splitn(3, '_');
    let prefix = parts.next()?;
    let id = parts.next()?;
    let secret = parts.next()?;
    if prefix != TOKEN_PREFIX
        || id.len() != ID_CHARS
        || secret.len() < 32
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(id)
}

fn dead_past_retention(key: &ApiKey, now: i64) -> bool {
    let dead_at = key.revoked_at.or(key.expires_at);
    dead_at.is_some_and(|at| now.saturating_sub(at) > RETENTION_SECS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    Active,
    Expired,
    Revoked,
}

impl ApiKey {
    fn status(&self, now: i64) -> KeyStatus {
        if self.revoked_at.is_some() {
            KeyStatus::Revoked
        } else if self.expires_at.is_some_and(|at| at <= now) {
            KeyStatus::Expired
        } else {
            KeyStatus::Active
        }
    }

    fn view(&self, now: i64) -> ApiKeyView {
        ApiKeyView {
            id: self.id.clone(),
            name: self.name.clone(),
            prefix: self.prefix.clone(),
            last4: self.last4.clone(),
            scopes: self.scopes.clone(),
            created_at: self.created_at,
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
            last_used_at: self.last_used_at,
            last_used_ip: self.last_used_ip.clone(),
            request_count: self.request_count,
            status: match self.status(now) {
                KeyStatus::Active => "active",
                KeyStatus::Expired => "expired",
                KeyStatus::Revoked => "revoked",
            }
            .to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (Arc<ApiKeyStore>, Arc<kc_store::SqliteStore>, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        let sqlite = Arc::new(
            kc_store::SqliteStore::open(&dir.path().join("karucode.db"))
                .await
                .expect("store"),
        );
        let keys = ApiKeyStore::load(Arc::clone(&sqlite) as Arc<dyn Store>)
            .await
            .expect("api keys");
        (keys, sqlite, dir)
    }

    mod tempdir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "kc-apikeys-{}-{}",
                    std::process::id(),
                    uuid::Uuid::new_v4()
                ));
                std::fs::create_dir_all(&path).expect("temp dir");
                Self(path)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn request(name: &str) -> CreateKeyRequest {
        CreateKeyRequest {
            name: name.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_created_key_has_the_documented_shape_and_authenticates() {
        let (keys, _store, _dir) = store().await;
        let (view, token) = keys.create(request("ci-runner")).await.expect("create");
        assert_eq!(view.name, "ci-runner");
        assert_eq!(view.status, "active");
        assert_eq!(view.scopes, vec![Scope::Read, Scope::Write]);
        assert!(view.prefix.starts_with("kck_"));
        assert_eq!(view.prefix, format!("kck_{}", view.id));
        assert_eq!(view.last4.len(), 4);
        assert!(token.starts_with(&format!("{TOKEN_PREFIX}_{}_", view.id)));
        assert!(view.expires_at.is_none());

        let identity = keys.authenticate(&token).expect("authenticates");
        assert_eq!(identity.id, view.id);
        assert_eq!(identity.name, "ci-runner");
        assert_eq!(identity.scopes, vec![Scope::Read, Scope::Write]);
    }

    #[tokio::test]
    async fn only_the_hash_is_stored() {
        let (keys, store, _dir) = store().await;
        let (_view, token) = keys.create(request("bot")).await.expect("create");
        assert_eq!(keys.list().len(), 1);
        let raw = store
            .get_setting(SETTING_KEY)
            .await
            .expect("read")
            .expect("row");
        assert!(!raw.contains(&token));
        assert!(raw.contains(&hash_token(&token)));
    }

    #[tokio::test]
    async fn an_unknown_or_malformed_key_is_unknown() {
        let (keys, _store, _dir) = store().await;
        keys.create(request("bot")).await.expect("create");
        assert_eq!(keys.authenticate("not-a-key"), Err(KeyRejection::Unknown));
        assert_eq!(
            keys.authenticate("kck_0000000000000000_short"),
            Err(KeyRejection::Unknown)
        );
        let forged = format!("{TOKEN_PREFIX}_0000000000000000_{}", "a".repeat(43));
        assert_eq!(keys.authenticate(&forged), Err(KeyRejection::Unknown));
    }

    #[tokio::test]
    async fn an_expired_key_is_refused_and_says_so() {
        let (keys, _store, _dir) = store().await;
        let (_view, token) = keys.create(request("old")).await.expect("create");
        {
            let mut list = keys.write();
            list[0].expires_at = Some(now_secs() - 1);
        }
        assert_eq!(keys.authenticate(&token), Err(KeyRejection::Expired));
        assert_eq!(keys.live_count(), 0);
        assert_eq!(keys.list()[0].status, "expired");
    }

    #[tokio::test]
    async fn a_revoked_key_stops_immediately_and_can_be_purged() {
        let (keys, _store, _dir) = store().await;
        let (view, token) = keys.create(request("leaked")).await.expect("create");
        keys.revoke(&view.id, false).await.expect("revoke");
        assert_eq!(keys.authenticate(&token), Err(KeyRejection::Revoked));
        assert_eq!(keys.list()[0].status, "revoked");
        // Revoking twice is idempotent.
        keys.revoke(&view.id, false).await.expect("revoke again");
        // A hard delete removes the row.
        keys.revoke(&view.id, true).await.expect("purge");
        assert!(keys.list().is_empty());
        let error = keys.revoke(&view.id, false).await.unwrap_err();
        assert!(matches!(error, Error::NotFound(_)), "{error:?}");
    }

    #[tokio::test]
    async fn rotation_replaces_the_secret_and_keeps_identity() {
        let (keys, _store, _dir) = store().await;
        let (view, old_token) = keys
            .create(CreateKeyRequest {
                name: "rotating".into(),
                scopes: Some(vec!["admin".into()]),
                expires_in_days: Some(7),
                expires_at: None,
            })
            .await
            .expect("create");
        let (rotated, new_token) = keys.rotate(&view.id).await.expect("rotate");
        assert_eq!(rotated.id, view.id);
        assert_eq!(rotated.name, "rotating");
        assert_eq!(rotated.scopes, vec![Scope::Admin]);
        assert_eq!(rotated.expires_at, view.expires_at);
        assert_eq!(keys.authenticate(&old_token), Err(KeyRejection::Unknown));
        assert_eq!(
            keys.authenticate(&new_token).expect("new token").id,
            view.id
        );
    }

    #[tokio::test]
    async fn scopes_are_validated_and_admin_must_be_explicit() {
        let (keys, _store, _dir) = store().await;
        let (view, token) = keys.create(request("default")).await.expect("create");
        assert_eq!(view.scopes, vec![Scope::Read, Scope::Write]);
        let identity = keys.authenticate(&token).expect("auth");
        assert!(!scopes_allow(&identity.scopes, Scope::Admin));
        assert!(scopes_allow(&identity.scopes, Scope::Write));
        assert!(scopes_allow(&identity.scopes, Scope::Read));

        let error = keys
            .create(CreateKeyRequest {
                name: "bad".into(),
                scopes: Some(vec!["root".into()]),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown scope"));

        let error = keys
            .create(CreateKeyRequest {
                name: "empty".into(),
                scopes: Some(Vec::new()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("read+write"));
    }

    #[tokio::test]
    async fn expiry_validation_rejects_the_impossible() {
        let (keys, _store, _dir) = store().await;
        let now = now_secs();
        for bad in [
            CreateKeyRequest {
                name: "x".into(),
                expires_in_days: Some(0),
                ..Default::default()
            },
            CreateKeyRequest {
                name: "x".into(),
                expires_in_days: Some(MAX_LIFETIME_DAYS + 1),
                ..Default::default()
            },
            CreateKeyRequest {
                name: "x".into(),
                expires_at: Some(now - 10),
                ..Default::default()
            },
            CreateKeyRequest {
                name: "x".into(),
                expires_at: Some(now + DAY_SECS * (MAX_LIFETIME_DAYS + 2)),
                ..Default::default()
            },
            CreateKeyRequest {
                name: "x".into(),
                expires_in_days: Some(1),
                expires_at: Some(now + 1_000),
                ..Default::default()
            },
        ] {
            assert!(keys.create(bad).await.is_err());
        }
        assert!(keys.list().is_empty());
        let (view, _) = keys
            .create(CreateKeyRequest {
                name: "week".into(),
                expires_in_days: Some(7),
                ..Default::default()
            })
            .await
            .expect("create");
        assert!(view.expires_at.expect("expiry") > now);
    }

    #[tokio::test]
    async fn names_are_trimmed_and_validated() {
        let (keys, _store, _dir) = store().await;
        let (view, _) = keys.create(request("  spaced  ")).await.expect("create");
        assert_eq!(view.name, "spaced");
        assert!(keys.create(request("   ")).await.is_err());
        assert!(keys
            .create(request(&"n".repeat(MAX_NAME_CHARS + 1)))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn keys_reload_from_the_database() {
        let (keys, store, _dir) = store().await;
        let (view, token) = keys
            .create(CreateKeyRequest {
                name: "persistent".into(),
                expires_in_days: Some(30),
                ..Default::default()
            })
            .await
            .expect("create");
        drop(keys);
        let reloaded = ApiKeyStore::load(store as Arc<dyn Store>)
            .await
            .expect("reload");
        let identity = reloaded.authenticate(&token).expect("auth after reload");
        assert_eq!(identity.id, view.id);
        assert_eq!(reloaded.list().len(), 1);
    }

    #[tokio::test]
    async fn retention_prunes_dead_rows_at_load_and_writes_the_smaller_list() {
        let (keys, store, _dir) = store().await;
        let (view, _) = keys.create(request("ancient")).await.expect("create");
        keys.revoke(&view.id, false).await.expect("revoke");
        // Pretend the revocation happened past the retention window, then write
        // that state back the way a real month of uptime would have.
        {
            let mut list = keys.write();
            list[0].revoked_at = Some(now_secs() - RETENTION_SECS - 10);
        }
        let snapshot = keys.read().clone();
        keys.persist(&snapshot).await.expect("persist");
        drop(keys);

        let reloaded = ApiKeyStore::load(Arc::clone(&store) as Arc<dyn Store>)
            .await
            .expect("reload");
        assert!(reloaded.is_empty());
        let raw = store
            .get_setting(SETTING_KEY)
            .await
            .expect("read")
            .expect("row");
        let stored: Vec<ApiKey> = serde_json::from_str(&raw).expect("json");
        assert!(stored.is_empty(), "the pruned row was written back: {raw}");
    }

    #[tokio::test]
    async fn last_use_is_recorded_and_flushed() {
        let (keys, store, _dir) = store().await;
        let (view, token) = keys.create(request("metered")).await.expect("create");
        let identity = keys.authenticate(&token).expect("auth");
        // Creation itself wrote the row, so the first use is inside the
        // once-a-minute window. Pretend the window elapsed so the flush is
        // exercised without a sleep.
        keys.last_persist_secs.store(0, Ordering::Relaxed);
        keys.record_use(&identity.id, Some("127.0.0.1".parse().unwrap()))
            .await;
        keys.last_persist_secs.store(0, Ordering::Relaxed);
        keys.record_use(&identity.id, None).await;
        let listed = keys.list();
        assert_eq!(listed[0].request_count, 2);
        assert!(listed[0].last_used_at.is_some());
        assert_eq!(listed[0].last_used_ip.as_deref(), Some("127.0.0.1"));
        let raw = store
            .get_setting(SETTING_KEY)
            .await
            .expect("read")
            .expect("row");
        assert!(raw.contains("\"request_count\":2"), "{raw}");
        assert_eq!(keys.live_count(), 1);
        assert_eq!(listed[0].id, view.id);
    }

    #[tokio::test]
    async fn the_live_key_limit_is_enforced() {
        let (keys, _store, _dir) = store().await;
        for index in 0..MAX_KEYS {
            keys.create(request(&format!("key-{index}")))
                .await
                .expect("create");
        }
        let error = keys.create(request("one-too-many")).await.unwrap_err();
        assert!(error.to_string().contains("revoke one"));
        // A revoked key frees a slot immediately.
        let first = keys.list().last().expect("a key").id.clone();
        keys.revoke(&first, false).await.expect("revoke");
        keys.create(request("replacement")).await.expect("create");
    }

    #[tokio::test]
    async fn updating_expiry_and_name_is_refused_for_dead_keys() {
        let (keys, _store, _dir) = store().await;
        let (view, _) = keys.create(request("named")).await.expect("create");
        let updated = keys
            .update(
                &view.id,
                UpdateKeyRequest {
                    name: Some("renamed".into()),
                    expires_in_days: Some(2),
                    clear_expiry: None,
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.name, "renamed");
        assert!(updated.expires_at.is_some());
        let cleared = keys
            .update(
                &view.id,
                UpdateKeyRequest {
                    name: None,
                    expires_in_days: None,
                    clear_expiry: Some(true),
                },
            )
            .await
            .expect("clear");
        assert!(cleared.expires_at.is_none());
        keys.revoke(&view.id, false).await.expect("revoke");
        assert!(keys
            .update(
                &view.id,
                UpdateKeyRequest {
                    name: Some("nope".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err());
    }

    #[test]
    fn the_scope_matrix_matches_the_documented_surface() {
        assert_eq!(required_scope("/api/sessions", &Method::GET), Scope::Read);
        assert_eq!(required_scope("/api/sessions", &Method::POST), Scope::Write);
        assert_eq!(
            required_scope("/api/sessions/abc/messages", &Method::POST),
            Scope::Write
        );
        assert_eq!(required_scope("/metrics", &Method::GET), Scope::Read);
        assert_eq!(
            required_scope("//api/auth/api-keys", &Method::GET),
            Scope::Admin
        );
        assert_eq!(
            required_scope("/x/../api/terminals", &Method::GET),
            Scope::Admin
        );
        assert_eq!(
            required_scope("/api/terminals/t1/socket", &Method::GET),
            Scope::Admin
        );
        assert_eq!(
            required_scope("/api/approvals/a1", &Method::POST),
            Scope::Admin
        );
        assert_eq!(required_scope("/api/approvals", &Method::GET), Scope::Read);
        assert_eq!(required_scope("/api/config", &Method::PATCH), Scope::Admin);
        assert_eq!(required_scope("/api/config", &Method::GET), Scope::Read);
        assert_eq!(
            required_scope("/api/plugins/foo/reload", &Method::POST),
            Scope::Admin
        );
        assert_eq!(required_scope("/api/plugins", &Method::GET), Scope::Read);
        assert_eq!(
            required_scope("/api/update/apply", &Method::POST),
            Scope::Admin
        );
        assert_eq!(
            required_scope("/api/update/check", &Method::POST),
            Scope::Write
        );
        assert_eq!(
            required_scope("/api/workspace/file", &Method::PUT),
            Scope::Write
        );
    }

    #[test]
    fn write_and_admin_imply_read_but_not_each_other() {
        let read = [Scope::Read];
        let write = [Scope::Write];
        let admin = [Scope::Admin];
        assert!(scopes_allow(&read, Scope::Read));
        assert!(!scopes_allow(&read, Scope::Write));
        assert!(!scopes_allow(&read, Scope::Admin));
        assert!(scopes_allow(&write, Scope::Read));
        assert!(scopes_allow(&write, Scope::Write));
        assert!(!scopes_allow(&write, Scope::Admin));
        assert!(scopes_allow(&admin, Scope::Read));
        assert!(scopes_allow(&admin, Scope::Write));
        assert!(scopes_allow(&admin, Scope::Admin));
    }
}
