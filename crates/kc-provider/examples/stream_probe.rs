//! Diagnostic: show how an OpenAI-compatible endpoint delivers a streamed
//! completion to this exact HTTP client stack (reqwest + rustls, same features
//! as the app). Prints response headers and the arrival time of each SSE chunk.
//!
//!   KC_BASE_URL=https://gateway/v1 KC_API_KEY=... KC_MODEL=... \
//!     cargo run -p kc-provider --example stream_probe
//!
//! Why it exists: a gateway (or a CDN in front of it) can accept `stream: true`
//! and still buffer the body, which looks like "the model is slow" until you
//! measure chunk arrival times.

use futures_util::StreamExt;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("KC_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:4000/v1".into());
    let key = std::env::var("KC_API_KEY").unwrap_or_default();
    let model = std::env::var("KC_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));

    let prompt = std::env::var("KC_PROMPT")
        .unwrap_or_else(|_| "Count from 1 to 10, one number per line.".into());
    let mut body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": [{"role": "user", "content": prompt}],
    });
    // KC_TOOLS=1 sends a tool schema, like the real agent does.
    if std::env::var("KC_TOOLS").as_deref() == Ok("1") {
        body["tools"] = serde_json::json!([{
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }
        }]);
        body["tool_choice"] = serde_json::json!("auto");
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let started = Instant::now();
    let mut request = client.post(&url).json(&body);
    if !key.is_empty() {
        request = request.bearer_auth(&key);
    }
    let response = request.send().await?;

    println!("status: {}", response.status());
    for (name, value) in response.headers() {
        println!("header: {name}: {}", value.to_str().unwrap_or("<binary>"));
    }
    println!();

    let mut stream = response.bytes_stream();
    let mut chunks = 0usize;
    let mut bytes = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        chunks += 1;
        bytes += chunk.len();
        if chunks <= 10 || chunks.is_multiple_of(20) {
            let preview =
                String::from_utf8_lossy(&chunk[..chunk.len().min(90)]).replace('\n', "\\n");
            println!(
                "{:7.2}s chunk {:>3} ({} B): {}",
                started.elapsed().as_secs_f64(),
                chunks,
                chunk.len(),
                preview
            );
        }
    }
    println!(
        "\n{} chunks, {} bytes, {:.2}s",
        chunks,
        bytes,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
