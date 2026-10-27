//! Streaming behaviour against a local mock HTTP server (no network access).

mod support;

use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::provider::{
    ChatRequest, Provider, ProviderChunk, ProviderStream, ToolCallAccumulator,
};
use kc_provider::OpenAiProvider;
use std::time::{Duration, Instant};
use support::*;
use tokio_util::sync::CancellationToken;

fn model_config(server: &MockServer) -> ModelConfig {
    ModelConfig {
        base_url: format!("{}/v1", server.url("").trim_end_matches('/')),
        model: "mock-model".into(),
        request_timeout_secs: 10,
        ..Default::default()
    }
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        model: "mock-model".into(),
        messages: vec![kc_core::message::Message::user("hello")],
        tools: Vec::new(),
        temperature: 0.2,
        max_tokens: None,
        top_p: None,
        parallel_tool_calls: true,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    }
}

async fn start(config: ModelConfig, request: ChatRequest) -> (ProviderStream, CancellationToken) {
    let provider = OpenAiProvider::new(config).unwrap();
    let cancel = CancellationToken::new();
    let stream = provider.stream_chat(request, cancel.clone()).await.unwrap();
    (stream, cancel)
}

async fn collect(mut stream: ProviderStream) -> Vec<ProviderChunk> {
    let mut chunks = Vec::new();
    while let Some(item) = stream.next().await {
        chunks.push(item.unwrap_or_else(|e| panic!("unexpected stream error: {e}")));
    }
    chunks
}

fn expect_err(result: kc_core::Result<ProviderStream>) -> kc_core::Error {
    match result {
        Ok(_) => panic!("expected the provider to fail before streaming"),
        Err(err) => err,
    }
}

fn texts(chunks: &[ProviderChunk]) -> String {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            ProviderChunk::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn text_deltas_usage_and_done() {
    let body = "\
        data: {\"id\":\"1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
        data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4}}\n\n\
        data: [DONE]\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;

    assert_eq!(texts(&chunks), "Hello");
    assert_eq!(
        chunks
            .iter()
            .filter(|c| matches!(c, ProviderChunk::Finish { .. }))
            .count(),
        1
    );
    assert!(chunks.contains(&ProviderChunk::Finish {
        reason: "stop".into()
    }));
    assert!(chunks.contains(&ProviderChunk::Usage {
        input_tokens: 12,
        output_tokens: 4
    }));

    let request = server.request();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v1/chat/completions");
    let json = request.json();
    assert_eq!(json["model"], "mock-model");
    assert_eq!(json["stream"], true);
}

#[tokio::test]
async fn tool_calls_split_across_chunks_with_index_reuse() {
    let body = "\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"pa\"}}]}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\":\\\"a.txt\\\"}\"}}]}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"list_dir\",\"arguments\":\"{\\\"dir\\\":\\\".\\\"}\"}}]}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
        data: [DONE]\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;

    let mut accumulator = ToolCallAccumulator::default();
    let mut deltas = 0usize;
    for chunk in &chunks {
        if let ProviderChunk::ToolCallDelta {
            index,
            id,
            name,
            arguments_delta,
        } = chunk
        {
            deltas += 1;
            accumulator.apply(*index, id.clone(), name.clone(), arguments_delta.clone());
        }
    }
    assert_eq!(deltas, 5);
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_a");
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].arguments, "{\"path\":\"a.txt\"}");
    assert_eq!(calls[1].id, "call_b");
    assert_eq!(calls[1].name, "list_dir");
    assert_eq!(calls[1].arguments, "{\"dir\":\".\"}");
    assert!(chunks.contains(&ProviderChunk::Finish {
        reason: "tool_calls".into()
    }));
}

