//! OpenAI-compatible chat-completions provider with streamed text and tool calls.
//!
//! Scope: `POST {base_url}/chat/completions` with `stream: true`, SSE parsing,
//! tool-call delta accumulation, request timeouts, cancellation and redacted
//! error messages. Native Anthropic/Gemini providers can be added later by
//! implementing [`kc_core::provider::Provider`]; the runtime does not care.

// @kc-doc: provider-profiles
// @kc-title: Configure a model provider
// @kc-summary: Point karucode at any OpenAI-compatible endpoint, by environment or as named profiles, and choose the active model.
// @kc-tags: provider, model, config, setup, gateway, llm
// @kc-applies-to: env:KARUCODE_BASE_URL, env:KARUCODE_API_KEY, env:KARUCODE_MODEL, env:OPENAI_BASE_URL, env:OPENAI_API_KEY, env:KARUCODE_TEMPERATURE, env:KARUCODE_MAX_TOKENS, env:KARUCODE_REQUEST_TIMEOUT_SECS, env:KARUCODE_MODELS_DEV_URL, env:KARUCODE_RETRY_MAX_RETRIES, env:KARUCODE_RETRY_BASE_DELAY_MS, env:KARUCODE_RETRY_MAX_DELAY_MS, env:KARUCODE_RETRY_MAX_STREAM_RESTARTS, config:[[providers]], config:active_provider, config:active_model, config:[model], config:retry_max_retries, config:retry_base_delay_ms, config:retry_max_delay_ms, config:retry_max_stream_restarts, event:stream_retry, route:GET /api/config, route:PATCH /api/config, route:POST /api/provider/test, route:GET /api/models/catalog, ui:Settings
//
// # Fastest path: three environment variables
//
// ```bash
// export KARUCODE_BASE_URL=http://127.0.0.1:4000/v1   # the API root, not /chat/completions
// export KARUCODE_API_KEY=sk-...                      # omit for keyless local servers
// export KARUCODE_MODEL=claude-sonnet
// cargo run --release -p kc-server
// ```
//
// Anything that speaks `POST {base_url}/chat/completions` with streaming tool
// calls works: LiteLLM, vLLM, Ollama (`http://127.0.0.1:11434/v1`), llama.cpp
// (`http://127.0.0.1:8080/v1`), OpenRouter, or a hosted vendor API. There is no
// vendor-specific client to install.
//
// # Named profiles in config.toml
//
// More than one endpoint, or several models behind one gateway, belongs in the
// config file (`$KARUCODE_CONFIG`, default `~/.config/karucode/config.toml`):
//
// ```toml
// active_provider = "gateway"
// active_model = "claude-sonnet"
//
// [[providers]]
// id = "gateway"                 # 1-64 chars: letters, digits, '.', '_', '-'
// name = "LiteLLM gateway"
// provider = "openai_compatible"
// base_url = "http://127.0.0.1:4000/v1"
// api_key = "sk-litellm"
// request_timeout_secs = 300
//
//   [[providers.models]]
//   id = "claude-sonnet"         # the id sent to the endpoint
//   name = "Claude Sonnet"       # label; falls back to the id
//   context_window = 200000
//   max_output_tokens = 8192
//   tool_call = true
//   reasoning = true
//   cost_input = 3.0             # USD per million input tokens
//   cost_output = 15.0
//
// [[providers]]
// id = "local-vllm"
// name = "vLLM on this box"
// base_url = "http://127.0.0.1:8000/v1"
//
//   [[providers.models]]
//   id = "Qwen/Qwen2.5-Coder-32B-Instruct"
//   context_window = 32768
//   tool_call = true
// ```
//
// * `active_provider` / `active_model` are install-wide: every project and
//   session uses the same model.
// * `temperature`, `max_tokens`, `top_p` and `parallel_tool_calls` stay global in
//   `[model]`, so switching profiles does not change sampling.
// * With `[[providers]]` present, the connection fields of `[model]`
//   (`base_url`, `api_key`, `request_timeout_secs`, `extra_headers`, `extra_body`)
//   are ignored; without profiles, a `default` profile is synthesised from
//   `[model]`.
// * Limits: 16 profiles, 64 models per profile, unique model ids per profile,
//   `base_url` must be `http(s)://` without embedded credentials, model ids at
//   most 200 characters. `PATCH /api/config` enforces the same rules and returns
//   `400` naming the offending field.
//
// # Which layer wins
//
// 1. values saved from the UI (SQLite `settings`) - unless
//    `KARUCODE_LOCK_SETTINGS=1`;
// 2. environment variables;
// 3. the config file;
// 4. built-in defaults.
//
// Setting `KARUCODE_MODEL`, `KARUCODE_BASE_URL` or `KARUCODE_API_KEY` (or the
// `OPENAI_*` fallbacks) expresses them as a reserved `env` profile and selects
// it, so an env-driven deployment never silently keeps using a file profile.
// Keys are write-only: `GET /api/config` reports `api_key_set`, never the key.
//
// # Retries, and why a broken answer no longer fails the run
//
// A gateway dying mid-request is routine: a rotated upstream key answers 401, a
// channel in failover 404, a dying backend 502, a broken proxy 200 with an HTML
// splash, and a stream can stop mid-body with `error decoding response body`.
// Two budgets cover that, both global (`[model]`, not per profile):
//
// * `retry_max_retries` (default 50, `KARUCODE_RETRY_MAX_RETRIES`) covers
//   attempts that fail **before the first chunk**. It is deliberately
//   indiscriminate - any status, any transport error, an empty or unreadable
//   body - because the next attempt is a different upstream channel behind the
//   same URL. Backoff starts at `retry_base_delay_ms` (default 500), doubles per
//   attempt with jitter, and is capped by `retry_max_delay_ms` (default 30000),
//   so the ceiling rather than the attempt count bounds the total wait.
// * `retry_max_stream_restarts` (default 3,
//   `KARUCODE_RETRY_MAX_STREAM_RESTARTS`) covers the harder case: the answer
//   *started* streaming and then the stream died. Replaying it would duplicate
//   text you already saw, so karucode discards the partial attempt (nothing of
//   it is persisted), reports `stream_retry`, and re-issues the identical
//   request. Each restart is a full generation, which is why this budget is
//   small. `0` disables it, and a mid-answer death then fails the turn; when
//   the budget runs out the last partial text is kept and the run fails with
//   the upstream reason.
//
// ```toml
// [model]
// retry_max_retries = 50          # attempts = retries + 1; 0 disables
// retry_base_delay_ms = 500       # first delay; doubles per attempt (jittered)
// retry_max_delay_ms = 30000      # backoff ceiling for both layers
// retry_max_stream_restarts = 3   # restarts of a stream that died mid-answer
// ```
//
// A cancellation is never retried, and neither is a configuration error: no
// attempt count fixes an empty `base_url` or an unsendable header.
//
// # Model metadata without typing it
//
// The settings screen can prefill context window, limits, tool-calling support
// and cost from <https://models.dev>. The server fetches and caches that catalog
// itself (`GET /api/models/catalog?provider=...`, six-hour cache, `refresh=true`
// to force). Point `KARUCODE_MODELS_DEV_URL` at a mirror or an air-gapped copy - it
// must be `http(s)://`, is read once at startup, and a failed fetch only makes the
// catalog report `ok: false`; nothing else in the settings path depends on it.
//
// # Verify it works
//
// ```bash
// curl -s localhost:8787/api/health | jq          # active model + base_url
// curl -s -X POST localhost:8787/api/provider/test \
//      -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//      -d '{"profile":"gateway"}'                 # live probe, lists models
// ```
//
// A probe failure is returned as `ok: false` with a redacted reason, not a 500.
// Press the refresh button in Settings (or `PATCH /api/config` with the active
// provider/model) to switch models without restarting the server.
//
// # Gotchas
//
// * karucode never sends `tool_choice`. Explicit `tool_choice: "auto"` was
//   measured pushing time-to-first-token to 9-30 s on some gateways. Use
//   `[model.extra_body] tool_choice = ...` only if a gateway requires it.
// * `base_url` is the prefix, not the full URL: karucode appends
//   `/chat/completions` (and `/models` for a probe).
// * A model that does not support tool calls will answer, but it cannot edit
//   files or run commands; set `tool_call = false` in the model entry so the UI
//   warns instead of pretending.
// * Provider settings are not per project. Switching project does not switch
//   model.
//
// Full reference: `README.md` (Provider profiles and the model library) and
// `docs/API.md` (config routes).

