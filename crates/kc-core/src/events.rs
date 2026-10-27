use crate::approval::{ApprovalDecision, ApprovalRequest};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Semantic events emitted by the runtime and forwarded to the browser over SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionCreated { session_id: String, title: String },
    SessionRenamed { session_id: String, title: String },
    SessionDeleted { session_id: String },
    RunStarted { run_id: String, prompt: String },
    RunFinished { run_id: String, status: String, #[serde(default)] error: Option<String> },
    RunCancelled { run_id: String },
    IterationStarted { iteration: u32 },
    TextDelta { delta: String },
    ReasoningDelta { delta: String },
    Usage { input_tokens: u32, output_tokens: u32 },
    ToolCallStarted { call_id: String, name: String, arguments: serde_json::Value },
    ToolCallFinished {
        call_id: String,
        name: String,
        ok: bool,
        output: String,
        truncated: bool,
        duration_ms: u64,
    },
    CommandStarted { call_id: String, command: String, cwd: String },
    CommandOutput { call_id: String, stream: String, chunk: String },
    CommandFinished { call_id: String, exit_code: Option<i32>, duration_ms: u64, truncated: bool },
    ApprovalRequested { approval: ApprovalRequest },
    ApprovalResolved { approval_id: String, decision: ApprovalDecision },
    AgentSpawned {
        agent_id: String,
        name: String,
        role: String,
        task: String,
        model: String,
        parent_run_id: String,
    },
    AgentProgress { agent_id: String, iterations: u32, note: String },
    AgentFinished {
        agent_id: String,
        status: String,
        summary: String,
        #[serde(default)]
        error: Option<String>,
    },
    Notice { level: String, message: String },
    Error { message: String, #[serde(default)] kind: Option<String> },
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
