//! The agent loop.
//!
//! One iteration = one model round-trip plus the tool calls it asked for. The
//! loop is shared by top-level runs ([`start_run`]) and subagent loops
//! ([`run_loop`] driven from `subagent`), so cancellation, approvals, context
//! budgeting and event emission behave identically for both.
//!
//! Termination is always explicit: no tool calls (`completed`), cancellation
//! (`cancelled`), iteration or wall-clock limit (`limit_reached`), or a provider
//! failure (`failed`). Every path persists what actually happened and emits a
//! visible event.

use crate::approvals::{ApprovalScope, RuntimeApprovalGate};
use crate::context;
use crate::events::EmitCtx;
use crate::{RunHandle, RunState, Runtime, RuntimeInner, HISTORY_LIMIT};
use futures_util::{FutureExt, StreamExt};
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView};
use kc_core::config::{ApprovalMode, Limits, ModelConfig};
use kc_core::error::{Error, Result};
use kc_core::events::Event;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::message::{Message, Role, ToolCall};
use kc_core::provider::{ChatRequest, ProviderChunk, ProviderStream};
use kc_core::store::{NewMessage, RunStatus};
use kc_core::tool::{
    truncate_output, ApprovalGate, ApprovalHandle, Tool, ToolCtx, ToolOrigin, ToolOutcome,
    ToolRegistry,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Name of the runtime's built-in delegate tool.
pub(crate) const SKIPPED_PREFIX: &str = "[skipped: ";

/// Everything one loop needs. Built by [`start_run`] for a session run and by
/// the `spawn_agent` tool for a nested agent.
pub(crate) struct LoopSpec {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub agent_id: Option<AgentId>,
    pub depth: u8,
    pub is_subagent: bool,
    pub max_iterations: u32,
    /// Wall-clock budget that produced `deadline`, used for operator-facing
    /// messages only.
    pub time_budget_secs: u64,
    pub deadline: Instant,
    pub model: ModelConfig,
    pub limits: Limits,
    pub approval_mode: String,
    pub gate: Arc<dyn ApprovalGate>,
    /// Approval ids this loop scope created; expired at loop end/cancel without
    /// touching a concurrent sibling scope that shares the run id.
    pub approval_scope: ApprovalScope,
    pub tools: Arc<ToolRegistry>,
    pub cancel: CancellationToken,
    /// Conversation without the system message; the system prompt is rebuilt
    /// each iteration from the current tool set and hook contributions.
    pub history: Vec<Message>,
    pub run_state: Option<Arc<RunState>>,
}

impl LoopSpec {
    pub(crate) fn emit_ctx(&self, inner: &Arc<RuntimeInner>) -> EmitCtx {
        EmitCtx::new(
            Arc::clone(inner),
            self.session_id.0.clone(),
            self.run_id.0.clone(),
            self.agent_id.as_ref().map(|a| a.0.clone()),
        )
    }
}

pub(crate) struct LoopOutcome {
    pub status: RunStatus,
    pub error: Option<String>,
    /// Last assistant text produced (used as a subagent summary).
    pub text: String,
    pub iterations: u32,
}

impl LoopOutcome {
    fn new(status: RunStatus, error: Option<String>, text: String, iterations: u32) -> Self {
        Self {
            status,
            error,
            text,
            iterations,
        }
    }
}

enum StreamEnd {
    Complete,
    Cancelled,
    TimedOut,
    Failed(String),
}

/// Start a user-initiated run.
///
/// Validates the session, rejects a second concurrent run for the same session,
/// writes the `runs` row and the user message, then hands the loop to a
/// background task. `RunStarted` is emitted by the task, after the caller has
/// had a chance to subscribe.
pub(crate) async fn start_run(
    runtime: &Arc<Runtime>,
    session_id: &str,
    prompt: String,
) -> Result<RunHandle> {
    if prompt.trim().is_empty() {
        return Err(Error::BadRequest("message content may not be empty".into()));
    }
    let inner = Arc::clone(runtime.shared());
    let store = inner.deps.store.clone();

    store
        .get_session(session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

    let model = inner.deps.settings.model().await;
    let run_id = RunId::new();
    let state = Arc::new(RunState::new(
        run_id.clone(),
        SessionId::from_string(session_id),
        None,
        CancellationToken::new(),
        true,
    ));
    let key = state.key();

    {
        let mut runs = inner.runs.lock().expect("runs lock");
        if runs
            .values()
            .any(|s| s.top_level && s.session_id.0 == session_id)
        {
            return Err(Error::BadRequest(format!(
                "a run is already active for session {session_id}"
            )));
        }
        runs.insert(key.clone(), Arc::clone(&state));
    }

    let history = match begin_run(&inner, &state, &model, &prompt).await {
        Ok(history) => history,
        Err(e) => {
            // The `runs` row may already exist. Mark it failed instead of
            // leaving a permanently "running" run behind.
            if let Err(cleanup) = store
                .finish_run(&state.run_id.0, RunStatus::Failed, Some(&e.to_string()))
                .await
            {
                tracing::warn!(
                    run = %state.run_id.0,
                    error = %cleanup,
                    "could not mark the aborted run as failed"
                );
            }
            inner.runs.lock().expect("runs lock").remove(&key);
            return Err(e);
        }
    };

    let spawned_inner = Arc::clone(&inner);
    let spawned_state = Arc::clone(&state);
    let spawned_key = key.clone();
    tokio::spawn(async move {
        // A panic in a tool or provider must not leak the run: catch it here so
        // the row becomes `failed` and the session accepts work again.
        let outcome = std::panic::AssertUnwindSafe(run_top_level(
            Arc::clone(&spawned_inner),
            Arc::clone(&spawned_state),
            prompt,
            history,
            model,
        ))
        .catch_unwind()
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // `run_top_level` persists and emits every failure path; this is
                // the last-resort report for an error thrown by the teardown.
                tracing::error!(run = %spawned_state.run_id.0, error = %e, "run teardown failed");
                spawned_inner.emit(
                    Some(&spawned_state.session_id.0),
                    Some(&spawned_state.run_id.0),
                    None,
                    Event::Error {
                        message: e.to_string(),
                        kind: Some(e.code().into()),
                    },
                );
            }
            Err(payload) => {
                let message = format!("the run panicked: {}", panic_message(payload.as_ref()));
                tracing::error!(run = %spawned_state.run_id.0, message = %message, "run panicked");
                if let Err(e) = spawned_inner
                    .deps
                    .store
                    .finish_run(&spawned_state.run_id.0, RunStatus::Failed, Some(&message))
                    .await
                {
                    tracing::warn!(run = %spawned_state.run_id.0, error = %e, "could not record the panicked run");
                }
                spawned_inner.emit(
                    Some(&spawned_state.session_id.0),
                    Some(&spawned_state.run_id.0),
                    None,
                    Event::Error {
                        message: message.clone(),
                        kind: Some("internal".into()),
                    },
                );
                spawned_inner.emit(
                    Some(&spawned_state.session_id.0),
                    Some(&spawned_state.run_id.0),
                    None,
                    Event::RunFinished {
                        run_id: spawned_state.run_id.0.clone(),
                        status: RunStatus::Failed.as_str().to_string(),
                        error: Some(message),
                    },
                );
            }
        }
        spawned_inner
            .runs
            .lock()
            .expect("runs lock")
            .remove(&spawned_key);
    });

    Ok(RunHandle {
        run_id: run_id.0,
        session_id: session_id.to_string(),
    })
}