use async_trait::async_trait;
use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::error::{Error, Result};
use kc_core::provider::{ChatRequest, Provider, ProviderFactory, ProviderStream};
use reqwest::header::{HeaderMap, CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod errors;
pub mod request;
pub mod retry;
pub mod sse;
mod stream;

pub use request::{build_chat_body, chat_completions_url, models_url};
pub use retry::{RetryPolicy, RetryingProvider};

/// TCP/TLS handshake budget; the request itself is bounded by
/// `ModelConfig::request_timeout_secs`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Ceiling for a `/models` response body.
const MODELS_BODY_LIMIT: usize = 1024 * 1024;
const MIN_TIMEOUT_SECS: u64 = 5;

pub struct OpenAiProvider {
    pub config: ModelConfig,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(config: ModelConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // A per-read timeout keeps long generations alive while still
            // failing when the endpoint goes silent.
            .read_timeout(timeout_for(&config))
            .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Provider(format!("http client: {e}")))?;
        Ok(Self { config, client })
    }

    /// Build with a caller-provided client (custom proxy, TLS roots or pool).
    pub fn with_client(config: ModelConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Full chat-completions URL for the configured base.
    pub fn endpoint(&self) -> Result<String> {
        chat_completions_url(&self.config.base_url)
    }

    fn secrets(&self, headers: &HeaderMap) -> Vec<String> {
        let mut secrets = request::sensitive_values(headers);
        if let Some(key) = self.api_key() {
            secrets.push(key.to_string());
        }
        secrets
    }

    fn api_key(&self) -> Option<&str> {
        self.config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }
}

fn timeout_for(config: &ModelConfig) -> Duration {
    Duration::from_secs(config.request_timeout_secs.max(MIN_TIMEOUT_SECS))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ProviderStream> {
        let url = self.endpoint()?;
        let mut body = request::build_chat_body(&request);
        request::apply_config_extra_body(&mut body, &self.config.extra_body);
        let headers = request::build_headers(&self.config, &request)?;
        let secrets = self.secrets(&headers);
        let timeout = timeout_for(&self.config);

        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let send = self.client.post(&url).headers(headers).json(&body).send();
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            outcome = tokio::time::timeout(timeout, send) => match outcome {
                Err(_) => {
                    return Err(Error::Timeout(format!(
                        "provider at {url} sent no response headers within {}s",
                        timeout.as_secs()
                    )));
                }
                Ok(Err(err)) => return Err(errors::map_reqwest_error(&url, &err, timeout, &secrets)),
                Ok(Ok(response)) => response,
            },
        };

        let status = response.status();
        if !status.is_success() {
            return Err(errors::http_status_error(
                response,
                &url,
                &self.config.model,
                &secrets,
                false,
            )
            .await);
        }
        if status == reqwest::StatusCode::NO_CONTENT || response.content_length() == Some(0) {
            return Err(Error::Provider(format!(
                "provider at {url} returned an empty response body (status {status}); \
                 check that model `{}` supports streaming",
                self.config.model
            )));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.starts_with("application/json") {
            return Err(Error::Provider(format!(
                "provider at {url} answered with `{content_type}` instead of an event stream: \
                 the endpoint ignored `stream: true` (a buffering proxy or an incompatible API?)"
            )));
        }

        Ok(stream::stream_from_response(
            response.bytes_stream().boxed(),
            cancel,
            url,
            timeout,
            secrets,
        ))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = models_url(&self.config.base_url)?;
        let headers = request::build_headers_with(&self.config, "application/json", &[])?;
        let secrets = self.secrets(&headers);
        let timeout = timeout_for(&self.config);

        let send = self.client.get(&url).headers(headers).send();
        let response = match tokio::time::timeout(timeout, send).await {
            Err(_) => {
                return Err(Error::Timeout(format!(
                    "provider at {url} sent no response within {}s",
                    timeout.as_secs()
                )));
            }
            Ok(Err(err)) => return Err(errors::map_reqwest_error(&url, &err, timeout, &secrets)),
            Ok(Ok(response)) => response,
        };
        if !response.status().is_success() {
            return Err(errors::http_status_error(
                response,
                &url,
                &self.config.model,
                &secrets,
                true,
            )
            .await);
        }
        let body = read_limited(response, &url, MODELS_BODY_LIMIT, timeout, &secrets).await?;
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            Error::Provider(format!(
                "provider at {url} returned an unreadable model list: {e}"
            ))
        })?;
        parse_model_ids(&value).ok_or_else(|| {
            Error::Provider(format!(
                "provider at {url} returned no model list (expected `{{\"data\":[{{\"id\":\"...\"}}]}}` or a top-level array)"
            ))
        })
    }
}

