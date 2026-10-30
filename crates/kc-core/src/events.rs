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
    /// A steering message the user sent mid-run has been taken into the
    /// conversation. Emitted by the loop at the moment the message joins the
    /// request, so the transcript can show it where the model saw it rather than
    /// where the client optimistically rendered it.
    SteerApplied {
        /// The id `POST /api/sessions/:id/messages?mode=steer` returned.
        steer_id: String,
        content: String,
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
    Notice {
        level: String,
        message: String,
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
