//! `GET /api/events` — server-sent events.
//!
//! One broadcast receiver per client and one envelope per `data:` frame. The
//! runtime stamps `seq`/`ts_ms` and attaches the session/run/agent context, so
//! events keep their global ordering (session lifecycle events are published by
//! the server through `Runtime::emit` for the same reason).
//!
//! # Resuming a stream
//!
//! Every frame carries `id: {seq}` before its `data:` line, so a client that
//! reconnects can send `Last-Event-ID` and be handed the frames it missed instead
//! of refetching the session. Refetching mid-stream is not a safe substitute: the
//! snapshot only contains persisted messages, so it deletes output that is still
//! streaming.
//!
//! The replay comes from a globally byte- and count-bounded ring per runtime.
//! A recorder task per runtime fills it, so frames are kept even while no client
//! is connected - which is exactly the window a reconnect has to cover. When the
//! requested id is older than the ring, the gap cannot be filled from memory, so
//! the stream says so with a `resync` frame rather than silently skipping: the
//! client then refetches once, deliberately.
//!
//! # Backpressure and limits
//!
//! Events are pushed into a bounded channel. A client that cannot drain it within
//! [`SLOW_CLIENT_GRACE`] is disconnected, but a momentary stall (the browser
//! re-rendering a burst of deltas, a slow proxy hop) is waited out rather than
//! turned into a dropped stream. The wait happens in this client's own pump task,
//! so it never blocks or slows the agent loop. Because the pump stops consuming
//! the runtime broadcast while it waits, a long stall degrades to a
//! [`Event::StreamLagged`](kc_core::events::Event::StreamLagged) marker telling
//! the client to refetch.
//!
//! Two hard limits keep one client from costing unbounded memory:
//!
//! * [`MAX_SSE_CONNECTIONS`] concurrent streams. Beyond that a new connection is
//!   refused with an error frame instead of being accepted and starved.
//! * [`MAX_QUEUED_BYTES`] of queued frames per connection. The queue is bounded by
//!   *bytes*, not slots: one `tool_call_finished` frame can carry 24 000 bytes and
//!   `command_output` up to 32 000, so 1024 slots was ~32 MB of worst case. A
//!   client that exceeds the byte budget is dropped with a `notice` frame.
//!
//! `: keepalive` comments go out every 15 seconds, which is how a client can tell
//! a quiet stream from a dead one. Shutdown cancels the task so `axum::serve` can
//! drain.

use crate::error::{ApiError, ApiResult};
use crate::metrics::{self, MAX_QUEUED_BYTES, MAX_SSE_CONNECTIONS};
use crate::routes::{query_str, QueryMap};
use crate::state::{now_ms, AppState};
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use kc_core::events::{Event, EventEnvelope};
use kc_runtime::Runtime;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

/// How long between `: keepalive` comments. Must stay below the common 30s
/// reverse-proxy idle timeout.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Per-client buffer, in slots. A reader this far behind is effectively gone, but
/// the headroom absorbs a burst: one run can emit hundreds of deltas in a second
/// while the browser is busy rendering the ones it already has. The byte budget
/// ([`MAX_QUEUED_BYTES`]) is the limit that actually bounds memory.
const CLIENT_BUFFER: usize = 1024;
/// How long the pump waits for a full buffer to drain before giving up on the
/// client. Long enough to survive a render hiccup, short enough that a dead
/// socket does not hold a task forever.
const SLOW_CLIENT_GRACE: Duration = Duration::from_secs(5);
/// Maximum retained envelopes and encoded bytes per runtime.
const RING_CAPACITY: usize = 1000;
const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;

