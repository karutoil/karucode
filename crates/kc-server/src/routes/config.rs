//! `GET/PATCH /api/config`, `POST /api/provider/test`, `GET /api/stats`.

use crate::error::{ApiError, ApiResult};
use crate::routes::parse_body;
use crate::settings::ConfigPatch;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::Json;
use kc_core::config::{Config, ModelConfig};
use kc_core::message::Message;
use kc_core::provider::{ChatRequest, ProviderChunk};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Probe budget: `/models` used the provider's configured timeout, the fallback
/// chat probe is bounded so a silent endpoint cannot hang the request.
const CHAT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn get_config(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(state.config_payload().await))
}

pub async fn patch_config(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    // Check the lock before parsing so a locked deployment always answers 403,
    // even for a malformed body.
    if state.settings().is_locked() {
        return Err(ApiError::forbidden(
            "settings are locked (KARUCODE_LOCK_SETTINGS); change them in the config file or environment",
        ));
    }
    let patch: ConfigPatch = parse_body(&body)?;
    // `model.provider` is validated against the ids the running factory can
    // actually build, so the settings selector cannot save a provider that would
    // only fail later (or, worse, be silently accepted and ignored).
    if let Some(model) = &patch.model {
        if let Some(provider) = &model.provider {
            crate::settings::check_provider_id(provider, &state.providers().supported())
                .map_err(ApiError::from)?;
        }
    }
    state
        .settings()
        .update(&patch)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(state.config_payload().await))
}

/// `POST /api/provider/test` body. Every field is optional: an empty body
/// probes the active model exactly as before, and the settings screen can probe
/// a profile that is not saved or selected yet (`profile`), or a draft endpoint
/// the user is still typing (`base_url`/`api_key`/`model`).
///
/// A draft endpoint needs a draft key: naming a `base_url` without an `api_key`
/// drops the stored key, so this route cannot be used to replay the configured
/// credential at a host of the caller's choosing. See [`apply_body_overrides`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderTestBody {
    /// Saved profile id to probe instead of the active one.
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    /// Write-only; an empty string clears the key for this probe only.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    request_timeout_secs: Option<u64>,
}

/// `POST /api/provider/test` — always 200 with `{ok, models, error, latency_ms}`.
/// A failing endpoint is a result, not an HTTP error: the settings screen shows
/// the message next to the form.
pub async fn provider_test(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let body: ProviderTestBody = parse_body(&body)?;
    let started = Instant::now();
    let config: Config = state.settings().snapshot();

    let mut model = match body.profile.as_deref() {
        Some(id) => {
            let profile = config.providers.iter().find(|profile| profile.id == id);
            match profile {
                // Test the model the user is probing, not the profile's default:
                // its per-model limits and capabilities are what the run will use.
                Some(profile) => match body.model.as_deref() {
                    Some(model_id) => config.model_for(profile, model_id),
                    None => config.model_for_profile(profile),
                },
                None => {
                    return Err(ApiError::not_found(format!(
                        "no provider profile with id `{id}`"
                    )))
                }
            }
        }
        None => config.effective_model(),
    };
    apply_body_overrides(&mut model, &body);

    if let Err(error) = crate::settings::validate_base_url(&model.base_url) {
        return Ok(Json(probe_result(
            false,
            Vec::new(),
            Some(state.redact(&error.to_string())),
            started,
        )));
    }
    if model.model.trim().is_empty() {
        return Ok(Json(probe_result(
            false,
            Vec::new(),
            Some("a model id is required to probe the endpoint".into()),
            started,
        )));
    }

    let provider = match state.providers().build(&model).await {
        Ok(provider) => provider,
        Err(e) => {
            let message = state.redact(&format!("could not build the provider: {e}"));
            return Ok(Json(probe_result(
                false,
                Vec::new(),
                Some(message),
                started,
            )));
        }
    };

    match provider.list_models().await {
        Ok(models) => Ok(Json(probe_result(true, models, None, started))),
        Err(list_error) => {
            // Some gateways do not implement `GET /models`; a one-token chat is
            // the documented fallback probe.
            match chat_probe(Arc::clone(&provider), &model).await {
                Ok(()) => Ok(Json(probe_result(true, Vec::new(), None, started))),
                Err(chat_error) => {
                    let message = state.redact(&format!(
                        "model listing failed: {list_error}; chat probe failed: {chat_error}"
                    ));
                    Ok(Json(probe_result(
                        false,
                        Vec::new(),
                        Some(message),
                        started,
                    )))
                }
            }
        }
    }
}

/// Fold a `POST /api/provider/test` body onto the model that would be probed.
///
/// **The stored key only ever goes to the configured endpoint.** The body may
/// name another `base_url` (the settings screen probes a draft endpoint the user
/// is still typing), so a key taken from the store would be sent to whatever
/// host the request named - by an ordinary bearer-token client, outside the
/// admin gate and past `lock_settings`. The rule is therefore:
///
/// * no `base_url` in the body - probe the configured endpoint with the stored
///   key; that is the normal "test my provider" call and stays unchanged;
/// * a `base_url` in the body - the stored key is **dropped**. The caller must
///   pass its own `api_key`, or the probe goes out keyless (which is exactly
///   right for a local gateway and a clean failure everywhere else);
/// * an explicit `api_key` in the body always wins, including `""` to clear it.
fn apply_body_overrides(model: &mut ModelConfig, body: &ProviderTestBody) {
    if let Some(base_url) = &body.base_url {
        if base_url.trim() != model.base_url.trim() {
            model.api_key = None;
        }
        model.base_url = base_url.trim().to_string();
    }
    if let Some(model_id) = &body.model {
        model.model = model_id.trim().to_string();
    }
    if let Some(timeout) = body.request_timeout_secs {
        model.request_timeout_secs = timeout.clamp(1, 3600);
    }
    if let Some(key) = &body.api_key {
        let key = key.trim();
        model.api_key = (!key.is_empty()).then(|| key.to_string());
    }
}