/// Write the run row + user message and load the prior conversation.
async fn begin_run(
    inner: &Arc<RuntimeInner>,
    state: &Arc<RunState>,
    model: &ModelConfig,
    prompt: &str,
) -> Result<Vec<Message>> {
    let store = &inner.deps.store;
    store
        .create_run(&state.run_id.0, &state.session_id.0, &model.model, prompt)
        .await?;

    let user = Message::user(prompt);
    persist(inner, &state.session_id, &state.run_id, None, &user).await?;
    store.touch_session(&state.session_id.0).await?;

    let stored = store
        .list_messages(&state.session_id.0, HISTORY_LIMIT)
        .await?;
    let mut history: Vec<Message> = stored
        .into_iter()
        // Subagent transcripts share the session but must never leak into the
        // parent conversation; stored system prompts are rebuilt per iteration.
        .filter(|m| m.agent_id.is_none() && m.role != Role::System.as_str())
        .map(|m| m.to_message())
        .collect();
    if history.is_empty() {
        history.push(user);
    }
    Ok(history)
}

/// Run the loop for a session run and persist/emit its terminal state.
pub(crate) async fn run_top_level(
    inner: Arc<RuntimeInner>,
    state: Arc<RunState>,
    prompt: String,
    history: Vec<Message>,
    model: ModelConfig,
) -> Result<()> {
    let settings = inner.deps.settings.clone();
    let limits = settings.limits().await;
    let policy = settings.approval_policy().await;
    let tools = inner.tool_view();
    let ctx = EmitCtx::new(
        Arc::clone(&inner),
        state.session_id.0.clone(),
        state.run_id.0.clone(),
        None,
    );

    ctx.emit(Event::RunStarted {
        run_id: state.run_id.0.clone(),
        prompt,
    });

    let gate = RuntimeApprovalGate::new(
        Arc::clone(&inner),
        state.session_id.clone(),
        state.run_id.clone(),
        None,
        state.cancel.clone(),
        0,
    );
    let approval_scope = gate.scope();
    let gate: Arc<dyn ApprovalGate> = Arc::new(gate);

    let spec = LoopSpec {
        session_id: state.session_id.clone(),
        run_id: state.run_id.clone(),
        agent_id: None,
        depth: 0,
        is_subagent: false,
        max_iterations: limits.max_iterations.max(1),
        time_budget_secs: limits.max_run_secs.max(1),
        deadline: Instant::now() + Duration::from_secs(limits.max_run_secs.max(1)),
        model,
        limits,
        approval_mode: policy.mode.as_str().to_string(),
        gate,
        approval_scope,
        tools,
        cancel: state.cancel.clone(),
        history,
        run_state: Some(Arc::clone(&state)),
    };

    let outcome = run_loop(Arc::clone(&inner), spec).await;

    let mut finish_error: Option<Error> = None;
    if let Err(e) = inner
        .deps
        .store
        .finish_run(&state.run_id.0, outcome.status, outcome.error.as_deref())
        .await
    {
        ctx.error(
            "store",
            format!("could not record the final run status: {e}"),
        );
        finish_error = Some(e);
    }
    inner
        .deps
        .hooks
        .on_run_end(
            &state.session_id.0,
            &state.run_id.0,
            outcome.status.as_str(),
        )
        .await;

    if outcome.status == RunStatus::Cancelled {
        ctx.emit(Event::RunCancelled {
            run_id: state.run_id.0.clone(),
        });
    }
    ctx.emit(Event::RunFinished {
        run_id: state.run_id.0.clone(),
        status: outcome.status.as_str().to_string(),
        error: outcome.error.clone(),
    });

    match finish_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// The shared agent loop. Never panics on model or tool misbehaviour: every
/// failure becomes a status plus a visible event.
pub(crate) async fn run_loop(inner: Arc<RuntimeInner>, spec: LoopSpec) -> LoopOutcome {
    let scope = spec.approval_scope.clone();
    let session_id = spec.session_id.0.clone();
    let run_id = spec.run_id.0.clone();
    let run_wide = spec.agent_id.is_none();

    let outcome = run_loop_inner(Arc::clone(&inner), spec).await;

    // Whatever ended the loop, no approval it requested may stay pending: the
    // waiter is gone, and a stale card would block later runs. Because the scope
    // is per agent, this never expires a sibling subagent's request.
    let reason = if outcome.status == RunStatus::Cancelled {
        "run cancelled"
    } else {
        "run finished"
    };
    scope
        .expire_pending(&inner, &session_id, &run_id, run_wide, reason)
        .await;

    outcome
}

async fn run_loop_inner(inner: Arc<RuntimeInner>, mut spec: LoopSpec) -> LoopOutcome {
    let ctx = spec.emit_ctx(&inner);
    let mut history = std::mem::take(&mut spec.history);
    let mut iterations = 0u32;
    let mut last_text = String::new();

    loop {
        if spec.cancel.is_cancelled() {
            ctx.notice("info", "run cancelled");
            return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
        }
        if iterations >= spec.max_iterations {
            let message = if spec.is_subagent {
                format!("subagent stopped after {iterations} iterations (settings.subagents.max_iterations)")
            } else {
                format!("stopped after {iterations} iterations (limits.max_iterations)")
            };
            ctx.notice("warn", message.clone());
            let error = if spec.is_subagent {
                Some(message)
            } else {
                None
            };
            return LoopOutcome::new(RunStatus::LimitReached, error, last_text, iterations);
        }
        if Instant::now() >= spec.deadline {
            let message = if spec.is_subagent {
                format!(
                    "subagent timed out after {}s (settings.subagents.timeout_secs)",
                    spec.time_budget_secs
                )
            } else {
                format!(
                    "stopped: the run exceeded the {}s wall-clock limit (limits.max_run_secs)",
                    spec.time_budget_secs
                )
            };
            ctx.notice("warn", message.clone());
            let error = if spec.is_subagent {
                Some(message)
            } else {
                None
            };
            return LoopOutcome::new(RunStatus::LimitReached, error, last_text, iterations);
        }

        iterations += 1;
        if let Some(state) = &spec.run_state {
            state.iterations.store(iterations, Ordering::Relaxed);
        }
        ctx.emit(Event::IterationStarted {
            iteration: iterations,
        });
        if spec.is_subagent {
            if let Some(agent_id) = &spec.agent_id {
                ctx.emit(Event::AgentProgress {
                    agent_id: agent_id.0.clone(),
                    iterations,
                    note: format!("iteration {iterations}"),
                });
            }
        }

        // --- build the request ------------------------------------------------
        let extra = inner
            .deps
            .hooks
            .before_prompt(
                &spec.session_id.0,
                &inner.deps.workspace.root().to_string_lossy(),
            )
            .await;
        let tool_specs = spec.tools.specs();
        let system = kc_core::prompt::system_prompt(
            &inner.deps.workspace.root().to_string_lossy(),
            &tool_specs,
            &spec.approval_mode,
            &spec.limits,
            spec.is_subagent,
            &extra,
        );
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(Message::system(system));
        messages.extend(history.iter().cloned());
        let messages = context::fit(
            messages,
            spec.limits.context_max_chars,
            spec.limits.max_tool_output_bytes,
        );

        let request = ChatRequest {
            model: spec.model.model.clone(),
            messages,
            tools: tool_specs,
            temperature: spec.model.temperature,
            max_tokens: spec.model.max_tokens,
            top_p: spec.model.top_p,
            parallel_tool_calls: spec.model.parallel_tool_calls,
            extra_headers: spec
                .model
                .extra_headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            extra_body: spec
                .model
                .extra_body
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };

        // --- stream -----------------------------------------------------------
        let provider = match inner.deps.providers.build(&spec.model).await {
            Ok(provider) => provider,
            Err(e) => {
                if spec.cancel.is_cancelled() {
                    return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
                }
                let message = format!("could not start the provider: {e}");
                ctx.error("provider", message.clone());
                return LoopOutcome::new(RunStatus::Failed, Some(message), last_text, iterations);
            }
        };
        let stream = match provider.stream_chat(request, spec.cancel.clone()).await {
            Ok(stream) => stream,
            Err(e) => {
                // A cancel that lands before the request completes is still a
                // cancellation, not a provider failure.
                if spec.cancel.is_cancelled() {
                    ctx.notice("info", "run cancelled");
                    return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
                }
                let message = format!("provider request failed: {e}");
                ctx.error("provider", message.clone());
                return LoopOutcome::new(RunStatus::Failed, Some(message), last_text, iterations);
            }
        };

        let (text, calls, end) = consume(&ctx, &spec.cancel, spec.deadline, stream).await;

        if !text.is_empty() {
            last_text = text.clone();
        }

        match end {
            StreamEnd::Complete => {}
            StreamEnd::Cancelled => {
                if !text.is_empty() {
                    persist_assistant_text(&inner, &spec, &ctx, &text).await;
                }
                ctx.notice("info", "run cancelled");
                return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
            }
            StreamEnd::TimedOut => {
                if !text.is_empty() {
                    persist_assistant_text(&inner, &spec, &ctx, &text).await;
                }
                let message = if spec.is_subagent {
                    format!(
                        "subagent timed out after {}s (settings.subagents.timeout_secs)",
                        spec.time_budget_secs
                    )
                } else {
                    format!(
                        "stopped: the run exceeded the {}s wall-clock limit (limits.max_run_secs)",
                        spec.time_budget_secs
                    )
                };
                ctx.notice("warn", message.clone());
                let error = if spec.is_subagent {
                    Some(message)
                } else {
                    None
                };
                return LoopOutcome::new(RunStatus::LimitReached, error, last_text, iterations);
            }
            StreamEnd::Failed(message) => {
                if spec.cancel.is_cancelled() {
                    if !text.is_empty() {
                        persist_assistant_text(&inner, &spec, &ctx, &text).await;
                    }
                    ctx.notice("info", "run cancelled");
                    return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
                }
                if !text.is_empty() {
                    persist_assistant_text(&inner, &spec, &ctx, &text).await;
                }
                ctx.error("provider", message.clone());
                return LoopOutcome::new(RunStatus::Failed, Some(message), last_text, iterations);
            }
        }

        // --- persist the assistant turn ---------------------------------------
        let assistant = Message::assistant(text, calls.clone());
        if let Err(e) = persist(
            &inner,
            &spec.session_id,
            &spec.run_id,
            spec.agent_id.as_ref(),
            &assistant,
        )
        .await
        {
            let message = format!("could not persist the assistant message: {e}");
            ctx.error("store", message.clone());
            return LoopOutcome::new(RunStatus::Failed, Some(message), last_text, iterations);
        }
        history.push(assistant);

        if calls.is_empty() {
            return LoopOutcome::new(RunStatus::Completed, None, last_text, iterations);
        }

        // --- tools ------------------------------------------------------------
        let per_turn = spec.limits.max_tool_calls_per_turn.max(1);
        let skipped_by_cap = calls.len().saturating_sub(per_turn);
        if skipped_by_cap > 0 {
            ctx.notice(
                "warn",
                format!(
                    "skipped {skipped_by_cap} tool call(s): limits.max_tool_calls_per_turn is {per_turn}"
                ),
            );
        }

        for (index, call) in calls.iter().enumerate() {
            let skip_reason = if index >= per_turn {
                Some(format!(
                    "{SKIPPED_PREFIX}per-turn tool call limit ({per_turn}) reached]"
                ))
            } else if spec.cancel.is_cancelled() {
                Some(format!("{SKIPPED_PREFIX}run cancelled]"))
            } else {
                None
            };
            if let Some(reason) = skip_reason {
                // The assistant message already carries the call, so a tool
                // message must exist or the transcript becomes invalid for the
                // next request.
                let placeholder = Message::tool_result(call.id.clone(), call.name.clone(), reason);
                if let Err(e) = persist(
                    &inner,
                    &spec.session_id,
                    &spec.run_id,
                    spec.agent_id.as_ref(),
                    &placeholder,
                )
                .await
                {
                    let message = format!("could not persist the tool message: {e}");
                    ctx.error("store", message.clone());
                    return LoopOutcome::new(
                        RunStatus::Failed,
                        Some(message),
                        last_text,
                        iterations,
                    );
                }
                history.push(placeholder);
                continue;
            }

            let result = run_one_tool(&inner, &spec, &ctx, call).await;
            match result {
                Ok(message) => history.push(message),
                Err(e) => {
                    ctx.error("store", format!("could not persist the tool message: {e}"));
                    return LoopOutcome::new(
                        RunStatus::Failed,
                        Some(e.to_string()),
                        last_text,
                        iterations,
                    );
                }
            }
        }

        if spec.cancel.is_cancelled() {
            ctx.notice("info", "run cancelled");
            return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
        }
    }
}

async fn persist_assistant_text(
    inner: &Arc<RuntimeInner>,
    spec: &LoopSpec,
    ctx: &EmitCtx,
    text: &str,
) {
    let message = Message::assistant(text.to_string(), Vec::new());
    if let Err(e) = persist(
        inner,
        &spec.session_id,
        &spec.run_id,
        spec.agent_id.as_ref(),
        &message,
    )
    .await
    {
        ctx.error(
            "store",
            format!("could not persist the partial assistant message: {e}"),
        );
    }
}

async fn persist(
    inner: &Arc<RuntimeInner>,
    session_id: &SessionId,
    run_id: &RunId,
    agent_id: Option<&AgentId>,
    message: &Message,
) -> Result<i64> {
    let mut record = NewMessage::from_message(&session_id.0, Some(&run_id.0), message);
    record.agent_id = agent_id.map(|a| a.0.clone());
    inner.deps.store.append_message(&record).await
}

/// How much of the argument JSON an approval card shows.
const APPROVAL_ARGUMENTS_LIMIT: usize = 2_000;

/// Central approval gate for tools the runtime does not own.
///
/// Built-in shell/mutating tools gate themselves: they call
/// [`ToolCtx::approvals`] so they can show the exact command or diff. Plugin and
/// MCP tools are untrusted in that respect (buggy or malicious plugin code could
/// simply skip the ask), so the loop applies the approval policy here, before
/// invoke, using the tool's declared origin and class.
///
/// Returns `Some(outcome)` when the call must not be invoked, and `None` when it
/// may proceed. A policy denial is a tool error, never a run failure: the model
/// sees the refusal and can adapt.
async fn gate_external_tool(
    inner: &Arc<RuntimeInner>,
    spec: &LoopSpec,
    call: &ToolCall,
    tool: &Arc<dyn Tool>,
    arguments: &serde_json::Value,
) -> Option<ToolOutcome> {
    let origin = tool.spec().origin;
    // Built-ins self-gate; read-only built-ins need no gate at all.
    if origin == ToolOrigin::Builtin {
        return None;
    }
    // `requires_approval` is the tool's declaration (default: every class except
    // ReadOnly). A read-only external tool needs no prompt.
    if !tool.requires_approval() {
        return None;
    }

    let policy = inner.deps.settings.approval_policy().await;
    let view = ApprovalPolicyView {
        mode: policy.mode.clone(),
        allow_commands: policy.allow_commands.clone(),
        auto_approve_paths: policy.auto_approve_paths.clone(),
    };
    match view.mode {
        ApprovalMode::Auto => None,
        ApprovalMode::ReadOnly => Some(ToolOutcome::err("denied by policy: read_only mode")),
        ApprovalMode::Ask | ApprovalMode::AskCommands => {
            let kind = match &origin {
                ToolOrigin::Mcp { .. } => ApprovalKind::Mcp,
                ToolOrigin::Plugin { .. } => ApprovalKind::Plugin,
                _ => ApprovalKind::Other,
            };
            let arguments_json =
                serde_json::to_string(arguments).unwrap_or_else(|_| call.arguments.clone());
            let detail = format!(
                "{}\n{}",
                origin.label(),
                truncate_detail(&arguments_json, APPROVAL_ARGUMENTS_LIMIT)
            );
            let payload = serde_json::json!({
                "tool": call.name,
                "origin": origin,
                "arguments": arguments,
            });
            // Same handle the tool would use, so the store row, the
            // `approval_requested`/`approval_resolved` events and the
            // `allow_session` grants behave exactly like a built-in's request.
            let approvals = ApprovalHandle::new(
                Arc::clone(&spec.gate),
                spec.session_id.clone(),
                spec.run_id.clone(),
                spec.agent_id.clone(),
            );
            let decision = approvals
                .ask(kind, format!("Call {}", call.name), detail, payload)
                .await;
            match decision {
                ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession => None,
                ApprovalDecision::Deny => Some(ToolOutcome::err(format!(
                    "approval denied by user for `{}`; the tool was not invoked",
                    call.name
                ))),
                ApprovalDecision::Expired => Some(ToolOutcome::err(format!(
                    "approval timed out for `{}`; the tool was not invoked",
                    call.name
                ))),
            }
        }
    }
}

/// Truncate approval detail without splitting a UTF-8 character.
fn truncate_detail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [truncated]", &text[..end])
}

