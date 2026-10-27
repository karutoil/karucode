//! Behavioural tests for the agent loop, approvals, cancellation, context
//! budgeting and subagents.
//!
//! Every await that waits for the runtime is wrapped in `tokio::time::timeout`
//! (inside `Harness::next_event` / `wait_idle`) so a regression that hangs the
//! loop fails the test instead of blocking CI.

use crate::approvals::ApprovalRegistry;
use crate::subagent::SPAWN_AGENT_TOOL;
use crate::tests_support::{
    ApprovalTool, EchoTool, FakeSettings, Harness, HarnessBuilder, Script, ScriptedFactory,
};
use crate::{ActiveRun, Runtime, RuntimeDeps, RecoveryReport};
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView, ApprovalRequest};
use kc_core::config::{ApprovalMode, ApprovalPolicy, Limits, SubagentConfig};
use kc_core::error::Error;
use kc_core::events::{Event, EventEnvelope};
use kc_core::hooks::NoHooks;
use kc_core::message::Role;
use kc_core::paths::Workspace;
use kc_core::provider::ProviderChunk;
use kc_core::store::{AgentRecord, ApprovalRecord, RunStatus, Store};
use kc_core::tool::{ToolOutcome, ToolRegistry};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::SessionCreated { .. } => "session_created",
        Event::SessionRenamed { .. } => "session_renamed",
        Event::SessionDeleted { .. } => "session_deleted",
        Event::RunStarted { .. } => "run_started",
        Event::RunFinished { .. } => "run_finished",
        Event::RunCancelled { .. } => "run_cancelled",
        Event::IterationStarted { .. } => "iteration_started",
        Event::TextDelta { .. } => "text_delta",
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
        Event::Notice { .. } => "notice",
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
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "seq not increasing: {seqs:?}");

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
    assert!(second.iter().any(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()));
    assert!(second.iter().any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("call_1")));
    assert_eq!(requests[1].model, "fake-model");
}

#[tokio::test]
async fn multiple_tool_calls_run_sequentially_and_in_order() {
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
    assert_eq!(roles, vec!["user", "assistant", "tool", "tool", "assistant"]);
    assert_eq!(messages[2].content, "echoed:{\"n\":1}");
    assert_eq!(messages[3].content, "echoed:{\"n\":2}");
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
    let tool_message = messages.iter().find(|m| m.role == "tool").expect("tool message");
    assert!(tool_message.content.contains("unknown tool `does_not_exist`"));
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

#[tokio::test]
async fn large_tool_output_is_truncated_to_the_configured_budget() {
    let limits = Limits {
        max_tool_output_bytes: 200,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", &format!("{{\"blob\":\"{}\"}}", "x".repeat(4000)))]),
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
    assert!(tool_message.content.len() < 600, "tool content: {} bytes", tool_message.content.len());
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
            Event::Notice { message, .. } if message.contains("skipped 2 tool call") => Some(message.clone()),
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
    let messages = h.session_messages().await;
    let tool_message = messages.iter().find(|m| m.role == "tool").unwrap();
    assert!(tool_message.content.contains("not allowed:expired"));
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
    let h = HarnessBuilder::new(vec![Script::text("unused")]).build().await;
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
    .settings(
        FakeSettings::default().with_policy(ApprovalPolicy {
            mode: ApprovalMode::Auto,
            ..Default::default()
        }),
    )
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
    .settings(
        FakeSettings::default().with_policy(ApprovalPolicy {
            mode: ApprovalMode::ReadOnly,
            ..Default::default()
        }),
    )
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
// cancellation, limits, provider failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_mid_stream_marks_the_run_cancelled() {
    let mut h = HarnessBuilder::new(vec![Script::Hang(vec![
        ProviderChunk::Text("partial answer".into()),
    ])])
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
    let assistant = messages.iter().find(|m| m.role == "assistant").expect("partial text persisted");
    assert_eq!(assistant.content, "partial answer");
    assert!(h.wait_idle().await.is_empty());
    assert!(!h.runtime.is_running(h.session()));
    assert!(!h.runtime.cancel("s_unknown_session"));
}

#[tokio::test]
async fn iteration_limit_stops_the_run_with_limit_reached() {
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
    let notice = events.iter().any(|e| matches!(&e.event, Event::Notice { message, .. } if message.contains("max_iterations")));
    assert!(notice, "expected a notice about the iteration limit: {:?}", kinds(&events));
    assert_eq!(h.factory.requests().len(), 2, "the third script must not be consumed");
    assert_eq!(h.store.runs()[0].status, "limit_reached");
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
    assert!(runs[0].error.as_deref().unwrap_or_default().contains("connection refused"));

    let finished = events.iter().rev().find_map(|e| match &e.event {
        Event::RunFinished { error, .. } => Some(error.clone()),
        _ => None,
    });
    assert!(finished.flatten().is_some(), "run_finished must carry the error");
    assert!(h.wait_idle().await.is_empty());
}

