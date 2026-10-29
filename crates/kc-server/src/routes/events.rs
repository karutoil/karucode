//! `GET /api/events` — server-sent events.
//!
//! One broadcast receiver per client and one envelope per `data:` frame. The
//! runtime stamps `seq`/`ts_ms` and attaches the session/run/agent context, so
//! events keep their global ordering (session lifecycle events are published by
//! the server through `Runtime::emit` for the same reason).
//!
//! Backpressure: events are pushed into a bounded channel. A client that cannot
//! drain it within [`SLOW_CLIENT_GRACE`] is disconnected, but a momentary stall
//! (the browser re-rendering a burst of deltas, a slow proxy hop) is waited out
//! rather than turned into a dropped stream. The wait happens in this client's
//! own pump task, so it never blocks or slows the agent loop. Because the pump
//! stops consuming the runtime broadcast while it waits, a long stall degrades to
//! a [`Event::StreamLagged`](kc_core::events::Event::StreamLagged) marker telling
//! the client to refetch. `: keepalive` comments go out every 15 seconds, which is
//! how a client can tell a quiet stream from a dead one. Shutdown cancels the task
//! so `axum::serve` can drain.

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
/// Per-client buffer. A reader this far behind is effectively gone, but the
/// headroom absorbs a burst: one run can emit hundreds of deltas in a second
/// while the browser is busy rendering the ones it already has.
const CLIENT_BUFFER: usize = 1024;
/// How long the pump waits for a full buffer to drain before giving up on the
/// client. Long enough to survive a render hiccup, short enough that a dead
/// socket does not hold a task forever.
const SLOW_CLIENT_GRACE: Duration = Duration::from_secs(5);

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
                    // A `StreamLagged` marker rather than a synthetic `notice`: the
                    // client keys off the type, so it cannot be picked up by text
                    // matching, and it is exempt from the client's seq dedupe.
                    last_seq = lagged_seq(last_seq, skipped);
                    let marker = EventEnvelope {
                        seq: last_seq,
                        ts_ms: now_ms(),
                        session_id: filter.clone(),
                        run_id: None,
                        agent_id: None,
                        event: Event::StreamLagged { skipped },
                    };
                    match encode(&marker, &None) {
                        Some(chunk) => chunk,
                        None => continue,
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        };

        // A slow client must not backpressure the run loop: the pump is per
        // client, so waiting here only delays this one stream. Waiting beats
        // disconnecting, because a dropped stream loses the rest of the run.
        match tokio::time::timeout(SLOW_CLIENT_GRACE, tx.send(chunk)).await {
            Ok(Ok(())) => {}
            // The body stream was dropped: nobody is reading any more.
            Ok(Err(_)) => break,
            Err(_) => {
                tracing::debug!(
                    grace_secs = SLOW_CLIENT_GRACE.as_secs(),
                    "SSE client stopped draining; closing the stream"
                );
                break;
            }
        }
    }
}

/// The `seq` for a `StreamLagged` marker.
///
/// `last_seq` is the newest frame this client received and `skipped` how many the
/// broadcast ring dropped, so the next live frame carries `last_seq + skipped + 1`.
/// Stamping the marker with the last *dropped* seq keeps it newer than anything the
/// client has seen while staying strictly below the next live frame: a marker
/// stamped one higher would make the client discard that frame as a duplicate of
/// itself, silently losing the event that unblocks it.
fn lagged_seq(last_seq: u64, skipped: u64) -> u64 {
    last_seq.saturating_add(skipped)
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
    fn a_lag_marker_never_takes_the_next_live_sequence() {
        // Client saw seq 10; frames 11..=13 were dropped, so the next live frame is 14.
        let marker = lagged_seq(10, 3);
        assert_eq!(marker, 13);
        assert!(
            marker > 10,
            "the client must accept the marker over last seen"
        );
        assert!(marker < 14, "and must not shadow the next live envelope");

        // A second lag after the marker: last seen is 13, one frame dropped.
        let second = lagged_seq(marker, 1);
        assert_eq!(second, 14);
        assert!(second > marker, "a later marker still has to be accepted");
        assert!(
            second < 15,
            "and still must not shadow the next live envelope"
        );
    }

    #[test]
    fn lagged_seq_saturates_instead_of_wrapping() {
        assert_eq!(lagged_seq(u64::MAX - 1, 5), u64::MAX);
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
