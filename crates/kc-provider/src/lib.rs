//! OpenAI-compatible chat-completions provider with streamed text and tool calls.
//!
//! Scope: `POST {base_url}/chat/completions` with `stream: true`, SSE parsing,
//! tool-call delta accumulation, request timeouts, cancellation and redacted
//! error messages. Native Anthropic/Gemini providers can be added later by
//! implementing [`kc_core::provider::Provider`]; the runtime does not care.

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
pub mod sse;
mod stream;

pub use request::{build_chat_body, chat_completions_url, models_url};

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
        Ok(Arc::new(OpenAiProvider::new(config.clone())?))
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