pub async fn stream(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let filter = query_str(&query, "session").map(|s| s.to_string());
    let resume_from = last_event_id(&headers, &query);

    // Refuse before doing any work: a connection that cannot be served must not
    // take a slot, a task or a queue.
    let Some(guard) = metrics::try_open_sse_connection() else {
        tracing::warn!(
            limit = MAX_SSE_CONNECTIONS,
            "refused an event stream: the concurrent-connection cap is reached"
        );
        return Ok(refusal_response());
    };

    let runtime_events = state.runtime().subscribe();
    let rings = ensure_recorder(state.runtime());
    let replay = rings.replay(filter.as_deref(), resume_from);
    let shutdown = state.shutdown_token();

    let queued = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<Bytes>(CLIENT_BUFFER);
    tokio::spawn(pump(
        runtime_events,
        filter,
        shutdown,
        tx,
        replay,
        Arc::clone(&queued),
        guard,
    ));

    // The byte budget is released as the body stream hands each chunk to the
    // socket, so `queued` tracks what is actually still waiting.
    let body_stream = futures_util::stream::unfold((rx, queued), |(mut rx, queued)| async move {
        rx.recv().await.map(|chunk| {
            queued.fetch_sub(chunk.len(), Ordering::AcqRel);
            (Ok::<Bytes, Infallible>(chunk), (rx, queued))
        })
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

/// The `Last-Event-ID` the client asked to resume from.
///
/// The header is the standard mechanism; `?last_event_id=` is accepted as well
/// because a `ReadableStream` fetch cannot always set headers (the same reason
/// `?token=` exists). A value that is not a number is ignored rather than
/// rejected: resuming is an optimisation, and a bad cursor must not break the
/// stream.
fn last_event_id(headers: &HeaderMap, query: &QueryMap) -> Option<u64> {
    let raw = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| query_str(query, "last_event_id").map(str::to_string))?;
    match raw.trim().parse::<u64>() {
        Ok(seq) => Some(seq),
        Err(_) => {
            tracing::debug!("ignoring a Last-Event-ID that is not a sequence number");
            None
        }
    }
}

/// The answer when [`MAX_SSE_CONNECTIONS`] is already reached.
///
/// A single error frame on a normal event stream, not a `503`: the client already
/// knows how to read frames and how to back off, and a bare status code would
/// surface as an opaque "stream failed" banner.
fn refusal_response() -> Response {
    // A fresh connection has seen nothing, so any seq is newer than its watermark.
    let frame = notice_frame(
        "error",
        &format!(
            "too many concurrent event streams (limit {MAX_SSE_CONNECTIONS}); \
             close another tab or client and reconnect"
        ),
        None,
        1,
    );
    let stream = futures_util::stream::once(async move { Ok::<Bytes, Infallible>(frame) });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

#[allow(clippy::too_many_arguments)]
async fn pump(
    mut runtime_rx: broadcast::Receiver<EventEnvelope>,
    filter: Option<String>,
    shutdown: tokio_util::sync::CancellationToken,
    tx: mpsc::Sender<Bytes>,
    replay: Replay,
    queued: Arc<AtomicUsize>,
    // Held for the lifetime of the stream so the gauge is released on every exit
    // path, including a panic in this task.
    _guard: metrics::SseConnectionGuard,
) {
    // Replay first, then live. The receiver was subscribed *before* the ring was
    // read, so a frame published in between is delivered live and skipped here by
    // the `seq <= last_seq` check - no gap, no duplicate.
    let mut last_seq: u64 = 0;
    if let Some(resync) = replay.resync {
        let frame = encode_resync(&resync, filter.as_deref());
        if !send_frame(&tx, &queued, frame, last_seq).await {
            return;
        }
        last_seq = resync.newest;
    }
    for envelope in replay.frames {
        let chunk = envelope.frame.clone();
        if !send_frame(&tx, &queued, chunk, last_seq).await {
            return;
        }
        last_seq = last_seq.max(envelope.seq);
    }

    let mut keepalive = tokio::time::interval_at(Instant::now() + KEEPALIVE, KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let chunk = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = tx.closed() => break,
            _ = keepalive.tick() => Bytes::from_static(b": keepalive\n\n"),
            received = runtime_rx.recv() => match received {
                Ok(envelope) => {
                    // Already delivered by the replay above.
                    if envelope.seq <= last_seq {
                        continue;
                    }
                    match encode(&envelope, &filter) {
                        Some(chunk) => {
                            last_seq = last_seq.max(envelope.seq);
                            chunk
                        }
                        None => continue,
                    }
                }
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

        if !send_frame(&tx, &queued, chunk, last_seq).await {
            break;
        }
    }
}

/// Queue one frame, honouring the per-connection byte budget.
///
/// Returns `false` when the stream must end: the client is gone, it stopped
/// draining for [`SLOW_CLIENT_GRACE`], or it has more than [`MAX_QUEUED_BYTES`]
/// waiting. The last case sends a `notice` first (best effort - the queue is full
/// by definition) so the disconnect is visible rather than a silent truncation.
async fn send_frame(
    tx: &mpsc::Sender<Bytes>,
    queued: &AtomicUsize,
    chunk: Bytes,
    last_seq: u64,
) -> bool {
    if exceeds_queue_limit(queued.load(Ordering::Acquire), chunk.len()) {
        tracing::warn!(
            limit = MAX_QUEUED_BYTES,
            "SSE client exceeded its queue budget; closing the stream"
        );
        // Stamped one past the newest frame this client has seen. A `seq` of 0
        // would be discarded by the client's dedupe (`seq <= lastSeq`) and the
        // disconnect would look like a silent truncation. This is the last frame
        // the connection will ever send, so it cannot shadow a live one.
        let notice = notice_frame(
            "error",
            &format!(
                "this event stream fell more than {MAX_QUEUED_BYTES} bytes behind and was \
                 closed; reconnect with Last-Event-ID to resume"
            ),
            None,
            last_seq.saturating_add(1),
        );
        let _ = tx.try_send(notice);
        return false;
    }

    queued.fetch_add(chunk.len(), Ordering::AcqRel);
    // A slow client must not backpressure the run loop: the pump is per client, so
    // waiting here only delays this one stream. Waiting beats disconnecting,
    // because a dropped stream loses the rest of the run.
    match tokio::time::timeout(SLOW_CLIENT_GRACE, tx.send(chunk)).await {
        Ok(Ok(())) => true,
        // The body stream was dropped: nobody is reading any more.
        Ok(Err(_)) => false,
        Err(_) => {
            tracing::debug!(
                grace_secs = SLOW_CLIENT_GRACE.as_secs(),
                "SSE client stopped draining; closing the stream"
            );
            false
        }
    }
}

/// True when queueing `incoming` more bytes would exceed the budget.
fn exceeds_queue_limit(queued: usize, incoming: usize) -> bool {
    queued.saturating_add(incoming) > MAX_QUEUED_BYTES
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
///
/// The `id:` line is what makes `Last-Event-ID` resume possible; it carries the
/// same `seq` the JSON body does, so the two can never disagree.
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
    Some(frame(envelope.seq, &text))
}

/// One SSE frame: `id:` then `data:` then a blank line.
fn frame(seq: u64, text: &str) -> Bytes {
    Bytes::from(format!("id: {seq}\ndata: {text}\n\n"))
}

/// A `notice` frame, used for the connection-cap and queue-budget messages.
///
/// `seq` is explicit because the client drops any frame whose `seq` is not newer
/// than the last one it saw: a synthetic frame stamped 0 would never be shown.
fn notice_frame(level: &str, message: &str, session: Option<&str>, seq: u64) -> Bytes {
    let envelope = EventEnvelope {
        seq,
        ts_ms: now_ms(),
        session_id: session.map(str::to_string),
        run_id: None,
        agent_id: None,
        event: Event::Notice {
            level: level.into(),
            message: message.into(),
        },
    };
    encode(&envelope, &None).unwrap_or_else(|| Bytes::from_static(b": notice\n\n"))
}

/// The `resync` frame: the requested cursor is older than the ring, so the gap
/// cannot be replayed from memory.
///
/// Additive to the documented event shape - same envelope fields, a `type` the
/// client does not know yet (and therefore ignores) plus the numbers that explain
/// the gap. `seq` is the newest frame the ring holds, so the client's watermark
/// moves forward and the live frames that follow are not treated as duplicates.
fn encode_resync(resync: &Resync, session: Option<&str>) -> Bytes {
    let value = json!({
        "seq": resync.newest,
        "ts_ms": now_ms(),
        "session_id": session,
        "run_id": serde_json::Value::Null,
        "agent_id": serde_json::Value::Null,
        "type": "resync",
        "requested_id": resync.requested,
        "oldest_available": resync.oldest,
        "reason": "the requested Last-Event-ID is older than the replay buffer",
    });
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    frame(resync.newest, &text)
}

// ---------------------------------------------------------------------------
// Replay ring
// ---------------------------------------------------------------------------

/// What a reconnect can be given.
struct Replay {
    frames: Vec<ReplayFrame>,
    resync: Option<Resync>,
}

struct Resync {
    requested: u64,
    oldest: u64,
    newest: u64,
}

/// A single FIFO bounds the whole runtime, including session metadata. Retaining
/// serialized frames avoids keeping arbitrarily large event payloads unaccounted.
#[derive(Default)]
struct SessionRings {
    inner: Mutex<RingsInner>,
}

#[derive(Clone)]
struct ReplayFrame {
    seq: u64,
    session: Option<String>,
    frame: Bytes,
    size: usize,
}

#[derive(Default)]
struct RingsInner {
    frames: VecDeque<ReplayFrame>,
    bytes: usize,
    /// Highest sequence that may have been lost, for any session. A global
    /// watermark is deliberately conservative for filtered streams: remembering
    /// a watermark per evicted session would itself grow without bound.
    lost_through: u64,
    newest: u64,
    lagged: bool,
}

impl SessionRings {
    fn push(&self, envelope: EventEnvelope) {
        let seq = envelope.seq;
        let frame = encode(&envelope, &None);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.lagged {
            inner.lost_through = inner.lost_through.max(seq.saturating_sub(1));
            inner.lagged = false;
        }
        inner.newest = inner.newest.max(seq);
        let Some(frame) = frame else {
            inner.lost_through = inner.lost_through.max(seq);
            return;
        };
        let session = envelope.session_id;
        // Include the duplicated session string and fixed allocation overhead in
        // the budget, not just the wire payload.
        let size = frame
            .len()
            .saturating_add(session.as_ref().map_or(0, |s| s.capacity()))
            .saturating_add(std::mem::size_of::<ReplayFrame>());
        if size > MAX_REPLAY_BYTES {
            inner.lost_through = inner.lost_through.max(seq);
            return;
        }
        while inner.frames.len() >= RING_CAPACITY || inner.bytes > MAX_REPLAY_BYTES - size {
            let old = inner.frames.pop_front().expect("nonempty replay ring");
            inner.bytes -= old.size;
            inner.lost_through = inner.lost_through.max(old.seq);
        }
        inner.bytes += size;
        inner.frames.push_back(ReplayFrame {
            seq,
            session,
            frame,
            size,
        });
    }

    fn mark_lagged(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).lagged = true;
    }

    /// Missing data triggers resync, including gaps from global eviction or a
    /// lagging recorder. A fresh connection never requests historical data.
    fn replay(&self, filter: Option<&str>, after: Option<u64>) -> Replay {
        let Some(after) = after else {
            return Replay {
                frames: Vec::new(),
                resync: None,
            };
        };
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Until the first surviving event after a lag arrives, its upper bound
        // is unknown, even when the cursor is ahead of the recorder.
        if after < inner.lost_through || inner.lagged {
            return Replay {
                frames: Vec::new(),
                resync: Some(Resync {
                    requested: after,
                    oldest: inner
                        .frames
                        .front()
                        .map_or(inner.newest.saturating_add(1), |e| e.seq),
                    newest: inner.newest.max(after),
                }),
            };
        }
        let frames = inner
            .frames
            .iter()
            .filter(|e| e.seq > after && filter.is_none_or(|s| e.session.as_deref() == Some(s)))
            .cloned()
            .collect();
        Replay {
            frames,
            resync: None,
        }
    }
}

/// One ring set per runtime, keyed by the runtime's address.
///
/// Keyed rather than a single global so two runtimes in one process (the test
/// harness builds one per test) never replay each other's events. The entry holds
/// a [`Weak`] to the runtime as well as the address: an address alone is not an
/// identity, because the allocator reuses a freed runtime's address for the next
/// one, and that new runtime would then inherit the dead one's ring - replaying
/// stale frames to a client whose `Last-Event-ID` happened to match.
fn rings() -> &'static Mutex<HashMap<usize, RecorderEntry>> {
    static RINGS: OnceLock<Mutex<HashMap<usize, RecorderEntry>>> = OnceLock::new();
    RINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct RecorderEntry {
    runtime: Weak<Runtime>,
    rings: Arc<SessionRings>,
}

/// The ring set for `runtime`, starting its recorder the first time it is asked
/// for.
///
/// The recorder is what makes resume work across a disconnect: it keeps consuming
/// the broadcast while no client is attached, so the frames published during the
/// gap are in the ring when the client comes back.
fn ensure_recorder(runtime: &Arc<Runtime>) -> Arc<SessionRings> {
    let key = Arc::as_ptr(runtime) as usize;
    let mut map = rings().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = map.get(&key) {
        // Only reuse the entry if it belongs to *this* runtime, not to a previous
        // one that happened to be allocated at the same address.
        if entry.runtime.upgrade().is_some() {
            return Arc::clone(&entry.rings);
        }
    }
    // Drop entries whose runtime is gone, so the map does not grow by one per
    // runtime for the life of the process.
    map.retain(|_, entry| entry.runtime.upgrade().is_some());

    let rings = Arc::new(SessionRings::default());
    map.insert(
        key,
        RecorderEntry {
            runtime: Arc::downgrade(runtime),
            rings: Arc::clone(&rings),
        },
    );
    drop(map);

    let mut receiver = runtime.subscribe();
    let recorder = Arc::clone(&rings);
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(envelope) => recorder.push(envelope),
                // The recorder fell behind the broadcast ring. Those frames are
                // gone; the next client that needs them gets a `resync`.
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "event replay recorder lagged");
                    recorder.mark_lagged();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    rings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64, session: Option<&str>, event: Event) -> EventEnvelope {
        EventEnvelope {
            seq,
            ts_ms: 1,
            session_id: session.map(|s| s.to_string()),
            run_id: None,
            agent_id: None,
            event,
        }
    }

    fn notice(seq: u64, session: Option<&str>) -> EventEnvelope {
        envelope(
            seq,
            session,
            Event::Notice {
                level: "info".into(),
                message: format!("event {seq}"),
            },
        )
    }

    #[test]
    fn encoding_keeps_the_envelope_seq_and_flattens_the_event() {
        let env = envelope(7, Some("s_1"), Event::TextDelta { delta: "hi".into() });
        let bytes = encode(&env, &None).unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        // The `id:` line is what `Last-Event-ID` resume keys off.
        assert!(text.starts_with("id: 7\ndata: {"), "frame was: {text}");
        assert!(text.ends_with("\n\n"));
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("data line");
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed["seq"], 7);
        assert_eq!(parsed["ts_ms"], 1);
        assert_eq!(parsed["type"], "text_delta");
        assert_eq!(parsed["delta"], "hi");
        assert_eq!(parsed["session_id"], "s_1");
    }

    #[test]
    fn every_frame_carries_an_id_matching_its_seq() {
        for seq in [1u64, 42, 9999] {
            let bytes = encode(&notice(seq, None), &None).unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(
                text.starts_with(&format!("id: {seq}\n")),
                "frame for seq {seq} was: {text}"
            );
            let data = text
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("data line");
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
            assert_eq!(parsed["seq"], seq, "id and body disagree: {text}");
        }
    }

    #[test]
    fn a_reconnect_replays_only_the_frames_after_the_cursor() {
        let rings = SessionRings::default();
        for seq in 1..=5 {
            rings.push(notice(seq, Some("s_1")));
        }
        let replay = rings.replay(Some("s_1"), Some(3));
        assert!(replay.resync.is_none(), "a clean resume must not resync");
        let seqs: Vec<u64> = replay.frames.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![4, 5], "replayed the wrong frames");
    }

    #[test]
    fn a_resume_at_the_newest_frame_replays_nothing() {
        let rings = SessionRings::default();
        for seq in 1..=3 {
            rings.push(notice(seq, Some("s_1")));
        }
        let replay = rings.replay(Some("s_1"), Some(3));
        assert!(replay.frames.is_empty());
        assert!(replay.resync.is_none());
    }

    #[test]
    fn a_cursor_older_than_the_ring_produces_a_resync() {
        let rings = SessionRings::default();
        // Fill past the ring so seq 1..=10 are evicted.
        for seq in 1..=(RING_CAPACITY as u64 + 10) {
            rings.push(notice(seq, Some("s_1")));
        }
        let replay = rings.replay(Some("s_1"), Some(1));
        let resync = replay
            .resync
            .expect("a cursor older than the ring must resync");
        assert!(replay.frames.is_empty(), "a resync must not also replay");
        assert_eq!(resync.requested, 1);
        assert_eq!(resync.oldest, 11, "the ring should start at seq 11");
        assert_eq!(resync.newest, RING_CAPACITY as u64 + 10);

        // The frame is additive: same envelope fields, a `type` the client ignores.
        let text = String::from_utf8(encode_resync(&resync, Some("s_1")).to_vec()).unwrap();
        assert!(text.starts_with(&format!("id: {}\n", resync.newest)));
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("data line");
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed["type"], "resync");
        assert_eq!(parsed["requested_id"], 1);
        assert_eq!(parsed["oldest_available"], 11);
        assert_eq!(parsed["session_id"], "s_1");
    }

    #[test]
    fn the_ring_is_bounded_per_session() {
        let rings = SessionRings::default();
        for seq in 1..=(RING_CAPACITY as u64 * 2) {
            rings.push(notice(seq, Some("s_1")));
        }
        let inner = rings.inner.lock().unwrap();
        assert_eq!(inner.frames.len(), RING_CAPACITY);
        assert_eq!(
            inner.frames.front().map(|e| e.seq),
            Some(RING_CAPACITY as u64 + 1),
            "the oldest frames should have been evicted"
        );
    }

    #[test]
    fn a_session_filtered_replay_never_leaks_another_session() {
        let rings = SessionRings::default();
        rings.push(notice(1, Some("s_1")));
        rings.push(notice(2, Some("s_2")));
        rings.push(notice(3, Some("s_1")));
        rings.push(notice(4, None));

        let replay = rings.replay(Some("s_1"), Some(0));
        let seqs: Vec<u64> = replay.frames.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 3], "replay leaked another session");

        // An unfiltered stream sees everything, in seq order.
        let all = rings.replay(None, Some(0));
        let seqs: Vec<u64> = all.frames.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
    }

    #[test]
    fn an_empty_ring_replays_nothing_and_does_not_resync() {
        let rings = SessionRings::default();
        let replay = rings.replay(Some("s_1"), Some(5));
        assert!(replay.frames.is_empty());
        assert!(
            replay.resync.is_none(),
            "a first connection must not be told to resync"
        );
    }

    /// A connection with no `Last-Event-ID` is fresh, not a cursor at zero. Once
    /// the ring has evicted its first frames, treating it as `Some(0)` would tell
    /// every new client to resync for no reason.
    #[test]
    fn a_fresh_connection_never_resyncs_even_after_the_ring_evicted_frames() {
        let rings = SessionRings::default();
        for seq in 1..=(RING_CAPACITY as u64 + 10) {
            rings.push(notice(seq, Some("s_1")));
        }
        let replay = rings.replay(Some("s_1"), None);
        assert!(replay.frames.is_empty(), "a fresh stream replays nothing");
        assert!(
            replay.resync.is_none(),
            "a fresh stream must not be told to resync"
        );
        // The same ring with an explicit cursor of 0 *is* a gap.
        assert!(rings.replay(Some("s_1"), Some(0)).resync.is_some());
    }

    #[test]
    fn replay_budget_is_global_and_eviction_resyncs_filtered_clients() {
        let rings = SessionRings::default();
        for seq in 1..=12 {
            rings.push(envelope(
                seq,
                Some(&format!("session-{seq}")),
                Event::TextDelta {
                    delta: "x".repeat(1024 * 1024),
                },
            ));
        }
        let inner = rings.inner.lock().unwrap();
        assert!(inner.bytes <= MAX_REPLAY_BYTES);
        assert!(inner.frames.len() < 12);
        drop(inner);
        assert!(rings.replay(None, Some(0)).resync.is_some());
        assert!(rings.replay(Some("session-1"), Some(0)).resync.is_some());
        assert!(rings.replay(Some("session-1"), None).resync.is_none());
    }

    #[test]
    fn oversized_frame_and_recorder_lag_force_resync() {
        let rings = SessionRings::default();
        rings.push(envelope(
            1,
            Some("a"),
            Event::TextDelta {
                delta: "x".repeat(MAX_REPLAY_BYTES),
            },
        ));
        assert!(rings.replay(Some("a"), Some(0)).resync.is_some());
        assert!(rings.inner.lock().unwrap().frames.is_empty());
        rings.push(notice(2, Some("b")));
        rings.mark_lagged();
        assert!(rings.replay(None, Some(2)).resync.is_some());
        assert!(
            rings.replay(None, Some(3)).resync.is_some(),
            "lag upper bound is not known yet"
        );
        rings.push(notice(5, Some("b")));
        assert!(rings.replay(None, Some(2)).resync.is_some());
        assert!(rings.replay(None, Some(4)).resync.is_none());
    }

    #[test]
    fn the_queue_budget_is_measured_in_bytes() {
        // A single 32 000-byte `command_output` frame is fine...
        assert!(!exceeds_queue_limit(0, 32_000));
        // ...but 1024 of them is not, which is what the slot-only bound allowed.
        assert!(exceeds_queue_limit(1023 * 32_000, 32_000));
        assert!(exceeds_queue_limit(MAX_QUEUED_BYTES, 1));
        assert!(!exceeds_queue_limit(MAX_QUEUED_BYTES - 1, 1));
        // Saturating, so a bogus counter cannot wrap into "under budget".
        assert!(exceeds_queue_limit(usize::MAX, 1));
    }

    #[test]
    fn the_connection_cap_refuses_past_the_limit_and_releases_on_drop() {
        // A leaked registry keeps this test off the process-wide one.
        let metrics: &'static metrics::Metrics = Box::leak(Box::new(metrics::Metrics::default()));
        let mut guards = Vec::new();
        for _ in 0..MAX_SSE_CONNECTIONS {
            guards.push(
                metrics
                    .try_open_sse()
                    .expect("a slot below the cap must be granted"),
            );
        }
        assert!(
            metrics.try_open_sse().is_none(),
            "the cap was not enforced at {MAX_SSE_CONNECTIONS}"
        );
        // Dropping one frees exactly one slot.
        guards.pop();
        assert!(metrics.try_open_sse().is_some());
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
            7,
            Some("s_2"),
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&other, &Some("s_1".to_string())).is_none());
        let none = envelope(
            7,
            None,
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&none, &Some("s_1".to_string())).is_none());
        let matched = envelope(
            7,
            Some("s_1"),
            Event::Notice {
                level: "info".into(),
                message: "x".into(),
            },
        );
        assert!(encode(&matched, &Some("s_1".to_string())).is_some());
    }

    /// A consumer that never drains must be dropped once its queue passes the byte
    /// budget, not buffered indefinitely. This is the regression the slot-only
    /// bound allowed: 1024 slots x 32 000 bytes is ~32 MB per stalled client.
    #[tokio::test]
    async fn a_slow_consumer_is_dropped_instead_of_buffering_without_bound() {
        // A channel big enough that the *slot* limit is never what stops us, so
        // only the byte budget can end the stream.
        let (tx, mut rx) = mpsc::channel::<Bytes>(CLIENT_BUFFER);
        let queued = AtomicUsize::new(0);
        let frame = Bytes::from(vec![b'x'; 32_000]);

        // Push frames without ever reading: the queue grows until the budget is
        // exceeded, then `send_frame` reports that the stream must end.
        // `last_seq` is the newest frame the client has actually received, which
        // is what `pump` passes in.
        let mut last_seq = 0u64;
        let mut sent = 0usize;
        let mut dropped = false;
        for seq in 1..=(MAX_QUEUED_BYTES / frame.len() + 10) {
            if !send_frame(&tx, &queued, frame.clone(), last_seq).await {
                dropped = true;
                break;
            }
            last_seq = seq as u64;
            sent += 1;
        }
        assert!(dropped, "a stalled consumer was never dropped");
        assert!(
            queued.load(Ordering::Acquire) <= MAX_QUEUED_BYTES,
            "the queue exceeded its byte budget: {} bytes",
            queued.load(Ordering::Acquire)
        );
        // The budget, not the slot count, is what stopped it.
        assert!(
            sent < CLIENT_BUFFER,
            "the slot limit ended the stream before the byte budget did"
        );

        // The final frame is the visible `notice`, stamped one past the newest
        // frame the client received so its dedupe does not swallow it.
        let mut last = None;
        while let Ok(chunk) = rx.try_recv() {
            last = Some(chunk);
        }
        let text = String::from_utf8(last.expect("no frame was queued").to_vec()).unwrap();
        assert!(text.contains("\"type\":\"notice\""), "frame was: {text}");
        assert!(text.contains("\"level\":\"error\""), "frame was: {text}");
        assert!(
            text.starts_with(&format!("id: {}\n", last_seq + 1)),
            "the notice must be newer than the last frame received: {text}"
        );
    }

    /// A consumer that keeps draining is never dropped, however many frames pass
    /// through: the budget is about what is *queued*, not what was sent.
    ///
    /// The drain is inline rather than a spawned task so the test is
    /// deterministic: a task that has not been scheduled yet would let the queue
    /// grow and the assertion would race the scheduler.
    #[tokio::test]
    async fn a_draining_consumer_is_never_dropped() {
        let (tx, mut rx) = mpsc::channel::<Bytes>(CLIENT_BUFFER);
        let queued = AtomicUsize::new(0);
        let frame = Bytes::from(vec![b'x'; 32_000]);
        let mut total = 0usize;

        for seq in 1..=(MAX_QUEUED_BYTES / frame.len() * 2) {
            assert!(
                send_frame(&tx, &queued, frame.clone(), seq as u64).await,
                "a draining consumer was dropped at frame {seq}"
            );
            // Model the body stream: each chunk handed to the socket releases its
            // bytes from the budget.
            while let Ok(chunk) = rx.try_recv() {
                queued.fetch_sub(chunk.len(), Ordering::AcqRel);
                total += chunk.len();
            }
        }
        assert!(
            total > MAX_QUEUED_BYTES,
            "the test did not move enough bytes: {total}"
        );
        assert_eq!(
            queued.load(Ordering::Acquire),
            0,
            "a fully drained queue must report zero queued bytes"
        );
    }

    #[test]
    fn last_event_id_is_read_from_the_header_and_the_query() {
        let mut headers = HeaderMap::new();
        assert_eq!(last_event_id(&headers, &QueryMap::new()), None);
        headers.insert("last-event-id", "42".parse().unwrap());
        assert_eq!(last_event_id(&headers, &QueryMap::new()), Some(42));

        // A non-numeric cursor is ignored, not fatal.
        headers.insert("last-event-id", "not-a-number".parse().unwrap());
        assert_eq!(last_event_id(&headers, &QueryMap::new()), None);

        // The query fallback, for clients that cannot set headers.
        let mut query = QueryMap::new();
        query.insert("last_event_id".into(), "7".into());
        assert_eq!(last_event_id(&HeaderMap::new(), &query), Some(7));
    }
}
