//! Web-UI credentials and sessions: the password, the passkeys and the cookies.
//!
//! Everything here is stored in the SQLite `settings` table under `web_auth.*`
//! keys, which means three things on purpose:
//!
//! * **Not in `config.toml`.** A password hash or a cookie's hash must not be
//!   copied into a repository, and config files get copied around.
//! * **Not in `GET /api/config`.** The settings payload is built from
//!   [`kc_core::config::Config`], which never contains any of this.
//! * **Not in memory only.** A restart must not sign the operator out, and a
//!   passkey must survive it - that is the difference between a toy and a login.
//!
//! The password is hashed with Argon2id and verified in constant time by the
//! `argon2` crate. A session cookie is a random 32-byte token; only its SHA-256
//! is stored, so a copy of the database cannot be turned into a login. Passkey
//! private keys never leave the authenticator - only its public key is here, and
//! it can only *verify*.

use crate::webauthn::{self, AssertionInput, Ceremony, RegisteredCredential, RegistrationInput};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use kc_core::store::Store;
use kc_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Settings key holding the Argon2 PHC string of the password.
pub const KEY_PASSWORD: &str = "web_auth.password";
/// Settings key holding the passkey list (JSON).
pub const KEY_PASSKEYS: &str = "web_auth.passkeys";
/// Settings key holding the session list (JSON).
pub const KEY_SESSIONS: &str = "web_auth.sessions";
/// Settings key holding the onboarding marker.
pub const KEY_ONBOARDING: &str = "web_auth.onboarding";

/// The session cookie's name.
pub const SESSION_COOKIE: &str = "karucode_session";

/// Minimum password length. Ten is the floor below which a password on an
/// internet-reachable coding agent stops being a credential; the wizard shows
/// the same number so the form and the server cannot disagree.
pub const MIN_PASSWORD_LEN: usize = 10;
/// Longest password accepted, to bound the Argon2 work per attempt.
pub const MAX_PASSWORD_LEN: usize = 1024;

/// Failed sign-ins allowed from one key before a lockout.
pub const MAX_FAILURES: u32 = 8;
/// How long the failure window is, in seconds.
pub const FAILURE_WINDOW_SECS: u64 = 15 * 60;
/// How long a lockout lasts, in seconds.
pub const LOCKOUT_SECS: u64 = 5 * 60;
/// How long a WebAuthn challenge stays valid.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One registered passkey. The public key is stored; the private key lives in the
/// authenticator and is never sent anywhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Passkey {
    /// base64url credential id, the handle the browser sends back.
    pub id: String,
    /// What the operator called it ("MacBook Touch ID", "YubiKey").
    pub name: String,
    /// base64url SEC1 uncompressed P-256 point.
    pub public_key: String,
    /// Signature counter at the last successful assertion.
    #[serde(default)]
    pub counter: u32,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
}

/// Browser-facing view of a passkey: no key material, no counter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyView {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

impl From<&Passkey> for PasskeyView {
    fn from(passkey: &Passkey) -> Self {
        Self {
            id: passkey.id.clone(),
            name: passkey.name.clone(),
            created_at: passkey.created_at,
            last_used_at: passkey.last_used_at,
        }
    }
}

/// How a session was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMethod {
    Password,
    Passkey,
}

impl SessionMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionMethod::Password => "password",
            SessionMethod::Passkey => "passkey",
        }
    }
}

/// One signed-in browser. Only the hash of its token is stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// SHA-256 of the cookie value, base64url.
    pub token_hash: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub method: SessionMethod,
    /// The passkey's name for a passkey session, "password" otherwise.
    pub label: String,
}

impl Session {
    fn is_expired(&self, now: i64) -> bool {
        self.expires_at <= now
    }
}

/// A challenge that has been handed to the browser and is waiting for its answer.
#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub challenge: String,
    pub kind: ChallengeKind,
    created: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    Register,
    Login,
}

/// The stored state, kept in memory and mirrored to the settings table.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredState {
    #[serde(default)]
    password_hash: Option<String>,
    #[serde(default)]
    passkeys: Vec<Passkey>,
    #[serde(default)]
    sessions: Vec<Session>,
}