/// Execute a single tool call: emit start/finish, invoke, truncate, persist.
async fn run_one_tool(
    inner: &Arc<RuntimeInner>,
    spec: &LoopSpec,
    ctx: &EmitCtx,
    call: &ToolCall,
) -> Result<Message> {
    let started = Instant::now();
    let parsed: Option<serde_json::Value> = if call.arguments.trim().is_empty() {
        Some(serde_json::json!({}))
    } else {
        serde_json::from_str(&call.arguments).ok()
    };
    ctx.emit(Event::ToolCallStarted {
        call_id: call.id.clone(),
        name: call.name.clone(),
        arguments: parsed
            .clone()
            .unwrap_or_else(|| serde_json::Value::String(call.arguments.clone())),
    });

    let max_bytes = spec.limits.max_tool_output_bytes;
    let outcome = match (parsed, spec.tools.get(&call.name)) {
        (None, _) => kc_core::tool::ToolOutcome::err(format!(
            "invalid JSON arguments for tool `{}`: {}",
            call.name, call.arguments
        )),
        (Some(_), None) => {
            let available = spec.tools.names().join(", ");
            kc_core::tool::ToolOutcome::err(format!(
                "unknown tool `{}`; available tools: {}",
                call.name,
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available
                }
            ))
        }
        (Some(arguments), Some(tool)) => {
            if let Some(denied) = gate_external_tool(inner, spec, call, &tool, &arguments).await {
                denied
            } else {
                let tool_ctx = ToolCtx {
                    workspace: Arc::clone(&inner.deps.workspace),
                    cancel: spec.cancel.clone(),
                    approvals: kc_core::tool::ApprovalHandle::new(
                        Arc::clone(&spec.gate),
                        spec.session_id.clone(),
                        spec.run_id.clone(),
                        spec.agent_id.clone(),
                    ),
                    events: ctx.sink(),
                    settings: inner.deps.settings.clone(),
                    depth: spec.depth,
                };
                let request = kc_core::tool::ToolCallRequest {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments,
                };
                match std::panic::AssertUnwindSafe(tool.invoke(request, tool_ctx))
                    .catch_unwind()
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(payload) => kc_core::tool::ToolOutcome::err(format!(
                        "tool `{}` panicked: {}",
                        call.name,
                        panic_message(payload.as_ref())
                    )),
                }
            }
        }
    };

    let (output, cut) = truncate_output(&outcome.content, max_bytes);
    let truncated = cut || outcome.truncated;
    let duration_ms = started.elapsed().as_millis() as u64;

    ctx.emit(Event::ToolCallFinished {
        call_id: call.id.clone(),
        name: call.name.clone(),
        ok: outcome.ok,
        output: output.clone(),
        truncated,
        duration_ms,
    });
    inner
        .deps
        .hooks
        .on_tool_result(&spec.session_id.0, &call.name, outcome.ok)
        .await;

    let message = Message::tool_result(call.id.clone(), call.name.clone(), output);
    persist(
        inner,
        &spec.session_id,
        &spec.run_id,
        spec.agent_id.as_ref(),
        &message,
    )
    .await?;
    Ok(message)
}

