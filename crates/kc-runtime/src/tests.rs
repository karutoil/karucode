//! Behavioural tests for the agent loop, approvals, cancellation, context
//! budgeting and subagents.
//!
//! Every await that waits for the runtime is wrapped in `tokio::time::timeout`
//! (inside `Harness::next_event` / `wait_idle`) so a regression that hangs the
//! loop fails the test instead of blocking CI.

use crate::approvals::{ApprovalRegistry, RuntimeApprovalGate};
use crate::subagent::SPAWN_AGENT_TOOL;
use crate::tests_support::{
    test_inner_with_settings, ApprovalTool, BigOutputTool, EchoTool, ExternalTool, FailingTool,
    FakeSettings, FollowUpTool, GateTool, Harness, HarnessBuilder, ImageTool, LegacyPromptHook,
    PanicTool, ProbeTool, RequiredArgsTool, Script, ScriptedFactory, SlowCancelTool, SteerTool,
    TurnRecordingHook, WriteWorkspaceFileTool,
};
use crate::{ActiveRun, QueuedInput, QueuedKind, RecoveryReport, Runtime, RuntimeDeps};
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView, ApprovalRequest};
use kc_core::config::{ApprovalMode, ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
use kc_core::error::Error;
use kc_core::events::{Event, EventEnvelope};
use kc_core::hooks::{Hooks, NoHooks};
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::message::{Role, ToolCall};
use kc_core::paths::Workspace;
use kc_core::provider::ProviderChunk;
use kc_core::store::{AgentRecord, ApprovalRecord, RunStatus, Store};
use kc_core::tool::{ApprovalGate, ToolClass, ToolOrigin, ToolOutcome, ToolRegistry};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Arbitrary hook futures that deliberately never return. Tests observe their
/// entry counter, then prove cancellation/deadlines still let the run settle.
#[derive(Default)]
struct BlockingHooks {
    tool_results: AtomicUsize,
    run_ends: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for BlockingHooks {
    async fn on_tool_result(&self, _session_id: &str, _tool: &str, _ok: bool) {
        self.tool_results.fetch_add(1, Ordering::SeqCst);
        futures_util::future::pending::<()>().await;
    }

    async fn on_run_end(&self, _session_id: &str, _run_id: &str, _status: &str) {
        self.run_ends.fetch_add(1, Ordering::SeqCst);
        futures_util::future::pending::<()>().await;
    }
}

/// Queues from `before_prompt`, which is after that iteration's normal drain
/// and immediately before a no-tool final response. This deterministically
/// exercises the final-response arrival window without racing a worker thread.
#[derive(Default)]
struct FinalResponseSteerHooks {
    runtime: Mutex<Weak<Runtime>>,
    sent: AtomicBool,
}

impl FinalResponseSteerHooks {
    fn arm(&self, runtime: &Arc<Runtime>) {
        *self.runtime.lock().expect("steer hook lock") = Arc::downgrade(runtime);
    }
}

#[async_trait::async_trait]
impl Hooks for FinalResponseSteerHooks {
    async fn before_prompt(&self, session_id: &str, _workspace: &str) -> Vec<String> {
        self.queue_once(session_id).await;
        Vec::new()
    }
}

impl FinalResponseSteerHooks {
    async fn queue_once(&self, session_id: &str) {
        if self.sent.swap(true, Ordering::SeqCst) {
            return;
        }
        let runtime = self
            .runtime
            .lock()
            .expect("steer hook lock")
            .upgrade()
            .expect("runtime armed before submit");
        runtime
            .steer(session_id, "apply the final-turn correction")
            .expect("the final-response iteration still accepts a steer");
    }
}

struct PanicParentToolHook;

#[async_trait::async_trait]
impl Hooks for PanicParentToolHook {
    async fn on_tool_result(&self, _session_id: &str, tool: &str, _ok: bool) {
        if tool == "parent_gate" {
            panic!("parent panics after the loop's mail drain");
        }
    }
}

#[derive(Default)]
struct PanicRunEndHooks {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for PanicRunEndHooks {
    async fn on_run_end(&self, _session_id: &str, _run_id: &str, _status: &str) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("run-end hook broke after status persistence");
    }
}

fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::SessionCreated { .. } => "session_created",
        Event::SessionRenamed { .. } => "session_renamed",
        Event::SessionModelChanged { .. } => "session_model_changed",
        Event::SessionRewound { .. } => "session_rewound",
        Event::SessionDeleted { .. } => "session_deleted",
        Event::SteerApplied { .. } => "steer_applied",
        Event::QueuedMessages { .. } => "queued_messages",
        Event::RunStarted { .. } => "run_started",
        Event::RunFinished { .. } => "run_finished",
        Event::RunCancelled { .. } => "run_cancelled",
        Event::IterationStarted { .. } => "iteration_started",
        Event::StreamRetry { .. } => "stream_retry",
        Event::TextDelta { .. } => "text_delta",
        Event::TextReplaced { .. } => "text_replaced",
        Event::ReasoningDelta { .. } => "reasoning_delta",
        Event::Usage { .. } => "usage",
        Event::ToolCallStarted { .. } => "tool_call_started",
        Event::ToolCallFinished { .. } => "tool_call_finished",
        Event::CommandStarted { .. } => "command_started",
        Event::CommandOutput { .. } => "command_output",
        Event::CommandFinished { .. } => "command_finished",
        Event::ApprovalRequested { .. } => "approval_requested",
        Event::ApprovalResolved { .. } => "approval_resolved",
        Event::AgentSpawned { .. } => "agent_spawned",
        Event::AgentProgress { .. } => "agent_progress",
        Event::AgentFinished { .. } => "agent_finished",
        Event::AgentPaused { .. } => "agent_paused",
        Event::AgentMessage { .. } => "agent_message",
        Event::AgentNote { .. } => "agent_note",
        Event::Notice { .. } => "notice",
        Event::Notification { .. } => "notification",
        Event::StreamLagged { .. } => "stream_lagged",
        Event::Error { .. } => "error",
    }
}

fn kinds(events: &[EventEnvelope]) -> Vec<&'static str> {
    events.iter().map(|e| event_kind(&e.event)).collect()
}

fn count(events: &[EventEnvelope], kind: &str) -> usize {
    kinds(events).iter().filter(|k| **k == kind).count()
}

fn finished_status(events: &[EventEnvelope]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match &e.event {
            Event::RunFinished { status, .. } => Some(status.clone()),
            _ => None,
        })
        .expect("run_finished event")
}

async fn wait_for_kind(h: &mut Harness, kind: &str) -> (EventEnvelope, Vec<EventEnvelope>) {
    let mut seen = Vec::new();
    loop {
        let event = h.next_event().await;
        let matches = event_kind(&event.event) == kind;
        seen.push(event.clone());
        if matches {
            return (event, seen);
        }
    }
}

fn approval_from(event: &EventEnvelope) -> ApprovalRequest {
    match &event.event {
        Event::ApprovalRequested { approval } => approval.clone(),
        other => panic!("expected approval_requested, got {other:?}"),
    }
}

fn tool_finished(events: &[EventEnvelope]) -> Vec<(String, bool, String, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolCallFinished {
                call_id,
                ok,
                output,
                truncated,
                ..
            } => Some((call_id.clone(), *ok, output.clone(), *truncated)),
            _ => None,
        })
        .collect()
}

/// Assert no `tool` message starts an exchange: every tool result must follow
/// the assistant tool_call it answers (or another result of the same turn).
fn assert_no_orphan_tool_messages(messages: &[kc_core::message::Message]) {
    for (i, message) in messages.iter().enumerate() {
        if message.role != Role::Tool {
            continue;
        }
        let id = message
            .tool_call_id
            .as_deref()
            .unwrap_or_else(|| panic!("tool message without a tool_call_id at {i}"));
        let attached = i > 0
            && match messages[i - 1].role {
                Role::Tool => true,
                Role::Assistant => messages[i - 1].tool_calls.iter().any(|call| call.id == id),
                _ => false,
            };
        assert!(
            attached,
            "orphan tool message at index {i} (id {id}): {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plain_text_run_persists_messages_and_completes() {
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("Hello".into()),
        ProviderChunk::Text(", world".into()),
        ProviderChunk::Usage {
            input_tokens: 11,
            output_tokens: 4,
            cached_tokens: None,
        },
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("hi there").await;
    let kinds = kinds(&events);

    assert_eq!(kinds.first(), Some(&"run_started"));
    assert_eq!(kinds.last(), Some(&"run_finished"));
    assert_eq!(count(&events, "iteration_started"), 1);
    assert_eq!(count(&events, "text_delta"), 2);
    assert_eq!(count(&events, "usage"), 1);
    assert_eq!(finished_status(&events), "completed");

    // seq is monotonically increasing across the whole stream.
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq not increasing: {seqs:?}"
    );

    let messages = h.session_messages().await;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "assistant"]);
    assert_eq!(messages[1].content, "Hello, world");
    assert!(messages.iter().all(|m| m.agent_id.is_none()));

    let runs = h.store.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "completed");
    assert_eq!(runs[0].model, "fake-model");
    assert_eq!(runs[0].prompt, "hi there");
    assert!(runs[0].finished_at.is_some());
    assert!(h.runtime.events_emitted() > 3);
    assert!(h.wait_idle().await.is_empty());
    assert!(!h.runtime.is_running(h.session()));
}

#[tokio::test]
async fn a_session_run_uses_its_own_model_selection() {
    let mut h = HarnessBuilder::new(vec![Script::text("done")])
        .build()
        .await;

    // No selection: the install-wide default model is used (baseline).
    let _ = h.run_to_finish("first").await;
    assert_eq!(h.factory.requests()[0].model, "fake-model");

    // Pin *this* session to another model, then run again in the same session.
    let selection = kc_core::config::SessionModelSelection {
        provider: "gateway".into(),
        model: "big-model".into(),
        reasoning_effort: Some("high".into()),
    };
    h.store
        .set_session_model(h.session(), Some(&selection))
        .await
        .expect("pin the session model");

    let _ = h.run_to_finish("second").await;
    let requests = h.factory.requests();
    assert_eq!(
        requests[1].model, "big-model",
        "the session selection must reach the provider"
    );
    // The reasoning effort rides in `extra_body`, which the provider forwards.
    assert_eq!(
        requests[1].extra_body.get("reasoning_effort"),
        Some(&serde_json::Value::String("high".into()))
    );

    // The run record names the model that actually ran.
    let runs = h.store.runs();
    assert_eq!(runs.last().map(|run| run.model.as_str()), Some("big-model"));

    // Clearing the override goes back to the default for the next run.
    h.store
        .set_session_model(h.session(), None)
        .await
        .expect("clear the session model");
    let _ = h.run_to_finish("third").await;
    assert_eq!(h.factory.requests()[2].model, "fake-model");
}

#[tokio::test]
async fn a_subagent_inherits_the_sessions_model_selection() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("child work", None, false),
        Script::text("child summary"),
        Script::text("parent done"),
    ])
    .build()
    .await;

    let selection = kc_core::config::SessionModelSelection {
        provider: "gateway".into(),
        model: "pinned-model".into(),
        reasoning_effort: None,
    };
    h.store
        .set_session_model(h.session(), Some(&selection))
        .await
        .expect("pin the session model");

    let _ = h.run_to_finish("delegate").await;
    let requests = h.factory.requests();
    assert_eq!(requests[0].model, "pinned-model", "the parent run");
    assert_eq!(
        requests[1].model, "pinned-model",
        "the subagent inherits the session selection"
    );
    assert_eq!(h.store.agents()[0].model, "pinned-model");
}

#[tokio::test]
async fn usage_is_read_when_it_arrives_after_the_finish_chunk() {
    // Real OpenAI-compatible endpoints put `usage` in a chunk *after* the one
    // that carries `finish_reason`. Breaking out of the stream at the finish
    // chunk is how a harness silently stops accounting for tokens (and caches).
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("ok".into()),
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
        ProviderChunk::Usage {
            input_tokens: 100,
            output_tokens: 4,
            cached_tokens: Some(98),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("hi").await;
    assert_eq!(finished_status(&events), "completed");
    let usage = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
            } => Some((*input_tokens, *output_tokens, *cached_tokens)),
            _ => None,
        })
        .expect("the usage event must survive a finish chunk that came first");
    assert_eq!(usage, (100, 4, Some(98)));
}

#[tokio::test]
async fn every_provider_call_is_persisted_for_the_session_view() {
    // The `usage` event is live-only, so a reload cannot answer "what has this
    // session cost". One row per call must be left behind - for the main agent
    // and for each subagent, tagged with the agent so the view can split them.
    let mut h = HarnessBuilder::new(vec![
        spawn_call("review the diff", None, false),
        // The subagent's own call.
        Script::Chunks(vec![
            ProviderChunk::Text("done".into()),
            ProviderChunk::Usage {
                input_tokens: 12,
                output_tokens: 2,
                cached_tokens: Some(6),
            },
            ProviderChunk::Finish {
                reason: "stop".into(),
            },
        ]),
        // The parent's second call, which answers with the summary.
        Script::Chunks(vec![
            ProviderChunk::Text("spawning".into()),
            ProviderChunk::Usage {
                input_tokens: 40,
                output_tokens: 3,
                cached_tokens: None,
            },
            ProviderChunk::Finish {
                reason: "stop".into(),
            },
        ]),
    ])
    .build()
    .await;

    let events = h.run_to_finish("go").await;
    assert_eq!(finished_status(&events), "completed");

    let rows = h.store.usage();
    assert_eq!(rows.len(), 2, "one row per provider call: {rows:?}");
    let main = rows
        .iter()
        .find(|row| row.agent_id.is_none())
        .expect("a row for the run itself");
    assert_eq!(main.input_tokens, 40);
    assert_eq!(main.output_tokens, 3);
    assert_eq!(main.cached_tokens, None, "unknown is not 0");
    assert!(!main.run_id.is_empty());
    let child = rows
        .iter()
        .find(|row| row.agent_id.is_some())
        .expect("a row for the subagent");
    assert_eq!(child.input_tokens, 12);
    assert_eq!(child.cached_tokens, Some(6));
}

#[tokio::test]
async fn a_stream_that_repeats_usage_is_accounted_once() {
    // Some OpenAI-compatible gateways put the `usage` object on every streamed
    // chunk, repeating (or growing) the call's running totals. Every frame is the
    // same call, not a new one: accounting each of them wrote hundreds of rows per
    // call and inflated a session's totals by ~100x. The newest block is the
    // call's final accounting.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("ok".into()),
        ProviderChunk::Usage {
            input_tokens: 10,
            output_tokens: 1,
            cached_tokens: Some(5),
        },
        ProviderChunk::Usage {
            input_tokens: 28,
            output_tokens: 2,
            cached_tokens: Some(20),
        },
        ProviderChunk::Usage {
            input_tokens: 31,
            output_tokens: 3,
            cached_tokens: Some(25),
        },
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("hi").await;
    assert_eq!(
        count(&events, "usage"),
        1,
        "one usage frame per provider call: {:?}",
        kinds(&events)
    );

    let rows = h.store.usage();
    assert_eq!(rows.len(), 1, "one row per provider call: {rows:?}");
    assert_eq!(rows[0].input_tokens, 31);
    assert_eq!(rows[0].output_tokens, 3);
    assert_eq!(
        rows[0].cached_tokens,
        Some(25),
        "the newest block is the call's total"
    );
}

#[tokio::test]
async fn a_running_subagent_persists_its_iteration_count() {
    // A refresh (or a second tab) reads the agents table, not the SSE stream. The
    // row used to carry its real count only once the agent finished, so every
    // running subagent read as `0` iterations. The child hangs mid-iteration so
    // the row is observed while it is still live.
    let mut h = HarnessBuilder::new(vec![
        spawn_call("child work", None, false),
        Script::Hang(vec![ProviderChunk::Text("child thinking".into())]),
        Script::text("parent done"),
    ])
    .build()
    .await;

    h.submit("delegate").await;

    // The count is persisted before the frame is published, so receiving it is
    // proof the row a refresh would read is already up to date.
    let child_id = loop {
        if let Event::AgentProgress {
            agent_id,
            iterations,
            ..
        } = h.next_event().await.event
        {
            assert_eq!(iterations, 1);
            break agent_id;
        }
    };

    let row = h
        .store
        .agents()
        .into_iter()
        .find(|agent| agent.id == child_id)
        .expect("the child's row");
    assert_eq!(
        row.iterations, 1,
        "the stored row must show the iteration a refresh reads, not the initial 0"
    );
    assert_eq!(row.status, "running");

    // Unwind the hung child so the test leaves no live loop behind.
    assert!(h.runtime.cancel(h.session()));
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    let settled = h
        .store
        .agents()
        .into_iter()
        .find(|agent| agent.id == child_id)
        .expect("the child's row");
    assert_eq!(settled.iterations, 1, "the final write keeps the count too");
}

#[tokio::test]
async fn a_degenerate_reasoning_loop_is_abandoned_and_retried() {
    // A reasoning model can repeat one short block in `reasoning_content` for
    // hundreds of thousands of tokens. That channel is streamed and discarded,
    // so nothing downstream can notice - the turn has to be cut here. The second
    // attempt answers normally, so the run still completes.
    let looped: String = "Let me write. Go. Now. OK. ".repeat(40);
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![
            ProviderChunk::Reasoning(looped),
            ProviderChunk::Text("never reached".into()),
        ]),
        Script::text("Here is the answer."),
    ])
    .build()
    .await;

    let events = h.run_to_finish("write the thing").await;
    assert_eq!(finished_status(&events), "completed");
    let retry = events.iter().find_map(|e| match &e.event {
        Event::StreamRetry { reason, .. } => Some(reason.clone()),
        _ => None,
    });
    let reason = retry.expect("a degenerate reasoning loop must be retried, not consumed");
    assert!(reason.contains("reasoning repeated"), "{reason}");
    // The loop's text never reached the transcript.
    let messages = h.session_messages().await;
    assert!(
        !messages.iter().any(|m| m.content.contains("Let me write")),
        "{messages:?}"
    );
    assert!(messages.iter().any(|m| m.content == "Here is the answer."));
    // Two attempts were made, not forty tokens' worth of loop.
    assert_eq!(h.factory.requests().len(), 2);
}

#[tokio::test]
async fn a_degenerate_text_loop_is_trimmed_and_the_turn_still_completes() {
    // Repetition in the answer itself is trimmed instead of retried: the model
    // did produce something, and stopping the run over it would be worse than
    // keeping the part that was not a loop.
    let mut chunks: Vec<ProviderChunk> = Vec::new();
    chunks.push(ProviderChunk::Text("Nine findings follow. ".into()));
    for _ in 0..10 {
        chunks.push(ProviderChunk::Text("Let me write. Go. Now. OK. ".into()));
    }
    let mut h = HarnessBuilder::new(vec![Script::Chunks(chunks)])
        .build()
        .await;

    let events = h.run_to_finish("write").await;
    assert_eq!(finished_status(&events), "completed");
    let messages = h.session_messages().await;
    let answer = messages
        .iter()
        .find(|m| m.role == "assistant")
        .expect("an assistant turn");
    assert!(
        answer.content.starts_with("Nine findings follow."),
        "answer was {:?}",
        answer.content
    );
    assert!(
        !answer.content.contains("Let me write"),
        "the repeated tail was persisted: {}",
        answer.content
    );
}

/// The same model, served by ten different gateways: the case that used to be
/// mistaken for a loop.
fn enumerated_gateways() -> String {
    [
        "openrouter",
        "a6api/ali",
        "together",
        "fireworks",
        "deepinfra",
        "novita",
        "hyperbolic",
        "parasail",
        "synthetic",
        "ncompass",
    ]
    .iter()
    .map(|provider| format!("{provider}/deepseek-v3.2-flash-1017"))
    .collect::<Vec<_>>()
    .join(", ")
}

#[tokio::test]
async fn reasoning_that_enumerates_providers_is_not_called_a_loop() {
    // The reported bug. A model weighing up ten gateways that serve one model
    // repeats the model id ten times in its reasoning while every provider is new.
    // Counting a repeated 24-character window made that look exactly like a
    // degenerate loop: the attempt was thrown away, the identical request was
    // re-issued, and once the restart budget was spent the run failed. A loop is a
    // cycle; a shared name is not.
    let reasoning = format!(
        "Which gateways serve deepseek-v3.2-flash-1017? {}. Ten providers, one model. \
         The cheapest is enough, so answer.",
        enumerated_gateways()
    );
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Reasoning(reasoning),
        ProviderChunk::Text("Ten gateways serve it.".into()),
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("which gateways serve it?").await;
    assert_eq!(finished_status(&events), "completed");
    assert!(
        !events
            .iter()
            .any(|e| matches!(&e.event, Event::StreamRetry { .. })),
        "an enumeration must not abandon the attempt: {events:?}"
    );
    assert_eq!(
        h.factory.requests().len(),
        1,
        "the request must not be re-issued"
    );
    let messages = h.session_messages().await;
    assert!(
        messages
            .iter()
            .any(|m| m.content == "Ten gateways serve it."),
        "{messages:?}"
    );
}

#[tokio::test]
async fn the_turns_reasoning_is_persisted_with_the_assistant_row() {
    // The reported bug: a reloaded session showed no thinking block. The stream
    // forwarded reasoning only to the live client and never stored it, even
    // though the schema, `StoredMessage::reasoning_content` and the API contract
    // all expect the turn to carry its analysis. It has to be accumulated across
    // deltas, not just emitted.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Reasoning("weigh the two options. ".into()),
        ProviderChunk::Reasoning("the second is cheaper.".into()),
        ProviderChunk::Text("Use the second.".into()),
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("which option?").await;
    assert_eq!(finished_status(&events), "completed");
    let messages = h.session_messages().await;
    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant" && m.content == "Use the second.")
        .expect("assistant row");
    assert_eq!(
        assistant.reasoning_content.as_deref(),
        Some("weigh the two options. the second is cheaper.")
    );
    // The live view is unchanged: the deltas still go out as they arrive.
    assert!(events.iter().any(|e| matches!(
        &e.event,
        Event::ReasoningDelta { delta } if delta == "weigh the two options. "
    )));
}

#[tokio::test]
async fn a_tool_calling_turn_keeps_its_reasoning_with_no_prose() {
    // A reasoning model that thinks and then acts streams no text at all; the
    // stored row is reasoning + tool calls, and that row is what puts the
    // thinking block above the tool card after a reload.
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![
            ProviderChunk::Reasoning("read the directory first".into()),
            ProviderChunk::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("list_dir".into()),
                arguments_delta: Some("{\"path\":\".\"}".into()),
            },
            ProviderChunk::Finish {
                reason: "tool_calls".into(),
            },
        ]),
        Script::text("done"),
    ])
    .build()
    .await;

    let events = h.run_to_finish("look around").await;
    assert_eq!(finished_status(&events), "completed");
    let messages = h.session_messages().await;
    let acting = messages
        .iter()
        .find(|m| m.role == "assistant" && !m.tool_calls.is_empty())
        .expect("tool-calling assistant row");
    assert_eq!(acting.content, "");
    assert_eq!(
        acting.reasoning_content.as_deref(),
        Some("read the directory first")
    );
}

#[tokio::test]
async fn an_enumeration_in_the_answer_is_not_trimmed() {
    // The same list in the answer text, where cutting it would silently delete the
    // part of the reply the user asked for.
    let answer = format!("Ten gateways serve it: {}.", enumerated_gateways());
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text(answer.clone()),
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("which gateways?").await;
    assert_eq!(finished_status(&events), "completed");
    let messages = h.session_messages().await;
    let stored = messages
        .iter()
        .find(|m| m.role == "assistant")
        .expect("an assistant turn");
    assert_eq!(stored.content, answer);
}

#[tokio::test]
async fn repetitive_fenced_code_is_preserved_and_prose_loops_still_stop() {
    let code = format!(
        "Example:\n```rust\n{}\n```\nDone.",
        "    println!(\"repeated statement\");\n".repeat(30)
    );
    // Split every character, including the fence, as a streaming provider may.
    let mut chunks: Vec<_> = code
        .chars()
        .map(|c| ProviderChunk::Text(c.to_string()))
        .collect();
    chunks.push(ProviderChunk::Finish {
        reason: "stop".into(),
    });
    let mut h = HarnessBuilder::new(vec![Script::Chunks(chunks)])
        .build()
        .await;
    let events = h.run_to_finish("show code").await;
    assert_eq!(finished_status(&events), "completed", "{events:?}");
    assert!(h
        .session_messages()
        .await
        .iter()
        .any(|m| m.role == "assistant" && m.content == code));

    let input = format!(
        "~~~~\n{}~~~~\nA preface. {}",
        "repeated code statement here\n".repeat(20),
        "Let me write. Go. Now. OK. ".repeat(12)
    );
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![ProviderChunk::Text(input)])])
        .build()
        .await;
    h.run_to_finish("show code").await;
    let messages = h.session_messages().await;
    let answer = messages.iter().find(|m| m.role == "assistant").unwrap();
    assert!(answer.content.contains("A preface."));
    assert!(!answer.content.contains("Let me write"));
}

#[tokio::test]
async fn spaced_closing_fences_resume_prose_repetition_detection() {
    for (open, close) in [("```rust", "``` \t\r"), ("~~~~", "  ~~~~~\t ")] {
        let input = format!(
            "{open}\n{}{close}\nA preface. {}",
            "repeated code statement here\n".repeat(20),
            "Let me write. Go. Now. OK. ".repeat(14)
        );
        let chunks = input
            .chars()
            .map(|c| ProviderChunk::Text(c.to_string()))
            .collect();
        let mut h = HarnessBuilder::new(vec![Script::Chunks(chunks)])
            .build()
            .await;
        h.run_to_finish("show code").await;
        let messages = h.session_messages().await;
        let answer = messages.iter().find(|m| m.role == "assistant").unwrap();
        assert!(answer.content.contains("A preface."), "{}", answer.content);
        assert!(
            !answer.content.contains("Let me write"),
            "{}",
            answer.content
        );
    }
}

#[test]
fn the_repetition_detector_ignores_normal_text() {
    use crate::agent::{detect_loop, REPEATS_REASONING, REPEATS_TEXT};

    // Ordinary code, layout and a line or two repeated are not loops.
    assert!(detect_loop(
        "fn main() {\n    let x = compute();\n    println!(\"{x}\");\n}\n",
        REPEATS_REASONING
    )
    .is_none());
    assert!(detect_loop(&"- ".repeat(200), REPEATS_REASONING).is_some());
    assert!(detect_loop(&"| flaky | rerun |\n".repeat(3), REPEATS_REASONING).is_none());

    // What the model actually did: the same few words, over and over.
    let looped = "Let me write. Go. Now. OK. ".repeat(12);
    let caught = detect_loop(&looped, REPEATS_REASONING).expect("a loop must be caught");
    assert!(caught.block.contains("Let me write"), "{}", caught.block);

    // The same loop with the whitespace varied - which is what defeated the
    // first, exact-cycle version of this detector.
    let varied = "Let me write.\n\nGo.\nNow.\n\nOK.\n\n".repeat(10);
    assert!(detect_loop(&varied, REPEATS_REASONING).is_some());

    // A single character forever, and a whole paragraph ten times, are both loops.
    assert!(detect_loop(&"a".repeat(600), REPEATS_REASONING).is_some());
    let para = "the parser rejects an empty path and returns a bad request error ";
    assert!(detect_loop(&para.repeat(10), REPEATS_REASONING).is_some());

    // Answer text is content, so it needs ten repeats before anything is cut:
    // eight is enough for reasoning, not for the answer.
    assert!(detect_loop(&looped, REPEATS_TEXT).is_some());
    assert!(detect_loop(&"Let me write. Go. Now. OK. ".repeat(8), REPEATS_TEXT).is_none());

    // A rule, a border and a capped sweep of code are punctuation and layout, not
    // a stuck model: a repeated block of one character has to run absurdly long.
    assert!(detect_loop(&"-".repeat(300), REPEATS_TEXT).is_none());
    assert!(detect_loop(&"=".repeat(600), REPEATS_TEXT).is_none());
    assert!(detect_loop(&"*".repeat(120), REPEATS_REASONING).is_none());
}

