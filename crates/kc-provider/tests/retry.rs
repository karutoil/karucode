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
        admission: None,
        model: "mock-model".into(),
        messages: vec![kc_core::message::Message::user("hello")],
        tools: Vec::new(),
        temperature: 0.2,
        send_temperature: true,
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

// A 401 is permanent for a single endpoint: a revoked key is not fixed by
// re-sending the request. It used to be retried 51 times (~20 minutes, 51 billed
// attempts) before the user saw the real error.
#[tokio::test]
async fn does_not_retry_unauthorized_by_default() {
    let failure = response(
        "401 Unauthorized",
        "application/json",
        "{\"error\":{\"message\":\"channel key rotated\"}}",
    );
    let server = flaky_server(usize::MAX, failure).await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let err = stream
        .next()
        .await
        .expect("a terminal error item")
        .expect_err("a 401 must fail the attempt");
    let text = err.to_string();
    assert!(text.contains("401"), "{text}");
    assert!(
        !text.contains("gave up after"),
        "a permanent failure must not report a retry budget: {text}"
    );
    assert_eq!(hit_count(&server), 1, "a 401 must be sent exactly once");
    assert!(stream.next().await.is_none());
}

// A malformed request stays malformed: 400/413/422 are never retried.
#[tokio::test]
async fn does_not_retry_a_bad_request() {
    let failure = response(
        "400 Bad Request",
        "application/json",
        "{\"error\":{\"message\":\"unsupported field: foo\"}}",
    );
    let server = flaky_server(usize::MAX, failure).await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let err = stream
        .next()
        .await
        .expect("a terminal error item")
        .expect_err("a 400 must fail the attempt");
    assert!(err.to_string().contains("400"), "{err}");
    assert_eq!(hit_count(&server), 1, "a 400 must be sent exactly once");
    assert!(stream.next().await.is_none());
}

// 429 is transient: the endpoint is busy, not the request.
#[tokio::test]
async fn retries_rate_limits() {
    let failure = response(
        "429 Too Many Requests",
        "application/json",
        "{\"error\":{\"message\":\"slow down\"}}",
    );
    let server = flaky_server(2, failure).await;

    let stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok");
    assert_eq!(hit_count(&server), 3, "two 429s then the winner");
}

// The stream layer's own `retryable: false` must be honoured: an in-stream
// `invalid_request_error` is a rejected request, not a dead channel. It must
// reach the consumer *as a chunk*, still marked non-retryable: raising it as an
// `Err(Error::Provider(..))` made the runtime treat it as transport-shaped and
// restart the whole stream (4 billed regenerations of one rejected request).
#[tokio::test]
async fn honours_a_non_retryable_stream_error() {
    let body = "data: {\"error\":{\"message\":\"unsupported parameter: foo\",\
                \"type\":\"invalid_request_error\"}}\n\n";
    let server = spawn(move |_req, mut socket| async move {
        write_sse(&mut socket, body).await;
    })
    .await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let item = stream
        .next()
        .await
        .expect("a terminal item")
        .expect("a rejected request is delivered as a chunk, not as a transport error");
    match item {
        ProviderChunk::Error { message, retryable } => {
            assert!(message.contains("unsupported parameter"), "{message}");
            assert!(
                !retryable,
                "the verdict must survive to the runtime, which restarts on `retryable`"
            );
        }
        other => panic!("expected an Error chunk, got {other:?}"),
    }
    assert_eq!(
        hit_count(&server),
        1,
        "retryable: false must not be re-sent"
    );
    assert!(stream.next().await.is_none());
}

