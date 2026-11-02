use crate::approval::{ApprovalDecision, ApprovalRequest};
use crate::config::SessionModelSelection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Semantic events emitted by the runtime and forwarded to the browser over SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionCreated {
        session_id: String,
        title: String,
    },
    SessionRenamed {
        session_id: String,
        title: String,
    },
    /// A session's per-session model selection was set or cleared. `model` is
    /// `null` when the session went back to the install-wide default.
    SessionModelChanged {
        session_id: String,
        model: Option<SessionModelSelection>,
    },
    SessionDeleted {
        session_id: String,
    },
    /// A session's transcript was rewound: every message after `message_id` is
    /// hidden. Clients re-render the transcript rather than splicing it, because
    /// a rewind can remove an assistant turn *and* the tool results inside it.
    SessionRewound {
        session_id: String,
        /// The message the user picked (an assistant turn is normalised to the
        /// prompt that produced it by the time this is emitted, so this is the
        /// prompt's id).
        message_id: i64,
        /// How many messages the rewind hid. `0` means the rewind was a no-op -
        /// a retry of the newest prompt - which is worth telling apart from a
        /// deliberate "hide nothing".
        messages_removed: usize,
    },
    /// A steering message the user sent mid-run has been taken into the
    /// conversation. Emitted by the loop at the moment the message joins the
    /// request, so the transcript can show it where the model saw it rather than
    /// where the client optimistically rendered it.
    SteerApplied {
        /// The id `POST /api/sessions/:id/messages?mode=steer` returned.
        steer_id: String,
        content: String,
    },
    /// One of a session's queued messages was added or removed.
    ///
    /// Emitted on every mutation - accepted, recalled, drained into the
    /// conversation, taken as a follow-up, or dropped with the session - and it
    /// carries the *whole* queue rather than a delta. A client that missed a
    /// frame then cannot drift: the newest pair of lists is the truth, and a
    /// second browser tab showing the same session stays in step without
    /// polling `GET /api/sessions/:id`.
    QueuedMessages {
        session_id: String,
        /// Both queues, oldest first.
        steers: Vec<crate::queue::QueuedMessage>,
        follow_ups: Vec<crate::queue::QueuedMessage>,
    },
    RunStarted {
        run_id: String,
        prompt: String,
    },
    RunFinished {
        run_id: String,
        status: String,
        #[serde(default)]
        error: Option<String>,
    },
    RunCancelled {
        run_id: String,
    },
    IterationStarted {
        iteration: u32,
    },
    /// The provider stream died after it had started delivering, and the
    /// runtime is throwing that partial attempt away before re-issuing the
    /// identical request. Everything already streamed for this turn belongs to
    /// the abandoned attempt and must be dropped by the client.
    StreamRetry {
        /// Upstream attempts made for this turn, including the one that just
        /// failed (1 = the first attempt failed and one restart is next).
        attempt: u32,
        /// Total attempts this turn may make (`retry_max_stream_restarts + 1`).
        max_attempts: u32,
        /// Why the attempt died, for display and for the run log.
        reason: String,
    },
    TextDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        /// Prompt tokens the provider served from its prefix cache. `None` when
        /// the endpoint does not report it, so the UI can show "unknown"
        /// instead of a fake 0% hit rate.
        #[serde(skip_serializing_if = "Option::is_none")]
        cached_tokens: Option<u32>,
    },
    ToolCallStarted {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolCallFinished {
        call_id: String,
        name: String,
        ok: bool,
        output: String,
        truncated: bool,
        duration_ms: u64,
        /// Images the tool returned for the model to look at (base64). Sent to
        /// every client so the chat can show the preview live; a text-only
        /// model never produces them, because the tool refuses instead.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<crate::message::ImageAttachment>,
    },
    CommandStarted {
        call_id: String,
        command: String,
        cwd: String,
    },
    CommandOutput {
        call_id: String,
        stream: String,
        chunk: String,
    },
    CommandFinished {
        call_id: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        truncated: bool,
    },
    ApprovalRequested {
        approval: ApprovalRequest,
    },
    ApprovalResolved {
        approval_id: String,
        decision: ApprovalDecision,
    },
    AgentSpawned {
        agent_id: String,
        name: String,
        role: String,
        task: String,
        model: String,
        parent_run_id: String,
        /// True when the spawn detached, so the roster can show it without
        /// waiting for a refresh from the store (which is where the row's own
        /// copy lives). Defaulted so a frame from an older build still parses.
        #[serde(default)]
        background: bool,
    },
    AgentProgress {
        agent_id: String,
        iterations: u32,
        note: String,
    },
    AgentFinished {
        agent_id: String,
        status: String,
        summary: String,
        #[serde(default)]
        error: Option<String>,
    },
    /// A live subagent was paused or resumed by a human or by its parent.
    ///
    /// Separate from `agent_progress` because it changes whether the agent can
    /// spend anything at all, and the roster has to say so: a paused agent still
    /// holds a subagent slot, so a list that showed it as `running` would make the
    /// next spawn look like it had free capacity it does not have.
    AgentPaused {
        agent_id: String,
        /// `true` when the pause landed, `false` when it was lifted.
        paused: bool,
        /// Who asked, for the roster: `user` or the parent agent id.
        by: String,
    },
    /// A message was delivered to (or queued for) another agent.
    ///
    /// The payload is the *delivery*, not the message text: the text is already
    /// in the recipient's transcript, and a mailbox frame is meant for the roster
    /// ("sarah has 2 unread"), not for a second copy of the conversation.
    AgentMessage {
        from_agent_id: Option<String>,
        /// The recipient: an agent id, or `run:<id>` for a message to the
        /// top-level run of a session (which is what a child means by "parent"
        /// when it was spawned by the main agent).
        to: String,
        /// `delivered` when the recipient's loop took it at the top of its next
        /// iteration, `queued` when it is parked or still working.
        state: String,
    },
    /// A live subagent's own status report: what it says it is doing and how far
    /// along it claims to be. Advisory, and independent of the iteration counter.
    AgentNote {
        agent_id: String,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<f32>,
    },
    Notice {
        level: String,
        message: String,
    },
    /// A human-facing notification: the run settled, something is waiting for an
    /// answer, or a plugin asked for attention.
    ///
    /// This is the one signal a client acts on *outside* the transcript, so it is a
    /// distinct event rather than a `notice` with a level: the UI keys its sound
    /// cue, its OS notification and its unread badge off the type, and `notice` is
    /// already used for advisory text that belongs inline.
    ///
    /// Emitted after the state it describes is persisted, and suppressed entirely
    /// by `[notifications] enabled = false` — a disabled server sends no such frame,
    /// so a client never has to trust itself to ignore one.
    Notification {
        notification: crate::notify::Notify,
    },
    Error {
        message: String,
        #[serde(default)]
        kind: Option<String>,
    },
    /// The SSE pump could not keep up with this client and dropped frames.
    ///
    /// Emitted by `kc-server` rather than the runtime: it describes the transport,
    /// not the agent. A client must treat it as "refetch the session", and must
    /// *not* use its `seq` as a dedupe watermark (see `routes::events`).
    StreamLagged {
        skipped: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub ts_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(flatten)]
    pub event: Event,
}