#[tokio::test]
async fn provider_error_chunk_keeps_the_partial_assistant_text() {
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![
        ProviderChunk::Text("half an answer".into()),
        ProviderChunk::Error {
            message: "stream broke mid-sentence".into(),
            retryable: true,
        },
    ])])
    .build()
    .await;

    let events = h.run_to_finish("fail partially").await;
    assert_eq!(finished_status(&events), "failed");
    assert!(count(&events, "error") == 1);

    let messages = h.session_messages().await;
    let assistant = messages.iter().find(|m| m.role == "assistant").expect("partial text");
    assert_eq!(assistant.content, "half an answer");
    assert!(assistant.tool_calls.is_empty());
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
    assert_eq!(finished_status(&h.collect_until_finished().await), "cancelled");

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
    let h = HarnessBuilder::new(vec![Script::text("unused")]).build().await;
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
    let mut h = HarnessBuilder::new(vec![Script::text("never used")]).build().await;
    h.store.fail_appends(true);
    let err = h
        .runtime
        .submit(h.session(), "hello".to_string())
        .await
        .expect_err("submit must report the storage failure");
    assert!(matches!(err, Error::Store(_)), "got {err:?}");
    h.store.fail_appends(false);

    assert!(h.runtime.active_runs().is_empty(), "no half-started run may stay active");
    let runs = h.store.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
    assert!(runs[0].error.as_deref().unwrap_or_default().contains("disk is full"));

    // The session is usable again afterwards.
    let events = h.run_to_finish("try again").await;
    assert_eq!(finished_status(&events), "completed");
}

// ---------------------------------------------------------------------------
// context budget
// ---------------------------------------------------------------------------

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
        let mut user = crate::tests_support::new_message(h.session(), "user", &format!("old question {i} {}", "x".repeat(120)));
        user.run_id = Some("r_old".into());
        h.store.append_message(&user).await.unwrap();
        let mut assistant = crate::tests_support::new_message(h.session(), "assistant", &"y".repeat(120));
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
        .find(|m| m.content.starts_with("[earlier conversation trimmed:"))
        .expect("trim notice in the request");
    assert_eq!(trim_notice.role, Role::Assistant);
    assert!(
        trim_notice.content.contains("24 messages"),
        "notice should count dropped messages: {}",
        trim_notice.content
    );
    assert_eq!(sent.last().unwrap().role, Role::User);
    assert_eq!(sent.last().unwrap().content, "the current question");
    // The system prompt is bigger than the whole budget, so everything that may
    // be dropped was dropped: only the system prompt, the notice and the last
    // user turn survive.
    assert_eq!(sent.len(), 3, "unexpected survivors: {sent:?}");
    assert!(!sent.iter().any(|m| m.content.contains("old question")));
    let body: usize = sent
        .iter()
        .skip(1)
        .map(|m| m.content.chars().count())
        .sum();
    assert!(body <= 600, "droppable content over budget: {body}");
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
        Event::AgentSpawned { agent_id, name, role, model, .. } => {
            Some((agent_id.clone(), name.clone(), role.clone(), model.clone()))
        }
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
    assert!(agents[0].error.as_deref().unwrap_or_default().contains("subagent model exploded"));

    assert_eq!(count(&events, "agent_finished"), 1);
    let failed_tool = tool_finished(&events);
    assert_eq!(failed_tool.len(), 1);
    assert!(!failed_tool[0].1, "a failed subagent reports ok=false");

    let messages = h.session_messages().await;
    assert_eq!(messages.last().unwrap().content, "parent carried on");
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
        requests[1].tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
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
        spawn_call("first task", Some(&["src/shared.rs"]), false),
        spawn_call("second task", Some(&["src/shared.rs"]), false),
        Script::text("first summary"),
        Script::text("second summary"),
        Script::text("parent done"),
    ])
    .build()
    .await;

    // Pre-existing claim owned by somebody else.
    h.store
        .claim_paths(h.session(), "a_other", &["src/shared.rs".to_string()], "write")
        .await
        .unwrap();

    let events = h.run_to_finish("two agents").await;
    assert_eq!(finished_status(&events), "completed");
    let warnings: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolCallFinished { output, .. } => Some(output.clone()),
            _ => None,
        })
        .filter(|output| output.contains("warnings:"))
        .collect();
    assert!(!warnings.is_empty(), "expected claim warnings: {:?}", kinds(&events));
    assert!(warnings[0].contains("a_other"));
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
    assert_eq!(h.session_messages().await.last().unwrap().content, "parent recovered");
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
    assert_eq!(records[0].agent_id.as_deref(), Some(child_agent_id.as_str()));
    assert_eq!(records[0].status, "resolved");
    assert_eq!(h.store.agents()[0].status, "completed");
}

// ---------------------------------------------------------------------------
// recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recover_marks_interrupted_work_and_expires_approvals() {
    let h = HarnessBuilder::new(vec![Script::text("unused")]).build().await;

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
    assert_eq!(tools.names(), vec!["echo".to_string(), SPAWN_AGENT_TOOL.to_string()]);
    assert!(tools.get(SPAWN_AGENT_TOOL).is_some());
    assert_eq!(Runtime::builtin_tool_names(), &[SPAWN_AGENT_TOOL]);

    let mut replacement = ToolRegistry::new();
    replacement.register(EchoTool::boxed("other"));
    h.runtime.set_tools(Arc::new(replacement));
    let tools = h.runtime.tools();
    assert_eq!(tools.names(), vec!["other".to_string(), SPAWN_AGENT_TOOL.to_string()]);
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
        workspace,
    });
    assert_eq!(runtime.active_runs().len(), 0);
    let mut registry = ToolRegistry::new();
    crate::subagent::install_builtins(&mut registry, &runtime);
    assert!(registry.get(SPAWN_AGENT_TOOL).is_some());
    assert_eq!(crate::agent::default_approval_kind(kc_core::tool::ToolClass::Shell), ApprovalKind::ShellCommand);
    let _ = ToolOutcome::ok("x");
    let _ = RunStatus::Completed;
}