// The wall-clock budget stops a long retry chain even when the attempt count
// would still allow it: 50 retries against a 30s ceiling is ~20 minutes.
#[tokio::test]
async fn wall_clock_budget_stops_a_long_retry_chain() {
    let failure = response("503 Service Unavailable", "text/plain", "always down");
    let server = flaky_server(usize::MAX, failure).await;

    let inner: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(model_config(&server)).unwrap());
    // A generous attempt budget (100) with a 300ms wall-clock budget and a
    // 100ms backoff: only the clock can stop this quickly.
    let provider = RetryingProvider::new(
        inner,
        RetryPolicy::new(100, Duration::from_millis(100), Duration::from_millis(100))
            .with_max_total(Duration::from_millis(300)),
    );

    let mut stream = provider
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let started = Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the wall-clock budget must end the retries, not run 100 attempts")
        .expect("a terminal item")
        .expect_err("expected failure");
    let elapsed = started.elapsed();
    assert!(err.to_string().contains("503"), "{err}");
    assert!(
        elapsed < Duration::from_secs(5),
        "the budget must stop the chain, took {elapsed:?}"
    );
    let hits = hit_count(&server);
    assert!(
        hits < 100,
        "the wall-clock budget must cut the attempt count short, got {hits}"
    );
    assert!(stream.next().await.is_none());
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

// --- permanent failures are not retried --------------------------------------
//
// Three shapes reached the retry layer with no status to classify and were
// therefore treated as transient: a 200 carrying an error body, a 200 with no
// body at all, and a completion delivered only as `data: [DONE]`. Each attempt
// is a billed generation, so a quota error was retried 51 times and a [DONE]
// completion four times.

/// A gateway that reports a *rejection* with a 200 and an `error` body. The
/// reason is in the body and nowhere else, so it has to be surfaced - and the
/// identical request must not be sent again.
#[tokio::test]
async fn a_200_with_a_json_error_body_is_not_retried_and_names_the_reason() {
    let failure = response(
        "200 OK",
        "application/json",
        "{\"error\":{\"message\":\"You exceeded your current quota, \
         please check your plan and billing details. (insufficient_quota)\",\
         \"type\":\"insufficient_quota\"}}",
    );
    let server = flaky_server(usize::MAX, failure).await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let err = stream
        .next()
        .await
        .expect("a terminal error item")
        .expect_err("a 200 carrying a rejection must fail the attempt");
    let text = err.to_string();
    assert!(
        text.contains("insufficient_quota"),
        "the upstream reason must reach the user: {text}"
    );
    assert!(text.contains("200"), "{text}");
    assert!(
        !text.contains("gave up after"),
        "a permanent failure must not report a retry budget: {text}"
    );
    assert_eq!(
        hit_count(&server),
        1,
        "a permanent rejection must be sent exactly once, not 51 times"
    );
    assert!(stream.next().await.is_none());
}

/// A 200 with no body is the endpoint answering "nothing", which it will answer
/// again. It used to be retried for the full attempt budget.
#[tokio::test]
async fn an_empty_body_is_not_retried() {
    let server = spawn(move |_req, mut socket| async move {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let _ = socket.shutdown().await;
    })
    .await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let err = stream
        .next()
        .await
        .expect("a terminal error item")
        .expect_err("an empty body must fail the attempt");
    assert!(err.to_string().contains("empty response body"), "{err}");
    assert!(
        !err.to_string().contains("gave up after"),
        "an empty body is a permanent answer, not a retry budget: {err}"
    );
    assert_eq!(hit_count(&server), 1, "an empty body must be sent once");
    assert!(stream.next().await.is_none());
}

/// A successful completion delivered only as `[DONE]` is a finished (empty)
/// answer. It used to be read as "the stream died before sending anything" and
/// regenerated four times for one turn.
#[tokio::test]
async fn a_done_only_completion_finishes_without_restarting() {
    let server = spawn(move |_req, mut socket| async move {
        write_sse(&mut socket, "data: [DONE]\n\n").await;
    })
    .await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    let mut seen = Vec::new();
    while let Some(item) = stream.next().await {
        seen.push(item.expect("a [DONE]-only completion is not an error"));
    }
    assert_eq!(
        seen,
        vec![ProviderChunk::Finish {
            reason: "stop".into()
        }],
        "the turn must finish with empty text, not restart"
    );
    assert_eq!(
        hit_count(&server),
        1,
        "[DONE] is a finished answer, not a reason to re-bill a generation"
    );
}