/// Sink used by tools to publish streamed output. The runtime fills in the
/// envelope context (session/run/agent), so tools only produce the payload.
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

pub fn noop_sink() -> EventSink {
    Arc::new(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{Notify, NotifyKind};

    /// The exact JSON the browser receives. The UI switches on `type` and reads
    /// `notification`, so the flattened shape is part of the contract with
    /// `web/src/api/types.ts` and cannot be changed casually.
    #[test]
    fn a_notification_event_serialises_flat_with_type_notification() {
        let notify = Notify::new(
            NotifyKind::RunFinished,
            "run:r_1",
            "Run finished",
            "All 3 files updated.",
            "s_1",
            "Fix the login redirect",
            crate::notify::RUNTIME_SOURCE,
        );
        let envelope = EventEnvelope {
            seq: 7,
            ts_ms: 1_700_000_000_000,
            session_id: Some("s_1".into()),
            run_id: Some("r_1".into()),
            agent_id: None,
            event: Event::Notification {
                notification: notify.clone(),
            },
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["type"], serde_json::json!("notification"));
        assert_eq!(json["seq"], serde_json::json!(7));
        assert_eq!(json["session_id"], serde_json::json!("s_1"));
        // The payload is nested under `notification`, not flattened into the
        // envelope: the envelope already owns `ts_ms` and would clash.
        assert_eq!(
            json["notification"]["kind"],
            serde_json::json!("run_finished")
        );
        assert_eq!(json["notification"]["id"], serde_json::json!("run:r_1"));
        assert_eq!(
            json["notification"]["session_title"],
            serde_json::json!("Fix the login redirect")
        );
        assert!(
            json.get("agent_id").is_none(),
            "an absent agent must be omitted"
        );

        let back: EventEnvelope = serde_json::from_value(json).unwrap();
        match back.event {
            Event::Notification { notification } => assert_eq!(notification, notify),
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    /// An older client ignores unknown `type` values, which is what makes adding
    /// this event additive rather than a protocol break.
    #[test]
    fn an_unknown_event_type_is_representable_for_forward_compatibility() {
        let raw: serde_json::Value = serde_json::json!({
            "seq": 1, "ts_ms": 0, "type": "something_new",
        });
        // The client-side reducers default to "ignore"; on this side the proof is
        // that a payload with no known tag simply fails to parse rather than
        // producing a wrong event.
        assert!(serde_json::from_value::<Event>(raw).is_err());
    }
}
