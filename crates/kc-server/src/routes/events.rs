//! `GET /api/events` — server-sent events.
//!
//! One broadcast receiver per client and one envelope per `data:` frame. The
//! runtime stamps `seq`/`ts_ms` and attaches the session/run/agent context, so
//! events keep their global ordering (session lifecycle events are published by
//! the server through `Runtime::emit` for the same reason).
//!
//! Backpressure: events are pushed into a bounded channel with `try_send`. A
//! client that stops reading fills the channel and is disconnected instead of
//! blocking (or slowing) the agent loop. `: keepalive` comments go out every
//! 15 seconds. Shutdown cancels the task so `axum::serve` can drain.

use crate::error::{ApiError, ApiResult};
use crate::routes::{query_str, QueryMap};
use crate::state::{now_ms, AppState};
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use kc_core::events::{Event, EventEnvelope};
use serde_json::json;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

/// How long between `: keepalive` comments. Must stay below the common 30s
/// reverse-proxy idle timeout.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Per-client buffer. A reader this far behind is effectively gone.
const CLIENT_BUFFER: usize = 256;

pub async fn stream(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Response> {
    let filter = query_str(&query, "session").map(|s| s.to_string());
    let runtime_events = state.runtime().subscribe();
    let shutdown = state.shutdown_token();

    let (tx, rx) = mpsc::channel::<Bytes>(CLIENT_BUFFER);
    tokio::spawn(pump(runtime_events, filter, shutdown, tx));

    let body_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<Bytes, Infallible>(chunk), rx))
    });

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONNECTION, "keep-alive")
        // Disable proxy buffering (nginx and friends).
        .header(
            header::HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )
        .body(Body::from_stream(body_stream))
        .map_err(|e| ApiError::internal(format!("could not build the event stream: {e}")))?;
    Ok(response)
}

async fn pump(
    mut runtime_rx: broadcast::Receiver<EventEnvelope>,
    filter: Option<String>,
    shutdown: tokio_util::sync::CancellationToken,
    tx: mpsc::Sender<Bytes>,
) {
    let mut last_seq: u64 = 0;
    let mut keepalive = tokio::time::interval_at(Instant::now() + KEEPALIVE, KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let chunk = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = tx.closed() => break,
            _ = keepalive.tick() => Bytes::from_static(b": keepalive\n\n"),
            received = runtime_rx.recv() => match received {
                Ok(envelope) => match encode(&envelope, &filter) {
                    Some(chunk) => {
                        last_seq = last_seq.max(envelope.seq);
                        chunk
                    }
                    None => continue,
                },
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "SSE client lagged; dropping events");
                    // Tell the UI to resynchronise. The synthetic envelope is
                    // stamped just after the last sequence this client saw so
                    // the frontend's dedupe accepts it.
                    last_seq = last_seq.wrapping_add(skipped).wrapping_add(1);
                    let notice = EventEnvelope {
                        seq: last_seq,
                        ts_ms: now_ms(),
                        session_id: filter.clone(),
                        run_id: None,
                        agent_id: None,
                        event: Event::Notice {
                            level: "warn".into(),
                            message:
                                "event stream lagged: some events were dropped. Refresh the session to resynchronise."
                                    .into(),
                        },
                    };
                    match encode(&notice, &None) {
                        Some(chunk) => chunk,
                        None => continue,
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        };

        // A slow client must not backpressure the run loop.
        if tx.try_send(chunk).is_err() {
            tracing::debug!("SSE client could not keep up; closing the stream");
            break;
        }
    }
}

/// Serialise one envelope as an SSE frame, applying the optional session filter.
fn encode(envelope: &EventEnvelope, filter: &Option<String>) -> Option<Bytes> {
    if let Some(session) = filter {
        if envelope.session_id.as_deref() != Some(session.as_str()) {
            return None;
        }
    }
    let mut value = serde_json::to_value(envelope).ok()?;
    // The flattened event enum already contains `type`; keep the envelope
    // fields in the documented order-independent object.
    if let Some(object) = value.as_object_mut() {
        object.insert("seq".into(), json!(envelope.seq));
        object.insert("ts_ms".into(), json!(envelope.ts_ms));
    }
    let text = serde_json::to_string(&value).ok()?;
    Some(Bytes::from(format!("data: {text}\n\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(session: Option<&str>, event: Event) -> EventEnvelope {
        EventEnvelope {
            seq: 7,
            ts_ms: 1,
            session_id: session.map(|s| s.to_string()),
            run_id: None,
            agent_id: None,
            event,
        }
    }

    #[test]
    fn encoding_keeps_the_envelope_seq_and_flattens_the_event() {
        let env = envelope(Some("s_1"), Event::TextDelta { delta: "hi".into() });
        let bytes = encode(&env, &None).unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.starts_with("data: {"));
        assert!(text.ends_with("\n\n"));
        let parsed: serde_json::Value =
            serde_json::from_str(text.trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(parsed["seq"], 7);
        assert_eq!(parsed["ts_ms"], 1);
        assert_eq!(parsed["type"], "text_delta");
        assert_eq!(parsed["delta"], "hi");
        assert_eq!(parsed["session_id"], "s_1");
    }

    #[test]
    fn session_filter_drops_other_sessions_and_session_less_events() {
        let other = envelope(
            Some("s_2"),
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&other, &Some("s_1".to_string())).is_none());
        let none = envelope(
            None,
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&none, &Some("s_1".to_string())).is_none());
        let matched = envelope(
            Some("s_1"),
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&matched, &Some("s_1".to_string())).is_some());
    }
}
