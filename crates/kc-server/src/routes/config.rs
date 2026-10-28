//! `GET/PATCH /api/config`, `POST /api/provider/test`, `GET /api/stats`.

use crate::error::{ApiError, ApiResult};
use crate::routes::parse_body;
use crate::settings::ConfigPatch;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::Json;
use kc_core::config::Config;
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
                Some(profile) => config.model_for_profile(profile),
                None => {
                    return Err(ApiError::not_found(format!(
                        "no provider profile with id `{id}`"
                    )))
                }
            }
        }
        None => config.effective_model(),
    };
    if let Some(base_url) = &body.base_url {
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
    model: &kc_core::config::ModelConfig,
) -> Result<(), String> {
    let cancel = CancellationToken::new();
    let request = ChatRequest {
        model: model.model.clone(),
        messages: vec![Message::user("Reply with the single word: ok")],
        tools: Vec::new(),
        temperature: 0.0,
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
}