#[tokio::test]
async fn deltas_split_mid_json_and_mid_utf8_are_reassembled() {
    let body = "\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Héllo\"}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" wörld\"},\"finish_reason\":\"stop\"}]}\n\n\
        data: [DONE]\n\n";
    let bytes = format!("{SSE_HEAD}{body}").into_bytes();
    let server = spawn(move |_req, mut socket| {
        let bytes = bytes.clone();
        async move {
            for (n, part) in bytes.chunks(7).enumerate() {
                socket.write_all(part).await.unwrap();
                socket.flush().await.unwrap();
                if n % 3 == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            let _ = socket.shutdown().await;
        }
    })
    .await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    assert_eq!(texts(&chunks), "Héllo wörld");
    assert!(chunks.contains(&ProviderChunk::Finish {
        reason: "stop".into()
    }));
}

#[tokio::test]
async fn unauthorized_error_never_leaks_the_api_key() {
    const KEY: &str = "sk-live-super-secret-1234567890";
    let leaky_body = format!(
        "{{\"error\":{{\"message\":\"Incorrect API key provided: {KEY}. You can find your key at ...\"}}}}"
    );
    let server = spawn(move |_req, mut socket| {
        let response = response("401 Unauthorized", "application/json", &leaky_body);
        async move {
            socket.write_all(&response).await.unwrap();
            let _ = socket.shutdown().await;
        }
    })
    .await;

    let mut config = model_config(&server);
    config.api_key = Some(KEY.into());
    let provider = OpenAiProvider::new(config).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(!text.contains(KEY), "api key leaked: {text}");
    assert!(text.contains("401"), "{text}");
    assert!(
        text.to_lowercase().contains("authentication failed"),
        "{text}"
    );
    assert_eq!(
        server.request().header("authorization").as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert!(matches!(err, kc_core::Error::Provider(_)));
}

#[tokio::test]
async fn server_error_is_reported_with_status_and_excerpt() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response("500 Internal Server Error", "text/plain", "internal boom");
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(model_config(&server)).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(text.contains("upstream provider error"), "{text}");
    assert!(text.contains("500"), "{text}");
    assert!(text.contains("internal boom"), "{text}");
}

#[tokio::test]
async fn malformed_sse_chunk_emits_error_chunk_and_ends_stream() {
    let body = "\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"}}]}\n\n\
        data: {\"choices\":[{\"delta\": {\"content\": }]}\n\n\
        data: {\"choices\":[{\"delta\":{\"content\":\"never\"}}]}\n\n\
        data: [DONE]\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;

    assert_eq!(chunks.len(), 2, "unexpected chunks: {chunks:?}");
    assert_eq!(chunks[0], ProviderChunk::Text("a".into()));
    match &chunks[1] {
        ProviderChunk::Error { message, retryable } => {
            assert!(message.contains("malformed JSON"), "{message}");
            assert!(!retryable);
        }
        other => panic!("expected Error chunk, got {other:?}"),
    }
}

