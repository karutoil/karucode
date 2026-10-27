//! Live settings: config file + environment, then per-install overrides from the
//! SQLite `settings` table.
//!
//! Precedence is defined by `kc_core::config`: (1) rows saved from the UI, (2)
//! environment, (3) config file, (4) defaults. [`Config::load`] already merges
//! (2)-(4); this module layers the persisted rows on top unless
//! `KARUCODE_LOCK_SETTINGS` removed that layer.
//!
//! The runtime reads settings through [`SettingsHandle`] on every step, so a
//! successful [`SettingsHandle::update`] hot-swaps the in-memory [`Config`] after
//! the rows were persisted. The API key is write-only: it is stored in the
//! database, kept in memory, and never returned by any getter used for JSON.

use crate::error::ApiError;
use async_trait::async_trait;
use kc_core::config::{ApprovalMode, ApprovalPolicy, Config, Limits, ModelConfig, SubagentConfig};
use kc_core::error::{Error, Result};
use kc_core::settings::Settings;
use kc_core::store::Store;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tracing::warn;

/// Settings keys persisted in the `settings` table. Kept in one place so the
/// read and write paths cannot drift.
mod keys {
    pub const MODEL_PROVIDER: &str = "model.provider";
    pub const MODEL_BASE_URL: &str = "model.base_url";
    pub const MODEL_NAME: &str = "model.model";
    pub const MODEL_API_KEY: &str = "model.api_key";
    pub const MODEL_TEMPERATURE: &str = "model.temperature";
    pub const MODEL_MAX_TOKENS: &str = "model.max_tokens";
    pub const MODEL_TOP_P: &str = "model.top_p";
    pub const MODEL_REQUEST_TIMEOUT: &str = "model.request_timeout_secs";
    pub const MODEL_PARALLEL_TOOL_CALLS: &str = "model.parallel_tool_calls";

    pub const LIMITS_MAX_ITERATIONS: &str = "limits.max_iterations";
    pub const LIMITS_MAX_RUN_SECS: &str = "limits.max_run_secs";
    pub const LIMITS_MAX_TOOL_OUTPUT_BYTES: &str = "limits.max_tool_output_bytes";
    pub const LIMITS_MAX_COMMAND_OUTPUT_BYTES: &str = "limits.max_command_output_bytes";
    pub const LIMITS_COMMAND_TIMEOUT_SECS: &str = "limits.command_timeout_secs";
    pub const LIMITS_MAX_COMMAND_TIMEOUT_SECS: &str = "limits.max_command_timeout_secs";
    pub const LIMITS_MAX_FILE_READ_BYTES: &str = "limits.max_file_read_bytes";
    pub const LIMITS_SEARCH_MAX_RESULTS: &str = "limits.search_max_results";
    pub const LIMITS_CONTEXT_MAX_CHARS: &str = "limits.context_max_chars";
    pub const LIMITS_MAX_TOOL_CALLS_PER_TURN: &str = "limits.max_tool_calls_per_turn";

    pub const APPROVALS_MODE: &str = "approvals.mode";
    pub const APPROVALS_ALLOW_COMMANDS: &str = "approvals.allow_commands";
    pub const APPROVALS_AUTO_APPROVE_PATHS: &str = "approvals.auto_approve_paths";
    pub const APPROVALS_TIMEOUT_SECS: &str = "approvals.timeout_secs";

    pub const SUBAGENTS_MAX_CONCURRENT: &str = "subagents.max_concurrent";
    pub const SUBAGENTS_MAX_ITERATIONS: &str = "subagents.max_iterations";
    pub const SUBAGENTS_ALLOW_RECURSIVE: &str = "subagents.allow_recursive";
    pub const SUBAGENTS_MAX_DEPTH: &str = "subagents.max_depth";
    pub const SUBAGENTS_TIMEOUT_SECS: &str = "subagents.timeout_secs";
}

// ---------------------------------------------------------------------------
// PATCH payloads (docs/API.md `PATCH /api/config`)
// ---------------------------------------------------------------------------

/// Distinguishes "field absent" (`None`) from "present and null" (`Some(None)`).
fn double_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPatch {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    #[serde(default, deserialize_with = "double_option")]
    pub max_tokens: Option<Option<u32>>,
    #[serde(default, deserialize_with = "double_option")]
    pub top_p: Option<Option<f32>>,
    pub request_timeout_secs: Option<u64>,
    pub parallel_tool_calls: Option<bool>,
    /// Write-only. Empty strings are treated as absent.
    pub api_key: Option<String>,
    /// Erase the stored API key (wins over `api_key`).
    pub clear_api_key: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsPatch {
    pub max_iterations: Option<u32>,
    pub max_run_secs: Option<u64>,
    pub max_tool_output_bytes: Option<usize>,
    pub max_command_output_bytes: Option<usize>,
    pub command_timeout_secs: Option<u64>,
    pub max_command_timeout_secs: Option<u64>,
    pub max_file_read_bytes: Option<usize>,
    pub search_max_results: Option<usize>,
    pub context_max_chars: Option<usize>,
    pub max_tool_calls_per_turn: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalsPatch {
    pub mode: Option<String>,
    pub allow_commands: Option<Vec<String>>,
    pub auto_approve_paths: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentsPatch {
    pub max_concurrent: Option<usize>,
    pub max_iterations: Option<u32>,
    pub allow_recursive: Option<bool>,
    pub max_depth: Option<u8>,
    pub timeout_secs: Option<u64>,
}

/// Body of `PATCH /api/config`. Every section is optional; unknown fields are
/// rejected so a typo in the browser cannot silently do nothing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPatch {
    pub model: Option<ModelPatch>,
    pub limits: Option<LimitsPatch>,
    pub approvals: Option<ApprovalsPatch>,
    pub subagents: Option<SubagentsPatch>,
}

// ---------------------------------------------------------------------------
// SettingsHandle
// ---------------------------------------------------------------------------

/// Live settings source. Cheap to clone via `Arc`; the runtime holds the trait
/// object and always sees the current values.
pub struct SettingsHandle {
    config: RwLock<Config>,
    store: Arc<dyn Store>,
    /// Serialises read-modify-write updates so two concurrent PATCH requests
    /// cannot interleave their persisted rows.
    update_lock: tokio::sync::Mutex<()>,
}

// A manual Debug impl so accidentally logging a handle cannot print the key.
impl std::fmt::Debug for SettingsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = self.read();
        f.debug_struct("SettingsHandle")
            .field("workspace", &cfg.workspace_root)
            .field("model", &cfg.model.model)
            .field("base_url", &cfg.model.base_url)
            .field("api_key_set", &api_key_set(&cfg.model))
            .field("lock_settings", &cfg.lock_settings)
            .finish()
    }
}