/// Why a sign-in attempt was refused, in the form the route turns into an HTTP
/// status. Every variant is a *result*, not a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// No credential of that kind is configured.
    NotConfigured,
    /// Wrong password / unknown passkey / failed assertion.
    Rejected(String),
    /// Too many failures from this key; carries the seconds remaining.
    LockedOut(u64),
    /// The request was malformed (a bad challenge id, a missing field).
    BadRequest(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::NotConfigured => write!(f, "no sign-in method is configured"),
            LoginError::Rejected(message) => write!(f, "{message}"),
            LoginError::LockedOut(secs) => write!(
                f,
                "too many failed sign-in attempts; try again in {secs} seconds"
            ),
            LoginError::BadRequest(message) => write!(f, "{message}"),
        }
    }
}

/// The web-UI credential store.
pub struct WebAuth {
    store: Arc<dyn Store>,
    state: RwLock<StoredState>,
    challenges: RwLock<HashMap<String, PendingChallenge>>,
    failures: RwLock<HashMap<String, Failures>>,
    /// Serialises read-modify-write persistence so two logins cannot mint a
    /// session on top of each other's snapshot.
    write_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone)]
struct Failures {
    count: u32,
    first_at: Instant,
    locked_until: Option<Instant>,
}

impl WebAuth {
    /// Load the credentials and sessions from the settings table.
    ///
    /// An unreadable row is **ignored with a warning**, not a hard error: a
    /// corrupt passkey list must not stop the server from starting, or the only
    /// way back in would be editing SQLite by hand. A corrupt *password* row is
    /// dropped the same way, which means the install falls back to "no sign-in" -
    /// which a remote bind then refuses, loudly, instead of silently serving.
    pub async fn load(store: Arc<dyn Store>) -> Result<Arc<Self>> {
        let mut state = StoredState::default();
        let rows = store
            .all_settings()
            .await
            .map_err(|e| Error::Store(format!("could not read web-auth settings: {e}")))?;
        for (key, value) in rows {
            match key.as_str() {
                KEY_PASSWORD => {
                    if value.trim().is_empty() {
                        state.password_hash = None;
                    } else if value.starts_with("$argon2") {
                        state.password_hash = Some(value);
                    } else {
                        tracing::warn!(
                            key = KEY_PASSWORD,
                            "ignoring a password row that is not an Argon2 hash; set the password \
                             again from the Access settings"
                        );
                    }
                }
                KEY_PASSKEYS => match serde_json::from_str::<Vec<Passkey>>(&value) {
                    Ok(passkeys) => state.passkeys = passkeys,
                    Err(error) => tracing::warn!(
                        key = KEY_PASSKEYS,
                        "ignoring an unreadable passkey list: {error}"
                    ),
                },
                KEY_SESSIONS => match serde_json::from_str::<Vec<Session>>(&value) {
                    Ok(sessions) => {
                        let now = now_secs();
                        state.sessions = sessions
                            .into_iter()
                            .filter(|session| !session.is_expired(now))
                            .collect();
                    }
                    Err(error) => tracing::warn!(
                        key = KEY_SESSIONS,
                        "ignoring an unreadable session list (every browser will have to sign in \
                         again): {error}"
                    ),
                },
                _ => {}
            }
        }
        Ok(Arc::new(Self {
            store,
            state: RwLock::new(state),
            challenges: RwLock::new(HashMap::new()),
            failures: RwLock::new(HashMap::new()),
            write_lock: tokio::sync::Mutex::new(()),
        }))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, StoredState> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, StoredState> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a password is set.
    pub fn password_set(&self) -> bool {
        self.read().password_hash.is_some()
    }

    /// Whether any sign-in method is configured.
    pub fn has_credential(&self) -> bool {
        let state = self.read();
        state.password_hash.is_some() || !state.passkeys.is_empty()
    }

    /// Browser-facing passkey list.
    pub fn passkeys(&self) -> Vec<PasskeyView> {
        self.read().passkeys.iter().map(PasskeyView::from).collect()
    }

    pub fn passkey_count(&self) -> usize {
        self.read().passkeys.len()
    }

    fn passkey(&self, id: &str) -> Option<Passkey> {
        self.read()
            .passkeys
            .iter()
            .find(|passkey| passkey.id == id)
            .cloned()
    }

    /// Set or replace the password. Refuses anything the wizard would refuse.
    pub async fn set_password(&self, password: &str) -> Result<()> {
        validate_password(password)?;
        let password = password.to_string();
        let hash = tokio::task::spawn_blocking(move || hash_password(&password))
            .await
            .map_err(|e| Error::Config(format!("password hashing failed: {e}")))??;
        let _guard = self.write_lock.lock().await;
        self.persist_password(Some(hash)).await
    }

    /// Remove the password. Passkeys, if any, keep working.
    pub async fn clear_password(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.persist_password(None).await
    }

    async fn persist_password(&self, hash: Option<String>) -> Result<()> {
        let value = hash.clone().unwrap_or_default();
        self.store
            .set_setting(KEY_PASSWORD, &value)
            .await
            .map_err(|e| Error::Store(format!("could not save the password: {e}")))?;
        self.write().password_hash = hash;
        // A password change invalidates every existing cookie: that is the whole
        // point of changing it after a suspected leak.
        if !self.with_sessions(|sessions| sessions.is_empty()) {
            self.persist_sessions(Vec::new()).await?;
        }
        Ok(())
    }

    /// Verify a password without creating a session.
    pub async fn verify_password(&self, password: &str) -> bool {
        let Some(hash) = self.read().password_hash.clone() else {
            return false;
        };
        let password = password.to_string();
        tokio::task::spawn_blocking(move || verify_password_hash(&hash, &password))
            .await
            .unwrap_or(false)
    }

    /// Register a verified credential as a passkey.
    pub async fn add_passkey(
        &self,
        credential: RegisteredCredential,
        name: &str,
    ) -> Result<PasskeyView> {
        let name = sanitize_passkey_name(name);
        let _guard = self.write_lock.lock().await;
        let passkey = Passkey {
            id: credential.credential_id.clone(),
            name,
            public_key: URL_SAFE_NO_PAD.encode(&credential.public_key),
            counter: credential.counter,
            created_at: now_secs(),
            last_used_at: None,
        };
        let mut next = self.read().passkeys.clone();
        if let Some(existing) = next.iter_mut().find(|entry| entry.id == passkey.id) {
            // Re-registering the same authenticator replaces it rather than
            // creating a duplicate row.
            *existing = passkey.clone();
        } else {
            next.push(passkey.clone());
        }
        self.persist_passkeys(next).await?;
        Ok(PasskeyView::from(&passkey))
    }

    pub async fn rename_passkey(&self, id: &str, name: &str) -> Result<PasskeyView> {
        let name = sanitize_passkey_name(name);
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().passkeys.clone();
        let Some(entry) = next.iter_mut().find(|entry| entry.id == id) else {
            return Err(Error::NotFound(format!("no passkey with id `{id}`")));
        };
        entry.name = name;
        let view = PasskeyView::from(&*entry);
        self.persist_passkeys(next).await?;
        Ok(view)
    }

    pub async fn remove_passkey(&self, id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().passkeys.clone();
        let before = next.len();
        next.retain(|entry| entry.id != id);
        if next.len() == before {
            return Err(Error::NotFound(format!("no passkey with id `{id}`")));
        }
        self.persist_passkeys(next).await
    }

    async fn persist_passkeys(&self, passkeys: Vec<Passkey>) -> Result<()> {
        let value = serde_json::to_string(&passkeys)
            .map_err(|e| Error::Store(format!("could not encode the passkey list: {e}")))?;
        self.store
            .set_setting(KEY_PASSKEYS, &value)
            .await
            .map_err(|e| Error::Store(format!("could not save the passkey list: {e}")))?;
        self.write().passkeys = passkeys;
        Ok(())
    }

    // ---- challenges -------------------------------------------------------

    /// Mint a challenge and remember it under a one-time id.
    pub fn mint_challenge(&self, kind: ChallengeKind) -> (String, String) {
        let challenge = webauthn::new_challenge();
        if challenge.is_empty() {
            // The OS random source failed; `new_challenge` already logged it.
            // Returning an empty challenge would mint a predictable ceremony.
            return (String::new(), String::new());
        }
        let id = webauthn::new_challenge();
        let mut challenges = self.challenges.write().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        challenges.retain(|_, pending| now.duration_since(pending.created) < CHALLENGE_TTL);
        challenges.insert(
            id.clone(),
            PendingChallenge {
                challenge: challenge.clone(),
                kind,
                created: now,
            },
        );
        (id, challenge)
    }

    /// Take a challenge for one ceremony: it is removed whether or not it is
    /// used, so a captured id cannot be replayed.
    pub fn take_challenge(&self, id: &str) -> Option<PendingChallenge> {
        let mut challenges = self.challenges.write().unwrap_or_else(|e| e.into_inner());
        let pending = challenges.remove(id)?;
        if pending.created.elapsed() >= CHALLENGE_TTL {
            return None;
        }
        Some(pending)
    }

    // ---- passkey ceremonies ----------------------------------------------

    /// Finish a registration ceremony: verify it against the challenge and store
    /// the credential.
    pub async fn register_passkey(
        &self,
        challenge_id: &str,
        name: &str,
        input: &RegistrationInput,
        origins: &[String],
    ) -> std::result::Result<PasskeyView, LoginError> {
        let pending = self
            .take_challenge(challenge_id)
            .ok_or_else(|| LoginError::BadRequest("this registration challenge expired".into()))?;
        if pending.kind != ChallengeKind::Register {
            return Err(LoginError::BadRequest(
                "that challenge was not issued for a registration".into(),
            ));
        }
        let ceremony = Ceremony {
            rp_id: rp_id_for(origins),
            origins: origins.to_vec(),
            challenge: pending.challenge,
        };
        let credential = webauthn::verify_registration(input, &ceremony)
            .map_err(|error| LoginError::Rejected(error.to_string()))?;
        self.add_passkey(credential, name)
            .await
            .map_err(|error| LoginError::BadRequest(error.to_string()))
    }

    /// Finish an assertion ceremony.
    ///
    /// Returns the passkey that was used, so the caller can label the session and
    /// tell the operator *which* key signed in.
    pub fn verify_assertion(
        &self,
        challenge_id: &str,
        input: &AssertionInput,
        origins: &[String],
    ) -> std::result::Result<Passkey, LoginError> {
        let pending = self
            .take_challenge(challenge_id)
            .ok_or_else(|| LoginError::BadRequest("this sign-in challenge expired".into()))?;
        if pending.kind != ChallengeKind::Login {
            return Err(LoginError::BadRequest(
                "that challenge was not issued for a sign-in".into(),
            ));
        }
        let passkey = self
            .passkey(&input.credential_id)
            .ok_or_else(|| LoginError::Rejected("that passkey is not registered".into()))?;
        let public_key = URL_SAFE_NO_PAD
            .decode(&passkey.public_key)
            .map_err(|_| LoginError::Rejected("the stored passkey is unreadable".into()))?;
        let ceremony = Ceremony {
            rp_id: rp_id_for(origins),
            origins: origins.to_vec(),
            challenge: pending.challenge,
        };
        let counter = webauthn::verify_assertion(input, &public_key, passkey.counter, &ceremony)
            .map_err(|error| LoginError::Rejected(error.to_string()))?;
        Ok(Passkey { counter, ..passkey })
    }

    /// Record a successful assertion: the new counter and the last-used time.
    pub async fn record_assertion_use(&self, id: &str, counter: u32) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().passkeys.clone();
        let Some(entry) = next.iter_mut().find(|entry| entry.id == id) else {
            return Ok(());
        };
        entry.counter = counter;
        entry.last_used_at = Some(now_secs());
        self.persist_passkeys(next).await
    }