#[tokio::test]
async fn cancellation_aborts_a_live_stream() {
    let server = spawn(move |_req, mut socket| async move {
        socket.write_all(SSE_HEAD.as_bytes()).await.unwrap();
        socket
            .write_all(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"first\"}}]}\n\n")
            .await
            .unwrap();
        socket.flush().await.unwrap();
        // Hold the connection open; the client must not wait for us.
        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;

    let (mut stream, cancel) = start(model_config(&server), chat_request()).await;
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("first chunk timed out")
        .expect("stream ended early")
        .unwrap();
    assert_eq!(first, ProviderChunk::Text("first".into()));

    let started = Instant::now();
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("cancellation did not abort the stream promptly");
    assert!(started.elapsed() < Duration::from_secs(2));
    match outcome {
        Some(Err(kc_core::Error::Cancelled)) => {}
        other => panic!("expected cancellation error, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn stream_without_done_terminator_still_finishes() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    assert_eq!(texts(&chunks), "partial");
    // No finish_reason arrived: report `eof` so the runtime can warn that the
    // answer may be incomplete, rather than claiming a clean stop.
    assert_eq!(
        chunks.last(),
        Some(&ProviderChunk::Finish {
            reason: "eof".into()
        })
    );
}

#[tokio::test]
async fn unterminated_final_event_is_parsed_at_eof() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"tail\"},\"finish_reason\":\"stop\"}]}";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    assert_eq!(texts(&chunks), "tail");
    assert!(chunks.contains(&ProviderChunk::Finish {
        reason: "stop".into()
    }));
}

#[tokio::test]
async fn empty_body_is_rejected_before_streaming() {
    let server = spawn(move |_req, mut socket| async move {
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(model_config(&server)).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    assert!(err.to_string().contains("empty response body"), "{err}");
}

#[tokio::test]
async fn reasoning_deltas_are_streamed() {
    let body = "\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"pondering\"}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\" still\"}}]}\n\n\
        data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n\
        data: [DONE]\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    let reasoning: String = chunks
        .iter()
        .filter_map(|chunk| match chunk {
            ProviderChunk::Reasoning(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, "pondering still");
    assert_eq!(texts(&chunks), "done");
}

#[tokio::test]
async fn in_stream_error_payload_is_surfaced() {
    let body = "\
        data: {\"error\":{\"message\":\"context length exceeded\",\"type\":\"invalid_request_error\"}}\n\n";
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    assert_eq!(chunks.len(), 1, "unexpected chunks: {chunks:?}");
    match &chunks[0] {
        ProviderChunk::Error { message, retryable } => {
            assert!(message.contains("context length exceeded"), "{message}");
            assert!(!retryable);
        }
        other => panic!("expected Error chunk, got {other:?}"),
    }
}

#[tokio::test]
async fn connection_refused_names_the_url() {
    // Bind then drop, so the port is almost certainly closed.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let config = ModelConfig {
        base_url: format!("http://{addr}/v1"),
        request_timeout_secs: 5,
        ..Default::default()
    };
    let provider = OpenAiProvider::new(config).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(text.contains(&addr.to_string()), "{text}");
    assert!(matches!(err, kc_core::Error::Provider(_)), "{text}");
}
#[tokio::test]
async fn rate_limit_error_is_actionable() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response(
            "429 Too Many Requests",
            "application/json",
            "{\"error\":\"slow down\"}",
        );
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(model_config(&server)).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(text.contains("429"), "{text}");
    assert!(text.to_lowercase().contains("rate limit"), "{text}");
    assert!(text.contains("slow down"), "{text}");
}

#[tokio::test]
async fn missing_endpoint_hints_at_base_url_and_model() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response("404 Not Found", "text/plain", "no route");
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(model_config(&server)).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(text.contains("404"), "{text}");
    assert!(text.contains("base_url"), "{text}");
    assert!(text.contains("mock-model"), "{text}");
}

#[tokio::test]
async fn json_body_instead_of_sse_is_a_clear_error() {
    let server = spawn(move |_req, mut socket| async move {
        let response = response(
            "200 OK",
            "application/json",
            "{\"choices\":[{\"message\":{\"content\":\"buffered\"}}]}",
        );
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    })
    .await;
    let provider = OpenAiProvider::new(model_config(&server)).unwrap();
    let err = expect_err(
        provider
            .stream_chat(chat_request(), CancellationToken::new())
            .await,
    );
    let text = err.to_string();
    assert!(text.contains("stream: true"), "{text}");
    assert!(text.contains("application/json"), "{text}");
}

#[tokio::test]
async fn object_form_tool_arguments_are_not_dropped() {
    // Some gateways send `arguments` as an object instead of a JSON string.
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_o\",",
        "\"function\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.txt\"}}}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let server =
        spawn(move |_req, mut socket| async move { write_sse(&mut socket, body).await }).await;
    let (stream, _cancel) = start(model_config(&server), chat_request()).await;
    let chunks = collect(stream).await;
    let delta = chunks
        .iter()
        .find_map(|chunk| match chunk {
            ProviderChunk::ToolCallDelta {
                arguments_delta, ..
            } => arguments_delta.clone(),
            _ => None,
        })
        .expect("tool call delta");
    let parsed: serde_json::Value = serde_json::from_str(&delta).expect("valid json arguments");
    assert_eq!(parsed["path"], serde_json::json!("a.txt"));
}

#[tokio::test]
async fn connect_error_redacts_the_api_key() {
    // Point at a closed port; the URL and message must not contain the key.
    let config = ModelConfig {
        api_key: Some("sk-connect-secret".into()),
        base_url: "http://127.0.0.1:1/v1".into(),
        model: "mock-model".into(),
        request_timeout_secs: 2,
        ..ModelConfig::default()
    };
    let provider = kc_provider::OpenAiProvider::new(config).unwrap();
    let err = provider
        .stream_chat(chat_request(), tokio_util::sync::CancellationToken::new())
        .await
        .err()
        .expect("connect must fail");
    let text = err.to_string();
    assert!(!text.contains("sk-connect-secret"), "{text}");
}