/// A `retryable: false` in-stream error must reach the runtime as a chunk with
/// the verdict intact. Raising it as a transport error made the runtime restart
/// the stream four times on a permanently rejected request.
#[tokio::test]
async fn a_non_retryable_stream_error_keeps_its_verdict() {
    let body = "data: {\"error\":{\"message\":\"model does not support tools\",\
                \"type\":\"invalid_request_error\"}}\n\n";
    let server = spawn(move |_req, mut socket| async move {
        write_sse(&mut socket, body).await;
    })
    .await;

    let mut stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    match stream.next().await.expect("a terminal item") {
        Ok(ProviderChunk::Error { message, retryable }) => {
            assert!(
                message.contains("model does not support tools"),
                "{message}"
            );
            assert!(
                !retryable,
                "a rejected request must not be marked retryable"
            );
        }
        other => panic!("the runtime decides restarts from this chunk, got {other:?}"),
    }
    assert_eq!(hit_count(&server), 1, "one request, one billed generation");
    assert!(stream.next().await.is_none());
}

/// The transient half of the same shape must still retry: a 200 + HTML splash
/// from a dying proxy is exactly what the retry layer exists for.
#[tokio::test]
async fn a_splash_page_still_retries_and_recovers() {
    let failure = response(
        "200 OK",
        "text/html",
        "<html><body>502 Bad Gateway</body></html>",
    );
    let server = flaky_server(2, failure).await;

    let stream = retrying(&server, 5)
        .stream_chat(chat_request(), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(texts(stream).await, "ok", "the retry must recover the turn");
    assert_eq!(hit_count(&server), 3, "two splashes then the winner");
}

#[tokio::test]
async fn mission_budget_is_enforced_at_each_physical_retry() {
    use kc_core::store::Store;
    let dir = std::env::temp_dir().join(format!("kc-retry-budget-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let store = Arc::new(
        kc_store::SqliteStore::open(dir.join("store.db"))
            .await
            .unwrap(),
    );
    store.create_project("p", "P", "/p").await.unwrap();
    store
        .create_mission("m", "p", "M", "", Some(2))
        .await
        .unwrap();
    store
        .create_mission_session("m", "r", "s", "p", "T", "/p", "prompt")
        .await
        .unwrap();
    let server = flaky_server(
        10,
        response("500 Internal Server Error", "text/plain", "boom"),
    )
    .await;
    let mut request = chat_request();
    request.admission = Some(kc_core::provider::AttemptAdmission {
        store: store.clone(),
        session_id: "s".into(),
    });
    let mut stream = retrying(&server, 5)
        .stream_chat(request, CancellationToken::new())
        .await
        .unwrap();
    let mut refused = false;
    while let Some(item) = stream.next().await {
        if let Err(error) = item {
            refused = error.to_string().contains("budget exhausted");
        }
    }
    assert!(refused);
    assert_eq!(hit_count(&server), 2);
    assert_eq!(
        store.mission_attempt_budget("m").await.unwrap(),
        Some((Some(2), 2))
    );
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn transient_http_status_wins_over_context_wording() {
    for (status, label) in [
        ("429 Too Many Requests", "429"),
        ("503 Service Unavailable", "503"),
    ] {
        let failure = response(
            status,
            "application/json",
            "{\"error\":{\"message\":\"upstream busy: too many tokens / context length\"}}",
        );
        let server = flaky_server(1, failure).await;
        let stream = retrying(&server, 2)
            .stream_chat(chat_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(texts(stream).await, "ok", "{label} was not retried");
        assert_eq!(hit_count(&server), 2, "{label} must retry exactly once");
    }
}
