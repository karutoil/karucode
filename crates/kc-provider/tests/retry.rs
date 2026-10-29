//! Retry behaviour of [`RetryingProvider`] against a flaky local mock server.
//!
//! These pin the contract the user hit in production: a gateway that dies
//! mid-request (`error decoding response body`) or answers an arbitrary error
//! status must be retried until an attempt lands on a healthy channel — but
//! never after the answer's first chunk reached the consumer.

mod support;

use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderStream};
use kc_provider::{OpenAiProvider, RetryPolicy, RetryingProvider};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;
use tokio_util::sync::CancellationToken;

const SSE_OK: &str = "\
    data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n\
    data: [DONE]\n\n";

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

/// Fast policy so the suite does not sleep through production delays.
fn fast_policy(max_retries: u32) -> RetryPolicy {
    RetryPolicy::new(
        max_retries,
        Duration::from_millis(5),
        Duration::from_millis(20),
    )
}

fn retrying(server: &MockServer, max_retries: u32) -> RetryingProvider {
    let inner: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(model_config(server)).unwrap());
    RetryingProvider::new(inner, fast_policy(max_retries))
}

/// Mock server that fails the first `failures` requests and then streams.
async fn flaky_server(failures: usize, failure: Vec<u8>) -> MockServer {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    spawn(move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        let failure = failure.clone();
        async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n < failures {
                socket.write_all(&failure).await.unwrap();
            } else {
                write_sse(&mut socket, SSE_OK).await;
            }
            let _ = socket.shutdown().await;
        }
    })
    .await
}

fn hit_count(server: &MockServer) -> usize {
    server.requests.lock().expect("request log poisoned").len()
}

async fn texts(stream: ProviderStream) -> String {
    let mut stream = stream;
    let mut out = String::new();
    while let Some(item) = stream.next().await {
        if let ProviderChunk::Text(delta) = item.expect("unexpected stream error") {
            out.push_str(&delta);
        }
    }
    out
}

#[tokio::test]
async fn retries_server_errors_until_an_attempt_succeeds() {
    let failure = response("500 Internal Server Error", "text/plain", "boom");
    let server = flaky_server(2, failure).await;

    let stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok");
    assert_eq!(hit_count(&server), 3, "two failed attempts plus the winner");
}

// The gateway answers 401/404 for a model the next channel serves fine, so a
// client error status is retried like any other failure.
#[tokio::test]
async fn retries_client_error_statuses_too() {
    let failure = response(
        "401 Unauthorized",
        "application/json",
        "{\"error\":{\"message\":\"channel key rotated\"}}",
    );
    let server = flaky_server(1, failure).await;

    let stream = retrying(&server, 3)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok");
    assert_eq!(hit_count(&server), 2);
}

// The exact production failure: headers look fine, the body dies before a single
// event arrives. The next attempt must be a fresh request.
#[tokio::test]
async fn retries_a_body_that_dies_before_any_content() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = spawn(move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // 200 + event-stream headers, then nothing at all: EOF before
                // data, the signature that reports as a decode error.
                socket.write_all(SSE_HEAD.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            } else {
                write_sse(&mut socket, SSE_OK).await;
            }
        }
    })
    .await;

    let stream = retrying(&server, 3)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok");
    assert_eq!(hit_count(&server), 2, "the dead body must be retried");
}

// Once a chunk reached the consumer, replaying the exchange would duplicate it.
#[tokio::test]
async fn never_retries_after_content_started() {
    let server = spawn(move |_req, mut socket| async move {
        // A real delta, then garbage: the failure is reported, not retried.
        socket.write_all(SSE_HEAD.as_bytes()).await.unwrap();
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .await
            .unwrap();
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{\"content\": }}]}\n\n")
            .await
            .unwrap();
        let _ = socket.shutdown().await;
    })
    .await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let mut seen = Vec::new();
    while let Some(item) = stream.next().await {
        seen.push(item.expect("stream error"));
    }
    assert!(
        seen.iter()
            .any(|c| matches!(c, ProviderChunk::Text(t) if t == "partial")),
        "the partial delta must reach the consumer: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|c| matches!(c, ProviderChunk::Error { message, .. } if message.contains("malformed JSON"))),
        "the failure must be surfaced after content started: {seen:?}"
    );
    assert_eq!(
        hit_count(&server),
        1,
        "no retry after content was delivered"
    );
}

