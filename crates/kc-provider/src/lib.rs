//! OpenAI-compatible chat-completions provider with streamed text and tool calls.
//!
//! Scope: `POST {base_url}/chat/completions` with `stream: true`, SSE parsing,
//! tool-call delta accumulation, request timeouts, cancellation and redacted
//! error messages. Native Anthropic/Gemini providers can be added later by
//! implementing [`kc_core::provider::Provider`]; the runtime does not care.

use async_trait::async_trait;
use kc_core::config::ModelConfig;
use kc_core::error::{Error, Result};
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderFactory, ProviderStream};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub mod sse;

pub struct OpenAiProvider {
    pub config: ModelConfig,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(config: ModelConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs.max(5)))
            .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Provider(format!("http client: {e}")))?;
        Ok(Self { config, client })
    }

    fn endpoint(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    async fn stream_chat(
        &self,
        _request: ChatRequest,
        _cancel: CancellationToken,
    ) -> Result<ProviderStream> {
        todo!("kc-provider: implement streamed chat completions")
    }
}

pub struct ProviderFactoryImpl;

#[async_trait]
impl ProviderFactory for ProviderFactoryImpl {
    async fn build(&self, config: &ModelConfig) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(OpenAiProvider::new(config.clone())?))
    }
}