    // ---- sessions ---------------------------------------------------------

    /// Create a session and return the raw cookie value.
    ///
    /// The raw token is returned once and never stored: only its SHA-256 goes
    /// into the database.
    pub async fn create_session(
        &self,
        method: SessionMethod,
        label: &str,
        lifetime_secs: u64,
    ) -> Result<(String, Session)> {
        let raw = webauthn::new_challenge();
        if raw.is_empty() {
            return Err(Error::Store(
                "the OS random source failed; cannot create a session".into(),
            ));
        }
        let session = Session {
            token_hash: hash_token(&raw),
            created_at: now_secs(),
            expires_at: now_secs() + lifetime_secs as i64,
            method,
            label: label.to_string(),
        };
        let _guard = self.write_lock.lock().await;
        let now = now_secs();
        let mut next: Vec<Session> = self
            .read()
            .sessions
            .iter()
            .filter(|existing| !existing.is_expired(now))
            .cloned()
            .collect();
        next.push(session.clone());
        // Bound the list: a browser that signs in every day for a year must not
        // grow a row without limit. The oldest go first.
        const MAX_SESSIONS: usize = 32;
        if next.len() > MAX_SESSIONS {
            next.sort_by_key(|session| session.created_at);
            let excess = next.len() - MAX_SESSIONS;
            next.drain(0..excess);
        }
        self.persist_sessions(next).await?;
        Ok((raw, session))
    }