// An exhausted budget must say so: a single failed call and 51 failed attempts
// are very different situations for the user staring at the screen.
#[tokio::test]
async fn exhausted_retries_report_the_attempt_count() {
    let failure = response("500 Internal Server Error", "text/plain", "always down");
    let server = flaky_server(usize::MAX, failure).await;

    let mut stream = retrying(&server, 2)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let err = stream
        .next()
        .await
        .expect("a terminal error item")
        .expect_err("expected failure");
    let text = err.to_string();
    assert!(
        text.contains("always down") || text.contains("500"),
        "{text}"
    );
    assert!(text.contains("gave up after 3 attempts"), "{text}");
    assert!(
        stream.next().await.is_none(),
        "the stream must end after failing"
    );
    assert_eq!(hit_count(&server), 3);
}

#[tokio::test]
async fn zero_retries_fails_after_one_attempt() {
    let failure = response("503 Service Unavailable", "text/plain", "gone");
    let server = flaky_server(usize::MAX, failure).await;

    let mut stream = retrying(&server, 0)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert!(stream.next().await.expect("error item").is_err());
    assert_eq!(hit_count(&server), 1);
    assert!(stream.next().await.is_none());
}

// A long backoff must not outlive a cancellation: the user pressed stop.
#[tokio::test]
async fn cancellation_interrupts_the_backoff() {
    let failure = response("500 Internal Server Error", "text/plain", "down");
    let server = flaky_server(usize::MAX, failure).await;

    let cancel = CancellationToken::new();
    let inner: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(model_config(&server)).unwrap());
    // 30s backoff: only cancellation can end this quickly.
    let provider = RetryingProvider::new(
        inner,
        RetryPolicy::new(50, Duration::from_secs(30), Duration::from_secs(30)),
    );

    let mut stream = provider
        .stream_chat(chat_request(), cancel.clone())
        .await
        .unwrap();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let started = Instant::now();
    let item = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("cancellation must interrupt the backoff, not wait 30s")
        .expect("a terminal item");
    let err = item.expect_err("expected cancellation");
    assert!(matches!(err, kc_core::Error::Cancelled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

// Exactly the production failure: the response declares a length it never
// fulfils and the socket closes, so reqwest reports the decode/body error whose
// message is "invalid response body from …: error decoding response body".
// The gateway must treat that as a retryable attempt, not a terminal failure.
#[tokio::test]
async fn retries_the_production_decoding_error() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = spawn(move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Promise 500 bytes, send 44, close: a mid-body decode failure.
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 500\r\nconnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                socket
                    .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"half")
                    .await
                    .unwrap();
                socket.flush().await.ok();
                let _ = socket.shutdown().await;
            } else {
                write_sse(&mut socket, SSE_OK).await;
            }
        }
    })
    .await;

    let stream = retrying(&server, 3)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok", "the retry must recover the turn");
    assert_eq!(hit_count(&server), 2, "the decode error must be retried");
}

// The retry layer must be reachable through the REAL construction path the
// server uses (ProviderFactoryImpl::build), not just when a test wraps a
// provider by hand.
#[tokio::test]
async fn factory_built_provider_retries() {
    use kc_core::provider::ProviderFactory;

    let failure = response("502 Bad Gateway", "text/plain", "boom");
    let server = flaky_server(1, failure).await;
    let mut config = model_config(&server);
    config.retry_max_retries = 2;
    config.retry_base_delay_ms = 5;
    config.retry_max_delay_ms = 20;

    let provider = kc_provider::ProviderFactoryImpl
        .build(&config)
        .await
        .unwrap();
    let stream = provider
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok");
    assert_eq!(hit_count(&server), 2, "factory-built providers must retry");
}