#[test]
fn a_list_that_repeats_one_long_name_is_not_a_loop() {
    // The reported bug. Reasoning that weighs up ten gateways serving the *same*
    // model repeats the model id ten times while every provider is new. The old
    // detector counted a repeated 24-character window and called that a loop, so
    // an attempt was thrown away and re-issued, then the run failed after the
    // restart budget was spent. A loop is a cycle; a shared name is not.
    use crate::agent::{detect_loop, REPEATS_REASONING, REPEATS_TEXT};

    let providers = [
        "openrouter/",
        "a6api/ali/",
        "together/",
        "fireworks/",
        "deepinfra/",
        "novita/",
        "hyperbolic/",
        "parasail/",
        "synthetic/",
        "ncompass/",
    ];
    let enumerated: String = providers
        .iter()
        .map(|p| format!("{p}deepseek-v3.2-flash-1017"))
        .collect::<Vec<_>>()
        .join(", ");
    for text in [
        format!("Which gateways serve it? {enumerated}. That is ten providers, one model."),
        // One per line, as a reader would write a list.
        providers
            .iter()
            .map(|p| format!("- {p}deepseek-v3.2-flash-1017"))
            .collect::<Vec<_>>()
            .join("\n"),
        // The same list as a markdown table with a long shared cell.
        providers
            .iter()
            .map(|p| format!("| {p} | deepseek-v3.2-flash-1017 | tier a |"))
            .collect::<Vec<_>>()
            .join("\n"),
        // One shared directory prefix, ten files.
        format!(
            "The files are {}.",
            ["agent", "context", "subagent", "restart", "budget", "journal", "emit", "hub"]
                .iter()
                .map(|f| format!("/home/karutoil/karucode/crates/kc-runtime/src/{f}.rs"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // A link list, where every line shares a long URL stem.
        (1..=10)
            .map(|n| {
                format!("- [web-ui.md](https://github.com/karutoil/karucode/blob/main/docs/{n}.md)")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        // Ten commands that share a flag tail but name different crates.
        (1..=10)
            .map(|n| format!("cargo test -p kc-crate-{n} --workspace --all-features"))
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        assert!(
            detect_loop(&text, REPEATS_REASONING).is_none(),
            "reasoning was called a loop: {text}"
        );
        assert!(
            detect_loop(&text, REPEATS_TEXT).is_none(),
            "answer text was called a loop: {text}"
        );
    }
}

#[test]
fn a_caught_loop_reports_the_block_and_keeps_what_came_before_it() {
    use crate::agent::trim_loop;
    use crate::agent::{detect_loop, REPEATS_TEXT};

    let answer = format!(
        "Nine findings follow. {}",
        "Let me write. Go. Now. OK. ".repeat(12)
    );
    let caught = detect_loop(&answer, REPEATS_TEXT).expect("a loop must be caught");
    // The sample is one period of the cycle named from where the run starts, so it
    // can be a rotation of what the model typed - it only has to be readable in the
    // notice, and one period long.
    assert_eq!(caught.block.chars().count(), 27, "{}", caught.block);
    assert!(
        format!("{b}{b}", b = caught.block).contains("Let me write. Go. Now. OK. "),
        "{}",
        caught.block
    );
    // Everything before the loop survives, including the full stop that ends it:
    // the cut lands on the first complete copy, not in the middle of one.
    let kept = trim_loop(&answer, caught.at);
    assert_eq!(kept, "Nine findings follow.");

    // A loop with no prose in front of it leaves an empty answer, not a fragment.
    let bare = "Let me write. Go. Now. OK. ".repeat(12);
    let caught = detect_loop(&bare, REPEATS_TEXT).expect("a loop must be caught");
    assert_eq!(trim_loop(&bare, caught.at), "");
}

#[tokio::test]
async fn a_provider_that_never_closes_after_finish_does_not_stall_the_turn() {
    // The runtime keeps reading after `finish_reason` to catch the trailing
    // usage block, but a gateway that holds the SSE connection open must not
    // keep the turn waiting: the window is bounded.
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![
        ProviderChunk::Text("answer".into()),
        ProviderChunk::Finish {
            reason: "stop".into(),
        },
    ])])
    .build()
    .await;

    let started = std::time::Instant::now();
    let events = h.run_to_finish("hi").await;
    assert_eq!(finished_status(&events), "completed");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the turn waited {:?} for a stream that never closed",
        started.elapsed()
    );
}

#[tokio::test]
async fn nothing_is_ever_sent_after_the_conversation() {
    // Hard-won: appending a runtime note (``system`` or ``user``, and even folded
    // into the last tool result) after a long history made this model deliberate
    // instead of acting - it burned its whole output budget on reasoning and never
    // emitted the write it was about to make. The same history without the note
    // committed immediately. So the request must be exactly the conversation:
    // one system prompt, then turns, and nothing synthetic at the end.
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{\"path\":\"a.txt\"}")]),
        Script::call(&[("call_2", "echo", "{\"path\":\"b.txt\"}")]),
        Script::text("done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let _ = h.run_to_finish("read both").await;
    for (i, request) in h.factory.requests().iter().enumerate() {
        let messages = &request.messages;
        assert_eq!(
            messages.iter().filter(|m| m.role == Role::System).count(),
            1,
            "request {i} carries an extra system message: it must be the only one"
        );
        assert_eq!(messages[0].role, Role::System);
        assert_ne!(
            messages.last().unwrap().role,
            Role::System,
            "request {i} ends with a synthetic system message"
        );
        assert!(
            !messages.iter().any(|m| {
                m.content.contains("runtime note") || m.content.contains("already opened")
            }),
            "request {i} carries a runtime note"
        );
    }
}

#[tokio::test]
async fn an_identical_read_is_blocked_instead_of_re_executed() {
    let arg = "{\"path\":\"a.txt\"}";
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", arg)]),
        Script::call(&[("call_2", "echo", arg)]),
        Script::call(&[("call_3", "echo", arg)]),
        Script::call(&[("call_4", "echo", arg)]),
        Script::call(&[("call_5", "echo", arg)]),
        Script::text("fine"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let _ = h.run_to_finish("read a.txt").await;
    let messages = h.session_messages().await;
    let tools: Vec<_> = messages.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 5);
    // 1-3 run and are answered identically; 4 and 5 are refused rather than
    // paying for the same page again.
    // The third identical answer is plain output (no note appended).
    assert!(!tools[2].content.contains("blocked"));
    assert!(
        tools[3].content.starts_with("[blocked:"),
        "{}",
        tools[3].content
    );
    assert!(
        tools[4].content.starts_with("[blocked:"),
        "{}",
        tools[4].content
    );
    // A block is a tool result, not a run failure.
    assert_eq!(h.store.runs()[0].status, "completed");
}

#[tokio::test]
async fn a_refused_repeat_is_let_through_rather_than_deadlocking_the_run() {
    // The refusal only knows what *this* loop has written. A subagent, the user or
    // an editor can change a file while `mutations` stays put, so a permanent block
    // would leave the model unable to re-read a page it knows changed. The refusal
    // is therefore bounded: the fourth and fifth calls are refused, the sixth runs.
    let arg = "{\"path\":\"a.txt\"}";
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", arg)]),
        Script::call(&[("call_2", "echo", arg)]),
        Script::call(&[("call_3", "echo", arg)]),
        Script::call(&[("call_4", "echo", arg)]),
        Script::call(&[("call_5", "echo", arg)]),
        Script::call(&[("call_6", "echo", arg)]),
        Script::text("fine"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let _ = h.run_to_finish("read a.txt").await;
    let messages = h.session_messages().await;
    let tools: Vec<_> = messages.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 6);
    assert!(
        tools[3].content.starts_with("[blocked:"),
        "{}",
        tools[3].content
    );
    assert!(
        tools[4].content.starts_with("[blocked:"),
        "{}",
        tools[4].content
    );
    assert!(
        !tools[5].content.starts_with("[blocked:"),
        "the block must not be permanent: {}",
        tools[5].content
    );
    assert_eq!(h.store.runs()[0].status, "completed");
}

#[tokio::test]
async fn an_image_tool_result_reaches_the_transcript_and_the_next_request() {
    // The pixels have to survive both hops: the persisted tool row (so a reload
    // still shows what the model saw) and the next request's messages, where
    // `kc-provider` re-emits them as a `user` message with an `image_url` part.
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "screenshot", "{}")]),
        Script::text("that is a blue button"),
    ])
    .tool(ImageTool::boxed("screenshot"))
    .model(kc_core::config::ModelConfig {
        supports_images: true,
        ..kc_core::config::ModelConfig::default()
    })
    .build()
    .await;

    let _ = h.run_to_finish("look at shot.png").await;
    let messages = h.session_messages().await;
    let tool_row = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("the tool result was persisted");
    assert_eq!(tool_row.images.len(), 1, "the image must be stored");
    assert_eq!(tool_row.images[0].media_type, "image/png");
    assert_eq!(tool_row.images[0].data, "iVBORw0KGgo=");

    let requests = h.factory.requests();
    assert_eq!(requests.len(), 2, "tool call, then the answer");
    let carried = requests[1]
        .messages
        .iter()
        .find(|m| m.role == kc_core::message::Role::Tool)
        .expect("the tool result is replayed");
    assert_eq!(carried.images.len(), 1, "the image is resent to the model");
}

#[tokio::test]
async fn a_text_only_model_gets_a_refusal_not_an_image() {
    // The runtime passes the session's effective model capability into the tool
    // context; with it off, no pixels are produced or stored.
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "screenshot", "{}")]),
        Script::text("I cannot see images"),
    ])
    .tool(ImageTool::boxed("screenshot"))
    .build()
    .await;

    let _ = h.run_to_finish("look at shot.png").await;
    let messages = h.session_messages().await;
    let tool_row = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("the refusal was persisted");
    assert!(tool_row.images.is_empty());
    assert!(
        tool_row.content.contains("cannot see images"),
        "{}",
        tool_row.content
    );
}

#[tokio::test]
async fn tool_call_result_is_fed_back_for_a_second_turn() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{\"path\":\"a.txt\"}")]),
        Script::text("all done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("read something").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "iteration_started"), 2);
    assert_eq!(count(&events, "tool_call_started"), 1);
    assert_eq!(count(&events, "tool_call_finished"), 1);

    let finished = tool_finished(&events);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].0, "call_1");
    assert!(finished[0].1, "tool should succeed");

    let messages = h.session_messages().await;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
    assert_eq!(messages[1].tool_calls.len(), 1);
    assert_eq!(messages[1].tool_calls[0].id, "call_1");
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(messages[2].name.as_deref(), Some("echo"));
    assert!(messages[2].content.contains("echoed:"));
    assert_eq!(messages[3].content, "all done");

    // The second request replays the assistant tool call and the tool result.
    let requests = h.factory.requests();
    assert_eq!(requests.len(), 2);
    let second = &requests[1].messages;
    assert!(second
        .iter()
        .any(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()));
    assert!(second
        .iter()
        .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("call_1")));
    assert_eq!(requests[1].model, "fake-model");
}

#[tokio::test]
async fn a_turn_cannot_exceed_its_byte_share() {
    // Half the context budget is this turn's allowance: a turn that returns more
    // would have its own results evicted before the model could read them.
    let limits = Limits {
        context_max_chars: 20_000,
        max_tool_calls_per_turn: 8,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("call_a", "big", "{\"bytes\":20000}"),
            ("call_b", "big", "{\"bytes\":20000}"),
        ]),
        Script::text("done"),
    ])
    .settings(FakeSettings::default().with_limits(limits))
    .tool(BigOutputTool::boxed("big"))
    .build()
    .await;

    let events = h.run_to_finish("read two big files").await;
    assert_eq!(finished_status(&events), "completed");

    let messages = h.session_messages().await;
    let tools: Vec<&kc_core::store::StoredMessage> =
        messages.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 2);
    let total: usize = tools.iter().map(|m| m.content.len()).sum();
    // 10_000 for the first call, the 2_000 floor for the second: the whole turn
    // stays inside one context budget instead of blowing through it.
    assert!(
        total <= 12_500,
        "two 20k results were kept whole: {total} bytes"
    );
    assert!(
        tools[0].content.contains("bytes omitted"),
        "the first result should be truncated, not dropped"
    );
}

#[tokio::test]
async fn multiple_tool_calls_run_and_persist_in_call_order() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("call_a", "echo", "{\"n\":1}"),
            ("call_b", "echo", "{\"n\":2}"),
        ]),
        Script::text("both done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("do two things").await;
    assert_eq!(finished_status(&events), "completed");

    let started: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolCallStarted { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["call_a".to_string(), "call_b".to_string()]);

    let finished: Vec<String> = tool_finished(&events).into_iter().map(|f| f.0).collect();
    assert_eq!(finished, vec!["call_a".to_string(), "call_b".to_string()]);

    let messages = h.session_messages().await;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "tool", "tool", "assistant"]
    );
    assert_eq!(messages[2].content, "echoed:{\"n\":1}");
    assert_eq!(messages[3].content, "echoed:{\"n\":2}");
}

// ---------------------------------------------------------------------------
// read-only concurrency
// ---------------------------------------------------------------------------

/// Ids of every `ToolCallStarted`, in the order the UI saw them.
fn tool_started(events: &[EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolCallStarted { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_read_only_turn_runs_concurrently_but_persists_in_call_order() {
    let probe = ProbeTool::read_only("probe");
    let peak = probe.peak();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            // The first call is deliberately the slowest, so completion order
            // (b, c, a) differs from call order (a, b, c).
            ("call_a", "probe", "{\"sleep_ms\":120,\"tag\":\"a\"}"),
            ("call_b", "probe", "{\"sleep_ms\":0,\"tag\":\"b\"}"),
            ("call_c", "probe", "{\"sleep_ms\":0,\"tag\":\"c\"}"),
        ]),
        Script::text("done"),
    ])
    .tool(probe.boxed())
    .build()
    .await;

    let events = h.run_to_finish("three independent reads").await;
    assert_eq!(finished_status(&events), "completed");

    // 1. They really overlapped: a strictly sequential turn never has more than
    //    one invocation in flight.
    assert_eq!(
        peak.load(Ordering::SeqCst),
        3,
        "the three read-only calls never ran at the same time"
    );

    // 2. The same set of events as the sequential path, one pair per call id,
    //    with the started events in call order.
    assert_eq!(
        tool_started(&events),
        vec![
            "call_a".to_string(),
            "call_b".to_string(),
            "call_c".to_string()
        ]
    );
    let finished = tool_finished(&events);
    assert_eq!(finished.len(), 3, "every call must finish exactly once");
    let mut finished_ids: Vec<&str> = finished.iter().map(|f| f.0.as_str()).collect();
    finished_ids.sort();
    assert_eq!(finished_ids, vec!["call_a", "call_b", "call_c"]);
    assert!(finished.iter().all(|f| f.1), "all three calls succeeded");
    // The interleaving is completion order - the slow call reports last - which
    // is fine because the two events are paired by id.
    assert_eq!(
        finished.last().map(|f| f.0.as_str()),
        Some("call_a"),
        "the 200ms call should have finished last: {finished:?}"
    );

    // 3. ...but the transcript is in CALL order, not completion order, and the
    //    next request is built from exactly those turns.
    let messages = h.session_messages().await;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "tool", "tool", "tool", "assistant"]
    );
    assert_eq!(
        messages[2].content, "probe:a",
        "history must be folded in call order: {messages:?}"
    );
    assert_eq!(messages[3].content, "probe:b");
    assert_eq!(messages[4].content, "probe:c");
    for (index, id) in ["call_a", "call_b", "call_c"].iter().enumerate() {
        assert_eq!(messages[2 + index].tool_call_id.as_deref(), Some(*id));
    }

    let requests = h.factory.requests();
    let second = requests.last().expect("a second request").clone();
    let carried: Vec<&str> = second
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        carried,
        vec!["probe:a", "probe:b", "probe:c"],
        "the next request must replay the results in call order"
    );
}

#[tokio::test]
async fn a_turn_with_one_mutating_call_stays_sequential() {
    let read = ProbeTool::read_only("probe");
    let write = ProbeTool::mutating("write_probe");
    let read_peak = read.peak();
    let write_peak = write.peak();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("call_a", "probe", "{\"sleep_ms\":50,\"tag\":\"a\"}"),
            ("call_b", "probe", "{\"sleep_ms\":20,\"tag\":\"b\"}"),
            ("call_w", "write_probe", "{\"sleep_ms\":0,\"tag\":\"w\"}"),
        ]),
        Script::text("done"),
    ])
    .tool(read.boxed())
    .tool(write.boxed())
    .build()
    .await;

    let events = h.run_to_finish("read two, write one").await;
    assert_eq!(finished_status(&events), "completed");

    // One mutating call in the turn and nothing overlaps: the sequential path.
    assert_eq!(
        read_peak.load(Ordering::SeqCst),
        1,
        "a turn carrying a mutating call must not run its reads concurrently"
    );
    assert_eq!(write_peak.load(Ordering::SeqCst), 1);

    // And the old guarantees still hold.
    assert_eq!(
        tool_started(&events),
        vec![
            "call_a".to_string(),
            "call_b".to_string(),
            "call_w".to_string()
        ]
    );
    let finished: Vec<String> = tool_finished(&events).into_iter().map(|f| f.0).collect();
    assert_eq!(
        finished,
        vec![
            "call_a".to_string(),
            "call_b".to_string(),
            "call_w".to_string()
        ],
        "the sequential path still finishes in call order"
    );
    let messages = h.session_messages().await;
    assert_eq!(messages[2].content, "probe:a");
    assert_eq!(messages[3].content, "probe:b");
    assert_eq!(messages[4].content, "probe:w");
}

#[tokio::test]
async fn a_failing_call_does_not_cost_its_siblings_their_results() {
    let probe = ProbeTool::read_only("probe");
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("call_a", "probe", "{\"sleep_ms\":10,\"tag\":\"a\"}"),
            ("call_boom", "boom", "{}"),
            ("call_panic", "panicky", "{}"),
            ("call_d", "probe", "{\"sleep_ms\":10,\"tag\":\"d\"}"),
        ]),
        Script::text("recovered"),
    ])
    .tool(probe.boxed())
    .tool(FailingTool::boxed("boom"))
    .tool(PanicTool::boxed("panicky"))
    .build()
    .await;

    let events = h.run_to_finish("four reads, two of them broken").await;
    assert_eq!(
        finished_status(&events),
        "completed",
        "a broken call must not fail the run"
    );

    // Every call answered, in call order, and the healthy ones kept their result.
    let messages = h.session_messages().await;
    let tools: Vec<&kc_core::store::StoredMessage> =
        messages.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 4, "all four calls need a result: {messages:?}");
    assert_eq!(tools[0].content, "probe:a");
    assert!(
        tools[1].content.contains("this call failed on purpose"),
        "{}",
        tools[1].content
    );
    assert!(
        tools[2].content.contains("panicked"),
        "a panic is that call's error result: {}",
        tools[2].content
    );
    assert_eq!(
        tools[3].content, "probe:d",
        "a sibling of a failing call keeps its result"
    );
    let ids: Vec<&str> = tools
        .iter()
        .map(|m| m.tool_call_id.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["call_a", "call_boom", "call_panic", "call_d"]);

    let finished = tool_finished(&events);
    let ok: std::collections::HashMap<&str, bool> = finished
        .iter()
        .map(|(id, ok, _, _)| (id.as_str(), *ok))
        .collect();
    assert_eq!(ok.get("call_a"), Some(&true));
    assert_eq!(ok.get("call_boom"), Some(&false));
    assert_eq!(ok.get("call_panic"), Some(&false));
    assert_eq!(ok.get("call_d"), Some(&true));
    // The request built after that turn is valid: every tool result still
    // answers the assistant's tool call, in call order.
    let requests = h.factory.requests();
    let follow_up = requests.last().expect("a second request");
    assert_no_orphan_tool_messages(&follow_up.messages);
}

#[tokio::test]
async fn a_turn_with_a_blocked_repeat_stays_sequential() {
    // Three identical reads first: the third answer earns the warning, and from
    // then on the same call is refused. A turn carrying that refusal must take the
    // sequential path - a `[blocked: ...]` result is a decision, and decisions are
    // made in call order before anything is spawned.
    let repeated = ProbeTool::read_only("probe_repeat");
    let fresh = ProbeTool::read_only("probe_fresh");
    let fresh_peak = fresh.peak();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("r1", "probe_repeat", "{\"sleep_ms\":0,\"tag\":\"same\"}"),
            ("r2", "probe_repeat", "{\"sleep_ms\":0,\"tag\":\"same\"}"),
        ]),
        Script::call(&[("r3", "probe_repeat", "{\"sleep_ms\":0,\"tag\":\"same\"}")]),
        Script::call(&[
            ("r4", "probe_repeat", "{\"sleep_ms\":0,\"tag\":\"same\"}"),
            ("b1", "probe_fresh", "{\"sleep_ms\":20,\"tag\":\"x\"}"),
            ("b2", "probe_fresh", "{\"sleep_ms\":20,\"tag\":\"y\"}"),
        ]),
        Script::text("done"),
    ])
    .tool(repeated.boxed())
    .tool(fresh.boxed())
    .build()
    .await;

    let events = h
        .run_to_finish("read the same thing twice, then again")
        .await;
    assert_eq!(finished_status(&events), "completed");
    // The fourth identical read was refused...
    let blocked = tool_finished(&events)
        .into_iter()
        .find(|(id, _, _, _)| id == "r4")
        .expect("r4 answered");
    assert!(!blocked.1, "the fourth identical read must be refused");
    assert!(blocked.2.contains("[blocked:"), "{}", blocked.2);
    // ...and because it was, the two fresh reads in that turn did not overlap.
    assert_eq!(
        fresh_peak.load(Ordering::SeqCst),
        1,
        "a turn with a repeat-blocked call must not run its other reads concurrently"
    );
    let messages = h.session_messages().await;
    let tools: Vec<&kc_core::store::StoredMessage> =
        messages.iter().filter(|m| m.role == "tool").collect();
    let ids: Vec<&str> = tools
        .iter()
        .map(|m| m.tool_call_id.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["r1", "r2", "r3", "r4", "b1", "b2"]);
}

#[tokio::test]
async fn cancelling_during_a_parallel_batch_still_ends_the_turn() {
    let a = SlowCancelTool::new("slow_a");
    let b = SlowCancelTool::new("slow_b");
    let a_started = a.started();
    let b_started = b.started();
    let mut h = HarnessBuilder::new(vec![Script::call(&[
        ("call_a", "slow_a", "{}"),
        ("call_b", "slow_b", "{}"),
    ])])
    .tool(Arc::new(a))
    .tool(Arc::new(b))
    .build()
    .await;

    h.submit("wait on two calls").await;
    // Both calls in flight at once before the cancel: neither tool can finish on
    // its own, so this only happens on the concurrent path. Events are drained
    // while waiting so the broadcast receiver cannot fall behind.
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while a_started.load(Ordering::SeqCst) < 1 || b_started.load(Ordering::SeqCst) < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "both parallel calls should have started"
        );
        while let Ok(event) = h.events.try_recv() {
            seen.push(event);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(h.runtime.cancel(h.session()), "cancel should find the run");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&seen), "cancelled");
    assert_eq!(count(&seen, "run_cancelled"), 1);
    // The turn ended promptly even though two tools were in flight, and the
    // transcript is still valid: every tool_call has its result.
    let messages = h.session_messages().await;
    let tools: Vec<&kc_core::store::StoredMessage> =
        messages.iter().filter(|m| m.role == "tool").collect();
    assert_eq!(tools.len(), 2, "both cancelled calls answer their call");
    assert!(h.wait_idle().await.is_empty());
}

// ---------------------------------------------------------------------------
// the workspace's instructions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_workspace_instructions_are_read_once_and_sent_on_every_request() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "rewrite", "{}")]),
        Script::text("done"),
    ])
    .tool(WriteWorkspaceFileTool::boxed(
        "rewrite",
        "AGENTS.md",
        "# AGENTS.md\nCHANGED MID-RUN: this must not reach the prompt.\n",
    ))
    .build()
    .await;
    std::fs::write(
        h.workspace_dir.join("AGENTS.md"),
        "# AGENTS.md\nORIGINAL RULE: run the tests before reporting.\n",
    )
    .expect("seed the instructions file");

    h.run_to_finish("follow the house rules").await;

    let requests = h.factory.requests();
    assert!(requests.len() >= 2, "the run should take two model turns");
    for (index, request) in requests.iter().enumerate() {
        let system = &request.messages[0];
        assert_eq!(system.role, Role::System, "message 0 must be the prompt");
        assert!(
            system
                .content
                .contains("ORIGINAL RULE: run the tests before reporting."),
            "request {index} lost the project's instructions"
        );
        assert!(
            system.content.contains("`AGENTS.md`"),
            "the block must name the file it came from"
        );
        assert!(
            !system.content.contains("CHANGED MID-RUN"),
            "request {index} re-read the file instead of reusing the run's copy"
        );
    }
    // Byte-identical, not merely equivalent: the cached prefix must not move.
    assert_eq!(
        requests[0].messages[0].content, requests[1].messages[0].content,
        "the instructions block changed between turns of one run"
    );
    // And the file on disk really did change, or the assertion above is vacuous.
    let on_disk = std::fs::read_to_string(h.workspace_dir.join("AGENTS.md")).unwrap();
    assert!(on_disk.contains("CHANGED MID-RUN"));
}

#[tokio::test]
async fn a_workspace_without_instructions_sends_no_instructions_block() {
    let mut h = HarnessBuilder::new(vec![Script::text("hi")]).build().await;
    h.run_to_finish("hello").await;
    let requests = h.factory.requests();
    assert!(
        !requests[0].messages[0]
            .content
            .contains("Project instructions:"),
        "an empty workspace grew an instructions block"
    );
}

// ---------------------------------------------------------------------------
// the prompt hook and the turn it answers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_legacy_prompt_hook_still_contributes_its_notes() {
    // A source that only implements `before_prompt`, as every embedding did
    // before `before_prompt_for_turn` existed: the default must delegate to it.
    let hook = LegacyPromptHook::new();
    let calls = hook.calls();
    let mut h = HarnessBuilder::new(vec![Script::text("hi")])
        .hooks(Arc::new(hook))
        .build()
        .await;

    h.run_to_finish("legacy question").await;
    let requests = h.factory.requests();
    assert!(
        requests[0].messages[0].content.contains("legacy hook note"),
        "the default before_prompt_for_turn dropped the legacy source's note"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "the runtime must call the hook, not bypass it"
    );
}

#[tokio::test]
async fn the_prompt_hook_sees_the_turn_it_is_answering() {
    let hook = TurnRecordingHook::new();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{\"n\":1}")]),
        Script::text("done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .hooks(Arc::new(hook.clone()))
    .build()
    .await;

    h.run_to_finish("what changed in the git log?").await;

    let requests = h.factory.requests();
    assert!(requests.len() >= 2);
    let canonical_workspace = std::fs::canonicalize(&h.workspace_dir)
        .expect("canonical workspace")
        .to_string_lossy()
        .to_string();
    let seen = hook.seen();
    assert_eq!(
        seen.len(),
        requests.len(),
        "the hook must run once per model call"
    );
    for (index, (workspace, turn)) in seen.iter().enumerate() {
        assert_eq!(
            turn.as_deref(),
            Some("what changed in the git log?"),
            "call {index} did not receive the user's turn"
        );
        assert_eq!(
            workspace, &canonical_workspace,
            "the workspace label must still be passed"
        );
    }
    // The old method must not have been used as a fallback by the runtime.
    assert!(
        !requests[0].messages[0].content.contains("WRONG HOOK"),
        "the runtime called before_prompt instead of before_prompt_for_turn"
    );
}

#[tokio::test]
async fn unknown_tool_is_a_tool_error_not_a_crash() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "does_not_exist", "{}")]),
        Script::text("recovered"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("call a ghost").await;
    assert_eq!(finished_status(&events), "completed");

    let finished = tool_finished(&events);
    assert_eq!(finished.len(), 1);
    assert!(!finished[0].1, "unknown tool must report a failure");
    assert!(finished[0].2.contains("unknown tool"));

    let messages = h.session_messages().await;
    let tool_message = messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("tool message");
    assert!(tool_message
        .content
        .contains("unknown tool `does_not_exist`"));
    assert!(tool_message.content.contains("echo"));
    assert_eq!(messages.last().unwrap().content, "recovered");
}

#[tokio::test]
async fn invalid_json_arguments_are_a_tool_error() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{not json")]),
        Script::text("ok"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("bad args").await;
    assert_eq!(finished_status(&events), "completed");
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(finished[0].2.contains("invalid JSON arguments"));

    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("invalid JSON arguments"));
}