    /// Whether a raw cookie value names a live session.
    pub fn session(&self, raw_token: &str) -> Option<Session> {
        if raw_token.is_empty() {
            return None;
        }
        let hash = hash_token(raw_token);
        let now = now_secs();
        self.read()
            .sessions
            .iter()
            .find(|session| session.token_hash == hash && !session.is_expired(now))
            .cloned()
    }

    /// Every live session, newest first, without the hashes.
    pub fn sessions(&self) -> Vec<SessionSummary> {
        let now = now_secs();
        let mut out: Vec<SessionSummary> = self
            .read()
            .sessions
            .iter()
            .filter(|session| !session.is_expired(now))
            .map(|session| SessionSummary {
                // A stable, non-reversible handle: the first 12 characters of the
                // hash identify a row for "sign out this one" without being
                // usable as a cookie.
                id: session.token_hash.chars().take(12).collect(),
                created_at: session.created_at,
                expires_at: session.expires_at,
                method: session.method,
                label: session.label.clone(),
            })
            .collect();
        out.sort_by_key(|session| std::cmp::Reverse(session.created_at));
        out
    }

    /// Sign one session out.
    pub async fn revoke_session(&self, raw_token: &str) -> Result<bool> {
        let hash = hash_token(raw_token);
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().sessions.clone();
        let before = next.len();
        next.retain(|session| session.token_hash != hash);
        if next.len() == before {
            return Ok(false);
        }
        self.persist_sessions(next).await?;
        Ok(true)
    }