fn probe_result(ok: bool, models: Vec<String>, error: Option<String>, started: Instant) -> Value {
    let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    json!({
        "ok": ok,
        "models": models,
        "error": error,
        "latency_ms": latency_ms,
    })
}

/// One-token completion. Success means the endpoint accepted a chat request.
async fn chat_probe(
    provider: Arc<dyn kc_core::provider::Provider>,
    model: &ModelConfig,
) -> Result<(), String> {
    let cancel = CancellationToken::new();
    let request = ChatRequest {
        model: model.model.clone(),
        messages: vec![Message::user("Reply with the single word: ok")],
        tools: Vec::new(),
        temperature: 0.0,
        send_temperature: false,
        max_tokens: Some(1),
        top_p: None,
        parallel_tool_calls: false,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };

    let probe = async {
        let mut stream = provider
            .stream_chat(request, cancel.clone())
            .await
            .map_err(|e| e.to_string())?;
        match futures_util::StreamExt::next(&mut stream).await {
            // A 200 with an empty stream still proves the endpoint responded.
            None => Ok(()),
            Some(Ok(ProviderChunk::Error { message, .. })) => Err(message),
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e.to_string()),
        }
    };

    let outcome = tokio::time::timeout(CHAT_PROBE_TIMEOUT, probe).await;
    cancel.cancel();
    match outcome {
        Ok(result) => result,
        Err(_) => Err(format!(
            "the endpoint accepted the request but streamed nothing within {}s",
            CHAT_PROBE_TIMEOUT.as_secs()
        )),
    }
}

pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let sessions = state.store().list_sessions().await?;
    let active_runs = state.runtime().active_runs().len();
    Ok(Json(json!({
        "uptime_secs": state.uptime_secs(),
        "rss_kb": rss_kb(),
        "sessions": sessions.len(),
        "active_runs": active_runs,
        "events_emitted": state.runtime().events_emitted(),
        "db_path": state.db_path().to_string_lossy(),
    })))
}

/// Resident set size in KiB from `/proc/self/status`; 0 where unavailable.
fn rss_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    if let Some(value) = rest.split_whitespace().next() {
                        return value.parse::<u64>().unwrap_or(0);
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_result_shape_matches_the_docs() {
        let value = probe_result(true, vec!["m".into()], None, Instant::now());
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["models"], json!(["m"]));
        assert_eq!(value["error"], Value::Null);
        assert!(value["latency_ms"].is_u64());
    }

    #[test]
    fn rss_is_reported() {
        // Not portable enough to assert a value; just ensure it does not panic.
        let _ = rss_kb();
    }

    fn stored_model() -> ModelConfig {
        ModelConfig {
            base_url: "https://configured.example/v1".into(),
            model: "configured-model".into(),
            api_key: Some("sk-STORED-SECRET".into()),
            ..ModelConfig::default()
        }
    }

    /// The reported bug: a body carrying only `base_url` used to probe the
    /// attacker-chosen host with the *stored* key, which is a credential
    /// exfiltration path for any bearer-token client (this route is not
    /// admin-gated and ignores `lock_settings`).
    #[test]
    fn a_base_url_override_never_carries_the_stored_key() {
        let body = ProviderTestBody {
            base_url: Some("https://attacker.example/v1".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.base_url, "https://attacker.example/v1");
        assert_eq!(
            model.api_key, None,
            "the stored key must not be replayed to a body-supplied base_url"
        );
    }

    /// The documented draft-endpoint case: both halves supplied, both used.
    #[test]
    fn a_base_url_override_with_its_own_key_still_works() {
        let body = ProviderTestBody {
            base_url: Some("https://draft.example/v1".into()),
            api_key: Some("sk-DRAFT-KEY".into()),
            model: Some("draft-model".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.base_url, "https://draft.example/v1");
        assert_eq!(model.api_key.as_deref(), Some("sk-DRAFT-KEY"));
        assert_eq!(model.model, "draft-model");
    }

    /// Probing the configured endpoint with the stored key is legitimate - it is
    /// what the settings screen does on "test this profile".
    #[test]
    fn probing_the_configured_endpoint_keeps_the_stored_key() {
        let body = ProviderTestBody {
            model: Some("other-model".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.base_url, "https://configured.example/v1");
        assert_eq!(
            model.api_key.as_deref(),
            Some("sk-STORED-SECRET"),
            "an unchanged endpoint keeps the configured key"
        );

        // Naming the same endpoint the config already uses is not an override:
        // it is the same request the UI would send.
        let body = ProviderTestBody {
            base_url: Some(" https://configured.example/v1 ".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.api_key.as_deref(), Some("sk-STORED-SECRET"));
    }

    /// An explicit `api_key` always wins - including `""`, which clears it.
    #[test]
    fn an_explicit_api_key_overrides_the_stored_one() {
        let body = ProviderTestBody {
            api_key: Some("sk-OTHER".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.api_key.as_deref(), Some("sk-OTHER"));

        let body = ProviderTestBody {
            api_key: Some("  ".into()),
            ..ProviderTestBody::default()
        };
        let mut model = stored_model();
        apply_body_overrides(&mut model, &body);
        assert_eq!(model.api_key, None, "an empty api_key clears the key");
    }
}