/// The model sees a *working* call, not just a complaint, when its arguments
/// fail to parse - and the repair is shown to it, never executed.
#[tokio::test]
async fn a_malformed_call_gets_the_working_version_back() {
    // A fenced object with a trailing comma: the two mistakes that actually
    // happen. It must parse after repair, and the tool must not have run.
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "```json\n{\"path\": \"a.txt\",}\n```")]),
        Script::text("ok"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("bad args").await;
    assert_eq!(finished_status(&events), "completed");
    let finished = tool_finished(&events);
    assert!(
        !finished[0].1,
        "a repaired call is still not run for the model"
    );

    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(
        tool_message.content.contains("invalid JSON arguments"),
        "{}",
        tool_message.content
    );
    assert!(
        tool_message.content.contains(
            "Re-issue the call as: {\"name\":\"echo\",\"arguments\":{\"path\":\"a.txt\"}}"
        ),
        "the repaired, runnable call must be in the result: {}",
        tool_message.content
    );
    // No second `echoed:` result: a guess is shown, never executed.
    assert!(
        !messages
            .iter()
            .any(|m| m.role == "tool" && m.content.starts_with("echoed:")),
        "a repaired argument string must not be executed"
    );
}

#[tokio::test]
async fn an_unparseable_call_still_shows_the_required_shape() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write", "totally not json")]),
        Script::text("ok"),
    ])
    .tool(RequiredArgsTool::boxed("write", &["path", "content"]))
    .build()
    .await;

    let events = h.run_to_finish("bad args").await;
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("Required shape:") && finished[0].2.contains("\"path\""),
        "{}",
        finished[0].2
    );
}

#[tokio::test]
async fn arguments_that_violate_the_schema_are_refused_before_the_tool_runs() {
    let tool = RequiredArgsTool::boxed("write", &["path"]);
    let invoked = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        // `path` is present but has the wrong JSON type.
        Script::call(&[("call_1", "write", "{\"path\": 7}")]),
        Script::text("ok"),
    ])
    .tool(tool)
    .build()
    .await;

    let events = h.run_to_finish("bad args").await;
    assert_eq!(finished_status(&events), "completed");
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("arguments rejected for `write`")
            && finished[0].2.contains("`path` must be string"),
        "{}",
        finished[0].2
    );
    assert!(
        finished[0].2.contains("Re-issue the call as:"),
        "{}",
        finished[0].2
    );
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        0,
        "a call the schema rejects must not reach the tool"
    );
}

#[tokio::test]
async fn a_missing_required_argument_is_named_with_the_example() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write", "{}")]),
        Script::text("ok"),
    ])
    .tool(RequiredArgsTool::boxed("write", &["path", "content"]))
    .build()
    .await;

    let events = h.run_to_finish("bad args").await;
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("missing required argument `path`")
            && finished[0]
                .2
                .contains("missing required argument `content`"),
        "{}",
        finished[0].2
    );
}

/// A tool that refuses the request itself still gets a hint, while a tool that
/// simply succeeded at failing (a non-zero exit code) must not be dressed up
/// as a parse error.
#[tokio::test]
async fn execution_failures_do_not_claim_the_arguments_were_wrong() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "flaky", "{\"path\":\"a.txt\"}")]),
        Script::text("ok"),
    ])
    .tool(RequiredArgsTool::failing("flaky", &["path"]))
    .build()
    .await;

    let events = h.run_to_finish("run it").await;
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(finished[0].2.contains("boom"), "{}", finished[0].2);
    assert!(
        !finished[0].2.contains("how to call"),
        "an execution failure must not carry a usage hint: {}",
        finished[0].2
    );
}

#[tokio::test]
async fn a_tool_that_refuses_the_request_itself_gets_the_working_call() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "picky", "{}")]),
        Script::text("ok"),
    ])
    .tool(RequiredArgsTool::refusing_optional("picky", &["pattern"]))
    .build()
    .await;

    let events = h.run_to_finish("go").await;
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0]
            .2
            .contains("invalid request: pattern may not be empty"),
        "{}",
        finished[0].2
    );
    assert!(
        finished[0].2.contains(
            "[how to call `picky`: {\"name\":\"picky\",\"arguments\":{\"pattern\":\"<string>\"}}"
        ),
        "{}",
        finished[0].2
    );
}

#[tokio::test]
async fn a_typo_in_a_tool_name_is_corrected_in_the_error() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echoo", "{}")]),
        Script::text("ok"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("typo").await;
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("Did you mean `echo`?"),
        "{}",
        finished[0].2
    );
}

#[tokio::test]
async fn large_tool_output_is_truncated_to_the_configured_budget() {
    let limits = Limits {
        max_tool_output_bytes: 200,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[(
            "call_1",
            "echo",
            &format!("{{\"blob\":\"{}\"}}", "x".repeat(4000)),
        )]),
        Script::text("done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_limits(limits))
    .build()
    .await;

    let events = h.run_to_finish("big").await;
    let finished = tool_finished(&events);
    assert!(finished[0].3, "truncated flag should be set");
    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(
        tool_message.content.len() < 600,
        "tool content: {} bytes",
        tool_message.content.len()
    );
    assert!(tool_message.content.contains("bytes omitted"));
}

#[tokio::test]
async fn tool_calls_beyond_the_per_turn_cap_are_skipped_with_a_notice() {
    let limits = Limits {
        max_tool_calls_per_turn: 1,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("call_1", "echo", "{\"n\":1}"),
            ("call_2", "echo", "{\"n\":2}"),
            ("call_3", "echo", "{\"n\":3}"),
        ]),
        Script::text("done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_limits(limits))
    .build()
    .await;

    let events = h.run_to_finish("too many").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "tool_call_started"), 1);
    let notice = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Notice { message, .. } if message.contains("skipped 2 tool call") => {
                Some(message.clone())
            }
            _ => None,
        })
        .expect("notice about skipped calls");
    assert!(notice.contains("max_tool_calls_per_turn"));

    let messages = h.session_messages().await;
    let tools: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(tools.len(), 3, "every call needs a tool message: {tools:?}");
    assert!(tools[1].contains("skipped"));
    assert!(tools[2].contains("skipped"));
}

// ---------------------------------------------------------------------------
// approvals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approval_allow_once_lets_the_tool_run() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("done"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("run the tests").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    assert_eq!(approval.kind, ApprovalKind::ShellCommand);
    assert_eq!(approval.session_id, h.session());
    assert_eq!(approval.payload["command"], "cargo test");

    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect("resolve");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&seen), "completed");
    assert_eq!(count(&seen, "approval_resolved"), 1);
    let records = h.store.approvals();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, "resolved");
    assert_eq!(records[0].decision.as_deref(), Some("allow_once"));

    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("allowed:allow_once"));
}

#[tokio::test]
async fn an_allow_is_not_delivered_when_its_store_write_fails() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("persisted answer"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("approve carefully").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    h.store.fail_approval_resolutions(true);
    let error = h
        .runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect_err("the durable decision failed");
    assert!(matches!(error, Error::Store(_)), "{error:?}");

    // Give a wrongly woken tool every opportunity to execute. The decision,
    // registry waiter and transcript must all remain pending/pristine.
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(h.runtime.shared().approvals.pending_count(), 1);
    assert_eq!(h.store.approvals()[0].status, "pending");
    assert!(!h.session_messages().await.iter().any(|m| m.role == "tool"));

    h.store.fail_approval_resolutions(false);
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect("retry after the store recovers");
    seen.extend(h.collect_until_finished().await);
    assert_eq!(finished_status(&seen), "completed");
    assert_eq!(
        h.store.approvals()[0].decision.as_deref(),
        Some("allow_once")
    );
    assert!(h
        .session_messages()
        .await
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("allowed:allow_once")));
}

#[tokio::test]
async fn cancellation_during_decision_persistence_expires_without_waking_the_tool() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("unused"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;
    h.submit("cancel a resolving approval").await;
    let (event, _seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);

    let store_gate = h.store.gate_next("resolve_approval");
    let runtime = Arc::clone(&h.runtime);
    let id = approval.id.clone();
    let resolving = tokio::spawn(async move {
        runtime
            .resolve_approval(&id, ApprovalDecision::AllowOnce)
            .await
    });
    store_gate.wait_entered().await;
    assert!(h.runtime.cancel(h.session()));
    store_gate.release();
    let result = tokio::time::timeout(Duration::from_secs(5), resolving)
        .await
        .expect("resolution joins")
        .expect("resolution task did not panic");
    assert!(matches!(result, Err(Error::NotFound(_))), "{result:?}");

    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    assert_eq!(h.store.approvals()[0].status, "expired");
    assert!(!h
        .session_messages()
        .await
        .iter()
        .any(|m| m.role == "tool" && m.content.starts_with("allowed:")));
    assert_eq!(h.runtime.shared().approvals.pending_count(), 0);
}

#[tokio::test]
async fn approval_deny_blocks_the_tool_and_the_run_continues() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("understood"),
    ])
    .tool(ApprovalTool::shell("run", "rm -rf /"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("delete everything").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::Deny)
        .await
        .expect("resolve");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&seen), "completed");
    let finished = tool_finished(&seen);
    assert!(!finished[0].1);
    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("not allowed:deny"));
    assert_eq!(messages.last().unwrap().content, "understood");
    assert_eq!(h.store.approvals()[0].decision.as_deref(), Some("deny"));
}

#[tokio::test]
async fn approval_timeout_expires_the_request() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 0,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("moved on"),
    ])
    .tool(ApprovalTool::shell("run", "cargo build"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    let events = h.run_to_finish("build it").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 1);
    assert_eq!(count(&events, "approval_resolved"), 1);
    let resolved = events.iter().find_map(|e| match &e.event {
        Event::ApprovalResolved { decision, .. } => Some(*decision),
        _ => None,
    });
    assert_eq!(resolved, Some(ApprovalDecision::Expired));

    let records = h.store.approvals();
    assert_eq!(records[0].status, "expired");
    assert_eq!(
        h.runtime.shared().approvals.pending_count(),
        0,
        "the waiter entry must be removed on timeout"
    );
    assert!(h
        .store
        .list_pending_approvals(Some(h.session()))
        .await
        .unwrap()
        .is_empty());
    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("not allowed:expired"));
}

#[tokio::test]
async fn cancelling_while_an_approval_is_pending_releases_the_waiter() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("unused"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("wait for approval").await;
    let (_event, _seen) = wait_for_kind(&mut h, "approval_requested").await;
    assert_eq!(h.runtime.shared().approvals.pending_count(), 1);

    h.runtime.shutdown().await;
    assert_eq!(
        h.runtime.shared().approvals.pending_count(),
        0,
        "shutdown must release every parked approval waiter"
    );
    assert!(h
        .store
        .list_pending_approvals(Some(h.session()))
        .await
        .unwrap()
        .is_empty());

    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn allow_session_is_remembered_for_later_identical_requests() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::call(&[("call_2", "run", "{}")]),
        Script::text("both ran"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("test twice").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowSession)
        .await
        .expect("resolve");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&seen), "completed");
    assert_eq!(
        count(&seen, "approval_requested"),
        1,
        "the second identical request must be auto-allowed: {:?}",
        kinds(&seen)
    );
    assert_eq!(h.store.approvals().len(), 1);
    let messages = h.session_messages().await;
    let tools: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(tools.len(), 2);
    assert!(tools[0].contains("allowed:allow_session"), "{:?}", tools);
    assert!(tools[1].contains("allowed:allow_once"), "{:?}", tools);
}

#[tokio::test]
async fn resolving_unknown_approvals_is_a_not_found_error() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;
    let err = h
        .runtime
        .resolve_approval("ap_missing", ApprovalDecision::AllowOnce)
        .await
        .expect_err("must fail");
    match err {
        Error::NotFound(message) => assert!(message.contains("ap_missing")),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn a_second_resolve_of_the_same_approval_errors_clearly() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("ok"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("go").await;
    let (event, _seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect("first resolve");
    let err = h
        .runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect_err("second resolve must fail");
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    let _ = h.collect_until_finished().await;
}

#[tokio::test]
async fn policy_allow_and_deny_never_create_approval_rows() {
    let mut auto = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("done"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::Auto,
        ..Default::default()
    }))
    .build()
    .await;
    let events = auto.run_to_finish("auto").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(auto.store.approvals().len(), 0);

    let mut deny = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "run", "{}")]),
        Script::text("done"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::ReadOnly,
        ..Default::default()
    }))
    .build()
    .await;
    let events = deny.run_to_finish("read only").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(deny.store.approvals().len(), 0);
    let denied = tool_finished(&events);
    assert!(!denied.is_empty() && !denied[0].1);
}

// ---------------------------------------------------------------------------
// external (plugin / MCP) tool gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_tool_in_ask_mode_asks_and_runs_when_allowed() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let tool = Arc::new(ExternalTool::mcp("mcp_write", "fs"));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[(
            "call_1",
            "mcp_write",
            "{\"path\":\"outside.txt\",\"contents\":\"pwned\"}",
        )]),
        Script::text("done"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("write outside the workspace").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    assert_eq!(approval.kind, ApprovalKind::Mcp);
    assert_eq!(approval.title, "Call mcp_write");
    assert_eq!(approval.session_id, h.session());
    assert!(
        approval.detail.contains("mcp:fs"),
        "detail must name the origin: {}",
        approval.detail
    );
    assert!(approval.detail.contains("outside.txt"));
    assert_eq!(approval.payload["tool"], "mcp_write");
    assert_eq!(approval.payload["arguments"]["path"], "outside.txt");
    assert_eq!(approval.payload["origin"]["kind"], "mcp");
    assert_eq!(invocations.load(Ordering::SeqCst), 0, "not invoked yet");

    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect("resolve");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&seen), "completed");
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("invoked:mcp_write"));
    assert_eq!(h.store.approvals()[0].status, "resolved");
}

#[tokio::test]
async fn denying_an_mcp_approval_skips_the_tool_and_the_run_continues() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let tool = Arc::new(ExternalTool::mcp("mcp_write", "fs"));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "mcp_write", "{\"path\":\"/tmp/x\"}")]),
        Script::text("continued"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("try to write").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::Deny)
        .await
        .expect("resolve");
    seen.extend(h.collect_until_finished().await);

    // The refusal is a tool error; the run keeps going.
    assert_eq!(finished_status(&seen), "completed");
    assert_eq!(invocations.load(Ordering::SeqCst), 0, "must not be invoked");
    let finished = tool_finished(&seen);
    assert_eq!(finished.len(), 1);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("denied by user"),
        "{}",
        finished[0].2
    );
    assert_eq!(
        h.session_messages().await.last().unwrap().content,
        "continued"
    );
}

#[tokio::test]
async fn plugin_tool_is_denied_without_invoking_in_read_only_mode() {
    let tool = Arc::new(ExternalTool::plugin("plugin_write", "fs-helper"));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "plugin_write", "{\"path\":\"/etc/passwd\"}")]),
        Script::text("after denial"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::ReadOnly,
        ..Default::default()
    }))
    .build()
    .await;

    let events = h.run_to_finish("try a mutation").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(h.store.approvals().len(), 0);
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        0,
        "side effect happened"
    );
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("denied by policy: read_only mode"),
        "{}",
        finished[0].2
    );
    assert_eq!(
        h.session_messages().await.last().unwrap().content,
        "after denial"
    );
}

#[tokio::test]
async fn external_tool_without_approval_requirement_runs_silently() {
    let tool = Arc::new(ExternalTool::mcp("mcp_read", "fs").with_require_approval(false));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "mcp_read", "{}")]),
        Script::text("done"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::Ask,
        ..Default::default()
    }))
    .build()
    .await;

    let events = h.run_to_finish("read").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(h.store.approvals().len(), 0);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn read_only_external_tool_does_not_prompt_in_ask_mode() {
    let tool = Arc::new(ExternalTool::new(
        "plugin_read",
        ToolOrigin::Plugin {
            name: "reader".into(),
        },
        ToolClass::ReadOnly,
    ));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "plugin_read", "{}")]),
        Script::text("done"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::Ask,
        ..Default::default()
    }))
    .build()
    .await;

    let events = h.run_to_finish("inspect").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn auto_mode_allows_external_tools_without_a_prompt() {
    let tool = Arc::new(ExternalTool::mcp("mcp_write", "fs"));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "mcp_write", "{}")]),
        Script::text("done"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::Auto,
        ..Default::default()
    }))
    .build()
    .await;

    let events = h.run_to_finish("allowed").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(h.store.approvals().len(), 0);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// cancellation, limits, provider failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_mid_stream_marks_the_run_cancelled() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text(
        "partial answer".into(),
    )])])
    .build()
    .await;

    h.submit("talk forever").await;
    let (event, mut seen) = wait_for_kind(&mut h, "text_delta").await;
    match &event.event {
        Event::TextDelta { delta } => assert_eq!(delta, "partial answer"),
        other => panic!("expected text_delta, got {other:?}"),
    }
    assert!(h.runtime.cancel(h.session()), "cancel should find the run");
    seen.extend(h.collect_until_finished().await);

    assert_eq!(count(&seen, "run_cancelled"), 1);
    assert_eq!(finished_status(&seen), "cancelled");
    let runs = h.store.runs();
    assert_eq!(runs[0].status, "cancelled");
    let messages = h.session_messages().await;
    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant")
        .expect("partial text persisted");
    assert_eq!(assistant.content, "partial answer");
    assert!(h.wait_idle().await.is_empty());
    assert!(!h.runtime.is_running(h.session()));
    assert!(!h.runtime.cancel("s_unknown_session"));
}

#[tokio::test]
async fn iteration_limit_stops_the_run_with_limit_reached() {
    // An operator-set ceiling still works, and it is a plain stop: the run ends
    // `limit_reached` and nothing is appended to the transcript to explain it.
    let limits = Limits {
        max_iterations: 2,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{}")]),
        Script::call(&[("call_2", "echo", "{}")]),
        Script::text("never reached"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_limits(limits))
    .build()
    .await;

    let events = h.run_to_finish("loop forever").await;
    assert_eq!(finished_status(&events), "limit_reached");
    assert_eq!(count(&events, "iteration_started"), 2);
    let notice = events.iter().any(
        |e| matches!(&e.event, Event::Notice { message, .. } if message.contains("max_iterations")),
    );
    assert!(
        notice,
        "expected a notice about the iteration limit: {:?}",
        kinds(&events)
    );
    assert_eq!(
        h.factory.requests().len(),
        2,
        "the third script must not be consumed"
    );
    assert_eq!(h.store.runs()[0].status, "limit_reached");
}

#[tokio::test]
async fn a_run_keeps_going_past_a_hundred_turns_without_a_continuation_turn() {
    // There is no segment any more: `max_iterations = 0` means the run is not
    // counting turns at all, so a long job runs to its own ending. Nothing is
    // injected into the conversation to "resume" it - a synthetic user turn would
    // be re-billed on every later request and could read to the model as a new
    // instruction, which is exactly what a long-horizon run must not have.
    let limits = Limits {
        max_iterations: 0,
        ..Default::default()
    };
    // Every turn asks something different, which is what a real long run looks
    // like: the point is that 40 turns in, nothing has interrupted it.
    let mut scripts: Vec<Script> = (0..40)
        .map(|i| {
            Script::call(&[(
                Box::leak(format!("call_{i}").into_boxed_str()),
                "echo",
                Box::leak(format!("{{\"text\":\"turn {i}\"}}").into_boxed_str()),
            )])
        })
        .collect();
    scripts.push(Script::text("finished the long job"));
    let mut h = HarnessBuilder::new(scripts)
        .tool(EchoTool::boxed("echo"))
        .settings(FakeSettings::default().with_limits(limits))
        .build()
        .await;

    let events = h.run_to_finish("a job that outlives any budget").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "iteration_started"), 41, "40 tool turns + the answer");
    assert_eq!(h.factory.requests().len(), 41, "every script was used");

    let messages = h.session_messages().await;
    assert!(
        !messages
            .iter()
            .any(|m| m.role == "user" && m.content.contains("runtime checkpoint")),
        "no continuation turn may be injected: {:?}",
        messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
    // One user turn in, one final answer out - nothing else was added.
    assert_eq!(
        messages.iter().filter(|m| m.role == "user").count(),
        1,
        "the run wrote no user turns of its own"
    );
    assert_eq!(h.store.runs()[0].status, "completed");
}

#[tokio::test]
async fn a_run_that_repeats_itself_stops_instead_of_burning_turns() {
    // The replacement for the segment counter: with no turn budget, a model that
    // re-issues the same calls and receives the same answers forever is caught by
    // evidence - four identical turns in a row. A write in between is progress,
    // so it resets (covered by the fix→test→fix tests elsewhere).
    let limits = Limits {
        max_iterations: 0,
        ..Default::default()
    };
    let identical = || Script::call(&[("call_same", "echo", "{\"text\":\"same\"}")]);
    // Enough copies that the guard, not the script list, has to be what ends it.
    let mut scripts = vec![identical(); 12];
    scripts.push(Script::text("never reached"));
    let mut h = HarnessBuilder::new(scripts)
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_limits(limits))
    .build()
    .await;

    let events = h.run_to_finish("a question already answered").await;
    assert_eq!(
        finished_status(&events),
        "limit_reached",
        "the loop guard must end the run: {:?}",
        kinds(&events)
    );
    let stopped = events.iter().any(|e| {
        matches!(&e.event, Event::Notice { message, .. }
            if message.contains("no new information"))
    });
    assert!(stopped, "the stop must say why: {:?}", kinds(&events));
    assert!(
        h.factory.requests().len() < 12,
        "the guard must end the run, not the script list: {}",
        h.factory.requests().len()
    );
}

#[tokio::test]
async fn provider_failure_marks_the_run_failed_with_a_visible_error() {
    let mut h = HarnessBuilder::new(vec![Script::Fail("connection refused".into())])
        .build()
        .await;

    let events = h.run_to_finish("this will fail").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(count(&events, "error"), 1);
    let error = events.iter().find_map(|e| match &e.event {
        Event::Error { message, kind } => Some((message.clone(), kind.clone())),
        _ => None,
    });
    let (message, kind) = error.expect("error event");
    assert!(message.contains("connection refused"), "{message}");
    assert_eq!(kind.as_deref(), Some("provider"));

    let runs = h.store.runs();
    assert_eq!(runs[0].status, "failed");
    assert!(runs[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("connection refused"));

    let finished = events.iter().rev().find_map(|e| match &e.event {
        Event::RunFinished { error, .. } => Some(error.clone()),
        _ => None,
    });
    assert!(
        finished.flatten().is_some(),
        "run_finished must carry the error"
    );
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn provider_error_chunk_keeps_the_partial_assistant_text() {
    // With mid-stream restarts switched off, a turn that dies after streaming
    // text fails honestly and keeps what the user already saw. (With the
    // default budget the same failure is restarted instead - see
    // `a_stream_that_dies_mid_answer_is_restarted_instead_of_failing`.)
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("half an answer".into()),
        ProviderChunk::Error {
            message: "stream broke mid-sentence".into(),
            retryable: true,
        },
    ])])
    .model(restarts(0))
    .build()
    .await;

    let events = h.run_to_finish("fail partially").await;
    assert_eq!(finished_status(&events), "failed");
    assert!(count(&events, "error") == 1);
    assert_eq!(count(&events, "stream_retry"), 0);

    let messages = h.session_messages().await;
    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant")
        .expect("partial text");
    assert_eq!(assistant.content, "half an answer");
    assert!(assistant.tool_calls.is_empty());
}

#[tokio::test]
async fn a_failed_turn_keeps_the_reasoning_it_had_already_streamed() {
    // The reader watched the thought arrive; a failure must not erase it from the
    // transcript, which is what a reload would show. A thought-only failure has no
    // prose at all, so the row is reasoning and nothing else.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Reasoning("check the headers before the body".into()),
        ProviderChunk::Error {
            message: "stream broke before any text".into(),
            retryable: true,
        },
    ])])
    .model(restarts(0))
    .build()
    .await;

    let events = h.run_to_finish("fail after thinking").await;
    assert_eq!(finished_status(&events), "failed");

    let messages = h.session_messages().await;
    let assistant = messages
        .iter()
        .find(|m| m.role == "assistant")
        .expect("partial thought");
    assert_eq!(assistant.content, "");
    assert_eq!(
        assistant.reasoning_content.as_deref(),
        Some("check the headers before the body")
    );
}

/// Model config with a small restart budget and a millisecond backoff, so a
/// restart test never sleeps through the production delays.
fn restarts(max_restarts: u32) -> ModelConfig {
    ModelConfig {
        retry_max_stream_restarts: max_restarts,
        retry_base_delay_ms: 1,
        retry_max_delay_ms: 2,
        ..ModelConfig::default()
    }
}

/// The failure this feature exists for: the gateway streams some reasoning and
/// then the body dies (`error decoding response body`). Nothing of the answer
/// was persisted, so the turn must be re-issued instead of failing the run.
#[tokio::test]
async fn a_stream_that_dies_mid_answer_is_restarted_instead_of_failing() {
    let mut h = HarnessBuilder::new(vec![
        Script::ChunksThenFail(
            vec![ProviderChunk::Reasoning("let me think about it".into())],
            "provider error: invalid response body from http://gateway/v1/chat/completions: \
             error decoding response body"
                .into(),
        ),
        Script::text("the answer"),
    ])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("keep going").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "error"), 0);

    let (attempt, max_attempts, reason) = events
        .iter()
        .find_map(|e| match &e.event {
            Event::StreamRetry {
                attempt,
                max_attempts,
                reason,
            } => Some((*attempt, *max_attempts, reason.clone())),
            _ => None,
        })
        .expect("a stream_retry event so the client can drop the partial attempt");
    assert_eq!(attempt, 1, "the first attempt died");
    assert_eq!(max_attempts, 4, "one try plus three restarts");
    assert!(
        reason.contains("decoding response body"),
        "the reason must name the real failure: {reason}"
    );

    assert_eq!(
        h.factory.requests().len(),
        2,
        "the identical request must be re-issued exactly once"
    );
    let assistants: Vec<_> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistants.len(), 1, "only the successful attempt persists");
    assert_eq!(assistants[0].content, "the answer");
}

#[tokio::test]
async fn a_restarted_attempt_discards_its_partial_tool_calls() {
    // Tool-call deltas of the abandoned attempt must not be dispatched or
    // persisted: the fresh attempt starts from the same history.
    let mut h = HarnessBuilder::new(vec![
        Script::ChunksThenFail(
            vec![ProviderChunk::ToolCallDelta {
                index: 0,
                id: Some("call_abandoned".into()),
                name: Some("no_such_tool".into()),
                arguments_delta: Some("{}".into()),
            }],
            "provider error: connection reset by peer".into(),
        ),
        Script::text("answered without tools"),
    ])
    .model(restarts(1))
    .build()
    .await;

    let events = h.run_to_finish("answer").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "tool_call_started"), 0);
    let messages = h.session_messages().await;
    assert!(messages.iter().all(|m| m.role != "tool"));
    assert!(messages
        .iter()
        .any(|m| m.role == "assistant" && m.content == "answered without tools"));
}

#[tokio::test]
async fn mid_stream_restarts_are_bounded_and_the_last_error_is_reported() {
    let scripts: Vec<Script> = (0..3)
        .map(|i| {
            Script::ChunksThenFail(
                vec![ProviderChunk::Text(format!("attempt {i}"))],
                "provider error: invalid response body: error decoding response body".into(),
            )
        })
        .collect();
    let mut h = HarnessBuilder::new(scripts)
        .model(restarts(2))
        .build()
        .await;

    let events = h.run_to_finish("answer").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(h.factory.requests().len(), 3, "one try plus two restarts");
    assert_eq!(count(&events, "stream_retry"), 2, "one event per restart");

    let error = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("the exhausted turn reports why it failed");
    assert!(error.contains("decoding response body"), "{error}");

    // The last attempt's partial text is what the user keeps, and no duplicate
    // from the abandoned attempts is persisted.
    let assistants: Vec<_> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0].content, "attempt 2");
}

#[tokio::test]
async fn a_rejected_request_is_not_restarted() {
    // A request-level error chunk (an unsupported option, an unknown model)
    // cannot be fixed by asking again, so the restart budget must stay
    // untouched. A *context overflow* is deliberately not this case any more:
    // it is recovered by compacting the window once (see
    // `an_overflowing_request_is_compacted_and_retried_once`), which is a change
    // to the request rather than a repeat of it.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![ProviderChunk::Error {
        message: "provider error: unsupported parameter: max_completion_tokens".into(),
        retryable: false,
    }])])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("this endpoint refuses the request").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(count(&events, "stream_retry"), 0);
    assert_eq!(h.factory.requests().len(), 1, "no second request");
}

/// The failure that started this work, verbatim from a real gateway: a session
/// whose history outgrew the item ceiling is refused even though it sits far
/// under any character budget.
const ITEM_CEILING_REFUSAL: &str = "provider error: [400]: [code=context_length_exceeded] \
     Input is too long: conversation history exceeds the limit on input items \
     (1002 > 1000 items). Compact the conversation and retry. If compaction also \
     exceeds the limit, shorten the history or start a new session.";