impl SettingsHandle {
    /// Create the handle and layer persisted rows over the file/env config.
    ///
    /// Failure policy, by layer:
    ///
    /// * the **file + environment** layer is validated first and a bad value
    ///   aborts startup — that is operational configuration and a torn value is
    ///   worth failing loudly for;
    /// * each **persisted row** is then applied on its own and kept only if the
    ///   resulting config is still valid. A stale or hand-edited row (an old
    ///   `model.base_url`, a limit that is out of range, a value written by a
    ///   previous build) is warned about and ignored, so it can never prevent
    ///   the server from starting: the file/env value for that key stays live;
    /// * an unreadable settings table is a hard error, because silently running
    ///   on stale defaults would be worse.
    pub async fn new(config: Config, store: Arc<dyn Store>) -> Result<Arc<Self>> {
        // Layer 3/2 (file, environment) and the CLI overrides on top of it.
        validate_settings(&config)?;
        let mut config = config;
        if !config.lock_settings {
            let rows = store
                .all_settings()
                .await
                .map_err(|e| Error::Store(format!("could not read persisted settings: {e}")))?;
            apply_rows_resilient(&mut config, &rows);
        }
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            store,
            update_lock: tokio::sync::Mutex::new(()),
        }))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        // A poisoned lock still contains a consistent snapshot; refusing to read
        // settings would take the whole server down.
        self.config.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Copy of the current config. Contains the API key: never serialise this
    /// directly; use `model.public_view()` for the browser.
    pub fn snapshot(&self) -> Config {
        self.read().clone()
    }

    pub fn is_locked(&self) -> bool {
        self.read().lock_settings
    }

    pub fn auth_token(&self) -> Option<String> {
        self.read().auth_token.clone()
    }

    /// Every secret that must not appear in an outgoing error message.
    pub fn secrets(&self) -> Vec<String> {
        let cfg = self.read();
        [cfg.model.api_key.clone(), cfg.auth_token.clone()]
            .into_iter()
            .flatten()
            .filter(|s| s.len() >= 4)
            .collect()
    }

    pub fn redact(&self, text: &str) -> String {
        let secrets = self.secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        crate::error::redact(text, &refs)
    }

    /// Apply a validated patch: persist changed keys first, then swap the
    /// in-memory config. When a write fails the previous values are restored
    /// best-effort and the old config stays live, so a failed PATCH is a no-op.
    pub async fn update(&self, patch: &ConfigPatch) -> Result<Config> {
        if self.is_locked() {
            return Err(Error::PermissionDenied(
                "settings are locked (KARUCODE_LOCK_SETTINGS); change them in the config file or environment"
                    .into(),
            ));
        }

        let _update = self.update_lock.lock().await;
        let old = self.snapshot();
        let mut next = old.clone();
        apply_patch(&mut next, patch)?;
        validate_settings(&next)?;

        let old_rows = rows_for(&old);
        let new_rows = rows_for(&next);
        let mut changed: Vec<(&'static str, String, String)> = Vec::new();
        for (key, value) in &new_rows {
            let previous = old_rows.get(key).cloned().unwrap_or_default();
            if &previous != value {
                changed.push((key, previous, value.clone()));
            }
        }

        let mut written: Vec<(&'static str, String)> = Vec::new();
        for (key, previous, value) in &changed {
            if let Err(e) = self.store.set_setting(key, value).await {
                // Roll back what we already wrote so the DB keeps matching the
                // still-live config.
                for (written_key, written_previous) in written.into_iter().rev() {
                    if let Err(rollback) =
                        self.store.set_setting(written_key, &written_previous).await
                    {
                        warn!(
                            key = written_key,
                            "could not roll back a settings write: {rollback}"
                        );
                    }
                }
                return Err(Error::Store(format!(
                    "could not persist setting `{key}`: {e}"
                )));
            }
            written.push((key, previous.clone()));
        }

        *self.config.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        Ok(next)
    }

    /// Persist and apply an empty API key (the `clear_api_key` flow).
    pub async fn clear_api_key(&self) -> Result<Config> {
        self.update(&ConfigPatch {
            model: Some(ModelPatch {
                clear_api_key: Some(true),
                ..ModelPatch::default()
            }),
            ..ConfigPatch::default()
        })
        .await
    }

    /// Browser-safe config document (`GET/PATCH /api/config`).
    pub async fn public_config(&self) -> serde_json::Value {
        let cfg = self.snapshot();
        serde_json::json!({
            "workspace": cfg.workspace_root.to_string_lossy(),
            "model": public_model(&cfg.model),
            "limits": cfg.limits,
            "approvals": cfg.approvals,
            "subagents": cfg.subagents,
            "settings_locked": cfg.lock_settings,
            "version": crate::VERSION,
        })
    }
}

#[async_trait]
impl Settings for SettingsHandle {
    async fn model(&self) -> ModelConfig {
        self.read().model.clone()
    }

    async fn limits(&self) -> Limits {
        self.read().limits.clone()
    }

    async fn approval_policy(&self) -> ApprovalPolicy {
        self.read().approvals.clone()
    }

    async fn subagent_config(&self) -> SubagentConfig {
        self.read().subagents.clone()
    }
}

/// Browser-safe model view. Rounds the f32 tunables so `0.2f32` does not appear
/// as `0.20000000298023224` in JSON (the documented shape shows `0.2`).
pub fn public_model(model: &ModelConfig) -> serde_json::Value {
    let round = |value: f32| ((value as f64) * 10_000.0).round() / 10_000.0;
    serde_json::json!({
        "provider": model.provider,
        "base_url": model.base_url,
        "model": model.model,
        "temperature": round(model.temperature),
        "max_tokens": model.max_tokens,
        "top_p": model.top_p.map(round),
        "request_timeout_secs": model.request_timeout_secs,
        "parallel_tool_calls": model.parallel_tool_calls,
        "api_key_set": api_key_set(model),
    })
}

pub fn api_key_set(model: &ModelConfig) -> bool {
    model
        .api_key
        .as_deref()
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Patch application + validation
// ---------------------------------------------------------------------------

fn apply_patch(config: &mut Config, patch: &ConfigPatch) -> Result<()> {
    if let Some(model) = &patch.model {
        // `clear_api_key` wins over a value in the same request so the browser
        // can always express "forget it".
        if model.clear_api_key == Some(true) {
            config.model.api_key = Some(String::new());
        }
        if let Some(provider) = &model.provider {
            // Store the canonical (trimmed) id: it was checked against the
            // factory's `supported()` list at the HTTP boundary.
            let trimmed = provider.trim();
            if !trimmed.is_empty() {
                config.model.provider = trimmed.to_string();
            }
        }
        if let Some(base_url) = &model.base_url {
            config.model.base_url = base_url.clone();
        }
        if let Some(name) = &model.model {
            config.model.model = name.clone();
        }
        if let Some(temperature) = model.temperature {
            config.model.temperature = temperature;
        }
        if let Some(max_tokens) = model.max_tokens {
            config.model.max_tokens = max_tokens;
        }
        if let Some(top_p) = model.top_p {
            config.model.top_p = top_p;
        }
        if let Some(timeout) = model.request_timeout_secs {
            config.model.request_timeout_secs = timeout;
        }
        if let Some(parallel) = model.parallel_tool_calls {
            config.model.parallel_tool_calls = parallel;
        }
        if model.clear_api_key != Some(true) {
            if let Some(key) = &model.api_key {
                let key = key.trim();
                if !key.is_empty() {
                    config.model.api_key = Some(key.to_string());
                }
            }
        }
    }

    if let Some(limits) = &patch.limits {
        if let Some(v) = limits.max_iterations {
            config.limits.max_iterations = v;
        }
        if let Some(v) = limits.max_run_secs {
            config.limits.max_run_secs = v;
        }
        if let Some(v) = limits.max_tool_output_bytes {
            config.limits.max_tool_output_bytes = v;
        }
        if let Some(v) = limits.max_command_output_bytes {
            config.limits.max_command_output_bytes = v;
        }
        if let Some(v) = limits.command_timeout_secs {
            config.limits.command_timeout_secs = v;
        }
        if let Some(v) = limits.max_command_timeout_secs {
            config.limits.max_command_timeout_secs = v;
        }
        if let Some(v) = limits.max_file_read_bytes {
            config.limits.max_file_read_bytes = v;
        }
        if let Some(v) = limits.search_max_results {
            config.limits.search_max_results = v;
        }
        if let Some(v) = limits.context_max_chars {
            config.limits.context_max_chars = v;
        }
        if let Some(v) = limits.max_tool_calls_per_turn {
            config.limits.max_tool_calls_per_turn = v;
        }
    }

    if let Some(approvals) = &patch.approvals {
        if let Some(mode) = &approvals.mode {
            let parsed = ApprovalMode::parse(mode).ok_or_else(|| {
                Error::BadRequest(format!(
                    "approvals.mode must be one of ask, ask_commands, auto, read_only (got `{mode}`)"
                ))
            })?;
            config.approvals.mode = parsed;
        }
        if let Some(list) = &approvals.allow_commands {
            config.approvals.allow_commands = clean_list("approvals.allow_commands", list)?;
        }
        if let Some(list) = &approvals.auto_approve_paths {
            config.approvals.auto_approve_paths = clean_list("approvals.auto_approve_paths", list)?;
        }
        if let Some(timeout) = approvals.timeout_secs {
            config.approvals.timeout_secs = timeout;
        }
    }

    if let Some(subagents) = &patch.subagents {
        if let Some(v) = subagents.max_concurrent {
            config.subagents.max_concurrent = v;
        }
        if let Some(v) = subagents.max_iterations {
            config.subagents.max_iterations = v;
        }
        if let Some(v) = subagents.allow_recursive {
            config.subagents.allow_recursive = v;
        }
        if let Some(v) = subagents.max_depth {
            config.subagents.max_depth = v;
        }
        if let Some(v) = subagents.timeout_secs {
            config.subagents.timeout_secs = v;
        }
    }

    Ok(())
}

fn clean_list(field: &str, list: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Err(Error::BadRequest(format!(
                "{field} may not contain empty entries"
            )));
        }
        if trimmed.contains('\0') {
            return Err(Error::BadRequest(format!(
                "{field} may not contain NUL bytes"
            )));
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// Validate every tunable that the UI can change. Called for the merged config,
/// so a patch can never produce a config that startup validation would reject.
pub fn validate_settings(config: &Config) -> Result<()> {
    validate_model(&config.model)?;
    // An unknown provider in the file or environment would otherwise start a
    // server that silently speaks a different wire format than configured.
    check_provider_id(
        &config.model.provider,
        &kc_core::provider::ProviderFactory::supported(&kc_provider::ProviderFactoryImpl),
    )?;
    validate_limits(&config.limits)?;
    validate_approvals(&config.approvals)?;
    validate_subagents(&config.subagents)
}

pub fn validate_model(model: &ModelConfig) -> Result<()> {
    if model.provider.trim().is_empty() {
        return Err(Error::BadRequest("model.provider may not be empty".into()));
    }
    validate_base_url(&model.base_url)?;
    if model.model.trim().is_empty() {
        return Err(Error::BadRequest("model.model may not be empty".into()));
    }
    if !model.temperature.is_finite() || !(0.0..=2.0).contains(&model.temperature) {
        return Err(Error::BadRequest(format!(
            "model.temperature must be between 0 and 2 (got {})",
            model.temperature
        )));
    }
    if let Some(max_tokens) = model.max_tokens {
        if max_tokens == 0 {
            return Err(Error::BadRequest(
                "model.max_tokens must be greater than 0 (or null for the provider default)".into(),
            ));
        }
        if max_tokens > 2_000_000 {
            return Err(Error::BadRequest(format!(
                "model.max_tokens must be at most 2000000 (got {max_tokens})"
            )));
        }
    }
    if let Some(top_p) = model.top_p {
        if !top_p.is_finite() || !(0.0..=1.0).contains(&top_p) {
            return Err(Error::BadRequest(format!(
                "model.top_p must be between 0 and 1 (got {top_p})"
            )));
        }
    }
    if model.request_timeout_secs == 0 {
        return Err(Error::BadRequest(
            "model.request_timeout_secs must be at least 1".into(),
        ));
    }
    Ok(())
}

/// Parse-level check for the endpoint. Scheme + non-empty authority, no control
/// characters. We deliberately do not resolve DNS here.
pub fn validate_base_url(raw: &str) -> Result<()> {
    let url = raw.trim();
    if url.is_empty() {
        return Err(Error::BadRequest(
            "model.base_url may not be empty; use an OpenAI-compatible API root such as http://127.0.0.1:4000/v1"
                .into(),
        ));
    }
    if url.contains('\n') || url.contains('\r') || url.contains('\0') || url.contains(' ') {
        return Err(Error::BadRequest(
            "model.base_url may not contain whitespace or control characters".into(),
        ));
    }
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| {
            Error::BadRequest(format!(
                "model.base_url must start with http:// or https:// (got `{url}`)"
            ))
        })?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(Error::BadRequest(format!(
            "model.base_url is missing a host (got `{url}`)"
        )));
    }
    validate_authority(authority, url)
}

/// Host/port check without DNS. Accepts IPv6 literals in brackets, IDN hosts and
/// an optional numeric port; rejects empty hosts and bogus ports.
fn validate_authority(authority: &str, url: &str) -> Result<()> {
    // Credentials in a base URL would be echoed by `GET /api/config`; the API
    // key belongs in `model.api_key`.
    if authority.contains('@') {
        return Err(Error::BadRequest(
            "model.base_url may not embed credentials; set model.api_key instead".into(),
        ));
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (inside, after) = rest.split_once(']').ok_or_else(|| {
            Error::BadRequest(format!(
                "model.base_url has an unterminated IPv6 host (got `{url}`)"
            ))
        })?;
        if inside.is_empty() {
            return Err(Error::BadRequest(format!(
                "model.base_url is missing a host (got `{url}`)"
            )));
        }
        match after {
            "" => (inside, None),
            suffix => match suffix.strip_prefix(':') {
                Some(port) => (inside, Some(port)),
                None => {
                    return Err(Error::BadRequest(format!(
                        "model.base_url has an invalid IPv6 host or port (got `{url}`)"
                    )))
                }
            },
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.trim().is_empty() {
        return Err(Error::BadRequest(format!(
            "model.base_url is missing a host (got `{url}`)"
        )));
    }
    if !authority.starts_with('[') && host.contains(':') {
        return Err(Error::BadRequest(format!(
            "model.base_url has an unbracketed IPv6 host; write it as http://[::1]:port/v1 (got `{url}`)"
        )));
    }
    if let Some(port) = port {
        if port.is_empty() || port.parse::<u16>().is_err() {
            return Err(Error::BadRequest(format!(
                "model.base_url has an invalid port `{port}` (expected 1-65535)"
            )));
        }
    }
    Ok(())
}

/// Inclusive numeric bounds for every tunable the UI may change. A PATCH that
/// leaves a value outside its range is rejected with `400 bad_request` naming
/// the field and the bounds — never silently clamped, because a silent clamp
/// hides the mistake from the operator. The upper bounds are the resource
/// limits themselves: `limits.max_file_read_bytes = 4_000_000_000` would make a
/// single read allocate gigabytes and OOM the process.
const LIMITS_BOUNDS: &[(&str, u64, u64)] = &[
    ("limits.max_file_read_bytes", 1, 5_000_000),
    ("limits.max_tool_output_bytes", 256, 1_000_000),
    ("limits.max_command_output_bytes", 256, 1_000_000),
    ("limits.context_max_chars", 10_000, 4_000_000),
    ("limits.max_iterations", 1, 500),
    ("limits.max_run_secs", 10, 86_400),
    ("limits.command_timeout_secs", 1, 3_600),
    ("limits.max_command_timeout_secs", 1, 3_600),
    ("limits.search_max_results", 1, 2_000),
    ("limits.max_tool_calls_per_turn", 1, 32),
];

const SUBAGENT_BOUNDS: &[(&str, u64, u64)] = &[
    ("subagents.max_concurrent", 1, 16),
    ("subagents.max_iterations", 1, 100),
    ("subagents.max_depth", 0, 4),
];

fn check_bounds(name: &str, value: u64, min: u64, max: u64) -> Result<()> {
    if value < min || value > max {
        return Err(Error::BadRequest(format!(
            "{name} must be between {min} and {max} (got {value})"
        )));
    }
    Ok(())
}

fn validate_limits(limits: &Limits) -> Result<()> {
    let values: [(&str, u64); 10] = [
        (
            "limits.max_file_read_bytes",
            limits.max_file_read_bytes as u64,
        ),
        (
            "limits.max_tool_output_bytes",
            limits.max_tool_output_bytes as u64,
        ),
        (
            "limits.max_command_output_bytes",
            limits.max_command_output_bytes as u64,
        ),
        ("limits.context_max_chars", limits.context_max_chars as u64),
        ("limits.max_iterations", u64::from(limits.max_iterations)),
        ("limits.max_run_secs", limits.max_run_secs),
        ("limits.command_timeout_secs", limits.command_timeout_secs),
        (
            "limits.max_command_timeout_secs",
            limits.max_command_timeout_secs,
        ),
        (
            "limits.search_max_results",
            limits.search_max_results as u64,
        ),
        (
            "limits.max_tool_calls_per_turn",
            limits.max_tool_calls_per_turn as u64,
        ),
    ];
    for ((name, value), (expected, min, max)) in values.iter().zip(LIMITS_BOUNDS) {
        debug_assert_eq!(*name, *expected, "bounds table and values disagree");
        check_bounds(name, *value, *min, *max)?;
    }
    Ok(())
}

fn validate_approvals(policy: &ApprovalPolicy) -> Result<()> {
    if policy.timeout_secs == 0 {
        return Err(Error::BadRequest(
            "approvals.timeout_secs must be at least 1".into(),
        ));
    }
    clean_list("approvals.allow_commands", &policy.allow_commands)?;
    clean_list("approvals.auto_approve_paths", &policy.auto_approve_paths)?;
    Ok(())
}

fn validate_subagents(subagents: &SubagentConfig) -> Result<()> {
    // `max_depth = 0` is meaningful: it forbids delegation entirely.
    let values: [(&str, u64); 3] = [
        ("subagents.max_concurrent", subagents.max_concurrent as u64),
        (
            "subagents.max_iterations",
            u64::from(subagents.max_iterations),
        ),
        ("subagents.max_depth", u64::from(subagents.max_depth)),
    ];
    for ((name, value), (expected, min, max)) in values.iter().zip(SUBAGENT_BOUNDS) {
        debug_assert_eq!(*name, *expected, "bounds table and values disagree");
        check_bounds(name, *value, *min, *max)?;
    }
    if subagents.timeout_secs == 0 {
        return Err(Error::BadRequest(
            "subagents.timeout_secs must be at least 1".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Persistence rows
// ---------------------------------------------------------------------------

/// Apply persisted rows one at a time, keeping each row only when the whole
/// config stays valid. A row that fails validation is warned about (naming the
/// key) and skipped, so its previous — file/env — value survives.
///
/// Validation runs per row, not once at the end, precisely so that one bad row
/// cannot take the process down; the aggregate cannot be invalid either, because
/// the starting config is validated and every accepted row keeps it valid.
fn apply_rows_resilient(config: &mut Config, rows: &[(String, String)]) {
    for row in rows {
        let previous = config.clone();
        apply_rows(config, std::slice::from_ref(row));
        if let Err(error) = validate_settings(config) {
            warn!(
                key = row.0.as_str(),
                %error,
                "ignoring a persisted settings row that is no longer valid; keeping the configured value"
            );
            *config = previous;
        }
    }
}

fn rows_for(config: &Config) -> BTreeMap<&'static str, String> {
    let mut rows = BTreeMap::new();
    let model = &config.model;
    rows.insert(keys::MODEL_PROVIDER, model.provider.clone());
    rows.insert(keys::MODEL_BASE_URL, model.base_url.clone());
    rows.insert(keys::MODEL_NAME, model.model.clone());
    rows.insert(
        keys::MODEL_API_KEY,
        model.api_key.clone().unwrap_or_default(),
    );
    rows.insert(keys::MODEL_TEMPERATURE, model.temperature.to_string());
    rows.insert(
        keys::MODEL_MAX_TOKENS,
        model.max_tokens.map(|v| v.to_string()).unwrap_or_default(),
    );
    rows.insert(
        keys::MODEL_TOP_P,
        model.top_p.map(|v| v.to_string()).unwrap_or_default(),
    );
    rows.insert(
        keys::MODEL_REQUEST_TIMEOUT,
        model.request_timeout_secs.to_string(),
    );
    rows.insert(
        keys::MODEL_PARALLEL_TOOL_CALLS,
        model.parallel_tool_calls.to_string(),
    );

    let limits = &config.limits;
    rows.insert(
        keys::LIMITS_MAX_ITERATIONS,
        limits.max_iterations.to_string(),
    );
    rows.insert(keys::LIMITS_MAX_RUN_SECS, limits.max_run_secs.to_string());
    rows.insert(
        keys::LIMITS_MAX_TOOL_OUTPUT_BYTES,
        limits.max_tool_output_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_COMMAND_OUTPUT_BYTES,
        limits.max_command_output_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_COMMAND_TIMEOUT_SECS,
        limits.command_timeout_secs.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_COMMAND_TIMEOUT_SECS,
        limits.max_command_timeout_secs.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_FILE_READ_BYTES,
        limits.max_file_read_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_SEARCH_MAX_RESULTS,
        limits.search_max_results.to_string(),
    );
    rows.insert(
        keys::LIMITS_CONTEXT_MAX_CHARS,
        limits.context_max_chars.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_TOOL_CALLS_PER_TURN,
        limits.max_tool_calls_per_turn.to_string(),
    );

    let approvals = &config.approvals;
    rows.insert(keys::APPROVALS_MODE, approvals.mode.as_str().to_string());
    rows.insert(
        keys::APPROVALS_ALLOW_COMMANDS,
        serde_json::to_string(&approvals.allow_commands).unwrap_or_else(|_| "[]".into()),
    );
    rows.insert(
        keys::APPROVALS_AUTO_APPROVE_PATHS,
        serde_json::to_string(&approvals.auto_approve_paths).unwrap_or_else(|_| "[]".into()),
    );
    rows.insert(
        keys::APPROVALS_TIMEOUT_SECS,
        approvals.timeout_secs.to_string(),
    );

    let subagents = &config.subagents;
    rows.insert(
        keys::SUBAGENTS_MAX_CONCURRENT,
        subagents.max_concurrent.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_MAX_ITERATIONS,
        subagents.max_iterations.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_ALLOW_RECURSIVE,
        subagents.allow_recursive.to_string(),
    );
    rows.insert(keys::SUBAGENTS_MAX_DEPTH, subagents.max_depth.to_string());
    rows.insert(
        keys::SUBAGENTS_TIMEOUT_SECS,
        subagents.timeout_secs.to_string(),
    );

    rows
}

fn parse_value<T: std::str::FromStr>(key: &str, value: &str) -> Option<T> {
    match value.trim().parse::<T>() {
        Ok(v) => Some(v),
        Err(_) => {
            // Never log the value: it may be a secret on a mistyped row.
            warn!(key, "ignoring a settings row that is not a valid number");
            None
        }
    }
}

fn parse_bool(key: &str, value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            warn!(key, "ignoring a settings row that is not a valid boolean");
            None
        }
    }
}

fn parse_list(key: &str, value: &str) -> Option<Vec<String>> {
    if value.trim().is_empty() {
        return Some(Vec::new());
    }
    if let Ok(list) = serde_json::from_str::<Vec<String>>(value) {
        return Some(list);
    }
    // Tolerate a hand-edited comma separated list.
    let list: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        warn!(
            key,
            "ignoring a settings row that is not a valid string list"
        );
        None
    } else {
        Some(list)
    }
}

fn apply_rows(config: &mut Config, rows: &[(String, String)]) {
    for (key, value) in rows {
        match key.as_str() {
            keys::MODEL_PROVIDER => config.model.provider = value.clone(),
            keys::MODEL_BASE_URL => config.model.base_url = value.clone(),
            keys::MODEL_NAME => config.model.model = value.clone(),
            keys::MODEL_API_KEY => config.model.api_key = Some(value.clone()),
            keys::MODEL_TEMPERATURE => {
                if let Some(v) = parse_value::<f32>(key, value) {
                    config.model.temperature = v;
                }
            }
            keys::MODEL_MAX_TOKENS => {
                if value.trim().is_empty() {
                    config.model.max_tokens = None;
                } else if let Some(v) = parse_value::<u32>(key, value) {
                    config.model.max_tokens = Some(v);
                }
            }
            keys::MODEL_TOP_P => {
                if value.trim().is_empty() {
                    config.model.top_p = None;
                } else if let Some(v) = parse_value::<f32>(key, value) {
                    config.model.top_p = Some(v);
                }
            }
            keys::MODEL_REQUEST_TIMEOUT => {
                if let Some(v) = parse_value::<u64>(key, value) {
                    config.model.request_timeout_secs = v;
                }
            }
            keys::MODEL_PARALLEL_TOOL_CALLS => {
                if let Some(v) = parse_bool(key, value) {
                    config.model.parallel_tool_calls = v;
                }
            }

            keys::LIMITS_MAX_ITERATIONS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_iterations = v;
                }
            }
            keys::LIMITS_MAX_RUN_SECS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_run_secs = v;
                }
            }
            keys::LIMITS_MAX_TOOL_OUTPUT_BYTES => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_tool_output_bytes = v;
                }
            }
            keys::LIMITS_MAX_COMMAND_OUTPUT_BYTES => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_command_output_bytes = v;
                }
            }
            keys::LIMITS_COMMAND_TIMEOUT_SECS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.command_timeout_secs = v;
                }
            }
            keys::LIMITS_MAX_COMMAND_TIMEOUT_SECS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_command_timeout_secs = v;
                }
            }
            keys::LIMITS_MAX_FILE_READ_BYTES => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_file_read_bytes = v;
                }
            }
            keys::LIMITS_SEARCH_MAX_RESULTS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.search_max_results = v;
                }
            }
            keys::LIMITS_CONTEXT_MAX_CHARS => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.context_max_chars = v;
                }
            }
            keys::LIMITS_MAX_TOOL_CALLS_PER_TURN => {
                if let Some(v) = parse_value(key, value) {
                    config.limits.max_tool_calls_per_turn = v;
                }
            }

            keys::APPROVALS_MODE => match ApprovalMode::parse(value) {
                Some(mode) => config.approvals.mode = mode,
                None => warn!(
                    key,
                    "ignoring an unknown approval mode in the settings table"
                ),
            },
            keys::APPROVALS_ALLOW_COMMANDS => {
                if let Some(list) = parse_list(key, value) {
                    config.approvals.allow_commands = list;
                }
            }
            keys::APPROVALS_AUTO_APPROVE_PATHS => {
                if let Some(list) = parse_list(key, value) {
                    config.approvals.auto_approve_paths = list;
                }
            }
            keys::APPROVALS_TIMEOUT_SECS => {
                if let Some(v) = parse_value(key, value) {
                    config.approvals.timeout_secs = v;
                }
            }

            keys::SUBAGENTS_MAX_CONCURRENT => {
                if let Some(v) = parse_value(key, value) {
                    config.subagents.max_concurrent = v;
                }
            }
            keys::SUBAGENTS_MAX_ITERATIONS => {
                if let Some(v) = parse_value(key, value) {
                    config.subagents.max_iterations = v;
                }
            }
            keys::SUBAGENTS_ALLOW_RECURSIVE => {
                if let Some(v) = parse_bool(key, value) {
                    config.subagents.allow_recursive = v;
                }
            }
            keys::SUBAGENTS_MAX_DEPTH => {
                if let Some(v) = parse_value(key, value) {
                    config.subagents.max_depth = v;
                }
            }
            keys::SUBAGENTS_TIMEOUT_SECS => {
                if let Some(v) = parse_value(key, value) {
                    config.subagents.timeout_secs = v;
                }
            }

            // Unknown keys belong to a newer build; ignore instead of failing.
            _ => {}
        }
    }
}