/// Consume the provider stream, emitting deltas live.
async fn consume(
    ctx: &EmitCtx,
    cancel: &CancellationToken,
    deadline: Instant,
    mut stream: ProviderStream,
) -> (String, Vec<ToolCall>, StreamEnd) {
    let mut text = String::new();
    let mut accumulator = kc_core::provider::ToolCallAccumulator::default();
    let mut end = StreamEnd::Complete;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                end = StreamEnd::Cancelled;
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                end = StreamEnd::TimedOut;
                break;
            }
            item = stream.next() => {
                let Some(item) = item else { break };
                match item {
                    Ok(ProviderChunk::Text(delta)) => {
                        if !delta.is_empty() {
                            text.push_str(&delta);
                            ctx.emit(Event::TextDelta { delta });
                        }
                    }
                    Ok(ProviderChunk::Reasoning(delta)) => {
                        if !delta.is_empty() {
                            ctx.emit(Event::ReasoningDelta { delta });
                        }
                    }
                    Ok(ProviderChunk::ToolCallDelta { index, id, name, arguments_delta }) => {
                        accumulator.apply(index, id, name, arguments_delta);
                    }
                    Ok(ProviderChunk::Usage { input_tokens, output_tokens }) => {
                        ctx.emit(Event::Usage { input_tokens, output_tokens });
                    }
                    Ok(ProviderChunk::Finish { .. }) => break,
                    Ok(ProviderChunk::Error { message, retryable }) => {
                        let suffix = if retryable { " (retryable)" } else { "" };
                        end = StreamEnd::Failed(format!("provider stream error: {message}{suffix}"));
                        break;
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            end = StreamEnd::Cancelled;
                            break;
                        }
                        end = StreamEnd::Failed(format!("provider stream failed: {e}"));
                        break;
                    }
                }
            }
        }
    }

    (text, accumulator.finish(), end)
}

/// Best-effort text for a caught panic payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    // `&Box<dyn Any + Send>` coerces to `&dyn Any` pointing at the box itself;
    // unwrap that (and any further nesting) instead of giving up.
    if let Some(inner) = payload.downcast_ref::<Box<dyn std::any::Any + Send>>() {
        return panic_message(&**inner);
    }
    if let Some(inner) = payload.downcast_ref::<Box<dyn std::any::Any + Send + Sync>>() {
        return panic_message(&**inner);
    }
    "unknown panic".to_string()
}

/// Map a tool class to the approval kind a tool would ask for. Exposed for
/// tools that need a default; the runtime itself trusts the tool's request.
pub fn default_approval_kind(class: kc_core::tool::ToolClass) -> ApprovalKind {
    match class {
        kc_core::tool::ToolClass::ReadOnly => ApprovalKind::Other,
        kc_core::tool::ToolClass::Mutating => ApprovalKind::FileWrite,
        kc_core::tool::ToolClass::Shell => ApprovalKind::ShellCommand,
        kc_core::tool::ToolClass::Delegate => ApprovalKind::Other,
        kc_core::tool::ToolClass::External => ApprovalKind::NetworkTool,
    }
}