#[tokio::test]
async fn an_overflowing_request_is_compacted_and_retried_once() {
    // Two scripts: the first request is refused for size, the second (a
    // re-compacted window, same conversation) answers. The run must reach
    // `completed`, the user must be told why, and exactly one retry must be
    // issued.
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![ProviderChunk::Error {
            message: ITEM_CEILING_REFUSAL.into(),
            retryable: false,
        }]),
        Script::text("recovered"),
    ])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("keep working").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "error"), 0, "an overflow is not a run error");

    // The notice names the reason and the setting that decided it.
    let notices: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::Notice { level, message } if level == "warn" => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|message| message.contains("too large")
            && message.contains("limits.context_max_chars")
            && message.contains("limits.max_context_messages")),
        "the forced compaction must be visible and name the setting: {notices:?}"
    );

    // Exactly one retry, of a request no larger than the first one.
    let requests = h.factory.requests();
    assert_eq!(requests.len(), 2, "one forced retry, not a loop");
    assert!(
        requests[1].messages.len() <= requests[0].messages.len(),
        "the retried window must not be bigger than the refused one"
    );

    // No duplicated assistant turn: the refused attempt persisted nothing, so
    // the transcript holds the retried answer once.
    let assistants: Vec<_> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistants.len(), 1, "{assistants:?}");
    assert_eq!(assistants[0].content, "recovered");
}

#[tokio::test]
async fn an_overflow_that_repeats_fails_the_run_without_a_third_request() {
    // The cut did not help: the second request is refused too. That is a real
    // failure - the run must say a forced compaction was already attempted, and
    // must not keep re-issuing the same refusal.
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![ProviderChunk::Error {
            message: ITEM_CEILING_REFUSAL.into(),
            retryable: false,
        }]),
        Script::Chunks(vec![ProviderChunk::Error {
            message: ITEM_CEILING_REFUSAL.into(),
            retryable: false,
        }]),
        Script::text("a third answer that must never be asked for"),
    ])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("keep working").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(h.factory.requests().len(), 2, "never a third request");

    let error = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Error { message, kind } => Some((message.clone(), kind.clone())),
            _ => None,
        })
        .expect("an error event");
    assert_eq!(error.1.as_deref(), Some("provider"));
    // The provider's own message survives, and the run admits what it already
    // tried instead of hiding it.
    assert!(error.0.contains("input items"), "{}", error.0);
    assert!(
        error.0.contains("forced compaction") && error.0.contains("already attempted"),
        "{}",
        error.0
    );
    assert_eq!(h.store.runs()[0].status, "failed");
}

#[tokio::test]
async fn an_overflow_on_a_later_turn_is_not_retried_a_second_time() {
    // One forced compaction per *run*, not per turn: the first turn spends it,
    // the second turn's refusal fails the run rather than silently re-cutting
    // the conversation every time the gateway says no.
    let mut h = HarnessBuilder::new(vec![
        // turn 1: refused for size, then answered with a tool call.
        Script::Chunks(vec![ProviderChunk::Error {
            message: ITEM_CEILING_REFUSAL.into(),
            retryable: false,
        }]),
        Script::call(&[("call_1", "echo", "{\"n\":1}")]),
        // turn 2: refused again. There is no second forced retry left.
        Script::Chunks(vec![ProviderChunk::Error {
            message: ITEM_CEILING_REFUSAL.into(),
            retryable: false,
        }]),
    ])
    .model(restarts(3))
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    let events = h.run_to_finish("keep working").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(
        h.factory.requests().len(),
        3,
        "two requests for turn 1, one for turn 2 - never a loop"
    );
    let error = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("an error event");
    assert!(error.contains("already"), "{error}");
}

#[tokio::test]
async fn a_partial_answer_before_an_overflow_is_not_persisted_twice() {
    // A gateway can stream a little and *then* reject; nothing was persisted for
    // that attempt, so the retry must not leave the partial text in the
    // transcript next to the real answer.
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![
            ProviderChunk::Text("half an answer".into()),
            ProviderChunk::Error {
                message: ITEM_CEILING_REFUSAL.into(),
                retryable: false,
            },
        ]),
        Script::text("the whole answer"),
    ])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("keep working").await;
    assert_eq!(finished_status(&events), "completed");
    // The client is told to drop what it rendered.
    assert_eq!(count(&events, "stream_retry"), 1);

    let assistants: Vec<_> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistants.len(), 1, "{assistants:?}");
    assert_eq!(assistants[0].content, "the whole answer");
}

#[tokio::test]
async fn a_body_that_stops_without_a_finish_reason_is_restarted() {
    let mut h = HarnessBuilder::new(vec![
        Script::Chunks(vec![ProviderChunk::Text("one and a half".into())]),
        Script::text("one and a half plus the rest"),
    ])
    .model(restarts(1))
    .build()
    .await;

    let events = h.run_to_finish("answer").await;
    assert_eq!(finished_status(&events), "completed");
    let reason = events
        .iter()
        .find_map(|e| match &e.event {
            Event::StreamRetry { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("a restart for the cut body");
    assert!(reason.contains("finish_reason"), "{reason}");
    assert!(h
        .session_messages()
        .await
        .iter()
        .any(|m| m.role == "assistant" && m.content == "one and a half plus the rest"));
}

#[tokio::test]
async fn finish_reason_length_is_reported_and_not_success() {
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("half an answer".into()),
        ProviderChunk::Finish {
            reason: "length".into(),
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("write a very long answer").await;
    assert_eq!(
        finished_status(&events),
        "limit_reached",
        "a truncated answer must not be reported as success"
    );
    let notice = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Notice { level, message }
                if level == "warn" && message.contains("max_tokens") =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .expect("a warn notice about max_tokens");
    assert!(
        notice.contains("incomplete") && notice.contains("truncated"),
        "notice must mark the text as incomplete/truncated: {notice}"
    );
    assert_eq!(count(&events, "error"), 0, "length is not a provider error");

    let messages = h.session_messages().await;
    assert!(messages
        .iter()
        .any(|m| m.role == "assistant" && m.content == "half an answer"));
}

#[tokio::test]
async fn a_stream_that_ends_without_a_finish_reason_fails_visibly() {
    // No Finish chunk: the provider closed the stream without saying why. With
    // restarts disabled that is a visible failure, and the partial text is kept.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![ProviderChunk::Text(
        "partial".into(),
    )])])
    .model(restarts(0))
    .build()
    .await;

    let events = h.run_to_finish("answer").await;
    assert_eq!(finished_status(&events), "failed");
    let error = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("an explicit provider error");
    assert!(
        error.contains("finish_reason"),
        "unexpected error message: {error}"
    );

    let messages = h.session_messages().await;
    assert!(messages
        .iter()
        .any(|m| m.role == "assistant" && m.content == "partial"));
}

#[tokio::test]
async fn a_stream_that_ends_with_the_providers_eof_reason_is_not_success() {
    // The real provider does not leave `finish_reason` as `None` when the body
    // is cut: `stream.rs` synthesises `Finish { reason: "eof" }` so the runtime
    // can tell "the gateway stopped talking" from "the model finished". If the
    // runtime only checks for `None`, that synthesis makes every truncated
    // stream look like a clean stop and the partial answer is persisted as a
    // normal, successful turn.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("half an answer".into()),
        ProviderChunk::Finish {
            reason: "eof".into(),
        },
    ])])
    .model(restarts(0))
    .build()
    .await;

    let events = h.run_to_finish("answer").await;
    assert_eq!(
        finished_status(&events),
        "failed",
        "an `eof` finish is a cut stream, not a completed answer"
    );
    let error = events
        .iter()
        .find_map(|e| match &e.event {
            Event::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("an explicit provider error");
    assert!(
        error.contains("finish_reason"),
        "unexpected error message: {error}"
    );
    // The partial text is still kept: losing it would be worse than showing it.
    assert!(h
        .session_messages()
        .await
        .iter()
        .any(|m| m.role == "assistant" && m.content == "half an answer"));
}

#[tokio::test]
async fn second_submit_for_the_same_session_is_rejected() {
    let mut h = HarnessBuilder::new(vec![
        Script::Hang(vec![ProviderChunk::Text("busy".into())]),
        Script::text("unused"),
    ])
    .build()
    .await;

    h.submit("first").await;
    let err = h
        .runtime
        .submit(h.session(), "second".to_string())
        .await
        .expect_err("concurrent run must be rejected");
    assert!(matches!(err, Error::BadRequest(_)), "got {err:?}");
    assert!(err.to_string().contains("already active"));

    // Wait until the first run is actually streaming before cancelling it.
    let _ = wait_for_kind(&mut h, "text_delta").await;
    assert!(h.runtime.cancel(h.session()));
    assert_eq!(
        finished_status(&h.collect_until_finished().await),
        "cancelled"
    );

    // Once the run finished the session accepts a new one.
    assert!(h.wait_idle().await.is_empty());
    h.runtime
        .submit(h.session(), "third".to_string())
        .await
        .expect("accepted after the previous run finished");
    let third = h.collect_until_finished().await;
    assert_eq!(finished_status(&third), "completed");
}

#[tokio::test]
async fn submitting_to_an_unknown_session_is_not_found() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;
    let err = h
        .runtime
        .submit("s_missing", "hello".to_string())
        .await
        .expect_err("unknown session");
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");

    let err = h
        .runtime
        .submit(h.session(), "   ".to_string())
        .await
        .expect_err("empty prompt");
    assert!(matches!(err, Error::BadRequest(_)), "got {err:?}");
}

#[tokio::test]
async fn shutdown_cancels_active_runs() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("hi".into())])])
        .build()
        .await;
    h.submit("stay alive").await;
    let _ = wait_for_kind(&mut h, "text_delta").await;

    tokio::time::timeout(Duration::from_secs(5), h.runtime.shutdown())
        .await
        .expect("shutdown must not hang");
    assert!(h.runtime.active_runs().is_empty());
}

#[tokio::test]
async fn a_store_failure_while_starting_a_run_does_not_leave_it_running() {
    let mut h = HarnessBuilder::new(vec![Script::text("never used")])
        .build()
        .await;
    h.store.fail_appends(true);
    let err = h
        .runtime
        .submit(h.session(), "hello".to_string())
        .await
        .expect_err("submit must report the storage failure");
    assert!(matches!(err, Error::Store(_)), "got {err:?}");
    h.store.fail_appends(false);

    assert!(
        h.runtime.active_runs().is_empty(),
        "no half-started run may stay active"
    );
    let runs = h.store.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
    assert!(runs[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("disk is full"));

    // The session is usable again afterwards.
    let events = h.run_to_finish("try again").await;
    assert_eq!(finished_status(&events), "completed");
}

#[tokio::test]
async fn a_panicking_tool_becomes_a_tool_error() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "boom", "{}")]),
        Script::text("recovered"),
    ])
    .tool(PanicTool::boxed("boom"))
    .build()
    .await;

    let events = h.run_to_finish("explode").await;
    assert_eq!(finished_status(&events), "completed");
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(finished[0].2.contains("panicked"));
    let messages = h.session_messages().await;
    assert_eq!(messages.last().unwrap().content, "recovered");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn a_panicking_provider_fails_the_run_without_wedging_the_session() {
    let mut h = HarnessBuilder::new(vec![
        Script::Panic("provider blew up".into()),
        Script::text("second attempt works"),
    ])
    .build()
    .await;

    let events = h.run_to_finish("boom").await;
    assert_eq!(finished_status(&events), "failed");
    assert!(events.iter().any(
        |e| matches!(&e.event, Event::Error { message, .. } if message.contains("provider blew up"))
    ));
    let runs = h.store.runs();
    assert_eq!(runs[0].status, "failed");
    assert!(
        h.wait_idle().await.is_empty(),
        "the run must be unregistered after a panic"
    );

    let second = h.run_to_finish("try again").await;
    assert_eq!(finished_status(&second), "completed");
}

#[tokio::test]
async fn a_parent_panic_reaps_children_and_clears_their_late_mail() {
    let (child_gate, child_handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "child of a panic", "name": "panic-child", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("clean next answer"),
    ])
    .tool(Arc::new(child_gate))
    .tool(Arc::new(parent_gate))
    .hooks(Arc::new(PanicParentToolHook))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "child of a panic",
        vec![Script::call(&[
            (
                "c_mail",
                crate::subagent::SEND_MESSAGE_TOOL,
                "{\"message\":\"stale panic mail\"}",
            ),
            ("c_gate", "gate", "{}"),
        ])],
    );

    h.submit("delegate before panic").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while child_handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "both gates must be reached"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    parent_handle.release();
    let first = h.collect_until_finished().await;
    assert_eq!(finished_status(&first), "failed");
    assert_eq!(count(&first, "run_finished"), 1);
    assert!(
        h.wait_idle().await.is_empty(),
        "parent and child both deregister"
    );
    assert_eq!(h.runtime.live_agent_count(h.session()), 0);
    assert_eq!(h.store.agents()[0].status, "cancelled");

    let second = h.run_to_finish("a fresh run after panic").await;
    assert_eq!(finished_status(&second), "completed");
    let messages = h.session_messages().await;
    assert!(
        !messages
            .iter()
            .any(|m| m.content.contains("stale panic mail")),
        "mail belonging to the panicked run leaked into its successor"
    );
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn finalization_resumes_after_a_hook_panic_without_duplicate_terminal_events() {
    let hooks = Arc::new(PanicRunEndHooks::default());
    let mut h = HarnessBuilder::new(vec![Script::text("work completed")])
        .hooks(hooks.clone())
        .build()
        .await;
    let mut events = h.run_to_finish("finish with a broken hook").await;
    // Include the outer recovery's post-hook frames as well.
    events.extend(h.collect_until_quiet().await);
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "run_finished"), 1, "{:?}", kinds(&events));
    assert_eq!(hooks.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.store.runs()[0].status, "completed");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn tool_result_and_run_end_hooks_are_cancellable() {
    let hooks = Arc::new(BlockingHooks::default());
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c_echo", "echo", "{}")]),
        Script::text("unreachable"),
    ])
    .tool(EchoTool::boxed("echo"))
    .hooks(hooks.clone())
    .build()
    .await;
    h.submit("cancel inside hook").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hooks.tool_results.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "tool hook not entered"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.runtime.cancel(h.session());
    let events = tokio::time::timeout(Duration::from_secs(2), h.collect_until_finished())
        .await
        .expect("cancelling a blocked hook must finish promptly");
    assert_eq!(finished_status(&events), "cancelled");
    assert!(h.wait_idle().await.is_empty());

    let hooks = Arc::new(BlockingHooks::default());
    let mut h = HarnessBuilder::new(vec![Script::text("final response")])
        .hooks(hooks.clone())
        .build()
        .await;
    h.submit("cancel run-end hook").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hooks.run_ends.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "run-end hook not entered"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.runtime.cancel(h.session());
    let events = tokio::time::timeout(Duration::from_secs(2), h.collect_until_finished())
        .await
        .expect("cancelling run-end hook must finish promptly");
    assert_eq!(finished_status(&events), "completed");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn a_blocked_hook_still_stops_at_the_run_deadline() {
    let hooks = Arc::new(BlockingHooks::default());
    let limits = Limits {
        max_run_secs: 1,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![Script::text("done")])
        .settings(FakeSettings::default().with_limits(limits))
        .hooks(hooks.clone())
        .build()
        .await;
    let started = tokio::time::Instant::now();
    let events = h.run_to_finish("deadline a broken run-end hook").await;
    assert_eq!(finished_status(&events), "completed");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(hooks.run_ends.load(Ordering::SeqCst), 1);
    assert!(count(&events, "notice") >= 1);
}

// ---------------------------------------------------------------------------
// context budget
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_context_budget_comes_from_the_models_window() {
    // A model that declares a 200k-token window with 8k of output room gets
    // ~730k chars to work with, so a 320k-char history must NOT be compacted.
    // The old global default (240k) would have digested it.
    let model = ModelConfig {
        model: "fake-model".into(),
        context_window: Some(200_000),
        max_output_tokens: Some(8_000),
        ..Default::default()
    };
    let limits = Limits {
        context_max_chars: 0,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![Script::text("thanks")])
        .settings(
            FakeSettings::default()
                .with_limits(limits.clone())
                .with_model(model.clone()),
        )
        .build()
        .await;

    // ~320k chars of history: comfortably over the old 240k default.
    for i in 0..40 {
        let body = format!("old question {i} {}", "x".repeat(8_000));
        let mut user = crate::tests_support::new_message(h.session(), "user", &body);
        user.run_id = Some("r_old".into());
        h.store.append_message(&user).await.unwrap();
        let mut assistant =
            crate::tests_support::new_message(h.session(), "assistant", &"y".repeat(200));
        assistant.run_id = Some("r_old".into());
        h.store.append_message(&assistant).await.unwrap();
    }

    let events = h.run_to_finish("the current question").await;
    assert_eq!(finished_status(&events), "completed");
    let sent = &h.factory.requests()[0].messages;
    assert!(
        !sent
            .iter()
            .any(|m| m.content.starts_with("[context digest:")),
        "a 320k-char history was compacted under a 730k-char budget"
    );
    assert!(
        sent.iter().any(|m| m.content.contains("old question 0 ")),
        "the oldest turn should still be in the request"
    );
    let total: usize = sent.iter().map(|m| m.content.chars().count()).sum();
    assert!(total > 300_000, "only {total} chars survived");

    // The same history under an explicit 240k ceiling is compacted, which proves
    // the assertion above is measuring the budget and not a no-op.
    let mut h2 = HarnessBuilder::new(vec![Script::text("thanks")])
        .settings(
            FakeSettings::default()
                .with_limits(Limits {
                    context_max_chars: 240_000,
                    ..Default::default()
                })
                .with_model(model),
        )
        .build()
        .await;
    for i in 0..40 {
        let body = format!("old question {i} {}", "x".repeat(8_000));
        let mut user = crate::tests_support::new_message(h2.session(), "user", &body);
        user.run_id = Some("r_old".into());
        h2.store.append_message(&user).await.unwrap();
        let mut assistant =
            crate::tests_support::new_message(h2.session(), "assistant", &"y".repeat(200));
        assistant.run_id = Some("r_old".into());
        h2.store.append_message(&assistant).await.unwrap();
    }
    let _ = h2.run_to_finish("the current question").await;
    let sent2 = &h2.factory.requests()[0].messages;
    assert!(
        sent2
            .iter()
            .any(|m| m.content.starts_with("[context digest:")),
        "an explicit 240k ceiling should still compact"
    );
}

#[tokio::test]
async fn old_history_is_trimmed_but_system_and_last_user_survive() {
    let limits = Limits {
        context_max_chars: 600,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![Script::text("thanks")])
        .settings(FakeSettings::default().with_limits(limits))
        .build()
        .await;

    // Seed an older conversation directly in the store.
    for i in 0..12 {
        let mut user = crate::tests_support::new_message(
            h.session(),
            "user",
            &format!("old question {i} {}", "x".repeat(120)),
        );
        user.run_id = Some("r_old".into());
        h.store.append_message(&user).await.unwrap();
        let mut assistant =
            crate::tests_support::new_message(h.session(), "assistant", &"y".repeat(120));
        assistant.run_id = Some("r_old".into());
        h.store.append_message(&assistant).await.unwrap();
    }

    let events = h.run_to_finish("the current question").await;
    assert_eq!(finished_status(&events), "completed");

    let requests = h.factory.requests();
    let sent = &requests[0].messages;
    assert_eq!(sent[0].role, Role::System);
    assert!(sent[0].content.contains("You are karucode"));
    let trim_notice = sent
        .iter()
        .find(|m| m.content.starts_with("[context digest: "))
        .expect("compaction notice in the request");
    // A digest is a `user` turn: the model never said it, and a transcript that
    // claims it did makes the model believe it already summarised the work.
    assert_eq!(trim_notice.role, Role::User);
    // The load itself is now bounded by the char budget (`history::trim_front_to_chars`),
    // so only the newest old messages are ever in memory; the notice counts what
    // was dropped from that window, not the whole stored transcript.
    assert!(
        trim_notice.content.contains("messages omitted"),
        "notice should count dropped messages: {}",
        trim_notice.content
    );
    assert_eq!(sent.last().unwrap().role, Role::User);
    assert_eq!(sent.last().unwrap().content, "the current question");
    // System prompt, the pinned opening question, the digest and the current
    // turn: the budget is smaller than everything else that was loaded.
    assert_eq!(sent.len(), 4, "unexpected survivors: {sent:?}");
    // The digest may name what was dropped, but it may never smuggle the
    // transcript back in: the notice is a summary, bounded with the budget.
    let digest_lines = trim_notice.content.lines().count();
    assert!(
        trim_notice.content.chars().count() <= 400,
        "digest must scale down with a small budget: {} chars",
        trim_notice.content.chars().count()
    );
    assert!(digest_lines <= 8, "digest kept {digest_lines} lines");
    let body: usize = sent.iter().skip(1).map(|m| m.content.chars().count()).sum();
    assert!(body <= 600, "droppable content over budget: {body}");
}

// ---------------------------------------------------------------------------
// transcript repair (B1: slice boundary, B2: failed tool-result persist)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn history_window_starting_with_an_orphan_tool_result_is_repaired() {
    let mut h = HarnessBuilder::new(vec![Script::text("ok")]).build().await;

    // Seed exactly one window's worth of rows so that, once `begin_run` appends
    // the new prompt, the 400-message window starts at the tool result whose
    // assistant tool_call fell outside the window.
    let mut assistant = crate::tests_support::new_message(h.session(), "assistant", "");
    assistant.run_id = Some("r_old".into());
    assistant.tool_calls = vec![ToolCall {
        id: "call_old".into(),
        name: "echo".into(),
        arguments: "{}".into(),
    }];
    h.store.append_message(&assistant).await.unwrap();

    let mut result = crate::tests_support::new_message(h.session(), "tool", "old result");
    result.tool_call_id = Some("call_old".into());
    result.name = Some("echo".into());
    h.store.append_message(&result).await.unwrap();

    for i in 0..(crate::HISTORY_LIMIT - 2) {
        let filler = crate::tests_support::new_message(h.session(), "user", &format!("filler {i}"));
        h.store.append_message(&filler).await.unwrap();
    }

    let events = h.run_to_finish("the current question").await;
    assert_eq!(finished_status(&events), "completed");

    let requests = h.factory.requests();
    let sent = &requests[0].messages;
    assert_eq!(sent[0].role, Role::System);
    assert_no_orphan_tool_messages(sent);
    assert!(
        !sent.iter().any(|m| m.role == Role::Tool),
        "the orphan result must be dropped from the request: {sent:?}"
    );
    assert_eq!(sent.last().unwrap().role, Role::User);
    assert_eq!(sent.last().unwrap().content, "the current question");
}

#[tokio::test]
async fn a_failed_tool_result_persist_is_repaired_before_the_next_request() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{}")]),
        Script::text("recovered"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    // Fail only the tool result (its content is `echoed:…`); the assistant turn
    // with the tool call is already persisted when the run fails.
    h.store.fail_appends_containing("echoed:");
    let events = h.run_to_finish("use the tool").await;
    assert_eq!(finished_status(&events), "failed");
    let stored = h.store.messages();
    assert!(
        stored
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "assistant tool call must be on disk"
    );
    assert!(!stored.iter().any(|m| m.role == "tool"));

    h.store.stop_failing_appends();
    let events = h.run_to_finish("try again").await;
    assert_eq!(finished_status(&events), "completed");

    let requests = h.factory.requests();
    assert_eq!(requests.len(), 2);
    let sent = &requests[1].messages;
    assert_no_orphan_tool_messages(sent);
    let repaired = sent
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("synthesized tool result in the request");
    assert!(
        repaired.content.contains("tool result missing"),
        "unexpected repair text: {}",
        repaired.content
    );

    // The repair is persisted, so a restart does not resurrect the broken
    // transcript — and it is idempotent: exactly one synthesized row.
    assert_eq!(
        h.store
            .messages()
            .iter()
            .filter(|m| m.role == "tool" && m.content.contains("tool result missing"))
            .count(),
        1
    );
}

#[tokio::test]
async fn recover_repairs_an_interrupted_tool_exchange() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;

    let mut assistant = crate::tests_support::new_message(h.session(), "assistant", "");
    assistant.run_id = Some("r_old".into());
    assistant.tool_calls = vec![ToolCall {
        id: "call_crashed".into(),
        name: "shell".into(),
        arguments: "{}".into(),
    }];
    h.store.append_message(&assistant).await.unwrap();

    let report = h.runtime.recover().await.expect("recover");
    assert_eq!(report.interrupted_runs, 0);
    let stored = h.store.messages();
    assert!(
        stored
            .iter()
            .any(|m| m.role == "tool" && m.content.contains("tool result missing")),
        "recover must persist the synthesized result: {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// subagents
// ---------------------------------------------------------------------------

fn spawn_call(task: &str, owned: Option<&[&str]>, recursive_ok: bool) -> Script {
    let _ = recursive_ok;
    let mut args = serde_json::json!({ "task": task, "name": "reviewer", "role": "reviewer" });
    if let Some(paths) = owned {
        args["owned_paths"] = serde_json::json!(paths);
    }
    Script::call(&[("call_spawn", SPAWN_AGENT_TOOL, &args.to_string())])
}

#[tokio::test]
async fn subagent_success_is_recorded_and_returns_its_summary() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("review the diff", Some(&["src/lib.rs"]), false),
        Script::text("the diff looks fine"),
        Script::text("parent finished"),
    ])
    .build()
    .await;

    let events = h.run_to_finish("delegate the review").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "agent_spawned"), 1);
    assert_eq!(count(&events, "agent_finished"), 1);
    assert!(count(&events, "agent_progress") >= 1);

    let spawned = events.iter().find_map(|e| match &e.event {
        Event::AgentSpawned {
            agent_id,
            name,
            role,
            model,
            ..
        } => Some((agent_id.clone(), name.clone(), role.clone(), model.clone())),
        _ => None,
    });
    let (agent_id, name, role, model) = spawned.expect("agent_spawned");
    assert_eq!(name, "reviewer");
    assert_eq!(role, "reviewer");
    assert_eq!(model, "fake-model");

    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    let agent = &agents[0];
    assert_eq!(agent.id, agent_id);
    assert_eq!(agent.status, "completed");
    assert_eq!(agent.iterations, 1);
    assert!(agent.summary.contains("the diff looks fine"));
    assert!(agent.error.is_none());
    assert_eq!(agent.owned_paths, vec!["src/lib.rs".to_string()]);
    assert!(agent.finished_at.is_some());
    assert_eq!(agent.parent_run_id, h.store.runs()[0].id);

    // The subagent transcript is persisted against the agent id only.
    let child_messages = h.store.messages_for_agent(&agent_id);
    assert_eq!(child_messages.len(), 2, "task + answer: {child_messages:?}");
    assert_eq!(child_messages[0].role, "user");
    assert_eq!(child_messages[0].content, "review the diff");
    assert_eq!(child_messages[1].role, "assistant");

    // The parent saw the summary as a tool result.
    let parent_messages = h.session_messages().await;
    let tool_message = parent_messages
        .iter()
        .find(|m| m.role == "tool" && m.agent_id.is_none())
        .expect("parent tool message");
    assert!(tool_message.content.contains("the diff looks fine"));
    assert!(tool_message.content.contains("completed"));

    // Claims are released once the agent finished.
    assert!(h.store.claims().is_empty());
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn subagent_failure_is_isolated_from_the_parent_run() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("do something impossible", None, false),
        Script::Fail("subagent model exploded".into()),
        Script::text("parent carried on"),
    ])
    .build()
    .await;

    let events = h.run_to_finish("delegate").await;
    assert_eq!(finished_status(&events), "completed");
    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "failed");
    assert!(agents[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("subagent model exploded"));

    assert_eq!(count(&events, "agent_finished"), 1);
    let failed_tool = tool_finished(&events);
    assert_eq!(failed_tool.len(), 1);
    assert!(!failed_tool[0].1, "a failed subagent reports ok=false");

    let messages = h.session_messages().await;
    assert_eq!(messages.last().unwrap().content, "parent carried on");
}