    /// Sign one session out by its public id (the Access settings list).
    pub async fn revoke_session_by_id(&self, id: &str) -> Result<bool> {
        if id.is_empty() {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let mut next = self.read().sessions.clone();
        let before = next.len();
        next.retain(|session| !session.token_hash.starts_with(id));
        if next.len() == before {
            return Ok(false);
        }
        self.persist_sessions(next).await?;
        Ok(true)
    }

    /// Sign every browser out.
    pub async fn revoke_all_sessions(&self) -> Result<usize> {
        let _guard = self.write_lock.lock().await;
        let removed = self.read().sessions.len();
        self.persist_sessions(Vec::new()).await?;
        Ok(removed)
    }

    async fn persist_sessions(&self, sessions: Vec<Session>) -> Result<()> {
        let value = serde_json::to_string(&sessions)
            .map_err(|e| Error::Store(format!("could not encode the session list: {e}")))?;
        self.store
            .set_setting(KEY_SESSIONS, &value)
            .await
            .map_err(|e| Error::Store(format!("could not save the session list: {e}")))?;
        self.write().sessions = sessions;
        Ok(())
    }

    fn with_sessions<T>(&self, f: impl FnOnce(&[Session]) -> T) -> T {
        f(&self.read().sessions)
    }

    // ---- lockout ----------------------------------------------------------

    /// Whether `key` is currently locked out, and for how long.
    pub fn locked_out_for(&self, key: &str) -> Option<u64> {
        let failures = self.failures.read().unwrap_or_else(|e| e.into_inner());
        let entry = failures.get(key)?;
        let locked_until = entry.locked_until?;
        let now = Instant::now();
        if locked_until <= now {
            return None;
        }
        Some((locked_until - now).as_secs().max(1))
    }

    /// Record a failed attempt; returns true when this attempt triggered a lockout.
    pub fn record_failure(&self, key: &str) -> bool {
        let mut failures = self.failures.write().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        // Drop stale entries so a long-lived server does not accumulate keys.
        failures.retain(|_, entry| match entry.locked_until {
            Some(until) => until > now,
            None => now.duration_since(entry.first_at).as_secs() < FAILURE_WINDOW_SECS,
        });
        let entry = failures.entry(key.to_string()).or_insert(Failures {
            count: 0,
            first_at: now,
            locked_until: None,
        });
        if now.duration_since(entry.first_at).as_secs() >= FAILURE_WINDOW_SECS {
            entry.count = 0;
            entry.first_at = now;
        }
        entry.count += 1;
        if entry.count >= MAX_FAILURES {
            entry.count = 0;
            entry.first_at = now;
            entry.locked_until = Some(now + Duration::from_secs(LOCKOUT_SECS));
            return true;
        }
        false
    }

    /// Clear the failure counter for one key after a successful sign-in.
    pub fn clear_failures(&self, key: &str) {
        self.failures
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// Forget every failure counter (tests, and "unlock now" from settings).
    pub fn clear_all_failures(&self) {
        self.failures
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    // ---- onboarding -------------------------------------------------------

    /// Whether the first-run wizard has been finished or skipped.
    pub async fn onboarding_completed(&self) -> bool {
        match self.store.all_settings().await {
            Ok(rows) => rows
                .into_iter()
                .any(|(key, value)| key == KEY_ONBOARDING && !value.is_empty()),
            Err(_) => false,
        }
    }

    /// Mark the wizard finished.
    pub async fn complete_onboarding(&self) -> Result<()> {
        self.store
            .set_setting(KEY_ONBOARDING, &now_ms().to_string())
            .await
            .map_err(|e| Error::Store(format!("could not record onboarding: {e}")))
    }
}

/// One live session as the settings screen lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub method: SessionMethod,
    pub label: String,
}

/// The relying party id for a set of acceptable origins: the host of the first.
fn rp_id_for(origins: &[String]) -> String {
    origins
        .iter()
        .find_map(|origin| {
            let rest = origin.split_once("://")?.1;
            let host = rest.split(['/', ':']).next()?;
            (!host.is_empty()).then(|| host.to_ascii_lowercase())
        })
        .unwrap_or_else(|| "localhost".to_string())
}

fn hash_token(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()))
}