/// Convert an update error into the HTTP shape (403 for locked settings).
pub fn update_error(error: Error) -> ApiError {
    ApiError::from(error)
}

/// Reject a provider id that the running factory cannot build.
///
/// The set of ids belongs to the factory ([`kc_core::provider::ProviderFactory::supported`]),
/// not to this module, so the check is exposed for the HTTP boundary: without it
/// `model.provider` would accept anything and the UI selector could claim a
/// backend that does not exist. The id is compared trimmed, so a copy-pasted
/// value with surrounding spaces is accepted (it is stored trimmed by the
/// caller) but never a different spelling.
pub fn check_provider_id(provider: &str, supported: &[String]) -> Result<()> {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return Err(Error::BadRequest("model.provider may not be empty".into()));
    }
    if supported.iter().any(|id| id == trimmed) {
        return Ok(());
    }
    Err(Error::BadRequest(format!(
        "model.provider must be one of {} (got `{trimmed}`)",
        supported.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::config::Config;

    fn test_config() -> Config {
        Config {
            lock_settings: true,
            ..Config::default()
        }
    }

    #[test]
    fn every_limit_is_bounded_and_names_the_field() {
        // (mutation, expected field name in the error)
        let cases: Vec<(&str, u64, u64)> = vec![
            ("limits.max_file_read_bytes", 0, 5_000_001),
            ("limits.max_tool_output_bytes", 255, 1_000_001),
            ("limits.max_command_output_bytes", 255, 1_000_001),
            ("limits.context_max_chars", 9_999, 4_000_001),
            ("limits.max_iterations", 0, 501),
            ("limits.max_run_secs", 9, 86_401),
            ("limits.command_timeout_secs", 0, 3_601),
            ("limits.max_command_timeout_secs", 0, 3_601),
            ("limits.search_max_results", 0, 2_001),
            ("limits.max_tool_calls_per_turn", 0, 33),
        ];
        for (field, low, high) in cases {
            for value in [low, high] {
                let mut cfg = Config::default();
                set_limit(&mut cfg, field, value);
                let error = validate_settings(&cfg).unwrap_err().to_string();
                assert!(
                    error.contains(field),
                    "{field}={value} should be rejected naming the field, got: {error}"
                );
                assert!(
                    error.contains("must be between"),
                    "{field}={value} should describe the bounds, got: {error}"
                );
            }
        }
    }

    #[test]
    fn subagent_bounds_are_enforced_and_depth_zero_is_allowed() {
        let cases: Vec<(&str, u64)> = vec![
            ("subagents.max_concurrent", 0),
            ("subagents.max_concurrent", 17),
            ("subagents.max_iterations", 0),
            ("subagents.max_iterations", 101),
            ("subagents.max_depth", 5),
        ];
        for (field, value) in cases {
            let mut cfg = Config::default();
            match field {
                "subagents.max_concurrent" => cfg.subagents.max_concurrent = value as usize,
                "subagents.max_iterations" => cfg.subagents.max_iterations = value as u32,
                _ => cfg.subagents.max_depth = value as u8,
            }
            let error = validate_settings(&cfg).unwrap_err().to_string();
            assert!(error.contains(field), "{field}={value}: {error}");
        }

        let mut cfg = Config::default();
        cfg.subagents.max_depth = 0;
        assert!(validate_settings(&cfg).is_ok(), "max_depth = 0 is valid");
    }

    #[test]
    fn default_settings_satisfy_the_bounds() {
        assert!(validate_settings(&Config::default()).is_ok());
    }

    fn set_limit(cfg: &mut Config, field: &str, value: u64) {
        match field {
            "limits.max_file_read_bytes" => cfg.limits.max_file_read_bytes = value as usize,
            "limits.max_tool_output_bytes" => cfg.limits.max_tool_output_bytes = value as usize,
            "limits.max_command_output_bytes" => {
                cfg.limits.max_command_output_bytes = value as usize
            }
            "limits.context_max_chars" => cfg.limits.context_max_chars = value as usize,
            "limits.max_iterations" => cfg.limits.max_iterations = value as u32,
            "limits.max_run_secs" => cfg.limits.max_run_secs = value,
            "limits.command_timeout_secs" => cfg.limits.command_timeout_secs = value,
            "limits.max_command_timeout_secs" => cfg.limits.max_command_timeout_secs = value,
            "limits.search_max_results" => cfg.limits.search_max_results = value as usize,
            "limits.max_tool_calls_per_turn" => cfg.limits.max_tool_calls_per_turn = value as usize,
            other => panic!("unknown limit {other}"),
        }
    }

    #[test]
    fn only_supported_provider_ids_are_accepted() {
        let supported = vec!["openai_compatible".to_string()];
        assert!(check_provider_id("openai_compatible", &supported).is_ok());
        assert!(check_provider_id("  openai_compatible  ", &supported).is_ok());

        let err = check_provider_id("anthropic", &supported)
            .unwrap_err()
            .to_string();
        assert!(err.contains("model.provider"), "{err}");
        assert!(err.contains("openai_compatible"), "{err}");

        assert!(check_provider_id("", &supported).is_err());
        assert!(check_provider_id("OpenAI_Compatible", &supported).is_err());
        // A factory that can build more than one provider lists them all.
        let more = vec!["openai_compatible".to_string(), "echo".to_string()];
        assert!(check_provider_id("echo", &more).is_ok());
        let err = check_provider_id("nope", &more).unwrap_err().to_string();
        assert!(err.contains("openai_compatible, echo"), "{err}");
    }

    #[test]
    fn invalid_persisted_rows_are_skipped_individually() {
        let mut cfg = Config::default();
        let base_url = cfg.model.base_url.clone();
        apply_rows_resilient(
            &mut cfg,
            &[
                // Out-of-range limit and a bogus URL: both must be ignored ...
                ("limits.max_iterations".into(), "99999".into()),
                ("model.base_url".into(), "ftp://nope".into()),
                // ... while a valid row still applies.
                ("model.temperature".into(), "0.5".into()),
                ("limits.search_max_results".into(), "7".into()),
            ],
        );
        assert_eq!(cfg.limits.max_iterations, 32, "invalid row ignored");
        assert_eq!(cfg.model.base_url, base_url, "invalid row ignored");
        assert!((cfg.model.temperature - 0.5).abs() < f32::EPSILON);
        assert_eq!(cfg.limits.search_max_results, 7);
        // The result is a config `SettingsHandle::update` would also accept.
        assert!(validate_settings(&cfg).is_ok());
    }

    #[test]
    fn unparsable_persisted_rows_are_ignored() {
        let mut cfg = Config::default();
        apply_rows_resilient(
            &mut cfg,
            &[
                ("model.temperature".into(), "hot".into()),
                ("approvals.mode".into(), "whatever".into()),
                ("some.future.key".into(), "x".into()),
            ],
        );
        assert_eq!(cfg.model.temperature, 0.2);
        assert_eq!(cfg.approvals.mode, ApprovalMode::Ask);
    }

    #[test]
    fn patch_validates_temperature_range() {
        let mut cfg = test_config();
        let patch = ConfigPatch {
            model: Some(ModelPatch {
                temperature: Some(2.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        let err = validate_settings(&cfg).unwrap_err();
        assert!(err.to_string().contains("temperature"));
    }

    #[test]
    fn patch_rejects_zero_max_tokens() {
        let mut cfg = test_config();
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    max_tokens: Some(Some(0)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(validate_settings(&cfg).is_err());
    }

    #[test]
    fn patch_clears_max_tokens_with_null() {
        let mut cfg = test_config();
        cfg.model.max_tokens = Some(100);
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    max_tokens: Some(None),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.model.max_tokens, None);
    }

    #[test]
    fn base_url_validation_checks_scheme_host_and_port() {
        assert!(validate_base_url("http://127.0.0.1:4000/v1").is_ok());
        assert!(validate_base_url("https://gateway.example.com/v1").is_ok());
        assert!(validate_base_url("http://[::1]:4000/v1").is_ok());
        assert!(validate_base_url("https://例え.jp/v1").is_ok());
        assert!(validate_base_url("http://host:notaport/v1").is_err());
        assert!(validate_base_url("http://:4000/v1").is_err());
        assert!(validate_base_url("http://[::1/v1").is_err());
        assert!(validate_base_url("http://[::1]x/v1").is_err());
        assert!(validate_base_url("http://host:70000/v1").is_err());
        assert!(validate_base_url("http:///v1").is_err());
        assert!(validate_base_url("host:4000").is_err());
        assert!(validate_base_url("http://a b/v1").is_err());
        assert!(validate_base_url("http://user:pass@host/v1").is_err());
        assert!(validate_base_url("http://::1:4000/v1").is_err());
    }

    #[test]
    fn patch_rejects_bad_base_url_and_mode() {
        let mut cfg = test_config();
        let err = apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    base_url: Some("ftp://example.com".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .and_then(|_| validate_settings(&cfg))
        .unwrap_err();
        assert!(err.to_string().contains("http://"));

        let err = apply_patch(
            &mut cfg,
            &ConfigPatch {
                approvals: Some(ApprovalsPatch {
                    mode: Some("sometimes".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("approvals.mode"));
    }

    #[test]
    fn patch_rejects_zero_concurrency() {
        let mut cfg = test_config();
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                subagents: Some(SubagentsPatch {
                    max_concurrent: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(validate_settings(&cfg).is_err());
    }

    #[test]
    fn api_key_is_cleared_and_never_in_public_config() {
        let mut cfg = test_config();
        cfg.model.api_key = Some("sk-secret".into());
        let patch = ConfigPatch {
            model: Some(ModelPatch {
                clear_api_key: Some(true),
                api_key: Some("sk-other".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        assert_eq!(cfg.model.api_key.as_deref(), Some(""));
        assert!(!api_key_set(&cfg.model));
    }

    #[test]
    fn rows_roundtrip_every_tunable() {
        let mut cfg = Config::default();
        cfg.model.temperature = 0.75;
        cfg.model.max_tokens = Some(512);
        cfg.model.top_p = Some(0.5);
        cfg.limits.max_iterations = 41;
        cfg.approvals.mode = ApprovalMode::AskCommands;
        cfg.approvals.allow_commands = vec!["cargo test".into(), "rg".into()];
        cfg.subagents.max_depth = 2;
        let rows: Vec<(String, String)> = rows_for(&cfg)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        let mut restored = Config::default();
        apply_rows(&mut restored, &rows);
        assert!((restored.model.temperature - 0.75).abs() < f32::EPSILON);
        assert_eq!(restored.model.max_tokens, Some(512));
        assert_eq!(restored.model.top_p, Some(0.5));
        assert_eq!(restored.limits.max_iterations, 41);
        assert_eq!(restored.approvals.mode, ApprovalMode::AskCommands);
        assert_eq!(restored.approvals.allow_commands.len(), 2);
        assert_eq!(restored.subagents.max_depth, 2);
    }

    #[test]
    fn invalid_rows_are_ignored_not_fatal() {
        let mut cfg = Config::default();
        apply_rows(
            &mut cfg,
            &[
                ("model.temperature".into(), "hot".into()),
                ("limits.max_iterations".into(), "-3".into()),
                ("approvals.mode".into(), "whatever".into()),
                ("model.api_key".into(), "sk-x".into()),
            ],
        );
        assert_eq!(cfg.model.temperature, 0.2);
        assert_eq!(cfg.limits.max_iterations, 32);
        assert_eq!(cfg.approvals.mode, ApprovalMode::Ask);
        assert_eq!(cfg.model.api_key.as_deref(), Some("sk-x"));
    }

    #[test]
    fn public_model_rounds_floats_and_hides_the_key() {
        let model = ModelConfig {
            temperature: 0.2,
            top_p: Some(0.95),
            api_key: Some("sk-secret".into()),
            ..ModelConfig::default()
        };
        let value = public_model(&model);
        assert_eq!(value["temperature"], serde_json::json!(0.2));
        assert_eq!(value["top_p"], serde_json::json!(0.95));
        assert_eq!(value["api_key_set"], serde_json::json!(true));
        assert!(!value.to_string().contains("sk-secret"));
    }

    #[test]
    fn public_config_hides_the_key() {
        let cfg = Config {
            lock_settings: false,
            model: ModelConfig {
                api_key: Some("sk-super-secret".into()),
                ..ModelConfig::default()
            },
            ..Config::default()
        };
        let text = serde_json::to_string(&cfg.model.public_view()).unwrap();
        assert!(!text.contains("sk-super-secret"));
        assert!(text.contains("\"api_key_set\":true"));
    }
}

#[cfg(test)]
mod provider_startup_tests {
    use super::*;

    fn config_with_provider(provider: &str) -> Config {
        Config {
            model: ModelConfig {
                provider: provider.into(),
                ..ModelConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn startup_refuses_an_unknown_provider_from_the_file_or_env() {
        let message = validate_settings(&config_with_provider("anthropic_native"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("anthropic_native"), "{message}");
        assert!(message.contains("openai_compatible"), "{message}");
    }

    #[test]
    fn startup_accepts_the_implemented_provider() {
        assert!(validate_settings(&config_with_provider("openai_compatible")).is_ok());
    }
}