#[tokio::test]
async fn a_subagent_runs_until_it_is_done_instead_of_dying_at_a_budget() {
    // The reported bug: a delegated coding task burned a batch of tool calls per
    // turn, hit `subagents.max_iterations` in a couple of minutes and came back
    // as `failed` while the work was still in flight. A subagent is bound by its
    // wall clock and its loop guards now, not by a turn counter with a refresh
    // ritual around it.
    let subagents = SubagentConfig {
        max_concurrent: 1,
        // No turn ceiling: the default, and the point of the test.
        max_iterations: 0,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 60,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("a long job", None, false),
        // Three turns, then the child's own final answer.
        Script::call(&[("c1", "echo", "{}")]),
        Script::call(&[("c2", "echo", "{}")]),
        Script::call(&[("c3", "echo", "{}")]),
        Script::call(&[("c4", "echo", "{}")]),
        Script::call(&[("c5", "echo", "{}")]),
        Script::text("child finished the long job"),
        Script::text("parent done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = h.run_to_finish("delegate a long job").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(h.factory.requests().len(), 8, "every script was used");

    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    let agent = &agents[0];
    assert_eq!(
        agent.status, "completed",
        "the child must outlive its turn ceiling: {:?}",
        agent.error
    );
    assert_eq!(agent.iterations, 6, "one iteration per model round-trip");
    assert!(agent.summary.contains("child finished the long job"));
    assert!(agent.error.is_none(), "{:?}", agent.error);

    // Nothing was injected into the child's transcript to carry it across a
    // boundary: it just kept working.
    let child_messages = h.store.messages_for_agent(&agent.id);
    assert!(
        !child_messages
            .iter()
            .any(|m| m.role == "user" && m.content.contains("runtime checkpoint")),
        "a subagent must not be handed a synthetic user turn"
    );

    // The child was told its own budget, not the run's.
    let child_request = h
        .factory
        .requests()
        .into_iter()
        .find(|r| r.messages[0].content.contains("You are a focused subagent"))
        .expect("a child request");
    assert!(
        child_request.messages[0].content.contains("no turn limit"),
        "{}",
        child_request.messages[0].content
    );

    let tool_message = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "tool" && m.agent_id.is_none())
        .expect("parent tool message");
    assert!(tool_message.content.contains("completed"));
    assert!(!tool_message.content.contains("max_iterations"));
}

#[tokio::test]
async fn a_subagent_without_continuation_still_stops_at_its_budget() {
    // `max_continues = 0` is the documented one-shot budget: a delegated task
    // that wants the old behaviour must still stop dead at `max_iterations`.
    let subagents = SubagentConfig {
        max_concurrent: 1,
        max_iterations: 2,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 60,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("a job with a hard budget", None, false),
        Script::call(&[("c1", "echo", "{}")]),
        Script::call(&[("c2", "echo", "{}")]),
        Script::text("must never be reached"),
        Script::text("parent done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = h.run_to_finish("delegate").await;
    assert_eq!(finished_status(&events), "completed", "the parent survives");
    let agents = h.store.agents();
    assert_eq!(agents[0].status, "failed");
    assert_eq!(agents[0].iterations, 2);
    let error = agents[0].error.clone().unwrap_or_default();
    assert!(
        error.contains("settings.subagents.max_iterations"),
        "{error}"
    );
    assert!(
        !error.contains("max_continues"),
        "there is no continuation to mention: {error}"
    );
    assert_eq!(
        h.factory.requests().len(),
        4,
        "the child stopped at two turns"
    );
}

#[tokio::test]
async fn recursive_subagents_are_disabled_by_default() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("outer task", None, false),
        // The child tries to spawn again: the tool must not be registered.
        Script::call(&[("call_nested", SPAWN_AGENT_TOOL, "{\"task\":\"nested\"}")]),
        Script::text("child gave up and finished"),
        Script::text("parent done"),
    ])
    .build()
    .await;

    let events = h.run_to_finish("delegate twice").await;
    assert_eq!(finished_status(&events), "completed");

    let requests = h.factory.requests();
    assert!(requests[0].tools.iter().any(|t| t.name == SPAWN_AGENT_TOOL));
    assert!(
        !requests[1].tools.iter().any(|t| t.name == SPAWN_AGENT_TOOL),
        "child tool list must not include spawn_agent: {:?}",
        requests[1]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
    );

    let agents = h.store.agents();
    assert_eq!(agents.len(), 1, "no grandchild may be spawned");
    let child_messages = h.store.messages_for_agent(&agents[0].id);
    let tool_message = child_messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("child tool error");
    assert!(tool_message.content.contains("unknown tool `spawn_agent`"));
}

#[tokio::test]
async fn recursion_is_allowed_when_enabled_and_depth_allows() {
    let subagents = SubagentConfig {
        max_concurrent: 2,
        max_iterations: 4,
        allow_recursive: true,
        max_depth: 2,
        timeout_secs: 30,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("outer task", None, true),
        Script::call(&[("call_nested", SPAWN_AGENT_TOOL, "{\"task\":\"inner task\"}")]),
        Script::text("inner summary"),
        Script::text("outer summary"),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = h.run_to_finish("delegate recursively").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "agent_spawned"), 2);
    let agents = h.store.agents();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|a| a.status == "completed"));

    let requests = h.factory.requests();
    assert!(requests[1].tools.iter().any(|t| t.name == SPAWN_AGENT_TOOL));
    assert!(requests[0].messages[0].content.contains("karucode"));
}

#[tokio::test]
async fn subagent_depth_limit_stops_further_nesting() {
    let subagents = SubagentConfig {
        max_concurrent: 2,
        max_iterations: 4,
        allow_recursive: true,
        max_depth: 1,
        timeout_secs: 30,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("outer task", None, true),
        Script::text("child summary"),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = h.run_to_finish("delegate").await;
    assert_eq!(finished_status(&events), "completed");
    let requests = h.factory.requests();
    assert!(
        !requests[1].tools.iter().any(|t| t.name == SPAWN_AGENT_TOOL),
        "depth 1 must not hand the delegate tool to the child"
    );
    assert_eq!(h.store.agents().len(), 1);
}

#[tokio::test]
async fn subagent_model_override_is_used_for_the_child_request() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[(
            "call_spawn",
            SPAWN_AGENT_TOOL,
            &serde_json::json!({ "task": "child work", "model": "cheap-model" }).to_string(),
        )]),
        Script::text("child summary"),
        Script::text("parent done"),
    ])
    .build()
    .await;

    let _ = h.run_to_finish("delegate").await;
    let requests = h.factory.requests();
    assert_eq!(requests[0].model, "fake-model");
    assert_eq!(requests[1].model, "cheap-model");
    assert_eq!(h.store.agents()[0].model, "cheap-model");
}

#[tokio::test]
async fn conflicting_path_claims_are_surfaced_as_warnings() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("first task", Some(&["src"]), false),
        Script::text("first summary"),
        Script::text("parent done"),
    ])
    .build()
    .await;

    // Pre-existing claim owned by somebody else, in a subdirectory of the path
    // the subagent will claim. `claim_paths` reports the overlap.
    h.store
        .claim_paths(h.session(), "a_other", &["src/lib.rs".to_string()], "write")
        .await
        .unwrap();

    let events = h.run_to_finish("one agent").await;
    assert_eq!(finished_status(&events), "completed");
    let warnings: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolCallFinished { output, .. } => Some(output.clone()),
            _ => None,
        })
        .filter(|output| output.contains("warnings:"))
        .collect();
    assert!(
        !warnings.is_empty(),
        "expected claim warnings: {:?}",
        kinds(&events)
    );
    assert!(warnings[0].contains("a_other"), "{}", warnings[0]);
    assert!(
        warnings[0].contains("src/lib.rs"),
        "the returned overlapping claim path must be named: {}",
        warnings[0]
    );
}

#[tokio::test]
async fn claims_are_released_when_the_subagent_task_cannot_be_persisted() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("child task", Some(&["src/lib.rs"]), false),
        Script::text("parent done"),
    ])
    .build()
    .await;

    // The subagent's task message is persisted after its claims are recorded;
    // fail exactly that append.
    h.store.fail_appends_containing("child task");
    let events = h.run_to_finish("go").await;
    assert_eq!(finished_status(&events), "completed");

    assert!(
        h.store.claims().is_empty(),
        "claims leaked after the early return: {:?}",
        h.store.claims()
    );
    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "failed");
    assert!(
        agents[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("could not persist the subagent task"),
        "{:?}",
        agents[0].error
    );
}

#[tokio::test]
async fn cancelling_the_parent_cancels_the_subagent() {
    let mut h = HarnessBuilder::new(vec![
        spawn_call("keep working", None, false),
        Script::Hang(vec![ProviderChunk::Text("child partial".into())]),
    ])
    .build()
    .await;

    h.submit("delegate and hang").await;
    // Wait for a delta that belongs to the subagent.
    let mut child_agent_id = None;
    loop {
        let event = h.next_event().await;
        match &event.event {
            Event::AgentSpawned { agent_id, .. } => child_agent_id = Some(agent_id.clone()),
            Event::TextDelta { .. } if event.agent_id.is_some() => break,
            _ => {}
        }
    }
    assert!(child_agent_id.is_some());
    let child_agent_id = child_agent_id.unwrap();

    assert!(h.runtime.cancel(h.session()));
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");

    let agent = h
        .store
        .agents()
        .into_iter()
        .find(|a| a.id == child_agent_id)
        .expect("agent record");
    assert_eq!(agent.status, "cancelled");
    assert!(h.wait_idle().await.is_empty());
    assert!(h.store.claims().is_empty());
}

#[tokio::test]
async fn subagent_timeout_marks_the_agent_failed_but_not_the_parent() {
    let subagents = SubagentConfig {
        max_concurrent: 1,
        max_iterations: 8,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 1,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("never finish", None, false),
        Script::Hang(vec![ProviderChunk::Text("still going".into())]),
        Script::text("parent recovered"),
    ])
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = tokio::time::timeout(Duration::from_secs(20), h.run_to_finish("go"))
        .await
        .expect("subagent timeout must not hang the parent");
    assert_eq!(finished_status(&events), "completed");
    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "failed");
    assert!(agents[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("timed out"));
    assert_eq!(
        h.session_messages().await.last().unwrap().content,
        "parent recovered"
    );
}

#[tokio::test]
async fn subagent_approvals_carry_the_agent_id() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("run the tests", None, false),
        Script::call(&[("call_run", "run", "{}")]),
        Script::text("child ran it"),
        Script::text("parent done"),
    ])
    .tool(ApprovalTool::shell("run", "cargo test"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("delegate the tests").await;
    let mut child_agent_id = None;
    let approval = loop {
        let event = h.next_event().await;
        if let Event::AgentSpawned { agent_id, .. } = &event.event {
            child_agent_id = Some(agent_id.clone());
        }
        if let Event::ApprovalRequested { approval } = &event.event {
            break approval.clone();
        }
    };
    let child_agent_id = child_agent_id.expect("agent_spawned before the approval");
    assert_eq!(approval.agent_id.as_deref(), Some(child_agent_id.as_str()));
    assert_eq!(approval.session_id, h.session());

    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .expect("resolve child approval");
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "completed");

    let records = h.store.approvals();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].agent_id.as_deref(),
        Some(child_agent_id.as_str())
    );
    assert_eq!(records[0].status, "resolved");
    assert_eq!(h.store.agents()[0].status, "completed");
}

/// Build an MCP approval request for the scope-expiry tests.
fn scoped_mcp_approval(id: &str, session: &str, run: &str, agent: &str) -> ApprovalRequest {
    ApprovalRequest {
        id: id.to_string(),
        kind: ApprovalKind::Mcp,
        title: format!("Call tool_{agent}"),
        detail: "mcp:fs".into(),
        payload: serde_json::json!({ "tool": format!("tool_{agent}") }),
        session_id: session.to_string(),
        run_id: run.to_string(),
        agent_id: Some(agent.to_string()),
    }
}

#[tokio::test]
async fn expiring_one_subagent_scope_keeps_a_sibling_approval_pending() {
    let inner = test_inner_with_settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::Ask,
        timeout_secs: 30,
        ..Default::default()
    }));
    inner
        .deps
        .store
        .create_session("s_scope", "scoped", "/w", None)
        .await
        .expect("session");

    // Two concurrent subagent loops share one parent run id but have their own
    // agent id and cancellation token.
    let run_id = RunId::new();
    let session_id = SessionId::from_string("s_scope");
    let cancel_a = CancellationToken::new();
    let cancel_b = CancellationToken::new();
    let gate_a = Arc::new(RuntimeApprovalGate::new(
        Arc::clone(&inner),
        session_id.clone(),
        run_id.clone(),
        Some(AgentId::from_string("a_one")),
        cancel_a.clone(),
        1,
    ));
    let gate_b = Arc::new(RuntimeApprovalGate::new(
        Arc::clone(&inner),
        session_id.clone(),
        run_id.clone(),
        Some(AgentId::from_string("a_two")),
        cancel_b.clone(),
        1,
    ));

    let request_a = scoped_mcp_approval("ap_a", "s_scope", &run_id.0, "a_one");
    let request_b = scoped_mcp_approval("ap_b", "s_scope", &run_id.0, "a_two");
    let task_a = tokio::spawn(async move { gate_a.request(request_a).await });
    let task_b = tokio::spawn(async move { gate_b.request(request_b).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let pending = inner
            .deps
            .store
            .list_pending_approvals(Some("s_scope"))
            .await
            .expect("pending");
        if pending.len() == 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "both scopes must have a pending approval, got {pending:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Cancel only scope A. Scope B's request must keep waiting.
    cancel_a.cancel();
    let decision_a = tokio::time::timeout(Duration::from_secs(5), task_a)
        .await
        .expect("scope A must return")
        .expect("join");
    assert_eq!(decision_a, ApprovalDecision::Expired);

    let pending = inner
        .deps
        .store
        .list_pending_approvals(Some("s_scope"))
        .await
        .expect("pending");
    assert_eq!(
        pending.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        vec!["ap_b"],
        "expiring scope A must not expire scope B: {pending:?}"
    );
    assert_eq!(
        inner.approvals.pending_count(),
        1,
        "the sibling's in-process waiter must still be registered"
    );
    assert_eq!(pending[0].status, "pending");

    // Clean up scope B so the test leaves no parked task.
    cancel_b.cancel();
    let decision_b = tokio::time::timeout(Duration::from_secs(5), task_b)
        .await
        .expect("scope B must return")
        .expect("join");
    assert_eq!(decision_b, ApprovalDecision::Expired);
    assert_eq!(inner.approvals.pending_count(), 0);
}

#[tokio::test]
async fn subagent_timeout_cancels_inflight_tool_before_dropping_the_loop() {
    let subagents = SubagentConfig {
        max_concurrent: 1,
        max_iterations: 8,
        allow_recursive: false,
        max_depth: 1,
        // Two seconds, not one: this asserts that a timed-out child's tool
        // observes the cancellation token during the 500ms grace period, and at a
        // one-second budget the whole suite running in parallel can starve the
        // tool's wake-up. The behaviour under test is unchanged; only the margin is.
        timeout_secs: 2,
        allow_background: true,
        max_background: 8,
        wait_timeout_secs: 60,
    };
    let slow = Arc::new(SlowCancelTool::new("slow"));
    let started = slow.started();
    let cancelled = slow.cancelled();
    let finished = slow.finished();
    let mut h = HarnessBuilder::new(vec![
        spawn_call("call the slow tool", None, false),
        Script::call(&[("call_slow", "slow", "{}")]),
        Script::text("parent recovered"),
    ])
    .tool(slow)
    .settings(FakeSettings::default().with_subagents(subagents))
    .build()
    .await;

    let events = tokio::time::timeout(Duration::from_secs(20), h.run_to_finish("go"))
        .await
        .expect("a stuck child tool must not hang the parent");
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(started.load(Ordering::SeqCst), 1, "child ran the tool");
    assert!(
        cancelled.load(Ordering::SeqCst),
        "the tool must observe cancellation during the grace period before the loop future is dropped"
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the tool must not finish on its own"
    );

    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "failed");
    assert!(agents[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("timed out"));

    assert!(h.wait_idle().await.is_empty(), "no child loop may survive");
    assert!(
        h.runtime.active_runs().iter().all(|r| r.agent_id.is_none()),
        "the timed-out subagent is no longer registered"
    );
    assert_eq!(
        h.session_messages().await.last().unwrap().content,
        "parent recovered"
    );
}

#[tokio::test]
async fn cancelling_the_parent_lets_the_subagent_tool_observe_cancellation() {
    let slow = Arc::new(SlowCancelTool::new("slow"));
    let started = slow.started();
    let cancelled = slow.cancelled();
    let mut h = HarnessBuilder::new(vec![
        spawn_call("call the slow tool", None, false),
        Script::call(&[("call_slow", "slow", "{}")]),
        Script::text("unused"),
    ])
    .tool(slow)
    .build()
    .await;

    h.submit("go").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while started.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never invoked the slow tool"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert!(h.runtime.cancel(h.session()));
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    assert!(
        cancelled.load(Ordering::SeqCst),
        "the child tool must see the parent's cancellation"
    );
    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "cancelled");
    assert!(h.wait_idle().await.is_empty());
}

// ---------------------------------------------------------------------------
// recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recover_marks_interrupted_work_and_expires_approvals() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;

    h.store
        .create_run("r_stale", h.session(), "fake-model", "old prompt")
        .await
        .unwrap();
    h.store
        .upsert_agent(&AgentRecord {
            id: "a_stale".into(),
            session_id: h.session().to_string(),
            parent_run_id: "r_stale".into(),
            parent_agent_id: None,
            name: "stale".into(),
            role: "worker".into(),
            task: "stale task".into(),
            status: "running".into(),
            model: "fake-model".into(),
            iterations: 3,
            summary: String::new(),
            error: None,
            owned_paths: vec![],
            created_at: 0,
            updated_at: 0,
            finished_at: None,
            background: false,
            note: String::new(),
            progress: None,
        })
        .await
        .unwrap();
    h.store
        .create_approval(&ApprovalRecord {
            id: "ap_stale".into(),
            session_id: h.session().to_string(),
            run_id: "r_stale".into(),
            agent_id: None,
            kind: "shell_command".into(),
            title: "Run".into(),
            detail: "cargo test".into(),
            payload: serde_json::json!({ "command": "cargo test" }),
            status: "pending".into(),
            decision: None,
            created_at: 0,
            resolved_at: None,
        })
        .await
        .unwrap();

    let report: RecoveryReport = h.runtime.recover().await.expect("recover");
    assert_eq!(report.interrupted_runs, 1);
    assert_eq!(report.interrupted_agents, 1);
    assert_eq!(report.expired_approvals, 1);

    assert_eq!(h.store.runs()[0].status, "interrupted");
    assert_eq!(h.store.agents()[0].status, "interrupted");
    assert_eq!(h.store.approvals()[0].status, "expired");

    // Idempotent: a second recovery finds nothing left to do.
    let again = h.runtime.recover().await.expect("recover again");
    assert_eq!(again.interrupted_runs, 0);
    assert_eq!(again.interrupted_agents, 0);
    assert_eq!(again.expired_approvals, 0);
}

// ---------------------------------------------------------------------------
// wiring / additive API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn builtins_are_present_in_the_tool_view_and_survive_set_tools() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .tool(EchoTool::boxed("echo"))
        .build()
        .await;

    let tools = h.runtime.tools();
    // The whole delegate family is a runtime built-in, not just `spawn_agent`:
    // the control tools need the runtime's live agent registry, which no server
    // tool can reach. Ordering is by name, so the mixed list is stable.
    assert_eq!(
        tools.names(),
        sorted_with(&["echo"], Runtime::builtin_tool_names())
    );
    for name in Runtime::builtin_tool_names() {
        assert!(tools.get(name).is_some(), "missing built-in `{name}`");
    }
    assert_eq!(
        Runtime::builtin_tool_names(),
        crate::subagent::DELEGATE_TOOLS
    );

    let mut replacement = ToolRegistry::new();
    replacement.register(EchoTool::boxed("other"));
    h.runtime.set_tools(Arc::new(replacement));
    let tools = h.runtime.tools();
    assert_eq!(
        tools.names(),
        sorted_with(&["other"], Runtime::builtin_tool_names())
    );
}

/// The registry's ordering contract: every name, sorted.
fn sorted_with(base: &[&str], builtins: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = base.iter().map(|name| name.to_string()).collect();
    names.extend(builtins.iter().map(|name| name.to_string()));
    names.sort();
    names.dedup();
    names
}

#[tokio::test]
async fn server_emitted_events_share_the_runtime_sequence() {
    let mut h = HarnessBuilder::new(vec![Script::text("done")])
        .build()
        .await;

    // The server publishes session lifecycle events through the same helper.
    let session_event = h.runtime.emit(
        Some(h.session()),
        None,
        None,
        Event::SessionRenamed {
            session_id: h.session().to_string(),
            title: "renamed".into(),
        },
    );
    assert_eq!(session_event.seq, 1, "the first published event gets seq 1");
    assert_eq!(session_event.session_id.as_deref(), Some(h.session()));
    assert!(session_event.run_id.is_none());
    assert_eq!(h.runtime.events_emitted(), 1);

    // Runtime events continue the same sequence: subscribers see one ordered
    // stream whether the event came from the server or from a run.
    let events = h.run_to_finish("hello").await;
    let kinds = kinds(&events);
    assert_eq!(kinds.first(), Some(&"session_renamed"));
    assert_eq!(kinds.last(), Some(&"run_finished"));
    let session_seen = events.first().expect("delivered to subscribers");
    assert_eq!(session_seen.seq, session_event.seq);
    assert_eq!(session_seen.ts_ms, session_event.ts_ms);

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "shared monotonic seq: {seqs:?}"
    );
    // The harness stops collecting at `run_finished`, but the run also publishes a
    // `notification` after it (deliberately last: a client that reacts to it must
    // find the terminal state already persisted). So the emitted counter is one
    // ahead of what this window saw, and only by that one frame.
    assert_eq!(h.runtime.events_emitted() as usize, events.len() + 1);
}

#[tokio::test]
async fn active_runs_reports_the_iteration_count() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("x".into())])])
        .build()
        .await;
    let handle = h.submit("count me").await;
    let _ = wait_for_kind(&mut h, "iteration_started").await;

    let active: Vec<ActiveRun> = h.runtime.active_runs();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].run_id, handle.run_id);
    assert_eq!(active[0].session_id, h.session());
    assert_eq!(active[0].iterations, 1);
    assert!(active[0].agent_id.is_none());

    h.runtime.cancel(h.session());
    let _ = h.collect_until_finished().await;
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn is_running_ignores_cancelled_runs() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;
    let run_id = RunId::new();
    let cancel = CancellationToken::new();
    let state = Arc::new(crate::RunState::new(
        run_id.clone(),
        SessionId::from_string(h.session()),
        None,
        cancel.clone(),
        true,
    ));
    h.runtime
        .shared()
        .runs
        .lock()
        .expect("runs lock")
        .insert(run_id.0.clone(), Arc::clone(&state));
    assert!(h.runtime.is_running(h.session()));

    cancel.cancel();
    assert!(
        !h.runtime.is_running(h.session()),
        "a cancelled run must not count as running"
    );

    h.runtime.shared().runs.lock().expect("runs lock").clear();
}

#[test]
fn approval_registry_resolves_once() {
    let registry = ApprovalRegistry::new();
    let mut rx = registry.insert("ap_1");
    assert!(registry.resolve("ap_1", ApprovalDecision::AllowOnce));
    assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::AllowOnce);
    assert!(!registry.resolve("ap_1", ApprovalDecision::AllowOnce));
    assert_eq!(registry.pending_count(), 0);
}

#[tokio::test]
async fn tool_outcomes_and_specs_are_reachable_without_a_store() {
    // Compile-time smoke test for the additive helpers the server may use.
    let _ = ApprovalPolicyView {
        mode: ApprovalMode::Ask,
        allow_commands: vec![],
        auto_approve_paths: vec![],
    };
    let dir = std::env::temp_dir();
    let workspace = Arc::new(Workspace::new(&dir).unwrap());
    let store = Arc::new(crate::tests_support::FakeStore::new());
    let factory: Arc<ScriptedFactory> = Arc::new(ScriptedFactory::new(vec![]));
    let runtime = Runtime::new(RuntimeDeps {
        store,
        providers: factory,
        tools: Arc::new(ToolRegistry::new()),
        settings: Arc::new(FakeSettings::default()),
        hooks: Arc::new(NoHooks),
        workspaces: Arc::new(crate::SingleWorkspace::new(workspace)),
    });
    assert_eq!(runtime.active_runs().len(), 0);
    let mut registry = ToolRegistry::new();
    crate::subagent::install_builtins(&mut registry, &runtime);
    assert!(registry.get(SPAWN_AGENT_TOOL).is_some());
    assert_eq!(
        crate::agent::default_approval_kind(kc_core::tool::ToolClass::Shell),
        ApprovalKind::ShellCommand
    );
    let _ = ToolOutcome::ok("x");
    let _ = RunStatus::Completed;
}

#[test]
fn panic_payloads_are_rendered_readably() {
    let payload = std::panic::catch_unwind(|| panic!("boom {}", 1)).unwrap_err();
    assert_eq!(crate::agent::panic_message(&*payload), "boom 1");
    let payload = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
    assert_eq!(crate::agent::panic_message(&*payload), "plain");
}

#[tokio::test]
async fn read_only_denies_external_tools_even_without_approval() {
    // `require_approval = false` may skip a prompt; it must not widen read_only
    // mode, which is a hard policy denial.
    let tool = Arc::new(ExternalTool::mcp("mcp_write", "fs").with_require_approval(false));
    let invocations = tool.invocations();
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "mcp_write", "{}")]),
        Script::text("after denial"),
    ])
    .tool(tool)
    .settings(FakeSettings::default().with_policy(ApprovalPolicy {
        mode: ApprovalMode::ReadOnly,
        ..Default::default()
    }))
    .build()
    .await;

    let events = h.run_to_finish("write something").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "approval_requested"), 0);
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        0,
        "side effect happened in read_only mode"
    );
    let finished = tool_finished(&events);
    assert!(!finished[0].1);
    assert!(
        finished[0].2.contains("denied by policy: read_only mode"),
        "{}",
        finished[0].2
    );
}

#[tokio::test]
async fn a_reused_tool_call_id_does_not_grow_the_transcript() {
    // Some OpenAI-compatible gateways (and scripted mocks) reuse the same
    // tool_call_id for every turn. Keying results by call id alone made the
    // repair synthesize a placeholder for every turn on every load: the store is
    // append-only, so a 200-call session grew to 20k rows. Matching the first
    // unused result *after* the call keeps repeated loads idempotent.
    let mut h = HarnessBuilder::new(vec![Script::text("ok"); 4])
        .build()
        .await;

    for turn in 0..3 {
        let mut assistant = crate::tests_support::new_message(h.session(), "assistant", "");
        assistant.run_id = Some(format!("r_{turn}"));
        assistant.tool_calls = vec![ToolCall {
            id: "call_repeated".into(),
            name: "echo".into(),
            arguments: "{}".into(),
        }];
        h.store.append_message(&assistant).await.unwrap();

        let mut result =
            crate::tests_support::new_message(h.session(), "tool", &format!("result {turn}"));
        result.tool_call_id = Some("call_repeated".into());
        result.name = Some("echo".into());
        h.store.append_message(&result).await.unwrap();
    }
    let seeded = h.store.messages().len();

    // Four loads, the way four follow-up runs would reload the session.
    for i in 0..4 {
        let events = h.run_to_finish(&format!("question {i}")).await;
        assert_eq!(finished_status(&events), "completed");
    }

    let messages = h.store.messages();
    let synthesized = messages
        .iter()
        .filter(|m| m.content == crate::history::MISSING_TOOL_RESULT)
        .count();
    assert_eq!(synthesized, 0, "no placeholder may be invented");
    // 4 runs x (user + assistant text) on top of the 6 seeded rows.
    assert_eq!(
        messages.len(),
        seeded + 4 * 2,
        "the transcript grew by exactly what the four runs added"
    );

    // Every turn still sees all three results, in order.
    let requests = h.factory.requests();
    let sent = &requests.last().unwrap().messages;
    let results: Vec<&str> = sent
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(results, vec!["result 0", "result 1", "result 2"]);
}

// ===========================================================================
// Steering and follow-ups
// ===========================================================================