async fn read_limited(
    mut response: reqwest::Response,
    url: &str,
    limit: usize,
    timeout: Duration,
    secrets: &[String],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    while buf.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit - buf.len();
                buf.extend_from_slice(&chunk[..room.min(chunk.len())]);
            }
            Ok(None) => break,
            Err(err) => {
                return Err(errors::map_reqwest_error(url, &err, timeout, secrets));
            }
        }
    }
    Ok(buf)
}

fn parse_model_ids(value: &serde_json::Value) -> Option<Vec<String>> {
    let mut ids: Vec<String> = Vec::new();
    let mut push = |entry: &serde_json::Value| {
        let id = match entry {
            serde_json::Value::String(id) => Some(id.as_str()),
            other => other.get("id").and_then(|id| id.as_str()),
        };
        if let Some(id) = id {
            if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    };
    let mut found = false;
    if let Some(array) = value.as_array() {
        found = true;
        for entry in array {
            push(entry);
        }
    }
    for key in ["data", "models"] {
        if let Some(array) = value.get(key).and_then(|v| v.as_array()) {
            found = true;
            for entry in array {
                push(entry);
            }
        }
    }
    if found {
        Some(ids)
    } else {
        None
    }
}

pub struct ProviderFactoryImpl;

impl Default for ProviderFactoryImpl {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ProviderFactory for ProviderFactoryImpl {
    async fn build(&self, config: &ModelConfig) -> Result<Arc<dyn Provider>> {
        let provider: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(config.clone())?);
        // Every run goes through the retry layer: the endpoints this talks to
        // fail transiently in ways a single attempt cannot survive — a dropped
        // connection, an arbitrary error status from a rotating channel, a
        // body that dies mid-decode. See `retry` for why retries happen inside
        // the stream and why they stop at the first delivered chunk.
        Ok(Arc::new(RetryingProvider::new(
            provider,
            RetryPolicy::from_config(config),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_ids_parse_from_every_shape() {
        assert_eq!(
            parse_model_ids(&json!({"data": [{"id": "a"}, {"id": "b"}, {"id": "a"}]})),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            parse_model_ids(&json!({"models": [{"id": "x"}, "y"]})),
            Some(vec!["x".to_string(), "y".to_string()])
        );
        assert_eq!(
            parse_model_ids(&json!([{"id": "m"}])),
            Some(vec!["m".to_string()])
        );
        assert_eq!(parse_model_ids(&json!({"data": []})), Some(vec![]));
        assert_eq!(parse_model_ids(&json!({"unexpected": 1})), None);
    }

    #[test]
    fn empty_api_key_is_treated_as_absent() {
        let config = ModelConfig {
            api_key: Some("   ".into()),
            ..ModelConfig::default()
        };
        let provider = OpenAiProvider::new(config).unwrap();
        assert!(provider.api_key().is_none());

        let config = ModelConfig {
            api_key: Some("sk-x".into()),
            ..ModelConfig::default()
        };
        let provider = OpenAiProvider::new(config).unwrap();
        assert_eq!(provider.api_key(), Some("sk-x"));
    }

    #[tokio::test]
    async fn factory_builds_openai_compatible_providers() {
        let factory = ProviderFactoryImpl;
        assert_eq!(factory.supported(), vec!["openai_compatible".to_string()]);
        let provider = factory.build(&ModelConfig::default()).await.unwrap();
        assert_eq!(provider.id(), "openai_compatible");
    }
}