/// Reject a password the wizard would also reject.
pub fn validate_password(password: &str) -> Result<()> {
    let trimmed = password.trim();
    if trimmed.is_empty() {
        return Err(Error::Config("the password is empty".into()));
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::Config(format!(
            "the password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(Error::Config(format!(
            "the password must be at most {MAX_PASSWORD_LEN} bytes"
        )));
    }
    Ok(())
}

fn sanitize_passkey_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "Passkey".to_string();
    }
    trimmed.chars().take(64).collect()
}

fn hash_password(password: &str) -> Result<String> {
    // argon2 0.6: password-hash 0.6 dropped `SaltString::generate`; the salt is
    // generated inside `hash_password` (getrandom is on through argon2's
    // default features), and `PasswordHash` now lives in the `phc` module.
    use argon2::password_hash::PasswordHasher;
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| Error::Config(format!("could not hash the password: {e}")))
}

fn verify_password_hash(hash: &str, password: &str) -> bool {
    use argon2::password_hash::{phc::PasswordHash, PasswordVerifier};
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (Arc<kc_store::SqliteStore>, tempfile_dir::Dir) {
        let dir = tempfile_dir::Dir::new();
        let store = Arc::new(
            kc_store::SqliteStore::open(&dir.path().join("karucode.db"))
                .await
                .expect("store"),
        );
        (store, dir)
    }

    /// A tiny self-cleaning temporary directory; `tempfile` is not a dependency.
    mod tempfile_dir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "kc-webauth-{}-{}",
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

    async fn auth() -> (Arc<WebAuth>, tempfile_dir::Dir) {
        let (store, dir) = store().await;
        (WebAuth::load(store).await.expect("web auth"), dir)
    }

    #[tokio::test]
    async fn a_fresh_install_has_no_credential_and_needs_setup() {
        let (auth, _dir) = auth().await;
        assert!(!auth.password_set());
        assert!(!auth.has_credential());
        assert_eq!(auth.passkey_count(), 0);
        assert!(auth.passkeys().is_empty());
        assert!(!auth.verify_password("anything").await);
    }

    #[tokio::test]
    async fn a_password_round_trips_and_only_the_hash_is_stored() {
        let (auth, dir) = auth().await;
        auth.set_password("correct horse battery").await.unwrap();
        assert!(auth.password_set());
        assert!(auth.has_credential());
        assert!(auth.verify_password("correct horse battery").await);
        assert!(!auth.verify_password("correct horse batterY").await);

        let raw = all_database_bytes(dir.path());
        assert!(
            !raw.contains("correct horse battery"),
            "the plaintext password must never be written to disk"
        );
        assert!(raw.contains("$argon2"), "the Argon2 hash should be stored");
    }

    #[tokio::test]
    async fn a_short_or_empty_password_is_refused_with_the_floor_in_the_message() {
        let (auth, _dir) = auth().await;
        for password in ["", "   ", "short"] {
            let error = auth.set_password(password).await.unwrap_err().to_string();
            assert!(
                error.contains(&MIN_PASSWORD_LEN.to_string()) || error.contains("empty"),
                "{error}"
            );
        }
        assert!(!auth.password_set());
    }

    #[tokio::test]
    async fn a_reloaded_server_keeps_the_password() {
        let (store, dir) = store().await;
        let auth = WebAuth::load(store.clone()).await.unwrap();
        auth.set_password("a ten-chars-password").await.unwrap();
        drop(auth);
        let reloaded = WebAuth::load(store).await.unwrap();
        assert!(reloaded.password_set());
        assert!(reloaded.verify_password("a ten-chars-password").await);
        drop(dir);
    }

    #[tokio::test]
    async fn setting_a_password_signs_every_browser_out() {
        let (auth, _dir) = auth().await;
        let (raw, _) = auth
            .create_session(SessionMethod::Password, "password", 3600)
            .await
            .unwrap();
        assert!(auth.session(&raw).is_some());
        auth.set_password("a ten-chars-password").await.unwrap();
        assert!(
            auth.session(&raw).is_none(),
            "a password change must invalidate existing cookies"
        );
    }

    #[tokio::test]
    async fn a_session_token_works_until_it_is_revoked() {
        let (auth, _dir) = auth().await;
        let (raw, session) = auth
            .create_session(SessionMethod::Passkey, "Touch ID", 60)
            .await
            .unwrap();
        assert_eq!(session.method, SessionMethod::Passkey);
        let found = auth.session(&raw).expect("a live session");
        assert_eq!(found.label, "Touch ID");
        assert!(auth.revoke_session(&raw).await.unwrap());
        assert!(auth.session(&raw).is_none());
        assert!(!auth.revoke_session(&raw).await.unwrap());
    }

    #[tokio::test]
    async fn only_the_hash_of_a_session_token_is_stored() {
        let (auth, dir) = auth().await;
        let (raw, _) = auth
            .create_session(SessionMethod::Password, "password", 60)
            .await
            .unwrap();
        assert!(!auth.sessions().is_empty());
        let text = all_database_bytes(dir.path());
        assert!(
            !text.contains(&raw),
            "the raw cookie value must never be written to disk"
        );
        assert!(text.contains(&hash_token(&raw)[..12]));
    }

    /// The database file plus its WAL: a commit lands in the WAL first, and the
    /// main file may not have it yet.
    fn all_database_bytes(dir: &std::path::Path) -> String {
        let mut bytes = Vec::new();
        for name in ["karucode.db", "karucode.db-wal"] {
            if let Ok(mut content) = std::fs::read(dir.join(name)) {
                bytes.append(&mut content);
            }
        }
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn an_expired_session_is_refused_and_pruned_on_load() {
        let (auth, _dir) = auth().await;
        let (raw, mut session) = auth
            .create_session(SessionMethod::Password, "password", 60)
            .await
            .unwrap();
        // Rewrite the row as expired, then reload.
        session.expires_at = now_secs() - 1;
        auth.persist_sessions(vec![session.clone()]).await.unwrap();
        assert!(auth.session(&raw).is_none());
        let reloaded = WebAuth::load(auth.store.clone()).await.unwrap();
        assert!(reloaded.sessions().is_empty());
    }

    #[tokio::test]
    async fn revoking_every_session_empties_the_list() {
        let (auth, _dir) = auth().await;
        let (first, _) = auth
            .create_session(SessionMethod::Password, "a", 60)
            .await
            .unwrap();
        let (second, _) = auth
            .create_session(SessionMethod::Password, "b", 60)
            .await
            .unwrap();
        assert_eq!(auth.sessions().len(), 2);
        assert_eq!(auth.revoke_all_sessions().await.unwrap(), 2);
        assert!(auth.session(&first).is_none());
        assert!(auth.session(&second).is_none());
        assert!(auth.sessions().is_empty());
    }

    #[tokio::test]
    async fn signing_out_one_session_keeps_the_others() {
        let (auth, _dir) = auth().await;
        let (first, _) = auth
            .create_session(SessionMethod::Password, "a", 60)
            .await
            .unwrap();
        let (second, _) = auth
            .create_session(SessionMethod::Password, "b", 60)
            .await
            .unwrap();
        let id = auth.sessions()[0].id.clone();
        assert!(auth.revoke_session_by_id(&id).await.unwrap());
        assert!(auth.session(&first).is_some() ^ auth.session(&second).is_some());
        assert_eq!(auth.sessions().len(), 1);
    }

    #[tokio::test]
    async fn the_session_list_is_bounded() {
        let (auth, _dir) = auth().await;
        for _ in 0..40 {
            auth.create_session(SessionMethod::Password, "x", 3600)
                .await
                .unwrap();
        }
        assert!(auth.sessions().len() <= 32, "{}", auth.sessions().len());
    }

    #[tokio::test]
    async fn a_challenge_is_single_use_and_expires() {
        let (auth, _dir) = auth().await;
        let (id, challenge) = auth.mint_challenge(ChallengeKind::Login);
        assert!(!challenge.is_empty());
        let pending = auth.take_challenge(&id).expect("the challenge");
        assert_eq!(pending.challenge, challenge);
        assert_eq!(pending.kind, ChallengeKind::Login);
        assert!(auth.take_challenge(&id).is_none(), "single use");
        assert!(auth.take_challenge("never-issued").is_none());
    }

    #[tokio::test]
    async fn a_ceremony_for_the_wrong_kind_is_refused() {
        let (auth, _dir) = auth().await;
        let (id, _) = auth.mint_challenge(ChallengeKind::Login);
        let input = RegistrationInput::default();
        let error = auth
            .register_passkey(&id, "key", &input, &["https://localhost".into()])
            .await
            .unwrap_err();
        assert!(matches!(error, LoginError::BadRequest(_)), "{error}");
    }

    #[tokio::test]
    async fn an_assertion_for_an_unknown_passkey_is_rejected() {
        let (auth, _dir) = auth().await;
        let (id, _) = auth.mint_challenge(ChallengeKind::Login);
        let input = AssertionInput {
            credential_id: "nope".into(),
            ..AssertionInput::default()
        };
        let error = auth
            .verify_assertion(&id, &input, &["https://localhost".into()])
            .unwrap_err();
        assert!(matches!(error, LoginError::Rejected(_)), "{error}");
    }

    #[tokio::test]
    async fn passkeys_are_named_renamed_and_removed() {
        let (auth, _dir) = auth().await;
        let view = auth
            .add_passkey(
                RegisteredCredential {
                    credential_id: "abc".into(),
                    public_key: vec![4u8; 65],
                    counter: 0,
                    user_verified: true,
                },
                "  ",
            )
            .await
            .unwrap();
        assert_eq!(view.name, "Passkey", "an empty name gets a default");
        let renamed = auth.rename_passkey("abc", "YubiKey").await.unwrap();
        assert_eq!(renamed.name, "YubiKey");
        assert_eq!(auth.passkeys()[0].name, "YubiKey");
        auth.remove_passkey("abc").await.unwrap();
        assert!(auth.passkeys().is_empty());
        assert!(auth.remove_passkey("abc").await.is_err());
        assert!(auth.rename_passkey("abc", "x").await.is_err());
    }

    #[tokio::test]
    async fn a_passkey_survives_a_reload() {
        let (store, _dir) = store().await;
        let auth = WebAuth::load(store.clone()).await.unwrap();
        auth.add_passkey(
            RegisteredCredential {
                credential_id: "abc".into(),
                public_key: vec![4u8; 65],
                counter: 3,
                user_verified: true,
            },
            "Touch ID",
        )
        .await
        .unwrap();
        let reloaded = WebAuth::load(store).await.unwrap();
        assert_eq!(reloaded.passkey_count(), 1);
        assert_eq!(reloaded.passkeys()[0].name, "Touch ID");
    }

    #[tokio::test]
    async fn failed_attempts_lock_the_key_out_and_then_expire() {
        let (auth, _dir) = auth().await;
        assert!(auth.locked_out_for("203.0.113.9").is_none());
        for _ in 0..MAX_FAILURES - 1 {
            assert!(!auth.record_failure("203.0.113.9"));
            assert!(auth.locked_out_for("203.0.113.9").is_none());
        }
        assert!(auth.record_failure("203.0.113.9"));
        let remaining = auth.locked_out_for("203.0.113.9").expect("locked out");
        assert!(remaining <= LOCKOUT_SECS, "{remaining}");
        // Another key is unaffected.
        assert!(auth.locked_out_for("198.51.100.1").is_none());
        auth.clear_failures("203.0.113.9");
        assert!(auth.locked_out_for("203.0.113.9").is_none());
    }

    #[test]
    fn the_relying_party_id_comes_from_the_origin_host() {
        assert_eq!(
            rp_id_for(&["https://karucode.example.com".into()]),
            "karucode.example.com"
        );
        assert_eq!(rp_id_for(&["http://127.0.0.1:8787".into()]), "127.0.0.1");
        assert_eq!(rp_id_for(&[]), "localhost");
    }

    #[tokio::test]
    async fn onboarding_starts_incomplete_and_stays_complete() {
        let (auth, _dir) = auth().await;
        assert!(!auth.onboarding_completed().await);
        auth.complete_onboarding().await.unwrap();
        assert!(auth.onboarding_completed().await);
        let reloaded = WebAuth::load(auth.store.clone()).await.unwrap();
        assert!(reloaded.onboarding_completed().await);
    }
}