/// A steer accepted mid-run becomes a real `user` message in the conversation,
/// in the request the model sees on its next turn.
///
/// The steer is issued from *inside* the tool call, which is the honest
/// reproduction of a user typing while the agent works - the run is provably in
/// flight, so the acceptance path is exercised rather than raced.
#[tokio::test]
async fn a_steer_sent_mid_run_joins_the_conversation() {
    let mut h = HarnessBuilder::new(vec![
        // Turn 1 asks for the tool that steers; turn 2 answers.
        Script::call(&[("call_1", "steer_now", "{}")]),
        Script::text("done, and I also renamed the test"),
    ])
    .build()
    .await;
    // The tool needs the runtime, which only exists after `build`, and the runtime
    // needs the tool. Registering it afterwards is exactly what `set_tools` is
    // for, and it is what the server does when a plugin loads.
    let steer = SteerTool::boxed(
        "steer_now",
        Arc::clone(&h.runtime),
        h.session(),
        "also rename the test to snake_case",
    );
    let mut registry = ToolRegistry::new();
    registry.register(steer);
    h.runtime.set_tools(Arc::new(registry));

    let events = h.run_to_finish("do the work").await;

    // The tool call ran and the steer was accepted (not refused as "no run").
    let tool_result = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "tool")
        .expect("the steering tool ran");
    assert!(
        tool_result.content.starts_with("steered:"),
        "the steer was refused mid-run: {}",
        tool_result.content
    );

    // The transcript carries the steer as a plain user turn, positioned after the
    // tool call it was sent during - not at the end of the run.
    let messages = h.session_messages().await;
    let user_texts: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        user_texts,
        vec!["do the work", "also rename the test to snake_case"],
        "the steer must be persisted as an ordinary user message"
    );

    // ...and the model saw it: the request for turn 2 contains it.
    let requests = h.factory.requests();
    assert!(requests.len() >= 2, "there must have been a second turn");
    let second = &requests[1].messages;
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("rename the test")),
        "the second request must carry the steer"
    );

    // The client was told when it landed.
    let applied: Vec<&EventEnvelope> = events
        .iter()
        .filter(|e| matches!(e.event, Event::SteerApplied { .. }))
        .collect();
    assert_eq!(applied.len(), 1, "exactly one steer was applied");
    match &applied[0].event {
        Event::SteerApplied { content, .. } => {
            assert_eq!(content, "also rename the test to snake_case")
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_final_response_steer_becomes_an_ordered_follow_up() {
    let hooks = Arc::new(FinalResponseSteerHooks::default());
    let mut h = HarnessBuilder::new(vec![
        Script::text("answer before the correction"),
        Script::text("answer with the correction"),
    ])
    .hooks(hooks.clone())
    .build()
    .await;
    hooks.arm(&h.runtime);

    h.submit("answer, but expect a late steer").await;
    let first = h.collect_until_finished().await;
    assert_eq!(finished_status(&first), "completed");
    assert_eq!(count(&first, "steer_applied"), 0);

    // Conversion happens during deregistration, after RunFinished. The same
    // finishing task schedules the converted message as the very next run.
    let second = h.collect_until_finished().await;
    assert_eq!(finished_status(&second), "completed");
    assert!(
        second.iter().any(|event| matches!(
            &event.event,
            Event::Notice { message, .. } if message.contains("will run as follow-ups")
        )),
        "{:?}",
        kinds(&second)
    );

    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content)
        .collect();
    assert_eq!(
        users,
        vec![
            "answer, but expect a late steer",
            "apply the final-turn correction"
        ]
    );
    let requests = h.factory.requests();
    assert!(requests[1].messages.iter().any(|message| {
        message.role == Role::User && message.content == "apply the final-turn correction"
    }));
    assert!(h.runtime.queued_steers(h.session()).is_empty());
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
    assert!(h.wait_idle().await.is_empty());
}

/// A steer is *not* a trailing note: nothing may be appended after the
/// conversation, because that makes a reasoning model deliberate instead of act.
/// The steer must sit where a user turn belongs.
#[tokio::test]
async fn a_steer_is_never_a_trailing_system_note() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c1", "steer_now", "{}")]),
        Script::text("handled"),
    ])
    .build()
    .await;
    let steer = SteerTool::boxed(
        "steer_now",
        Arc::clone(&h.runtime),
        h.session(),
        "one more thing",
    );
    let mut registry = ToolRegistry::new();
    registry.register(steer);
    h.runtime.set_tools(Arc::new(registry));

    let _ = h.run_to_finish("start").await;

    let requests = h.factory.requests();
    assert!(requests.len() >= 2, "the steer must have produced a turn");
    for request in requests {
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|m| m.role == Role::System)
                .count(),
            1,
            "the steer added a second system message"
        );
        assert_ne!(
            request.messages.last().map(|m| m.role),
            Some(Role::System),
            "a steer must never be appended after the conversation"
        );
    }
}

/// A follow-up still lands when the only thing "running" is a detached child.
///
/// This is the `has_active_run` question FIX 3 raised: the flag is top_level-only
/// and stays that way on purpose. A background child is *not* a run, so it can
/// neither accept a steer (there is nothing to fold it into) nor justify a
/// follow-up (there is no run to follow). The window in which a detached child
/// exists but its run does not is the run's own teardown, and the follow-up queue
/// is drained by the run that finishes - so a follow-up accepted mid-teardown
/// still starts.
#[tokio::test]
async fn a_detached_child_neither_accepts_a_steer_nor_blocks_a_follow_up() {
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "a long job", "name": "worker", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
        Script::text("follow-up done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory
        .key("a long job", vec![Script::call(&[("c1", "gate", "{}")])]);

    h.submit("delegate").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // The run is in flight, so a follow-up is accepted with a `202`-style id.
    let queued = h.follow_up("then summarise");
    assert!(queued.id.starts_with("followup_"));

    // Let the run end. Its child is reaped and the queued follow-up runs.
    parent_handle.release();
    let mut finished = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let event = h.next_event().await;
        if matches!(event.event, Event::RunFinished { .. }) {
            finished += 1;
            if finished == 2 {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {finished} of 2 runs finished; queue is {:?}",
            h.runtime.queued_follow_ups(h.session())
        );
    }
    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user" && m.agent_id.is_none())
        .map(|m| m.content)
        .collect();
    assert_eq!(
        users,
        vec!["delegate".to_string(), "then summarise".to_string()],
        "the accepted follow-up must not be lost to the run-end reaping"
    );
    assert!(
        !h.runtime.is_running(h.session()),
        "nothing may be left writing after the follow-up finishes"
    );
    let _ = h.wait_idle().await;
}

/// A steer with no run in flight is refused rather than silently queued.
#[tokio::test]
async fn steering_with_nothing_running_is_refused() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;
    let error = h
        .runtime
        .steer(h.session(), "anyone there?")
        .expect_err("a steer without a run has no meaning");
    assert!(matches!(error, Error::BadRequest(_)), "{error:?}");
    assert!(
        error.to_string().contains("no run in flight"),
        "the message must say what is wrong: {error}"
    );
    // Nothing was queued, so a later run does not inherit it.
    assert!(h.runtime.queued_steers(h.session()).is_empty());
}

/// The steer queue is bounded, so one session cannot grow it without limit.
#[tokio::test]
async fn the_steer_queue_is_bounded() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text(
        "thinking".into(),
    )])])
    .build()
    .await;
    h.submit("long job").await;
    // Wait for the run to be registered before steering at it.
    let _ = h.wait_idle().await;

    let mut accepted = 0usize;
    for i in 0..(crate::MAX_QUEUED_STEERS + 5) {
        match h.runtime.steer(h.session(), format!("steer {i}")) {
            Ok(_) => accepted += 1,
            Err(error) => {
                assert!(
                    error.to_string().contains("already has"),
                    "the rejection must name the limit: {error}"
                );
                break;
            }
        }
    }
    assert_eq!(accepted, crate::MAX_QUEUED_STEERS);
    assert_eq!(
        h.runtime.queued_steers(h.session()).len(),
        crate::MAX_QUEUED_STEERS
    );
    h.runtime.cancel(h.session());
}

/// A steer that never lands (the run ended first) is simply absent: it is not
/// persisted, so the transcript has no dangling user turn.
///
/// This is the race the acceptance guard exists for: `steer` checks `is_running`
/// and refuses once the run is gone, so a message can never be accepted into a
/// queue that nothing will ever drain. What matters for the transcript is that an
/// accepted-but-undrained message is *not* a stored turn - it only becomes one at
/// the moment the loop folds it in.
#[tokio::test]
async fn a_steer_after_the_run_is_refused_and_leaves_no_turn() {
    let mut h = HarnessBuilder::new(vec![Script::text("finished quickly")])
        .build()
        .await;
    let _ = h.run_to_finish("quick job").await;
    let _ = h.wait_idle().await;

    // The run is over: a steer now is refused, not queued.
    let error = h
        .runtime
        .steer(h.session(), "too late")
        .expect_err("a steer after the run must be refused");
    assert!(error.to_string().contains("no run in flight"), "{error}");
    assert!(
        h.runtime.queued_steers(h.session()).is_empty(),
        "nothing may be left waiting for a run that has ended"
    );

    // ...and the transcript holds exactly the original prompt.
    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content)
        .collect();
    assert_eq!(users, vec!["quick job".to_string()]);
}

/// A follow-up queued mid-run starts a *new run* once the current one finishes,
/// and its own user message is persisted by that next run.
#[tokio::test]
async fn a_queued_follow_up_starts_the_next_run() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c1", "queue_next", "{}")]),
        Script::text("first answer"),
        Script::text("second answer"),
    ])
    .build()
    .await;
    let tool = FollowUpTool::boxed(
        "queue_next",
        Arc::clone(&h.runtime),
        h.session(),
        "now run the tests",
    );
    let mut registry = ToolRegistry::new();
    registry.register(tool);
    h.runtime.set_tools(Arc::new(registry));

    let first = h.run_to_finish("do the first job").await;
    // The follow-up run starts only after the first finishes, so its events are
    // collected separately.
    let second = h.collect_until_finished().await;

    let first_done = first
        .iter()
        .any(|e| matches!(&e.event, Event::RunFinished { status, .. } if status == "completed"));
    assert!(first_done, "the first run must finish normally");
    assert!(
        second
            .iter()
            .any(|e| matches!(e.event, Event::RunStarted { .. })),
        "the follow-up must start a run"
    );

    // Two runs, each with its own user message.
    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content)
        .collect();
    assert_eq!(
        users,
        vec![
            "do the first job".to_string(),
            "now run the tests".to_string()
        ]
    );
    // The queue is empty: the follow-up was taken, not left to re-fire.
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
}

/// **Every** queued follow-up runs, not just the first.
///
/// The chain used to drain the queue with `while let Some(next) = take_follow_up`:
/// the run `start_run` had just started was registered as the session's
/// top-level run, so the second `start_run` was refused with "a run is already
/// active" and the `Err` arm ran `clear_follow_ups` - wiping the entire queue.
/// `MAX_QUEUED_FOLLOW_UPS` is 8, so up to seven accepted follow-ups (each with a
/// `202` and a `follow_up_id` the client could no longer recall) vanished in one
/// go. One entry was tested, which is exactly the case that worked.
#[tokio::test]
async fn every_queued_follow_up_runs_in_order() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c1", "queue_many", "{}")]),
        Script::text("first answer"),
        Script::text("second answer"),
        Script::text("third answer"),
        Script::text("fourth answer"),
    ])
    .build()
    .await;
    let tool = FollowUpTool::boxed_many(
        "queue_many",
        Arc::clone(&h.runtime),
        h.session(),
        vec![
            "then run the tests".to_string(),
            "then update the docs".to_string(),
            "then summarise".to_string(),
        ],
    );
    let mut registry = ToolRegistry::new();
    registry.register(tool);
    h.runtime.set_tools(Arc::new(registry));

    h.submit("do the first job").await;

    // Collect until the queue has drained *and* the store shows four prompts.
    // Each follow-up is a whole run, so this is four `run_finished` frames.
    let mut finished = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let event = h.next_event().await;
        if matches!(event.event, Event::RunFinished { .. }) {
            finished += 1;
            if finished == 4 {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {finished} of 4 runs finished; queue is {:?}",
            h.runtime.queued_follow_ups(h.session())
        );
    }

    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content)
        .collect();
    assert_eq!(
        users,
        vec![
            "do the first job".to_string(),
            "then run the tests".to_string(),
            "then update the docs".to_string(),
            "then summarise".to_string(),
        ],
        "every accepted follow-up must run, in the order it was queued"
    );
    assert!(
        h.runtime.queued_follow_ups(h.session()).is_empty(),
        "the queue must be drained, not leaked: {:?}",
        h.runtime.queued_follow_ups(h.session())
    );
}

#[tokio::test]
async fn a_follow_up_start_failure_is_explicit_and_leaves_no_schedulerless_entry() {
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c_queue", "queue_next", "{}")]),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("unused"),
        Script::text("manual retry recovered"),
    ])
    .build()
    .await;
    let queue_tool = FollowUpTool::boxed_many(
        "queue_next",
        Arc::clone(&h.runtime),
        h.session(),
        vec!["queued one".into(), "queued two".into()],
    );
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(parent_gate));
    registry.register(queue_tool);
    h.runtime.set_tools(Arc::new(registry));

    h.submit("queue before an append outage").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "parent gate not reached"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.runtime.queued_follow_ups(h.session()).len(), 2);

    // The current run's tool-result write fails, then the scheduled run's user
    // message write also fails. No later run remains to poll a re-queued entry.
    h.store.fail_appends(true);
    parent_handle.release();
    let first = h.collect_until_finished().await;
    assert_eq!(finished_status(&first), "failed");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !h.runtime.has_active_run(h.session())
            && h.runtime.queued_follow_ups(h.session()).is_empty()
            && h.store.runs().len() == 2
        {
            h.store.fail_appends(false);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "failed follow-up was left pending: {:?}",
            h.runtime.queued_follow_ups(h.session())
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut saw_explicit_failure = false;
    while let Ok(event) = h.events.try_recv() {
        if let Event::Notice { level, message } = event.event {
            if level == "warn" && message.contains("were removed") {
                saw_explicit_failure = true;
            }
        }
    }
    assert!(
        saw_explicit_failure,
        "the dropped accepted input needs a visible failure"
    );
    assert!(h.wait_idle().await.is_empty());

    let recovered = h.run_to_finish("manual retry").await;
    assert_eq!(finished_status(&recovered), "completed");
}

/// Queue insertion must hold the run registration lock until the entry is in
/// the queue; otherwise teardown can drain an empty queue and strand a 202.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_up_admission_serializes_with_deregistration() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![])])
        .build()
        .await;
    h.submit("long job").await;
    let inner = Arc::clone(h.runtime.shared());
    let queue_guard = inner.follow_ups.lock().unwrap();
    let runtime = Arc::clone(&h.runtime);
    let session = h.session().to_string();
    let worker = std::thread::spawn(move || runtime.enqueue_follow_up(&session, "after"));
    // Wait until admission has reached the queue lock. At that point it must
    // still own the runs lock; the old check-then-insert code did not.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if inner.runs.try_lock().is_err() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "admission did not hold the runs lock while waiting for the queue"
        );
        std::thread::yield_now();
    }
    drop(queue_guard);
    worker.join().unwrap().unwrap();
    assert_eq!(h.runtime.queued_follow_ups(h.session()).len(), 1);
    h.runtime.cancel(h.session());
}

/// A follow-up with nothing running is refused; a plain `send` is what it means.
#[tokio::test]
async fn a_follow_up_with_nothing_running_is_refused() {
    let h = HarnessBuilder::new(vec![Script::text("unused")])
        .build()
        .await;
    let error = h
        .runtime
        .enqueue_follow_up(h.session(), "after")
        .expect_err("a follow-up without a run has no meaning");
    assert!(matches!(error, Error::BadRequest(_)), "{error:?}");
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
}

/// The follow-up queue is bounded (each entry is a whole run).
#[tokio::test]
async fn the_follow_up_queue_is_bounded() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("long job").await;
    let _ = h.wait_idle().await;

    let mut accepted = 0usize;
    for i in 0..(crate::MAX_QUEUED_FOLLOW_UPS + 3) {
        match h
            .runtime
            .enqueue_follow_up(h.session(), format!("then {i}"))
        {
            Ok(_) => accepted += 1,
            Err(error) => {
                assert!(error.to_string().contains("already has"), "{error}");
                break;
            }
        }
    }
    assert_eq!(accepted, crate::MAX_QUEUED_FOLLOW_UPS);
    h.runtime.cancel(h.session());
}

/// Empty content is rejected by both queues, before reaching any storage.
#[tokio::test]
async fn an_empty_queued_message_is_rejected() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("job").await;
    let _ = h.wait_idle().await;

    assert!(h.runtime.steer(h.session(), "   \n ").is_err());
    assert!(h.runtime.enqueue_follow_up(h.session(), "\t").is_err());
    assert!(h.runtime.queued_steers(h.session()).is_empty());
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
    h.runtime.cancel(h.session());
}

// ---------------------------------------------------------------------------
// Recalling a queued message
// ---------------------------------------------------------------------------

fn inline_attachment() -> kc_core::queue::QueuedAttachment {
    kc_core::queue::QueuedAttachment {
        name: "notes.md".into(),
        path: "notes.md".into(),
        mode: kc_core::queue::AttachmentMode::Inline,
        size: 812,
        content: Some("# Note\n".into()),
    }
}

/// What the browser sends for "summarise this file": the rendered body goes to
/// the model, the draft and the chips are what a recall restores.
fn with_attachment() -> QueuedInput {
    QueuedInput::with_attachments(
        "summarise\n\nAttached 1 file:\n\n### notes.md (inlined, 812 B)\n\n```md\n# Note\n```",
        Some("summarise".into()),
        vec![inline_attachment()],
    )
}

/// Attachments survive the round trip through the queue, and the recalled message
/// hands back the draft and the chips rather than the rendered body.
#[tokio::test]
async fn a_queued_steer_keeps_its_attachments_and_recalls_them() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("long job").await;
    let _ = h.wait_idle().await;

    let queued = h.runtime.steer(h.session(), with_attachment()).unwrap();
    assert_eq!(queued.kind, QueuedKind::Steer);

    // What a reload or a second tab sees: content for the model, draft and
    // chips for the composer.
    let listed = h.runtime.queued_steers(h.session());
    assert_eq!(listed.len(), 1);
    let view = listed[0].public_view();
    assert_eq!(view["kind"], "steer");
    assert_eq!(view["draft"], "summarise");
    assert_eq!(view["attachments"][0]["name"], "notes.md");
    assert_eq!(view["attachments"][0]["mode"], "inline");
    assert_eq!(view["attachments"][0]["size"], 812);
    assert!(view["content"]
        .as_str()
        .unwrap()
        .contains("### notes.md (inlined, 812 B)"));

    // Recalling it takes it out of the queue and gives it back whole.
    let recalled = h.runtime.recall_queued(h.session(), &queued.id).unwrap();
    assert_eq!(recalled.id, queued.id);
    assert_eq!(recalled.recall_text(), "summarise");
    assert_eq!(recalled.attachments, vec![inline_attachment()]);
    assert!(
        h.runtime.queued_steers(h.session()).is_empty(),
        "a recalled message must not stay queued"
    );

    // The message never reached the conversation.
    let users: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content)
        .collect();
    assert_eq!(users, vec!["long job".to_string()]);
    h.runtime.cancel(h.session());
}

/// A follow-up is recalled the same way, out of its own queue.
#[tokio::test]
async fn a_queued_follow_up_can_be_recalled() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("long job").await;
    let _ = h.wait_idle().await;

    let queued = h
        .runtime
        .enqueue_follow_up(h.session(), with_attachment())
        .unwrap();
    assert_eq!(queued.kind, QueuedKind::FollowUp);
    assert_eq!(h.runtime.queued_follow_ups(h.session()).len(), 1);

    let recalled = h.runtime.recall_queued(h.session(), &queued.id).unwrap();
    assert_eq!(recalled.recall_text(), "summarise");
    assert_eq!(recalled.attachments.len(), 1);
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
    h.runtime.cancel(h.session());
}

/// A recall that names a message which is no longer queued is a 404, not a silent
/// success: the browser raced the run loop and must be told.
#[tokio::test]
async fn recalling_an_applied_message_is_not_found() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c1", "steer_now", "{}")]),
        Script::text("applied"),
    ])
    .build()
    .await;
    // The tool steers from inside the turn, so the run is provably in flight.
    let tool = SteerTool::boxed(
        "steer_now",
        Arc::clone(&h.runtime),
        h.session(),
        "one more thing",
    );
    let mut registry = ToolRegistry::new();
    registry.register(tool);
    h.runtime.set_tools(Arc::new(registry));

    let events = h.run_to_finish("start").await;
    let applied = events
        .iter()
        .find_map(|event| match &event.event {
            Event::SteerApplied { steer_id, .. } => Some(steer_id.clone()),
            _ => None,
        })
        .expect("the steer landed");

    // The loop drained it, so it is gone - and recalling it says so.
    let error = h
        .runtime
        .recall_queued(h.session(), &applied)
        .expect_err("an applied message cannot be recalled");
    assert!(matches!(error, Error::NotFound(_)), "{error:?}");
    assert!(
        error.to_string().contains("already have been applied"),
        "{error}"
    );
}

/// Queueing and recalling both publish the whole queue, so a second tab (or a
/// client that missed a frame) never shows a stale pending panel.
#[tokio::test]
async fn every_queue_mutation_publishes_the_whole_queue() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("long job").await;
    let _ = h.wait_idle().await;

    let queued = h.runtime.steer(h.session(), "adjust course").unwrap();
    let first = next_of_kind(&mut h, "queued_messages").await;
    match first.event {
        Event::QueuedMessages {
            session_id,
            steers,
            follow_ups,
        } => {
            assert_eq!(session_id, h.session());
            assert_eq!(steers.len(), 1);
            assert_eq!(steers[0].id, queued.id);
            assert!(follow_ups.is_empty());
        }
        other => panic!("expected queued_messages, got {other:?}"),
    }

    h.runtime.recall_queued(h.session(), &queued.id).unwrap();
    let second = next_of_kind(&mut h, "queued_messages").await;
    match second.event {
        Event::QueuedMessages { steers, .. } => {
            assert!(steers.is_empty(), "the recall must empty the queue")
        }
        other => panic!("expected queued_messages, got {other:?}"),
    }

    // A follow-up publishes too, carrying the other queue's contents untouched.
    h.runtime
        .enqueue_follow_up(h.session(), "then run the tests")
        .unwrap();
    let third = next_of_kind(&mut h, "queued_messages").await;
    match third.event {
        Event::QueuedMessages {
            steers, follow_ups, ..
        } => {
            assert!(steers.is_empty());
            assert_eq!(follow_ups.len(), 1);
        }
        other => panic!("expected queued_messages, got {other:?}"),
    }
    h.runtime.cancel(h.session());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_queue_snapshot_cannot_overtake_a_newer_mutation() {
    // No provider task: a real loop could legitimately drain the new item before
    // its delayed publisher runs, which tests consumption rather than publication.
    // A registered run admits queue entries without any concurrent consumer.
    let mut h = HarnessBuilder::new(vec![]).build().await;
    let synthetic = Arc::new(crate::RunState::new(
        RunId::new(),
        SessionId::from_string(h.session()),
        None,
        CancellationToken::new(),
        true,
    ));
    let synthetic_key = synthetic.key();
    h.runtime
        .inner
        .runs
        .lock()
        .expect("runs lock")
        .insert(synthetic_key.clone(), synthetic);
    let old = h.steer("old");
    loop {
        let event = h.next_event().await;
        if matches!(&event.event, Event::QueuedMessages { steers, .. } if steers.iter().any(|m| m.id == old.id))
        {
            break;
        }
    }

    let inner = Arc::clone(h.runtime.shared());
    // Blocking sequence publication holds the first publisher *after* it has
    // constructed the empty snapshot. While that publisher waits, a newer
    // accepted steer becomes visible in the actual map.
    let sequence = inner.seq.lock().expect("sequence lock");
    let runtime = Arc::clone(&h.runtime);
    let session = h.session().to_string();
    let old_id = old.id.clone();
    let recall = std::thread::spawn(move || runtime.recall_queued(&session, &old_id));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if inner.queue_publication.try_lock().is_err() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "first publisher did not enter"
        );
        std::thread::yield_now();
    }
    let runtime = Arc::clone(&h.runtime);
    let session = h.session().to_string();
    let newer = std::thread::spawn(move || runtime.steer(&session, "new"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if h.runtime
            .queued_steers(h.session())
            .iter()
            .any(|m| m.content == "new")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "newer mutation was not admitted"
        );
        std::thread::yield_now();
    }
    // The newer mutation must be waiting behind the older snapshot, not
    // constructing/publishing independently out of order.
    assert!(!newer.is_finished());
    drop(sequence);
    recall.join().unwrap().expect("recall old message");
    let new_message = newer.join().unwrap().expect("admit new message");
    let steers_after_join = h.runtime.queued_steers(h.session());
    let follow_ups_after_join = h.runtime.queued_follow_ups(h.session());

    // Both OS-thread joins guarantee their events are already buffered. Read by
    // sequence rather than assuming they are the only queue frames: an ordinary
    // loop drain may publish an additional empty frame around them.
    let mut empty_snapshot_seq = None;
    let mut new_snapshot_seq = None;
    let mut observed = Vec::new();
    while let Ok(event) = h.events.try_recv() {
        if let Event::QueuedMessages { steers, .. } = event.event {
            let contents: Vec<String> = steers.iter().map(|m| m.content.clone()).collect();
            if contents.is_empty() && empty_snapshot_seq.is_none() {
                empty_snapshot_seq = Some(event.seq);
            }
            if contents.iter().any(|content| content == "new") {
                new_snapshot_seq = Some(event.seq);
            }
            observed.push((event.seq, contents));
        } else {
            observed.push((event.seq, vec![format!("OTHER {:?}", event.event)]));
        }
    }
    let empty_snapshot_seq = empty_snapshot_seq.expect("recall's old empty snapshot");
    let new_snapshot_seq = new_snapshot_seq.unwrap_or_else(|| {
        panic!(
            "the newer queue-content snapshot; observed={observed:?}; steers={steers_after_join:?}; follow_ups={follow_ups_after_join:?}"
        )
    });
    assert!(
        empty_snapshot_seq < new_snapshot_seq,
        "old state followed the newer state: {observed:?}"
    );

    h.runtime
        .recall_queued(h.session(), &new_message.id)
        .expect("remove the synthetic queued entry");
    h.runtime
        .inner
        .runs
        .lock()
        .expect("runs lock")
        .remove(&synthetic_key);
    assert!(h.runtime.queued_steers(h.session()).is_empty());
    assert!(h.runtime.queued_follow_ups(h.session()).is_empty());
}

/// A queued message may not carry an unbounded chip list: the queue is memory
/// that lives for the length of a run.
#[tokio::test]
async fn a_queued_message_is_bounded_in_attachments() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("busy".into())])])
        .build()
        .await;
    h.submit("long job").await;
    let _ = h.wait_idle().await;

    let attachments = vec![inline_attachment(); kc_core::queue::MAX_QUEUED_ATTACHMENTS + 1];
    let error = h
        .runtime
        .steer(
            h.session(),
            QueuedInput::with_attachments("hi", None, attachments),
        )
        .expect_err("too many attachments");
    assert!(
        error.to_string().contains("at most 16 attachments"),
        "{error}"
    );
    assert!(
        h.runtime.queued_steers(h.session()).is_empty(),
        "a rejected request must queue nothing"
    );
    h.runtime.cancel(h.session());
}

/// Drain the queue frames until `kind` arrives, so an unrelated event cannot make
/// the assertion depend on ordering.
async fn next_of_kind(h: &mut Harness, kind: &str) -> EventEnvelope {
    for _ in 0..64 {
        let event = h.next_event().await;
        if event_kind(&event.event) == kind {
            return event;
        }
    }
    panic!("no `{kind}` event after 64 frames");
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// Collect every `notification` frame the harness sees, up to and including the
/// first one that follows a `run_finished`.
async fn notification_after_run(h: &mut Harness) -> (EventEnvelope, Vec<EventEnvelope>) {
    let mut seen = Vec::new();
    loop {
        let event = h.next_event().await;
        let is_notification = matches!(event.event, Event::Notification { .. });
        seen.push(event.clone());
        if is_notification {
            return (event, seen);
        }
    }
}

fn notify_of(event: &EventEnvelope) -> kc_core::notify::Notify {
    match &event.event {
        Event::Notification { notification } => notification.clone(),
        other => panic!("expected a notification, got {other:?}"),
    }
}

#[tokio::test]
async fn a_finished_run_emits_one_notification_after_the_terminal_state() {
    let mut h = HarnessBuilder::new(vec![Script::text("Done: the bug is fixed.")])
        .project("p_docs")
        .build()
        .await;

    h.submit("fix the bug").await;
    let (notification, seen) = notification_after_run(&mut h).await;

    // Ordering is the contract: the notification describes state, so the state
    // frame must already be on the wire when it arrives.
    let kinds: Vec<&str> = seen.iter().map(|e| event_kind(&e.event)).collect();
    let finished_at = kinds
        .iter()
        .position(|k| *k == "run_finished")
        .expect("run_finished");
    let notified_at = kinds
        .iter()
        .position(|k| *k == "notification")
        .expect("notification");
    assert!(
        finished_at < notified_at,
        "run_finished must precede its notification: {kinds:?}"
    );

    let notify = notify_of(&notification);
    assert_eq!(notify.kind, kc_core::notify::NotifyKind::RunFinished);
    assert_eq!(
        notify.id,
        format!("run:{}", notification.run_id.clone().unwrap())
    );
    assert_eq!(notify.source, kc_core::notify::RUNTIME_SOURCE);
    assert_eq!(notify.session_id, h.session());
    // The session's title travels with it so a notification is readable on its own.
    assert_eq!(notify.session_title, "test session");
    // And so does its project: the browser counts unread news per project tile,
    // and it only lists the sessions of the project it is showing.
    assert_eq!(notify.project_id, "p_docs");
    // The body is the first line of the answer.
    assert!(
        notify.body.contains("the bug is fixed"),
        "body should carry the answer's first line, got {:?}",
        notify.body
    );
    assert!(
        !notify.sticky,
        "a plain success should not pin itself to the screen"
    );
}

#[tokio::test]
async fn a_disabled_notification_config_emits_nothing_at_all() {
    // The master switch is enforced on the server: a client never has to trust
    // itself to ignore a frame, because the frame is never sent.
    let mut h = HarnessBuilder::new(vec![Script::text("hi")])
        .settings(FakeSettings::default().with_notifications(
            kc_core::config::NotificationsConfig {
                enabled: false,
                ..Default::default()
            },
        ))
        .build()
        .await;

    h.submit("anything").await;
    let seen = h.collect_until_finished().await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e.event, Event::Notification { .. })),
        "a disabled server must send no notification frames"
    );
}

