//! Event emission helpers.
//!
//! The runtime owns a single broadcast channel. Every event is stamped with a
//! monotonically increasing `seq` and a wall-clock `ts_ms` and carries the
//! session/run/agent it belongs to, so consumers can render a single ordered
//! stream for all sessions. Lagged receivers are simply dropped by the channel;
//! they are expected to re-fetch state, not to be a source of truth.

use crate::RuntimeInner;
use kc_core::events::{Event, EventEnvelope, EventSink};
use kc_core::store::UsageRecord;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Clamped at 0 for pre-epoch clocks.
pub(crate) fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => 0,
    }
}

/// A short, human-readable "how long ago" for a timestamp in milliseconds.
///
/// Used where a model needs a sense of elapsed time rather than a clock reading:
/// a message that says "sent 4m ago" tells it whether the sender is still
/// waiting, while an epoch number tells it nothing it can act on.
pub fn relative_ago(ts_ms: i64) -> String {
    let delta = now_ms().saturating_sub(ts_ms);
    if delta < 0 {
        return "just now".to_string();
    }
    let seconds = delta / 1_000;
    match seconds {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

impl RuntimeInner {
    /// Stamp, publish and return one event.
    ///
    /// Sending never fails the run: with zero subscribers (or only lagged ones)
    /// `broadcast::send` reports an error which is deliberately ignored — losing
    /// a UI notification must not abort the agent loop.
    pub(crate) fn emit(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
        agent_id: Option<&str>,
        event: Event,
    ) -> EventEnvelope {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let envelope = EventEnvelope {
            seq,
            ts_ms: now_ms(),
            session_id: session_id.map(|s| s.to_string()),
            run_id: run_id.map(|s| s.to_string()),
            agent_id: agent_id.map(|s| s.to_string()),
            event,
        };
        self.events_emitted.fetch_add(1, Ordering::Relaxed);
        let _ = self.events.send(envelope.clone());
        envelope
    }

    pub(crate) fn event_sink(
        self: &Arc<Self>,
        session_id: String,
        run_id: String,
        agent_id: Option<String>,
    ) -> EventSink {
        let inner = Arc::clone(self);
        Arc::new(move |event: Event| {
            inner.emit(Some(&session_id), Some(&run_id), agent_id.as_deref(), event);
        })
    }
}

/// Cheap cloneable "who am I emitting for" bundle.
#[derive(Clone)]
pub(crate) struct EmitCtx {
    pub inner: Arc<RuntimeInner>,
    pub session_id: String,
    pub run_id: String,
    pub agent_id: Option<String>,
}

impl EmitCtx {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        session_id: String,
        run_id: String,
        agent_id: Option<String>,
    ) -> Self {
        Self {
            inner,
            session_id,
            run_id,
            agent_id,
        }
    }

    pub(crate) fn emit(&self, event: Event) {
        self.inner.emit(
            Some(&self.session_id),
            Some(&self.run_id),
            self.agent_id.as_deref(),
            event,
        );
    }

    /// Sink handed to tools so they can publish command output events.
    pub(crate) fn sink(&self) -> EventSink {
        self.inner.event_sink(
            self.session_id.clone(),
            self.run_id.clone(),
            self.agent_id.clone(),
        )
    }

    pub(crate) fn notice(&self, level: &str, message: impl Into<String>) {
        self.emit(Event::Notice {
            level: level.into(),
            message: message.into(),
        });
    }

    pub(crate) fn error(&self, kind: &str, message: impl Into<String>) {
        self.emit(Event::Error {
            message: message.into(),
            kind: Some(kind.into()),
        });
    }

    /// Persist one provider call's token counts for the session information view.
    ///
    /// The `usage` event is live-only, so without this row a reload cannot answer
    /// "what has this session cost, and how much of it was cached". Accounting is
    /// strictly secondary to the run: a store failure is a warning, never an
    /// error, because losing a run over a metric would be the wrong trade.
    pub(crate) async fn record_usage(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: Option<u32>,
    ) {
        let record = UsageRecord {
            session_id: self.session_id.clone(),
            run_id: self.run_id.clone(),
            agent_id: self.agent_id.clone(),
            input_tokens,
            output_tokens,
            cached_tokens,
        };
        if let Err(e) = self.inner.deps.store.record_usage(&record).await {
            tracing::warn!(
                session = %self.session_id,
                run = %self.run_id,
                error = %e,
                "could not record provider usage"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::test_inner;

    #[test]
    fn sequence_and_timestamp_increase_monotonically() {
        let inner = test_inner();
        let mut rx = inner.events.subscribe();
        let a = inner.emit(
            Some("s_1"),
            Some("r_1"),
            None,
            Event::RunCancelled {
                run_id: "r_1".into(),
            },
        );
        let b = inner.emit(
            Some("s_1"),
            Some("r_1"),
            Some("a_1"),
            Event::Notice {
                level: "info".into(),
                message: "hi".into(),
            },
        );
        assert!(b.seq > a.seq);
        assert!(b.ts_ms >= a.ts_ms);
        assert_eq!(a.session_id.as_deref(), Some("s_1"));
        assert_eq!(b.agent_id.as_deref(), Some("a_1"));
        assert_eq!(rx.try_recv().unwrap().seq, a.seq);
        assert_eq!(rx.try_recv().unwrap().seq, b.seq);
        assert_eq!(inner.events_emitted.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn emitting_without_subscribers_is_not_an_error() {
        let inner = test_inner();
        let env = inner.emit(
            None,
            None,
            None,
            Event::Notice {
                level: "info".into(),
                message: "nobody listening".into(),
            },
        );
        assert_eq!(env.seq, 1);
    }
}
