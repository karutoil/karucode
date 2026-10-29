//! Behavioural tests for the agent loop, approvals, cancellation, context
//! budgeting and subagents.
//!
//! Every await that waits for the runtime is wrapped in `tokio::time::timeout`
//! (inside `Harness::next_event` / `wait_idle`) so a regression that hangs the
//! loop fails the test instead of blocking CI.

use crate::approvals::{ApprovalRegistry, RuntimeApprovalGate};
use crate::subagent::SPAWN_AGENT_TOOL;
use crate::tests_support::{
    test_inner_with_settings, ApprovalTool, EchoTool, ExternalTool, FakeSettings, Harness,
    HarnessBuilder, PanicTool, Script, ScriptedFactory, SlowCancelTool,
};
use crate::{ActiveRun, RecoveryReport, Runtime, RuntimeDeps};
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView, ApprovalRequest};
use kc_core::config::{ApprovalMode, ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
use kc_core::error::Error;
use kc_core::events::{Event, EventEnvelope};
use kc_core::hooks::NoHooks;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::message::{Role, ToolCall};
use kc_core::paths::Workspace;
use kc_core::provider::ProviderChunk;
use kc_core::store::{AgentRecord, ApprovalRecord, RunStatus, Store};
use kc_core::tool::{ApprovalGate, ToolClass, ToolOrigin, ToolOutcome, ToolRegistry};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::SessionCreated { .. } => "session_created",
        Event::SessionRenamed { .. } => "session_renamed",
        Event::SessionModelChanged { .. } => "session_model_changed",
        Event::SessionDeleted { .. } => "session_deleted",
        Event::RunStarted { .. } => "run_started",
        Event::RunFinished { .. } => "run_finished",
        Event::RunCancelled { .. } => "run_cancelled",
        Event::IterationStarted { .. } => "iteration_started",
        Event::StreamRetry { .. } => "stream_retry",
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
    assert_eq!(
        roles,
        vec!["user", "assistant", "tool", "tool", "assistant"]
    );
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
    let limits = Limits {
        max_iterations: 2,
        // One-shot budget: the pre-long-horizon behaviour, kept as an option.
        auto_continue: false,
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
async fn iteration_budget_is_a_checkpoint_not_a_finish_line() {
    // Three segments of two turns: the run must survive two checkpoints and
    // finish on the model's own terms instead of dying at the first budget.
    let limits = Limits {
        max_iterations: 2,
        max_continues: 2,
        auto_continue: true,
        ..Default::default()
    };
    let mut h = HarnessBuilder::new(vec![
        Script::call(&[("call_1", "echo", "{}")]),
        Script::call(&[("call_2", "echo", "{}")]),
        Script::call(&[("call_3", "echo", "{}")]),
        Script::call(&[("call_4", "echo", "{}")]),
        Script::call(&[("call_5", "echo", "{}")]),
        Script::text("finished the long job"),
    ])
    .tool(EchoTool::boxed("echo"))
    .settings(FakeSettings::default().with_limits(limits))
    .build()
    .await;

    let events = h
        .run_to_finish("a job that needs more than one budget")
        .await;
    assert_eq!(finished_status(&events), "completed");
    assert_eq!(count(&events, "iteration_started"), 6);
    assert_eq!(h.factory.requests().len(), 6, "every script was used");
    let checkpoints = events
        .iter()
        .filter(|e| {
            matches!(&e.event, Event::Notice { message, .. }
                if message.contains("continuing automatically"))
        })
        .count();
    assert_eq!(checkpoints, 2, "one notice per refreshed segment");

    // The checkpoint is persisted, so the next run's transcript explains the
    // jump instead of hiding it.
    let messages = h.session_messages().await;
    let nudges = messages
        .iter()
        .filter(|m| m.role == "user" && m.content.contains("[runtime checkpoint]"))
        .count();
    assert_eq!(nudges, 2);
    assert_eq!(h.store.runs()[0].status, "completed");
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
    // A request-level error chunk (context too long, unsupported model) cannot
    // be fixed by asking again, so the restart budget must stay untouched.
    let mut h = HarnessBuilder::new(vec![Script::Chunks(vec![ProviderChunk::Error {
        message: "provider error: this model's maximum context length is 4096 tokens".into(),
        retryable: false,
    }])])
    .model(restarts(3))
    .build()
    .await;

    let events = h.run_to_finish("too much context").await;
    assert_eq!(finished_status(&events), "failed");
    assert_eq!(count(&events, "stream_retry"), 0);
    assert_eq!(h.factory.requests().len(), 1, "no second request");
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
        .find(|m| m.content.starts_with("[compacted earlier conversation:"))
        .expect("compaction notice in the request");
    assert_eq!(trim_notice.role, Role::Assistant);
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
    // Only the system prompt, the digest and the last user turn survive: the
    // budget is smaller than everything else that was loaded.
    assert_eq!(sent.len(), 3, "unexpected survivors: {sent:?}");
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
async fn a_subagent_outlives_its_first_segment_instead_of_dying_at_the_budget() {
    // The reported bug: a delegated coding task burned a batch of tool calls per
    // turn, hit `subagents.max_iterations` in a couple of minutes and came back
    // as `failed` while the work was still in flight. A segment is a checkpoint
    // for a subagent exactly as it is for a run.
    let subagents = SubagentConfig {
        max_concurrent: 1,
        max_iterations: 2,
        max_continues: 2,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 60,
    };
    let mut h = HarnessBuilder::new(vec![
        spawn_call("a long job", None, false),
        // Three segments of two turns, then the child's own final answer.
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
        "the child must survive its checkpoints: {:?}",
        agent.error
    );
    assert_eq!(agent.iterations, 6, "one iteration per model round-trip");
    assert!(agent.summary.contains("child finished the long job"));
    assert!(agent.error.is_none(), "{:?}", agent.error);

    // Every checkpoint is persisted on the child transcript, so the transcript
    // explains the jump instead of hiding it.
    let child_messages = h.store.messages_for_agent(&agent.id);
    let checkpoints = child_messages
        .iter()
        .filter(|m| m.role == "user" && m.content.contains("[runtime checkpoint]"))
        .count();
    assert_eq!(checkpoints, 2, "one per refreshed segment");

    // The child was told its own budget, not the run's.
    let child_request = h
        .factory
        .requests()
        .into_iter()
        .find(|r| r.messages[0].content.contains("You are a focused subagent"))
        .expect("a child request");
    assert!(
        child_request.messages[0]
            .content
            .contains("2 model turns per segment"),
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
        max_continues: 0,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 60,
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
    assert!(error.contains("settings.subagents.max_iterations"), "{error}");
    assert!(error.contains("max_continues"), "{error}");
    assert_eq!(h.factory.requests().len(), 4, "the child stopped at two turns");
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
        max_continues: 0,
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
        max_continues: 0,
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
        max_continues: 0,
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
        max_continues: 0,
        allow_recursive: false,
        max_depth: 1,
        timeout_secs: 1,
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
    assert_eq!(
        tools.names(),
        vec!["echo".to_string(), SPAWN_AGENT_TOOL.to_string()]
    );
    assert!(tools.get(SPAWN_AGENT_TOOL).is_some());
    assert_eq!(Runtime::builtin_tool_names(), &[SPAWN_AGENT_TOOL]);

    let mut replacement = ToolRegistry::new();
    replacement.register(EchoTool::boxed("other"));
    h.runtime.set_tools(Arc::new(replacement));
    let tools = h.runtime.tools();
    assert_eq!(
        tools.names(),
        vec!["other".to_string(), SPAWN_AGENT_TOOL.to_string()]
    );
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
    assert_eq!(h.runtime.events_emitted() as usize, events.len());
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