#[tokio::test]
async fn a_per_event_switch_silences_only_that_event() {
    let mut h = HarnessBuilder::new(vec![Script::text("hi")])
        .settings(FakeSettings::default().with_notifications(
            kc_core::config::NotificationsConfig {
                run_finished: false,
                ..Default::default()
            },
        ))
        .build()
        .await;

    h.submit("anything").await;
    let seen = h.collect_until_finished().await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e.event, Event::Notification { .. })),
        "run_finished is switched off, so a completed run must be silent"
    );
    // The switch is specific, not a global mute.
    assert!(h.settings.notifications.needs_input);
    assert!(h.settings.notifications.error);
}

#[tokio::test]
async fn a_run_shorter_than_the_minimum_is_not_announced() {
    // min_run_secs exists for people who do not want a ping for a two-second
    // question. The test runs in milliseconds, so any minimum above 1 s suppresses.
    let mut h = HarnessBuilder::new(vec![Script::text("hi")])
        .settings(FakeSettings::default().with_notifications(
            kc_core::config::NotificationsConfig {
                min_run_secs: 3600,
                ..Default::default()
            },
        ))
        .build()
        .await;

    h.submit("anything").await;
    let seen = h.collect_until_finished().await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e.event, Event::Notification { .. })),
        "the run took well under an hour, so it must be suppressed"
    );
}

#[tokio::test]
async fn a_cancelled_run_does_not_notify_the_person_who_cancelled_it() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text("x".into())])])
        .build()
        .await;

    h.submit("long job").await;
    let _ = wait_for_kind(&mut h, "iteration_started").await;
    // `Runtime::cancel` is the human path (Escape / the stop button).
    assert!(h.runtime.cancel(h.session()));

    // Drain until the run settles, then assert no notification followed it.
    let mut seen = Vec::new();
    loop {
        let event = h.next_event().await;
        let done = matches!(event.event, Event::RunFinished { .. });
        seen.push(event);
        if done {
            break;
        }
    }
    assert!(!seen
        .iter()
        .any(|e| matches!(e.event, Event::Notification { .. })));
}

#[tokio::test]
async fn an_approval_raises_a_sticky_needs_input_notification() {
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write", "{}")]),
        Script::text("done"),
    ])
    .tool(ApprovalTool::write("write", "out.txt"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("write a file").await;
    let (event, before) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);

    // The approval notification follows the card, because the card is the thing
    // that carries the state. `wait_for_kind` already consumed the card, so the two
    // windows are stitched together before comparing positions.
    let (notify_event, after) = notification_after_run(&mut h).await;
    let seen: Vec<EventEnvelope> = before.into_iter().chain(after).collect();
    let kinds: Vec<&str> = seen.iter().map(|e| event_kind(&e.event)).collect();
    assert!(
        kinds
            .iter()
            .position(|k| *k == "approval_requested")
            .unwrap()
            < kinds.iter().position(|k| *k == "notification").unwrap(),
        "the card must exist before it is announced: {kinds:?}"
    );

    let notify = notify_of(&notify_event);
    assert_eq!(notify.kind, kc_core::notify::NotifyKind::NeedsInput);
    assert_eq!(notify.id, format!("approval:{}", approval.id));
    assert!(
        notify.sticky,
        "an approval must not scroll away before it is read"
    );
    assert!(
        notify.title.to_lowercase().contains("approval"),
        "{:?}",
        notify.title
    );

    // Let the run finish so the harness is not left with a pending gate.
    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_subagent_approval_notifies_once_through_the_parent_card() {
    // Two notifications for one card would be a bug: a subagent's approval is
    // raised through its parent's card.
    let policy = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write", "{}")]),
        Script::text("done"),
    ])
    .tool(ApprovalTool::write("write", "out.txt"))
    .settings(FakeSettings::default().with_policy(policy))
    .build()
    .await;

    h.submit("write a file").await;
    let (event, mut seen) = wait_for_kind(&mut h, "approval_requested").await;
    let approval = approval_from(&event);
    // Give any (incorrect) second notification time to arrive.
    while let Ok(Ok(next)) = tokio::time::timeout(Duration::from_millis(80), h.events.recv()).await
    {
        seen.push(next);
    }
    let approval_notifications = seen
        .iter()
        .filter(|e| match &e.event {
            Event::Notification { notification } => notification.id.starts_with("approval:"),
            _ => false,
        })
        .count();
    assert_eq!(
        approval_notifications, 1,
        "one card must produce exactly one notification"
    );

    h.runtime
        .resolve_approval(&approval.id, ApprovalDecision::AllowOnce)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// rewind
// ---------------------------------------------------------------------------
//
// A rewind hides the transcript from a chosen prompt onwards and, on request,
// puts the workspace back. These drive the real model loop with a scripted
// provider and a real file-writing tool, so the journal rows the revert replays
// come from the same code path production uses.

/// A tool that behaves like `write_file`: journal the before-image, then write.
///
/// It is deliberately a stand-in rather than `kc_tools::WriteFile` because the
/// runtime cannot depend on the tools crate (see `docs/ARCHITECTURE.md`); what
/// matters to a rewind is only that the tool calls `record_checkpoint` first,
/// which this does at the same point the real one does.
struct JournalingWrite {
    name: String,
    path: String,
    content: String,
    /// When set, the tool refuses instead of writing, to prove a refused write
    /// journals nothing.
    fail: bool,
}

#[async_trait::async_trait]
impl kc_core::tool::Tool for JournalingWrite {
    fn spec(&self) -> kc_core::tool::ToolSpec {
        kc_core::tool::ToolSpec {
            name: self.name.clone(),
            description: "overwrite one file".into(),
            parameters: serde_json::json!({"type": "object"}),
            origin: ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }

    async fn invoke(
        &self,
        _request: kc_core::tool::ToolCallRequest,
        ctx: kc_core::tool::ToolCtx,
    ) -> ToolOutcome {
        if self.fail {
            return ToolOutcome::err("refused");
        }
        ctx.record_checkpoint(&self.path, "write_file").await;
        let abs = ctx.workspace.root().join(&self.path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&abs, &self.content).expect("write");
        ToolOutcome::ok(format!("wrote {} bytes", self.content.len()))
    }
}

fn journaling_write(path: &str, content: &str) -> Arc<dyn kc_core::tool::Tool> {
    journaling_write_as("write_file", path, content)
}

fn journaling_write_as(name: &str, path: &str, content: &str) -> Arc<dyn kc_core::tool::Tool> {
    Arc::new(JournalingWrite {
        name: name.into(),
        path: path.into(),
        content: content.into(),
        fail: false,
    })
}

/// The file's bytes, or `None` when it does not exist.
fn read_file(dir: &std::path::Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(rel)).ok()
}

struct TestWorkspaceDir(std::path::PathBuf);

impl TestWorkspaceDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "kc-runtime-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create test workspace");
        Self(path)
    }
}

impl Drop for TestWorkspaceDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RecordingWorkspaces {
    default: Arc<Workspace>,
    calls: Mutex<Vec<String>>,
}

impl crate::WorkspaceProvider for RecordingWorkspaces {
    fn workspace_for(&self, _project_id: Option<&str>) -> kc_core::error::Result<Arc<Workspace>> {
        self.calls.lock().unwrap().push("default".into());
        Ok(Arc::clone(&self.default))
    }

    fn workspace_for_session(
        &self,
        _project_id: Option<&str>,
        recorded: &str,
    ) -> kc_core::error::Result<Arc<Workspace>> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("session:{recorded}"));
        Ok(Arc::new(Workspace::new(recorded)?))
    }
}

#[tokio::test]
async fn projectless_preview_and_revert_use_the_saved_session_workspace() {
    let recorded = TestWorkspaceDir::new("recorded");
    let default = TestWorkspaceDir::new("default");
    std::fs::write(recorded.0.join("existing.txt"), "before").unwrap();
    std::fs::write(default.0.join("existing.txt"), "DEFAULT: do not overwrite").unwrap();
    std::fs::write(default.0.join("created.txt"), "DEFAULT: do not delete").unwrap();
    let workspaces = Arc::new(RecordingWorkspaces {
        default: Arc::new(Workspace::new(&default.0).unwrap()),
        calls: Mutex::new(Vec::new()),
    });
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[
            ("write-existing", "write_file", "{}"),
            ("write-created", "write_file_again", "{}"),
        ]),
        Script::text("done"),
    ])
    .tool(journaling_write("existing.txt", "after"))
    .tool(journaling_write_as(
        "write_file_again",
        "created.txt",
        "new file",
    ))
    .workspaces(workspaces.clone())
    .session_workspace(recorded.0.clone())
    .build()
    .await;

    h.run_to_finish("change the recorded workspace").await;
    h.wait_idle().await;
    assert_eq!(
        read_file(&recorded.0, "existing.txt").as_deref(),
        Some("after")
    );
    assert_eq!(
        read_file(&recorded.0, "created.txt").as_deref(),
        Some("new file")
    );
    workspaces.calls.lock().unwrap().clear();
    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|message| message.role == "user")
        .expect("prompt")
        .id;

    let preview = h
        .runtime
        .rewind_preview(h.session(), prompt)
        .await
        .unwrap()
        .1;
    assert_eq!(preview.files.len(), 2);
    let outcome = h
        .runtime
        .rewind(h.session(), prompt, true, true)
        .await
        .unwrap();
    assert_eq!(outcome.outcomes.len(), 2);

    assert_eq!(
        read_file(&recorded.0, "existing.txt").as_deref(),
        Some("before")
    );
    assert_eq!(read_file(&recorded.0, "created.txt"), None);
    assert_eq!(
        read_file(&default.0, "existing.txt").as_deref(),
        Some("DEFAULT: do not overwrite")
    );
    assert_eq!(
        read_file(&default.0, "created.txt").as_deref(),
        Some("DEFAULT: do not delete")
    );
    let calls = workspaces.calls.lock().unwrap().clone();
    assert!(!calls.iter().any(|call| call == "default"), "{calls:?}");
    assert_eq!(
        calls.len(),
        2,
        "preview and revert each resolve the session: {calls:?}"
    );
}

/// Visible (non-rewound) messages of the harness session, oldest first.
async fn visible(h: &Harness) -> Vec<kc_core::store::StoredMessage> {
    h.store
        .list_messages(h.session(), 10_000)
        .await
        .expect("list messages")
}

#[tokio::test]
async fn rewinding_hides_the_tail_and_the_next_run_sees_only_what_survives() {
    let mut h = HarnessBuilder::new(vec![
        Script::text("first answer"),
        Script::text("second answer"),
        Script::text("third answer"),
    ])
    .build()
    .await;

    h.run_to_finish("first prompt").await;
    h.run_to_finish("second prompt").await;
    h.wait_idle().await;
    assert_eq!(visible(&h).await.len(), 4, "two prompt/answer pairs");

    // The second prompt is the one to revisit.
    let second = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user" && m.content == "second prompt")
        .expect("second prompt");

    let outcome = h
        .runtime
        .rewind(h.session(), second.id, false, true)
        .await
        .expect("rewind");
    assert_eq!(outcome.point.prompt, "second prompt");
    assert_eq!(
        outcome.point.messages_removed, 1,
        "only the answer after the chosen prompt is hidden"
    );
    assert!(
        outcome.outcomes.is_empty(),
        "a transcript-only rewind reverses nothing"
    );

    let after = visible(&h).await;
    assert_eq!(
        after.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        vec!["first prompt", "first answer", "second prompt"],
        "the chosen prompt survives; everything after it is hidden"
    );

    // The rows are marked, not deleted: the transcript is still whole on disk.
    assert_eq!(
        h.session_messages().await.len(),
        4,
        "a rewind hides, it does not delete"
    );
    assert!(h.store.last_rewind_at(h.session()).await.unwrap().is_some());

    // The next run is built from the rewind: the model sees the surviving three
    // messages plus the new prompt, never the hidden answer.
    h.run_to_finish("third prompt").await;
    let requests = h.factory.requests();
    let request = requests
        .last()
        .expect("the model was called after the rewind");
    let contents: Vec<String> = request.messages.iter().map(|m| m.content.clone()).collect();
    assert!(
        !contents.iter().any(|c| c == "second answer"),
        "the rewound answer must never reach the model again: {contents:?}"
    );
    assert!(contents.iter().any(|c| c == "first answer"));
    assert!(contents.iter().any(|c| c == "third prompt"));
}

#[tokio::test]
async fn a_rewind_refuses_while_a_run_is_in_flight() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![ProviderChunk::Text(
        "working".into(),
    )])])
    .build()
    .await;
    h.submit("do something slow").await;
    // Wait until the run is registered so `is_running` is true.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !h.runtime.is_running(h.session()) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let any = h
        .session_messages()
        .await
        .first()
        .map(|m| m.id)
        .unwrap_or(1);
    let err = h
        .runtime
        .rewind(h.session(), any, false, true)
        .await
        .expect_err("rewinding a live run would be undone by the running loop");
    assert!(err.to_string().contains("run in flight"), "{err}");
    h.runtime.cancel(h.session());
    h.wait_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rewind_and_retry_exclude_a_competing_submit_across_the_store_barrier() {
    let mut h = HarnessBuilder::new(vec![Script::text("original answer"), Script::Hang(vec![])])
        .build()
        .await;
    let initial = h.run_to_finish("original question").await;
    assert_eq!(finished_status(&initial), "completed");
    let prompt_id = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user" && m.content == "original question")
        .expect("original prompt")
        .id;

    let rewind_gate = h.store.gate_next("rewind");
    let runtime = Arc::clone(&h.runtime);
    let session = h.session().to_string();
    let retry = tokio::spawn(async move {
        runtime
            .rewind_and_run(&session, prompt_id, false, "replacement question".into())
            .await
    });
    rewind_gate.wait_entered().await;

    let runtime = Arc::clone(&h.runtime);
    let session = h.session().to_string();
    let competing =
        tokio::spawn(async move { runtime.submit(&session, "interloper question".into()).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !competing.is_finished(),
        "competing submit crossed the held rewind barrier"
    );
    assert_eq!(
        h.store.runs().len(),
        1,
        "no replacement/interloper row exists yet"
    );
    assert!(!h
        .session_messages()
        .await
        .iter()
        .any(|m| m.content == "interloper question"));

    rewind_gate.release();
    let retry = tokio::time::timeout(Duration::from_secs(5), retry)
        .await
        .expect("retry joins")
        .expect("retry task did not panic")
        .expect("retry and replacement run succeed together");
    assert!(retry.run_id.is_some());
    let competing = tokio::time::timeout(Duration::from_secs(5), competing)
        .await
        .expect("competing submit joins")
        .expect("competing task did not panic");
    assert!(
        matches!(competing, Err(Error::BadRequest(ref message)) if message.contains("already active")),
        "the replacement must already be registered when the other submit enters: {competing:?}"
    );
    assert!(!h
        .session_messages()
        .await
        .iter()
        .any(|m| m.content == "interloper question"));

    h.runtime.cancel(h.session());
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn a_cancelled_but_unwinding_run_still_blocks_rewind() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![])])
        .build()
        .await;
    h.submit("cancel, then try to rewind").await;
    let prompt_id = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt")
        .id;
    let finish_gate = h.store.gate_next("finish_run");
    assert!(h.runtime.cancel(h.session()));
    finish_gate.wait_entered().await;
    assert!(!h.runtime.is_running(h.session()));
    assert!(h.runtime.has_active_run(h.session()) || !h.runtime.active_runs().is_empty());

    let error = h
        .runtime
        .rewind(h.session(), prompt_id, false, true)
        .await
        .expect_err("cancelled is not quiescent until terminal storage finishes");
    assert!(error.to_string().contains("unwinding"), "{error}");
    finish_gate.release();
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "cancelled");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn rewind_preview_rejects_aggregate_images_before_mutation() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("c1", "write_file", "{}")]),
        Script::text("done"),
    ])
    .tool(journaling_write("a", "after"))
    .build()
    .await;
    std::fs::write(h.workspace_dir.join("a"), "before").unwrap();
    h.run_to_finish("edit").await;
    h.wait_idle().await;
    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .unwrap();
    let mut row = h.store.list_checkpoints(h.session(), 0).await.unwrap()[0].clone();
    row.path = "b".into();
    row.content = "x".repeat(8 * 1024 * 1024);
    h.store.record_checkpoint(&row).await.unwrap();
    let error = h
        .runtime
        .rewind_preview(h.session(), prompt.id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("before-images exceed"));
    assert!(h
        .runtime
        .rewind(h.session(), prompt.id, true, true)
        .await
        .is_err());
    assert_eq!(read_file(&h.workspace_dir, "a").as_deref(), Some("after"));
    assert!(!h.session_messages().await.is_empty());
}

#[tokio::test]
async fn reverting_puts_back_the_file_a_run_overwrote() {
    let mut h = HarnessBuilder::new(vec![
        // Turn one: overwrite a file that already exists.
        Script::call(&[("call_1", "write_file", "{}")]),
        Script::text("done"),
    ])
    .tool(journaling_write("existing.txt", "AFTER\n"))
    .build()
    .await;

    std::fs::write(h.workspace_dir.join("existing.txt"), "BEFORE\n").expect("seed");

    h.run_to_finish("overwrite the file").await;
    h.wait_idle().await;
    assert_eq!(
        read_file(&h.workspace_dir, "existing.txt").as_deref(),
        Some("AFTER\n")
    );

    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    let preview = h
        .runtime
        .rewind_preview(h.session(), prompt.id)
        .await
        .expect("preview")
        .1;
    assert_eq!(preview.files.len(), 1, "one file is affected");
    assert_eq!(preview.files[0].path, "existing.txt");
    assert_eq!(
        preview.files[0].restore.as_deref(),
        Some("BEFORE\n"),
        "the preview describes the state the revert restores"
    );
    assert!(!preview.files[0].removes_file);
    assert!(!preview.incomplete);
    // The preview must not have changed anything.
    assert_eq!(
        read_file(&h.workspace_dir, "existing.txt").as_deref(),
        Some("AFTER\n")
    );

    let outcome = h
        .runtime
        .rewind(h.session(), prompt.id, true, true)
        .await
        .expect("rewind with revert");
    assert_eq!(
        outcome.outcomes,
        vec![kc_core::checkpoint::UndoOutcome::Restored]
    );
    assert_eq!(
        read_file(&h.workspace_dir, "existing.txt").as_deref(),
        Some("BEFORE\n"),
        "the revert must restore the bytes the tool replaced"
    );
    assert!(
        h.store
            .list_checkpoints(h.session(), 0)
            .await
            .unwrap()
            .is_empty(),
        "an undone journal row is retired"
    );
}

#[tokio::test]
async fn reverting_removes_a_file_the_run_created() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write_file", "{}")]),
        Script::text("done"),
    ])
    .tool(journaling_write("brand-new.txt", "hello\n"))
    .build()
    .await;

    h.run_to_finish("create a file").await;
    h.wait_idle().await;
    assert!(h.workspace_dir.join("brand-new.txt").exists());

    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");
    let preview = h
        .runtime
        .rewind_preview(h.session(), prompt.id)
        .await
        .expect("preview")
        .1;
    assert_eq!(preview.files.len(), 1);
    assert!(
        preview.files[0].removes_file,
        "a file that did not exist is removed, not restored"
    );
    assert!(preview.files[0].restore.is_none());

    let outcome = h
        .runtime
        .rewind(h.session(), prompt.id, true, true)
        .await
        .expect("rewind");
    assert_eq!(
        outcome.outcomes,
        vec![kc_core::checkpoint::UndoOutcome::Removed]
    );
    assert!(
        !h.workspace_dir.join("brand-new.txt").exists(),
        "undoing a create removes the file"
    );
}

#[tokio::test]
async fn a_rewind_without_revert_leaves_the_workspace_alone() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write_file", "{}")]),
        Script::text("done"),
    ])
    .tool(journaling_write("kept.txt", "written\n"))
    .build()
    .await;
    std::fs::write(h.workspace_dir.join("kept.txt"), "original\n").expect("seed");

    h.run_to_finish("change the file").await;
    h.wait_idle().await;
    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    let outcome = h
        .runtime
        .rewind(h.session(), prompt.id, false, true)
        .await
        .expect("rewind");
    assert!(outcome.outcomes.is_empty());
    assert_eq!(
        read_file(&h.workspace_dir, "kept.txt").as_deref(),
        Some("written\n"),
        "keeping the changes is the default: the transcript is hidden, the files are not"
    );
    assert!(
        !h.store
            .list_checkpoints(h.session(), 0)
            .await
            .unwrap()
            .is_empty(),
        "the journal survives, so a later revert can still undo the work"
    );
}

#[tokio::test]
async fn a_run_that_only_read_files_reports_no_files_to_revert() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{}")]),
        Script::text("done"),
    ])
    .tool(EchoTool::boxed("echo"))
    .build()
    .await;

    h.run_to_finish("look around").await;
    h.wait_idle().await;
    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    let preview = h
        .runtime
        .rewind_preview(h.session(), prompt.id)
        .await
        .expect("preview")
        .1;
    assert!(
        preview.files.is_empty(),
        "nothing changed the workspace, so there is nothing to put back"
    );
    assert!(!preview.incomplete);
}

#[tokio::test]
async fn a_rewind_point_outside_the_session_is_refused() {
    let mut h = HarnessBuilder::new(vec![Script::text("hi")]).build().await;
    h.run_to_finish("hello").await;
    h.wait_idle().await;

    let err = h
        .runtime
        .rewind("s_someone_else", 1, false, true)
        .await
        .expect_err("a foreign session id must not rewind this one");
    assert!(
        matches!(err, Error::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn retrying_replaces_the_prompt_and_re_asks_the_model() {
    let mut h = HarnessBuilder::new(vec![
        Script::text("first attempt"),
        Script::text("second attempt"),
    ])
    .build()
    .await;

    h.run_to_finish("fix the build").await;
    h.wait_idle().await;
    let original = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    // The retry re-sends the same text; the rewind hides the old prompt so the
    // model does not see the request twice.
    let outcome = h
        .runtime
        .rewind_and_run(h.session(), original.id, false, original.content.clone())
        .await
        .expect("retry");
    assert_eq!(outcome.point.prompt, "fix the build");
    assert!(
        outcome.run_id.is_some(),
        "a retry starts a run, it is not a transcript edit"
    );
    h.collect_until_finished().await;
    h.wait_idle().await;

    let messages = h.session_messages().await;
    let visible: Vec<&str> = messages
        .iter()
        .filter(|m| m.rewound_at.is_none())
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        visible,
        vec!["fix the build", "second attempt"],
        "the retried prompt and the new answer are the whole visible transcript"
    );

    // The old prompt row still exists, hidden - a rewind never deletes.
    assert!(
        messages
            .iter()
            .any(|m| m.content == "first attempt" && m.rewound_at.is_some()),
        "the abandoned answer is hidden, not deleted"
    );

    // And the model was asked again, with the prompt exactly once.
    let requests = h.factory.requests();
    let last = requests.last().expect("a second model call");
    let prompts = last
        .messages
        .iter()
        .filter(|m| m.content == "fix the build")
        .count();
    assert_eq!(
        prompts, 1,
        "the prompt must not appear twice in one request"
    );
}

#[tokio::test]
async fn a_note_replaces_the_prompt_the_user_wants_to_move_on_from() {
    let mut h = HarnessBuilder::new(vec![
        Script::text("first attempt"),
        Script::text("answer to the note"),
    ])
    .build()
    .await;

    h.run_to_finish("add a login page").await;
    h.wait_idle().await;
    let original = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    let outcome = h
        .runtime
        .rewind_and_run(
            h.session(),
            original.id,
            false,
            "actually, add a signup page instead".to_string(),
        )
        .await
        .expect("note");
    assert!(outcome.run_id.is_some());
    h.collect_until_finished().await;
    h.wait_idle().await;

    let visible: Vec<String> = h
        .session_messages()
        .await
        .into_iter()
        .filter(|m| m.rewound_at.is_none())
        .map(|m| m.content)
        .collect();
    assert_eq!(
        visible,
        vec![
            "actually, add a signup page instead".to_string(),
            "answer to the note".to_string()
        ],
        "the note and its answer replace the abandoned turn"
    );

    let requests = h.factory.requests();
    let last = requests.last().expect("a second model call");
    let contents: Vec<&str> = last.messages.iter().map(|m| m.content.as_str()).collect();
    assert!(
        !contents.contains(&"add a login page"),
        "the model must not see the prompt the note replaced: {contents:?}"
    );
    assert!(contents.iter().any(|c| c.contains("signup page")));
}

#[tokio::test]
async fn retrying_with_revert_undoes_the_abandoned_turn_before_re_asking() {
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "write_file", "{}")]),
        Script::text("wrote it"),
        Script::text("second attempt"),
    ])
    .tool(journaling_write("config.toml", "port = 8080\n"))
    .build()
    .await;
    std::fs::write(h.workspace_dir.join("config.toml"), "port = 3000\n").expect("seed");

    h.run_to_finish("set the port").await;
    h.wait_idle().await;
    assert_eq!(
        read_file(&h.workspace_dir, "config.toml").as_deref(),
        Some("port = 8080\n")
    );

    let original = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");
    let outcome = h
        .runtime
        .rewind_and_run(h.session(), original.id, true, original.content.clone())
        .await
        .expect("retry with revert");
    assert_eq!(
        outcome.outcomes,
        vec![kc_core::checkpoint::UndoOutcome::Restored],
        "the abandoned turn's write is undone even though its prompt is the rewind point"
    );
    assert_eq!(
        read_file(&h.workspace_dir, "config.toml").as_deref(),
        Some("port = 3000\n"),
        "the file is back to what it was before the retried turn ran"
    );
    h.collect_until_finished().await;
    h.wait_idle().await;
}

#[tokio::test]
async fn rewind_reports_commit_and_emits_event_when_replacement_storage_fails() {
    let mut h = HarnessBuilder::new(vec![
        Script::text("abandoned answer"),
        Script::text("a later submit still works"),
    ])
    .build()
    .await;
    h.run_to_finish("question to replace").await;
    h.wait_idle().await;
    while h.events.try_recv().is_ok() {}
    let original = h
        .session_messages()
        .await
        .into_iter()
        .find(|message| message.role == "user")
        .expect("original prompt");

    // The rewind writes rewound_at first. begin_run then creates a run but its
    // user-message append fails: this is a post-commit startup failure, not a
    // rollback of a rewind that already happened.
    h.store.fail_appends(true);
    let outcome = h
        .runtime
        .rewind_and_run(h.session(), original.id, false, "replacement".into())
        .await
        .expect("a committed rewind is a structured outcome, not a lost Err");
    assert!(outcome.committed);
    assert_eq!(outcome.run_id, None);
    assert!(outcome
        .start_error
        .as_deref()
        .unwrap_or("")
        .contains("disk is full"));
    assert_eq!(outcome.point.user_message_id, original.id);
    assert!(
        visible(&h).await.is_empty(),
        "the original pair really was hidden"
    );

    let first = h.next_event().await;
    assert!(matches!(
        first.event,
        Event::SessionRewound { ref session_id, message_id, .. }
            if session_id == h.session() && message_id == original.id
    ));
    let notice = h.next_event().await;
    assert!(matches!(
        notice.event,
        Event::Notice { ref level, ref message }
            if level == "warn" && message.contains("replacement run could not start")
    ));
    assert!(h.store.runs().iter().any(|run| run.status == "failed"));

    // Failure did not leave a registered run or lifecycle lock behind.
    assert!(h.wait_idle().await.is_empty());
    assert!(h.runtime.inner.session_lifecycle.lock().unwrap().is_empty());
    h.store.fail_appends(false);
    let events = h.run_to_finish("later question").await;
    assert_eq!(finished_status(&events), "completed");
}

