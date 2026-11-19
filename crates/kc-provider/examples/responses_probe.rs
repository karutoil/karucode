//! Diagnostic: build the exact request karucode sends for a Responses profile
//! and stream it, printing every ProviderChunk and its arrival time.
//!
//!   KC_BASE_URL=... KC_API_KEY=... KC_MODEL=... KC_PROVIDER=openai_responses \
//!     cargo run -p kc-provider --example responses_probe

use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::message::Message;
use kc_core::provider::{ChatRequest, Provider, ProviderChunk};
use kc_core::tool::ToolSpec;
use kc_provider::OpenAiProvider;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("KC_BASE_URL")?;
    let key = std::env::var("KC_API_KEY")?;
    let model = std::env::var("KC_MODEL")?;
    let provider = std::env::var("KC_PROVIDER").unwrap_or_else(|_| "openai_responses".into());

    let config = ModelConfig {
        provider,
        base_url: base,
        api_key: Some(key),
        model: model.clone(),
        max_output_tokens: Some(8192),
        request_timeout_secs: 60,
        retry_max_retries: 0,
        ..Default::default()
    };

    let tools = vec![ToolSpec {
        name: "list_dir".into(),
        description: "List a directory inside the workspace.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"]
        }),
        origin: Default::default(),
    }];

    let request = ChatRequest {
        model,
        messages: vec![
            Message::system("You are karucode, a coding agent. Be brief."),
            Message::user("Say hi, then call list_dir on \".\"."),
        ],
        tools,
        temperature: 1.0,
        send_temperature: false,
        max_tokens: None,
        top_p: None,
        parallel_tool_calls: true,
        prompt_cache_key: None,
        extra_body: Default::default(),
        extra_headers: Default::default(),
        admission: None,
    };

    let snapshot = request.clone();
    let body = kc_provider::build_chat_body_for(&config, &snapshot);
    eprintln!(
        "=== request body ===\n{}\n",
        serde_json::to_string_pretty(&body)?
    );

    let started = Instant::now();
    let mut stream = OpenAiProvider::new(config)?
        .stream_chat(request, CancellationToken::new())
        .await?;
    while let Some(item) = stream.next().await {
        let at = started.elapsed().as_millis();
        match item {
            Ok(chunk) => {
                let label = match &chunk {
                    ProviderChunk::Text(t) => format!("Text {t:?}"),
                    ProviderChunk::Reasoning(t) => format!("Reasoning {t:?}"),
                    ProviderChunk::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    } => format!("ToolCallDelta idx={index} id={id:?} name={name:?} args={arguments_delta:?}"),
                    ProviderChunk::Usage {
                        input_tokens,
                        output_tokens,
                        cached_tokens,
                    } => format!("Usage in={input_tokens} out={output_tokens} cached={cached_tokens:?}"),
                    ProviderChunk::Finish { reason } => format!("Finish {reason:?}"),
                    ProviderChunk::Error { message, retryable } => {
                        format!("Error retryable={retryable}: {message}")
                    }
                };
                println!("[{at:>7}ms] {label}");
            }
            Err(err) => {
                println!("[{at:>7}ms] STREAM ERROR: {err}");
                break;
            }
        }
    }
    println!("[{}ms] stream ended", started.elapsed().as_millis());
    Ok(())
}