#[tokio::test]
async fn a_retry_without_a_prompt_is_refused_before_anything_changes() {
    let mut h = HarnessBuilder::new(vec![Script::text("hi")]).build().await;
    h.run_to_finish("hello").await;
    h.wait_idle().await;
    let prompt = h
        .session_messages()
        .await
        .into_iter()
        .find(|m| m.role == "user")
        .expect("prompt");

    let err = h
        .runtime
        .rewind_and_run(h.session(), prompt.id, false, "   ".to_string())
        .await
        .expect_err("an empty retry prompt has nothing to ask");
    assert!(err.to_string().contains("prompt to send"), "{err}");
    // The transcript is untouched: validation happens before the rewind.
    assert!(h
        .session_messages()
        .await
        .iter()
        .all(|m| m.rewound_at.is_none()));
}

// ---------------------------------------------------------------------------
// parallel and background subagents
// ---------------------------------------------------------------------------

/// One `spawn_agent` call with arbitrary arguments.
fn spawn_args(args: serde_json::Value) -> Script {
    Script::call(&[("call_spawn", SPAWN_AGENT_TOOL, &args.to_string())])
}

fn fan_out_args(tasks: serde_json::Value) -> Script {
    let args = serde_json::json!({ "tasks": tasks });
    Script::call(&[(
        "call_fan",
        crate::subagent::SPAWN_AGENTS_TOOL,
        &args.to_string(),
    )])
}

/// A `[subagents]` section with room to fan out.
fn fan_out_settings() -> SubagentConfig {
    SubagentConfig {
        max_concurrent: 4,
        max_iterations: 6,
        allow_recursive: true,
        max_depth: 3,
        timeout_secs: 30,
        allow_background: true,
        max_background: 4,
        wait_timeout_secs: 5,
    }
}

#[tokio::test]
async fn a_fan_out_runs_its_children_together_and_returns_one_batch() {
    // The point of `spawn_agents`: two blocking `spawn_agent` calls in one turn
    // execute one after the other, so they never overlap. A fan-out starts both
    // before waiting for either, which is what the limiter's peak records.
    let mut h = HarnessBuilder::new(vec![
        fan_out_args(serde_json::json!([
            { "task": "review the parser", "name": "reviewer" },
            { "task": "map the call sites", "name": "mapper" }
        ])),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "review the parser",
        vec![Script::text("the parser looks fine")],
    );
    h.factory
        .key("map the call sites", vec![Script::text("three call sites")]);

    let events = h.run_to_finish("fan out the review").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "agent_spawned"), 2, "{:?}", kinds(&events));
    assert_eq!(count(&events, "agent_finished"), 2);

    let agents = h.store.agents();
    assert_eq!(agents.len(), 2);
    assert!(
        agents.iter().all(|agent| agent.status == "completed"),
        "{agents:?}"
    );
    assert_eq!(
        h.runtime.subagents_peak(),
        2,
        "a fan-out must run its children concurrently"
    );

    let tool = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "call_fan")
        .expect("the fan-out's own result");
    assert!(tool.1, "a clean fan-out is ok: {}", tool.2);
    assert!(tool.2.contains("the parser looks fine"), "{}", tool.2);
    assert!(tool.2.contains("three call sites"), "{}", tool.2);
}

/// Twenty is more than the old finite ceiling of sixteen, so this fails if
/// `max_concurrent = 0` (the "no ceiling" spelling) is ever read as "allow
/// nothing" or clamped back to a small number on the way in.
///
/// It is a *background* fan-out on purpose: that path takes a slot with
/// `try_acquire` and is separately gated by `max_background`, so one test covers
/// both unlimited switches and the batch capacity check that sits between them.
#[tokio::test]
async fn an_unbounded_setting_starts_its_whole_fan_out() {
    const WIDTH: usize = 20;
    let tasks: Vec<serde_json::Value> = (0..WIDTH)
        .map(|i| {
            serde_json::json!({
                "task": format!("job {i}"),
                "name": format!("worker-{i}"),
            })
        })
        .collect();
    let args = serde_json::json!({ "tasks": tasks, "background": true });
    let settings = SubagentConfig {
        max_concurrent: 0,
        max_background: 0,
        ..fan_out_settings()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[(
            "call_fan",
            crate::subagent::SPAWN_AGENTS_TOOL,
            &args.to_string(),
        )]),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(settings))
    .build()
    .await;
    for i in 0..WIDTH {
        h.factory
            .key(&format!("job {i}"), vec![Script::text("done")]);
    }

    let events = h.run_to_finish("fan out twenty jobs").await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(
        count(&events, "agent_spawned"),
        WIDTH,
        "every child in an unbounded fan-out has to start: {:?}",
        kinds(&events)
    );

    let tool = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "call_fan")
        .expect("the fan-out's own result");
    assert!(
        tool.1,
        "an unbounded fan-out must not be refused: {}",
        tool.2
    );
    assert!(
        tool.2
            .contains(&format!("started {WIDTH} background subagents")),
        "the batch reports every child it started: {}",
        tool.2
    );
    assert!(
        !tool.2.contains("refused"),
        "nothing was refused: {}",
        tool.2
    );
    let live = h.store.agents();
    assert_eq!(live.len(), WIDTH, "every child has its own row: {live:?}");
}

#[tokio::test]
async fn a_background_spawn_returns_at_once_and_wait_collects_it() {
    let (gate, handle) = GateTool::pair("gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "a slow job",
            "name": "slowpoke",
            "background": true
        })),
        // The parent asks for the result in its next turn, while the child is
        // still inside its tool call. (A text-only answer would end the run.)
        Script::call(&[(
            "call_wait",
            crate::subagent::WAIT_FOR_AGENTS_TOOL,
            "{\"timeout_secs\":10}",
        )]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "a slow job",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("the child answered"),
        ],
    );

    let mut events = Vec::new();
    h.submit("delegate without blocking").await;
    // Read frames until the parent reaches its wait, releasing the child once it
    // is genuinely parked inside the tool.
    loop {
        let event = h.next_event().await;
        let waiting = matches!(
            &event.event,
            Event::ToolCallStarted { name, .. } if name == crate::subagent::WAIT_FOR_AGENTS_TOOL
        );
        let spawned = matches!(&event.event, Event::AgentSpawned { .. });
        events.push(event);
        if spawned {
            // The spawn tool has returned by now only if the child is detached;
            // the arrival counter proves the child is inside its first turn.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while handle.arrived() == 0 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the background child never started its turn"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            handle.release();
        }
        if waiting {
            break;
        }
    }
    events.extend(h.collect_until_finished().await);

    assert_eq!(finished_status(&events), "completed");

    let started = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "call_spawn")
        .expect("the spawn result");
    assert!(started.1, "{}", started.2);
    assert!(
        started.2.contains("background"),
        "the result has to say the agent kept working: {}",
        started.2
    );

    let waited = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "call_wait")
        .expect("the wait result");
    assert!(waited.1, "the child finished: {}", waited.2);
    assert!(
        waited.2.contains("the child answered"),
        "a wait returns the summary: {}",
        waited.2
    );

    let agents = h.store.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, "completed");
    assert!(agents[0].background, "the row records that it was detached");
}

#[tokio::test]
async fn a_background_spawn_is_refused_when_it_would_exceed_the_cap() {
    // `max_background` is a hard ceiling on live detached agents, and the refusal
    // names it: a model that reasons about the limit can adapt, while one that got
    // a silent queue would just wait forever.
    let (gate, handle) = GateTool::pair("gate");
    let settings = SubagentConfig {
        max_background: 1,
        ..fan_out_settings()
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "first job", "name": "first", "background": true
        })),
        spawn_args(serde_json::json!({
            "task": "second job", "name": "second", "background": true
        })),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .settings(FakeSettings::default().with_subagents(settings))
    .build()
    .await;
    h.factory.key(
        "first job",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("first finished"),
        ],
    );

    let mut events = Vec::new();
    h.submit("two background jobs").await;
    loop {
        let event = h.next_event().await;
        let second = matches!(
            &event.event,
            Event::ToolCallStarted { name, .. } if name == SPAWN_AGENT_TOOL
        );
        events.push(event);
        if second {
            break;
        }
    }
    handle.release();
    events.extend(h.collect_until_finished().await);
    assert_eq!(finished_status(&events), "completed");

    let results = tool_finished(&events);
    assert!(results[0].1, "the first spawn fits: {}", results[0].2);
    let refusal = &results[1];
    assert!(!refusal.1, "the second must be refused: {}", refusal.2);
    assert!(
        refusal.2.contains("max_background"),
        "the refusal has to name the limit: {}",
        refusal.2
    );
}

#[tokio::test]
async fn background_spawns_are_refused_when_the_setting_is_off() {
    let settings = SubagentConfig {
        allow_background: false,
        ..fan_out_settings()
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "detach me", "background": true
        })),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(settings))
    .build()
    .await;

    let events = h.run_to_finish("try to detach").await;
    let result = tool_finished(&events).remove(0);
    assert!(!result.1);
    assert!(
        result.2.contains("allow_background"),
        "the refusal has to name the switch: {}",
        result.2
    );
    assert!(h.store.agents().is_empty(), "nothing was spawned");
}

#[tokio::test]
async fn a_subagent_can_message_its_parent_and_the_parent_reads_it() {
    // The case the mailbox exists for: a child hits something only its parent can
    // answer. The message becomes an ordinary user turn in the parent's
    // conversation, labelled with who sent it.
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "look into the flaky test", "name": "probe"
        })),
        Script::text("parent done"),
    ])
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "look into the flaky test",
        vec![
            Script::call(&[(
                "call_msg",
                crate::subagent::SEND_MESSAGE_TOOL,
                "{\"message\":\"the fixture file is missing\"}",
            )]),
            Script::text("child done"),
        ],
    );

    let events = h.run_to_finish("delegate the investigation").await;
    assert_eq!(finished_status(&events), "completed");

    let sent = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "call_msg")
        .expect("the send_message result");
    assert!(sent.1, "{}", sent.2);

    // The parent's conversation carries the message as a user turn naming the
    // sender, and the parent loop emits its own frame when it takes the message.
    let messages = h.session_messages().await;
    let delivered = messages
        .iter()
        .find(|m| m.role == "user" && m.content.contains("[message from probe"));
    let delivered = delivered.unwrap_or_else(|| {
        panic!(
            "the parent must see the message: {:?}",
            messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect::<Vec<_>>()
        )
    });
    assert!(
        delivered.content.contains("the fixture file is missing"),
        "{}",
        delivered.content
    );
    // Every frame is queued at send time; delivery is the parent's own.
    let states: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.event {
            Event::AgentMessage { state, .. } => Some(state.clone()),
            _ => None,
        })
        .collect();
    assert!(states.contains(&"queued".to_string()), "{states:?}");
    assert!(states.contains(&"delivered".to_string()), "{states:?}");
}

#[tokio::test]
async fn pause_parks_a_live_agent_and_resume_lets_it_finish() {
    // A pause has to land between iterations, keep the agent visible, and be
    // liftable - all three are what make it usable rather than a slow kill.
    //
    // The parent parks in its own gate so the run is still in flight while the
    // test drives the child: a detached child may only exist for as long as the
    // run that spawned it does (see `a_run_ending_cancels_its_background_children`).
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "a long job", "name": "worker", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "a long job",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("child finished"),
        ],
    );

    h.submit("start it").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let agent_id = h.store.agents()[0].id.clone();
    // Pause through the same path the HTTP route uses.
    let status = h
        .runtime
        .control_agent(&agent_id, crate::AgentAction::Pause)
        .await
        .expect("pause a live agent");
    assert_eq!(status.as_str(), "paused");
    assert_eq!(
        h.store
            .get_agent(&agent_id)
            .await
            .unwrap()
            .unwrap()
            .agent
            .status,
        "paused",
        "the row has to agree with the roster"
    );
    // The child still exists: a pause is not a kill.
    assert_eq!(h.runtime.live_agent_count(h.session()), 1);

    assert_eq!(
        h.runtime
            .control_agent(&agent_id, crate::AgentAction::Resume)
            .await
            .unwrap()
            .as_str(),
        "running"
    );
    handle.release();

    // The child finishes on its own, then the parent is let go and completes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let agents = h.store.agents();
        if agents[0].status == "completed" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never finished: {}",
            agents[0].status
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    parent_handle.release();
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_resume_share_one_order_and_status_notes_stay_independent() {
    let mut h = HarnessBuilder::new(vec![]).build().await;
    let agent = AgentRecord {
        id: "a_control_order".into(),
        session_id: h.session().to_string(),
        parent_run_id: "r_parent".into(),
        parent_agent_id: None,
        name: "worker".into(),
        role: "worker".into(),
        task: "serialize controls".into(),
        status: "running".into(),
        model: "fake-model".into(),
        iterations: 0,
        summary: String::new(),
        error: None,
        owned_paths: Vec::new(),
        created_at: 0,
        updated_at: 0,
        finished_at: None,
        background: false,
        note: String::new(),
        progress: None,
    };
    h.store.upsert_agent(&agent).await.unwrap();
    let live = h.runtime.inner.hub.insert(
        &agent,
        agent.parent_run_id.clone(),
        1,
        CancellationToken::new(),
    );

    let pause_store = h.store.gate_next("transition_agent_status");
    let runtime = Arc::clone(&h.runtime);
    let pause = tokio::spawn(async move {
        runtime
            .control_agent("a_control_order", crate::AgentAction::Pause)
            .await
    });
    pause_store.wait_entered().await;
    assert!(live.is_paused(), "the live pause precedes its store await");
    assert_eq!(h.store.agents()[0].status, "running");

    // report_status updates neither wait for nor take the per-agent control guard.
    h.runtime
        .inner
        .hub
        .note("a_control_order", "working while paused".into(), Some(0.25));
    tokio::time::timeout(
        Duration::from_secs(1),
        h.store
            .update_agent_note("a_control_order", "working while paused", Some(0.25), 1),
    )
    .await
    .expect("status-note store update waited for pause/resume control")
    .unwrap();
    assert_eq!(live.note.lock().unwrap().as_str(), "working while paused");

    let runtime = Arc::clone(&h.runtime);
    let resume = tokio::spawn(async move {
        runtime
            .control_agent("a_control_order", crate::AgentAction::Resume)
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !resume.is_finished(),
        "resume crossed the paused store transition"
    );

    pause_store.release();
    assert_eq!(pause.await.unwrap().unwrap().as_str(), "paused");
    // The waiting resume may legally finish before this observer is scheduled.
    assert_eq!(resume.await.unwrap().unwrap().as_str(), "running");
    assert_eq!(h.store.agents()[0].status, "running");
    assert!(!live.is_paused());
    assert_eq!(h.store.agents()[0].note, "working while paused");

    let paused_event = h.events.try_recv().expect("pause event").event;
    let resumed_event = h.events.try_recv().expect("resume event").event;
    assert!(matches!(
        paused_event,
        Event::AgentPaused { paused: true, .. }
    ));
    assert!(matches!(
        resumed_event,
        Event::AgentPaused { paused: false, .. }
    ));
}

#[tokio::test]
async fn killing_an_agent_stops_it_and_records_cancelled() {
    // The parent parks in its own gate: the kill has to land while the run is
    // alive, which is the only window a detached child exists in.
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "a job to stop", "name": "doomed", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "a job to stop",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("never reached"),
        ],
    );

    h.submit("start it").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let agent_id = h.store.agents()[0].id.clone();
    let status = h
        .runtime
        .control_agent(&agent_id, crate::AgentAction::Cancel)
        .await
        .expect("cancel a live agent");
    assert_eq!(status.as_str(), "cancelled");

    // The child is settled before the parent is let go, and the parent then
    // completes on its own terms: a killed sibling is not the parent's problem.
    handle.release();
    parent_handle.release();
    let events = h.collect_until_finished().await;
    assert_eq!(
        finished_status(&events),
        "completed",
        "the parent carries on"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let agents = h.store.agents();
        if agents[0].status == "cancelled" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the killed agent never settled: {:?}",
            agents[0].status
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Its transcript stays readable: stopping is not deleting.
    assert!(
        !h.store.messages_for_agent(&agent_id).is_empty(),
        "a killed agent keeps its transcript"
    );
}

#[tokio::test]
async fn controlling_an_unknown_agent_is_refused() {
    let mut h = HarnessBuilder::new(vec![Script::text("parent done")])
        .settings(FakeSettings::default().with_subagents(fan_out_settings()))
        .build()
        .await;
    h.run_to_finish("nothing to delegate").await;

    let error = h
        .runtime
        .control_agent("a_missing", crate::AgentAction::Pause)
        .await
        .expect_err("an unknown id is not a silent success");
    assert!(
        matches!(error, Error::NotFound(_)),
        "an unknown agent is a 404: {error:?}"
    );
}

#[tokio::test]
async fn a_report_status_note_reaches_the_parents_listing() {
    // The parent parks so the listing is read while the run - and therefore its
    // detached child - is genuinely live.
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "index the repo", "name": "indexer", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "index the repo",
        vec![
            Script::call(&[(
                "call_note",
                crate::subagent::REPORT_STATUS_TOOL,
                "{\"note\":\"halfway through crates/\",\"progress\":0.5}",
            )]),
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("child finished"),
        ],
    );

    h.submit("delegate the index").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The note is stored on the row while the agent is still working, which is
    // what makes it visible to a parent that asks now rather than later.
    let agent = &h.store.agents()[0];
    assert_eq!(agent.note, "halfway through crates/");
    assert_eq!(agent.progress, Some(0.5));

    // And the parent's own listing shows it, so a model can steer on it.
    let listing = h
        .runtime
        .tools()
        .get(crate::subagent::LIST_SUBAGENTS_TOOL)
        .expect("the list tool");
    let outcome = listing
        .invoke(
            kc_core::tool::ToolCallRequest {
                call_id: "c_list".into(),
                name: crate::subagent::LIST_SUBAGENTS_TOOL.into(),
                arguments: serde_json::json!({}),
            },
            list_tool_ctx(&h),
        )
        .await;
    assert!(outcome.ok, "{}", outcome.content);
    assert!(
        outcome.content.contains("halfway through crates/"),
        "the parent must see the child's note: {}",
        outcome.content
    );

    handle.release();
    parent_handle.release();
    // Both the run's own frames and the child's are wanted: the child settles as
    // the parent is released, so `run_finished` is not the end of the stream.
    let events = h.collect_until_quiet().await;
    // The live frame carries the same claim, so a roster updates without polling.
    let note = events.iter().find_map(|event| match &event.event {
        Event::AgentNote { note, progress, .. } => Some((note.clone(), *progress)),
        _ => None,
    });
    assert_eq!(
        note,
        Some(("halfway through crates/".to_string(), Some(0.5))),
        "{:?}",
        kinds(&events)
    );
}

/// A tool context for calling a built-in delegate tool directly, as the main
/// agent of the harness session would.
fn list_tool_ctx(h: &Harness) -> kc_core::tool::ToolCtx {
    let gate: Arc<dyn ApprovalGate> = Arc::new(RuntimeApprovalGate::new(
        Arc::clone(h.runtime.shared()),
        SessionId::from_string(h.session()),
        RunId::from_string("r_probe"),
        None,
        CancellationToken::new(),
        0,
    ));
    kc_core::tool::ToolCtx {
        workspace: Arc::new(Workspace::new(&h.workspace_dir).expect("workspace")),
        cancel: CancellationToken::new(),
        approvals: kc_core::tool::ApprovalHandle::new(
            gate,
            SessionId::from_string(h.session()),
            RunId::from_string("r_probe"),
            None,
        ),
        events: Arc::new(|_| {}),
        settings: Arc::clone(&h.settings) as Arc<dyn kc_core::settings::Settings>,
        depth: 0,
        supports_images: false,
        checkpoints: None,
    }
}

#[tokio::test]
async fn a_child_cannot_control_an_agent_it_does_not_own() {
    // Ownership is transitive and enforced: a child may message a sibling, but it
    // may not stop one. Without this the hierarchy is advisory, not a hierarchy.
    let (gate, handle) = GateTool::pair("gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({ "task": "only child", "name": "one" })),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "only child",
        vec![
            Script::call(&[(
                "c_kill",
                crate::subagent::KILL_AGENTS_TOOL,
                "{\"agents\":[\"a_someone_else\"]}",
            )]),
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("one finished"),
        ],
    );

    h.submit("fan out").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    handle.release();
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "completed");

    let kill = tool_finished(&events)
        .into_iter()
        .find(|(call, _, _, _)| call == "c_kill")
        .expect("the kill attempt's result");
    assert!(!kill.1, "a child may not kill an unknown agent: {}", kill.2);
    assert!(
        kill.2.contains("no agent") || kill.2.contains("not yours"),
        "the refusal has to explain why: {}",
        kill.2
    );
    assert_eq!(h.store.agents()[0].status, "completed");
}

/// A detached child must not make the session look busy **while its run lives**.
///
/// `has_active_run` is top_level-only, so a session with a background child keeps
/// accepting prompts: a new run is registered next to the child's loop and the
/// child is reaped by its own run's end, not by the new one. That is the property
/// the flag exists for, and it is asserted here with the first run parked so the
/// child is genuinely live.
///
/// The earlier version of this test also waited for the *first* run to finish
/// before asserting, and then asserted the child was still live. That half
/// contradicted `subagent.rs`/`subagents.md` ("a background agent outlives the
/// turn that started it but not the run"): the run ending is exactly when the
/// child is reaped. The corrected behaviour is covered by
/// [`a_run_ending_cancels_its_background_children`].
#[tokio::test]
async fn a_detached_child_does_not_make_the_session_look_busy() {
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "a long job", "name": "worker", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    h.factory.key(
        "a long job",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("child finished"),
        ],
    );

    h.submit("start the long job").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // The child is live and the run is still in flight: exactly one *run* is
    // registered (the parent's), and the child is a loop of that run.
    assert_eq!(h.runtime.live_agent_count(h.session()), 1);
    assert!(
        h.runtime.is_running(h.session()),
        "the parent run is still working"
    );
    assert!(
        h.runtime.has_active_run(h.session()),
        "the parent run is the one the user typed into"
    );

    // The load-bearing half: `has_active_run` counts the user *run*, and the child's
    // own loop is registered beside it as an agent loop (`agent_id: Some`) rather
    // than as a second run. That is why a detached child never inflates the count.
    let active = h.runtime.active_runs();
    let runs_for_session: Vec<_> = active
        .iter()
        .filter(|run| run.session_id == h.session())
        .collect();
    assert_eq!(
        runs_for_session
            .iter()
            .filter(|run| run.agent_id.is_none())
            .count(),
        1,
        "exactly one user run is registered: {runs_for_session:?}"
    );
    assert_eq!(
        runs_for_session
            .iter()
            .filter(|run| run.agent_id.is_some())
            .count(),
        1,
        "the child's own loop is registered as an agent, not as a run: {runs_for_session:?}"
    );

    handle.release();
    parent_handle.release();
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "completed");
    let _ = h.wait_idle().await;
    // A new prompt is accepted as soon as the run is over, which is the
    // behaviour the flag exists for. No second script is queued: the submission
    // succeeding is the assertion.
    let started = h
        .runtime
        .submit(h.session(), "carry on please".to_string())
        .await;
    assert!(
        started.is_ok(),
        "a new run was refused after the previous one ended: {:?}",
        started.err()
    );
    h.runtime.cancel(h.session());
}

/// The run's end reaps the background children it started.
///
/// Documented behaviour (`subagent.rs` module doc: "A background agent outlives
/// the turn that started it but not the run: when the run ends its children are
/// cancelled", repeated in `docs/internal/subagents.md`). It was not implemented:
/// the run's cleanup tore down nothing, and the child's token was
/// `ctx.cancel.child_token()` - so dropping it **detached** rather than cancelled,
/// and a `background: true` agent kept working (up to
/// `subagents.timeout_secs`, four hours) after the run that spawned it reported
/// completed.
#[tokio::test]
async fn a_run_ending_cancels_its_background_children() {
    // No `allow_background: false` escape hatch: this is the shipped default
    // configuration for a background spawn.
    let (gate, handle) = GateTool::pair("gate");
    let (parent_gate, parent_handle) = GateTool::pair("parent_gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({
            "task": "an endless job", "name": "worker", "background": true
        })),
        Script::call(&[("c_parent", "parent_gate", "{}")]),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    .tool(Arc::new(parent_gate))
    .settings(FakeSettings::default().with_subagents(fan_out_settings()))
    .build()
    .await;
    // The child parks in its gate: only cancellation can end it, so "did the run
    // reap it" is unambiguous. The parent parks in its own gate so the child
    // genuinely starts before the run is allowed to end.
    h.factory.key(
        "an endless job",
        vec![Script::call(&[("c1", "gate", "{}")])],
    );

    h.submit("delegate and move on").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 || parent_handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The child is detached and working while the run is alive - the documented
    // and intended use.
    assert_eq!(h.runtime.live_agent_count(h.session()), 1);
    assert_eq!(h.store.agents()[0].status, "running");

    // Let the run finish. Everything it started must be reaped with it.
    parent_handle.release();
    let events = h.collect_until_finished().await;
    assert_eq!(finished_status(&events), "completed");

    // The child must not be live once the run is over...
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if h.runtime.live_agent_count(h.session()) == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the background child outlived the run that spawned it ({:?})",
            h.store
                .agents()
                .iter()
                .map(|a| a.status.clone())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // ...and its status must say *why*: cancelled (it honoured the token), never
    // left `running`.
    let agent = &h.store.agents()[0];
    assert_ne!(
        agent.status, "running",
        "the reaped child is still recorded as running"
    );
    assert!(
        agent.status == "cancelled" || agent.status == "interrupted",
        "expected cancelled or interrupted, got {}",
        agent.status
    );
    // Nothing is left writing for the session.
    assert!(
        !h.runtime.is_running(h.session()),
        "a child loop outlived its run"
    );
    // Exactly one model call for the child: it was stopped, not allowed to run a
    // second turn. Keyed on the child's first message, which is always its task.
    let child_requests = h
        .factory
        .requests()
        .into_iter()
        .filter(|request| {
            request
                .messages
                .iter()
                .skip(1)
                .any(|m| m.role == Role::User && m.content == "an endless job")
        })
        .count();
    assert_eq!(
        child_requests, 1,
        "the child kept working after its run ended"
    );
}

#[tokio::test]
async fn a_child_keeps_the_coordination_tools_but_not_the_spawning_ones() {
    // The split matters: a child that cannot report status is invisible to the
    // parent, and one that cannot message cannot say it is blocked - but a child
    // that can spawn with recursion off is exactly the unbounded fan-out the
    // switch exists to prevent.
    use crate::subagent::{COORDINATION_TOOLS, DELEGATION_ONLY_TOOLS};
    assert!(
        COORDINATION_TOOLS.contains(&crate::subagent::REPORT_STATUS_TOOL),
        "report_status has to be available to every child"
    );
    assert!(COORDINATION_TOOLS.contains(&crate::subagent::SEND_MESSAGE_TOOL));
    assert!(
        DELEGATION_ONLY_TOOLS.contains(&SPAWN_AGENT_TOOL),
        "spawning is what recursion gates"
    );

    let (gate, handle) = GateTool::pair("gate");
    let mut h = HarnessBuilder::new(vec![
        spawn_args(serde_json::json!({ "task": "look at this", "name": "child" })),
        Script::text("parent done"),
    ])
    .tool(Arc::new(gate))
    // Recursion off: the default, and the case that was silently stripping the
    // coordination tools too.
    .settings(FakeSettings::default().with_subagents(SubagentConfig {
        allow_recursive: false,
        ..fan_out_settings()
    }))
    .build()
    .await;
    h.factory.key(
        "look at this",
        vec![
            Script::call(&[("c1", "gate", "{}")]),
            Script::text("child finished"),
        ],
    );

    h.submit("delegate").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.arrived() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // The child's own request lists exactly which delegate tools it was given.
    let child_request = h
        .factory
        .requests()
        .into_iter()
        .find(|request| {
            request.messages[0]
                .content
                .starts_with("You are a focused subagent")
        })
        .expect("the child's request");
    let offered: Vec<&str> = child_request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    for name in COORDINATION_TOOLS {
        assert!(
            offered.contains(name),
            "a child must keep `{name}`: {offered:?}"
        );
    }
    for name in DELEGATION_ONLY_TOOLS {
        assert!(
            !offered.contains(name),
            "recursion is off, so a child must not get `{name}`: {offered:?}"
        );
    }

    handle.release();
    let _ = h.collect_until_finished().await;
}
