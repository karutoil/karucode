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
use crate::history::repair_history;
use crate::restart::StreamRestarts;
use crate::{RunHandle, RunState, Runtime, RuntimeInner, HISTORY_LIMIT};
use futures_util::{FutureExt, StreamExt};
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView};
use kc_core::config::{ApprovalMode, Limits, ModelConfig};
use kc_core::error::{Error, Result};
use kc_core::events::Event;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::message::{Message, ToolCall};
use kc_core::paths::Workspace;
use kc_core::prompt::Budget;
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

/// Prefix of the placeholder tool result written when a call is skipped (per-turn
/// cap or cancellation) but the assistant message already carries the call.
pub(crate) const SKIPPED_PREFIX: &str = "[skipped: ";

/// Everything one loop needs. Built by [`start_run`] for a session run and by
/// the `spawn_agent` tool for a nested agent.
pub(crate) struct LoopSpec {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub agent_id: Option<AgentId>,
    pub depth: u8,
    pub is_subagent: bool,
    /// Segment size, continuation count and wall clock, all in one place so the
    /// loop's arithmetic and the system prompt can never disagree about what
    /// this loop is allowed to do. A subagent gets
    /// [`Budget::for_subagent`], a run [`Budget::for_run`].
    pub budget: Budget,
    /// Wall-clock deadline derived from `budget.wall_secs` at construction.
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
    /// Directory every file/shell tool in this loop is confined to. Inherited
    /// by subagents from the tool context, so a child never escapes its parent's
    /// project.
    pub workspace: Arc<Workspace>,
    /// The rewind journal this loop's file tools record into. `None` for a
    /// subagent whose parent had none (tests, embeddings).
    pub journal: Option<Arc<crate::rewind::StoreJournal>>,
    /// Conversation without the system message; the system prompt is rebuilt
    /// each iteration from the current tool set and hook contributions.
    pub history: Vec<Message>,
    pub run_state: Option<Arc<RunState>>,
    /// Characters of conversation this loop may send, derived once from the
    /// resolved model (`limits.context_chars_for`). Carried on the loop rather
    /// than read from `limits` everywhere so the budget cannot drift between the
    /// request, the run window and the per-turn cap.
    pub context_max_chars: usize,
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
    /// The turn's answer did not arrive in full. `transient` says whether
    /// another attempt could plausibly land on a healthy channel: a body that
    /// died mid-flight or a stream that stopped without a `finish_reason` is
    /// worth restarting, a request the endpoint rejected is not.
    Failed {
        message: String,
        transient: bool,
    },
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

    let session = store
        .get_session(session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
    // The project decides the workspace before anything is written, so a run can
    // never start in the wrong directory.
    let workspace = inner
        .deps
        .workspaces
        .workspace_for(session.project_id.as_deref())?;

    // The session's own selection (or the install-wide default) decides the
    // model, so several sessions can run different models concurrently.
    let model = inner
        .deps
        .settings
        .resolve_model(session.model.as_ref())
        .await;
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
    kc_core::metrics::record_run_started();

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
    // One journal per run: it names the run that produced every row, so a rewind
    // can attribute a change to the turn it belongs to.
    let journal = crate::rewind::StoreJournal::new(
        inner.deps.store.clone(),
        state.session_id.clone(),
        state.run_id.clone(),
        None,
        Arc::clone(&workspace),
    );
    // Held across the run so a queued follow-up can start the next one. Cloning
    // the caller's handle is what lets the chain continue after the HTTP request
    // that started this run has returned.
    let runtime_for_follow_up = Arc::clone(runtime);
    tokio::spawn(async move {
        // A panic in a tool or provider must not leak the run: catch it here so
        // the row becomes `failed` and the session accepts work again.
        let outcome = std::panic::AssertUnwindSafe(run_top_level(
            Arc::clone(&spawned_inner),
            Arc::clone(&spawned_state),
            prompt,
            history,
            model,
            workspace,
            journal,
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

        // The run is over and its state is gone, so `is_running` is false and a new
        // `submit` is legal. Start the next follow-up, if the user queued one while
        // this run was working.
        //
        // This is deliberately a chain, not a fresh `tokio::spawn`: the follow-up
        // belongs to the same "user is waiting on this session" sequence, and a
        // chain keeps the ordering honest (one run at a time, queued in order). The
        // hop through [`start_run_boxed`] is what lets the compiler size the type -
        // `start_run` awaiting `start_run` directly would be an infinitely sized
        // future, and spawning it would be a `Send` cycle between the two.
        let session_id = spawned_state.session_id.0.clone();
        while let Some(next) = runtime_for_follow_up.take_follow_up(&session_id) {
            let runtime = Arc::clone(&runtime_for_follow_up);
            let prompt = next.content.clone();
            let next_id = next.id.clone();
            match start_run_boxed(runtime, session_id.clone(), prompt).await {
                Ok(handle) => {
                    runtime_for_follow_up.emit(
                        Some(&session_id),
                        Some(&handle.run_id),
                        None,
                        Event::Notice {
                            level: "info".into(),
                            message: "Starting the queued follow-up.".into(),
                        },
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        session = %session_id,
                        follow_up = %next_id,
                        error = %e,
                        "could not start a queued follow-up"
                    );
                    runtime_for_follow_up.emit(
                        Some(&session_id),
                        None,
                        None,
                        Event::Notice {
                            level: "warn".into(),
                            message: format!(
                                "A queued follow-up could not be started: {e}. Send it again when the session is ready."
                            ),
                        },
                    );
                    // Do not spin: the queue is cleared so the user gets the
                    // notice rather than the same failure retried forever.
                    runtime_for_follow_up.clear_follow_ups(&session_id);
                    break;
                }
            }
        }
    });

    Ok(RunHandle {
        run_id: run_id.0,
        session_id: session_id.to_string(),
    })
}

/// Type-erased [`start_run`].
///
/// `start_run`'s spawned task ends by starting the next queued follow-up through
/// this function. Calling `start_run` directly from inside that task makes the
/// future's type recursive (and its `Send`-ness circular through `submit`), which
/// the compiler cannot size. Boxing the future breaks the cycle at the cost of one
/// allocation per follow-up hop, which is nothing next to a model turn.
fn start_run_boxed(
    runtime: Arc<Runtime>,
    session_id: String,
    prompt: String,
) -> futures_util::future::BoxFuture<'static, Result<RunHandle>> {
    Box::pin(async move { start_run(&runtime, &session_id, prompt).await })
}

/// Write the run row + user message and load the prior conversation.
async fn begin_run(
    inner: &Arc<RuntimeInner>,
    state: &Arc<RunState>,
    model: &ModelConfig,
    prompt: &str,
) -> Result<Vec<Message>> {
    let store = &inner.deps.store;
    let limits = inner.deps.settings.limits().await;
    store
        .create_run(&state.run_id.0, &state.session_id.0, &model.model, prompt)
        .await?;

    let user = Message::user(prompt);
    persist(inner, &state.session_id, &state.run_id, None, &user).await?;
    store.touch_session(&state.session_id.0).await?;

    // The new user message is already persisted; repair the whole window now so
    // a slice boundary or an earlier interrupted tool call cannot make the
    // request invalid for the provider. The char cap keeps a session with
    // thousands of tool calls from loading (and cloning) its entire transcript.
    let mut history = repair_history(
        store,
        &state.session_id.0,
        HISTORY_LIMIT,
        limits.context_chars_for(model),
    )
    .await?;
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
    workspace: Arc<Workspace>,
    journal: Arc<crate::rewind::StoreJournal>,
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
        budget: Budget::for_run(&limits),
        deadline: Instant::now() + Duration::from_secs(limits.max_run_secs.max(1)),
        context_max_chars: limits.context_chars_for(&model),
        model,
        limits,
        approval_mode: policy.mode.as_str().to_string(),
        gate,
        approval_scope,
        tools,
        cancel: state.cancel.clone(),
        workspace,
        journal: Some(journal),
        history,
        run_state: Some(Arc::clone(&state)),
    };

    let outcome = run_loop(Arc::clone(&inner), spec).await;
    kc_core::metrics::record_run_finished();

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

    // Last, and only after the run's terminal state is persisted and announced:
    // a notification describes state, so a client that reacts to it by refetching
    // must already find the truth on the server.
    //
    // The title is read *here* rather than before the loop: it is only needed for the
    // notification, and a store round-trip on the run's critical path delayed
    // `RunStarted` (which the UI and the API both key "is it running" off) for no
    // benefit.
    let session_title = inner
        .deps
        .store
        .get_session(&state.session_id.0)
        .await
        .ok()
        .flatten()
        .map(|session| session.title)
        .unwrap_or_default();
    crate::notify::run_finished(
        &inner,
        &ctx,
        &session_title,
        crate::notify::RunFinished {
            run_id: &state.run_id.0,
            status: outcome.status,
            error: outcome.error.as_deref(),
            answer: &outcome.text,
            started_at_ms: state.started_at,
            finished_at_ms: crate::events::now_ms(),
            // Read here, not at the top of the run: the flag is set by `cancel()`
            // while the loop is in flight.
            cancelled_by_user: state.was_cancelled_by_user(),
        },
    )
    .await;

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
    // `iterations` counts every model round-trip in the loop; `segment_iterations`
    // only the current budget window. The loop stops on the segment budget only
    // when it may not continue (`auto_continue` off, `max_continues` reached) or
    // when the wall clock is gone. A subagent follows the same rule with its own
    // numbers - a delegated task that needs hours must not die at its first
    // segment while the run that spawned it keeps going.
    let mut iterations = 0u32;
    let mut segment_iterations = 0u32;
    let mut continues = 0u32;
    let max_segments = spec.budget.max_continues.saturating_add(1);
    let mut last_text = String::new();
    // Identical calls seen in this run, with the hash of the result they
    // returned and how many mutating calls had happened when that result was
    // produced. A model stuck in a loop re-issues the same call and gets the same
    // answer; the note below is the cheapest way to make that visible to it
    // (and to the transcript) instead of letting it burn the budget silently.
    let mut repeats: std::collections::HashMap<String, (u32, u64, u64)> =
        std::collections::HashMap::new();
    let mut mutations: u64 = 0;

    loop {
        if spec.cancel.is_cancelled() {
            ctx.notice("info", "run cancelled");
            return LoopOutcome::new(RunStatus::Cancelled, None, last_text, iterations);
        }
        // --- steering ---------------------------------------------------------
        // A message the user sent *while this run was working*. It becomes an
        // ordinary `user` turn here, before the request is built, so the model
        // sees it exactly where a follow-up would sit if the user had waited.
        //
        // Deliberately at the top of the iteration and not mid-stream: the turn
        // in flight is allowed to finish, because aborting it throws away the
        // model's tool plan and can persist a tool call with no result. Only a
        // top-level run takes steers; a subagent has its own transcript and no
        // user watching it.
        if !spec.is_subagent {
            let steers = inner.drain_steers(&spec.session_id.0);
            for steer in steers {
                let message = Message::user(&steer.content);
                if let Err(e) = persist(
                    &inner,
                    &spec.session_id,
                    &spec.run_id,
                    spec.agent_id.as_ref(),
                    &message,
                )
                .await
                {
                    ctx.error(
                        "store",
                        format!("could not persist a steering message: {e}"),
                    );
                }
                // The transcript and the request must agree: an unpersisted steer
                // is still one the model acted on, so it goes into `history`
                // either way - exactly like the continuation note below.
                history.push(message);
                ctx.emit(Event::SteerApplied {
                    steer_id: steer.id.clone(),
                    content: steer.content.clone(),
                });
            }
        }
        if segment_iterations >= spec.budget.max_iterations {
            let message = if spec.is_subagent {
                format!(
                    "subagent stopped after {iterations} iterations (settings.subagents.max_iterations \
is {} per segment, settings.subagents.max_continues is {})",
                    spec.budget.max_iterations, spec.budget.max_continues
                )
            } else {
                format!(
                    "stopped after {iterations} iterations (limits.max_iterations is {} per \
segment, limits.max_continues is {})",
                    spec.budget.max_iterations, spec.budget.max_continues
                )
            };
            // A budget that runs out mid-task is a checkpoint, not a finish
            // line: the previous turn asked for tools, so the work is still in
            // flight. Refresh the window and keep the same session, run and
            // transcript, bounded by `max_continues` and the wall clock.
            let can_continue = spec.budget.auto_continue
                && continues < spec.budget.max_continues
                && Instant::now() < spec.deadline
                && !spec.cancel.is_cancelled();
            if can_continue {
                continues += 1;
                segment_iterations = 0;
                let note = continuation_nudge(continues, max_segments, &spec);
                let message = Message::user(note);
                // Persist the checkpoint so the next run's transcript shows why
                // the conversation jumped, instead of a silent gap.
                if let Err(e) = persist(
                    &inner,
                    &spec.session_id,
                    &spec.run_id,
                    spec.agent_id.as_ref(),
                    &message,
                )
                .await
                {
                    ctx.error("store", format!("could not persist the checkpoint: {e}"));
                }
                history.push(message);
                ctx.notice(
                    "info",
                    format!(
                        "iteration budget refreshed after {iterations} iterations - continuing automatically (segment {}/{max_segments}, limits.auto_continue)",
                        continues + 1
                    ),
                );
            } else {
                ctx.notice("warn", message.clone());
                let error = if spec.is_subagent {
                    Some(message)
                } else {
                    None
                };
                return LoopOutcome::new(RunStatus::LimitReached, error, last_text, iterations);
            }
        }
        if Instant::now() >= spec.deadline {
            let message = if spec.is_subagent {
                format!(
                    "subagent timed out after {}s (settings.subagents.timeout_secs)",
                    spec.budget.wall_secs
                )
            } else {
                format!(
                    "stopped: the run exceeded the {}s wall-clock limit (limits.max_run_secs)",
                    spec.budget.wall_secs
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
        segment_iterations += 1;
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
        // Bounded like a tool call: `Hooks` is a public trait, and a plugin hook
        // that never returns would otherwise stall the loop with no deadline and
        // no way to cancel. On timeout the run continues with no extra context
        // rather than failing: the hook is advisory, and losing its notes is
        // cheaper than losing the run.
        // Bound before the `select!`: a `to_string_lossy()` temporary cannot be
        // borrowed across the await point.
        let workspace_label = spec.workspace.root().to_string_lossy().to_string();
        let extra = tokio::select! {
            biased;
            _ = spec.cancel.cancelled() => Vec::new(),
            _ = tokio::time::sleep_until(spec.deadline) => Vec::new(),
            notes = tokio::time::timeout(
                HOOK_BUDGET,
                inner
                    .deps
                    .hooks
                    .before_prompt(&spec.session_id.0, &workspace_label),
            ) => match notes {
                Ok(notes) => notes,
                Err(_) => {
                    ctx.notice(
                        "warn",
                        format!(
                            "a plugin hook did not answer within {}s; continuing without its notes",
                            HOOK_BUDGET.as_secs()
                        ),
                    );
                    Vec::new()
                }
            },
        };
        let tool_specs = spec.tools.specs();
        let system = kc_core::prompt::system_prompt_with_roots(
            &spec.workspace.root().to_string_lossy(),
            spec.workspace.extra_roots(),
            &tool_specs,
            &spec.approval_mode,
            &spec.budget,
            &extra,
        );
        let mut messages = Vec::with_capacity(history.len() + 2);
        messages.push(Message::system(system));
        messages.extend(history.iter().cloned());
        // The tail note is appended *after* the fitted conversation, so it can
        // never disturb the cached prefix: it costs its own few hundred tokens
        // and nothing else. Its size is reserved out of the context budget so the
        // request still respects the limit.
        // Deliberately NOTHING is appended after the conversation.
        //
        // Measured on a real 140-message session against this gateway, at the
        // point where the model decides to write a file:
        //   * the history alone                     -> `write_file(8308B)`, 4.0k tokens out
        //   * + a trailing `system` note            -> 8.0k tokens, reasoning only, NO action
        //   * + the same note folded into the last
        //     tool result                          -> 8.0k tokens, reasoning only, NO action
        //   * the same history at temperature 1.0   -> no action
        // A two-line note did the same thing. Any text after the last turn makes
        // this model deliberate instead of acting, and it can burn the whole
        // output budget reasoning about it. The same note at the FRONT of the
        // request committed normally, but a note that changes mid-run costs a
        // prompt-cache miss from position 1, so karucode sends no runtime notes
        // at all: the request is exactly the conversation.
        let messages = context::fit(
            messages,
            spec.context_max_chars,
            spec.limits.max_tool_output_bytes,
        );

        let request = ChatRequest {
            model: spec.model.model.clone(),
            messages,
            tools: tool_specs,
            temperature: spec.model.temperature,
            send_temperature: spec.model.send_temperature,
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

        // One turn, at most `retry_max_stream_restarts + 1` stream attempts. A
        // stream that dies *after* it started delivering cannot be retried one
        // level down (the provider layer hands chunks straight to the UI and
        // replaying them would duplicate the answer), so it is handled here:
        // the partial attempt is discarded - nothing was persisted for it - the
        // client is told to drop what it rendered, and the identical request is
        // re-issued. See `crate::restart`.
        let restarts = StreamRestarts::from_config(&spec.model);
        let mut attempt: u32 = 0;
        let (text, calls, end, finish_reason) = loop {
            attempt = attempt.saturating_add(1);
            let stream = match provider
                .stream_chat(request.clone(), spec.cancel.clone())
                .await
            {
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
                    return LoopOutcome::new(
                        RunStatus::Failed,
                        Some(message),
                        last_text,
                        iterations,
                    );
                }
            };

            let outcome = consume(&ctx, &spec.cancel, spec.deadline, stream).await;
            let restart = match &outcome.2 {
                StreamEnd::Failed { message, transient } => {
                    *transient
                        && !spec.cancel.is_cancelled()
                        && attempt < restarts.max_attempts()
                        && tokio::time::Instant::now() < spec.deadline
                        && {
                            tracing::warn!(
                                attempt,
                                max_attempts = restarts.max_attempts(),
                                error = %message,
                                "provider stream died mid-answer; restarting it"
                            );
                            ctx.emit(Event::StreamRetry {
                                attempt,
                                max_attempts: restarts.max_attempts(),
                                reason: message.clone(),
                            });
                            true
                        }
                }
                _ => false,
            };
            if !restart {
                break outcome;
            }

            let delay = restarts.delay(attempt.saturating_sub(1));
            tokio::select! {
                biased;
                _ = spec.cancel.cancelled() => {}
                _ = tokio::time::sleep(delay) => {}
            }
        };

        if !text.is_empty() {
            last_text = text.clone();
        }

        let mut hit_token_limit = false;
        match end {
            StreamEnd::Complete => {
                if finish_reason
                    .as_deref()
                    .is_some_and(|reason| reason.eq_ignore_ascii_case("length"))
                {
                    hit_token_limit = true;
                    ctx.notice("warn", TOKEN_LIMIT_NOTICE);
                }
            }
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
                        spec.budget.wall_secs
                    )
                } else {
                    format!(
                        "stopped: the run exceeded the {}s wall-clock limit (limits.max_run_secs)",
                        spec.budget.wall_secs
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
            StreamEnd::Failed { message, .. } => {
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
        history.push(assistant.clone());

        if calls.is_empty() {
            if hit_token_limit {
                // Do not report a truncated answer as a success: the turn ended
                // because the model ran out of output budget, not because it
                // finished answering.
                let error = if spec.is_subagent {
                    Some(TOKEN_LIMIT_NOTICE.to_string())
                } else {
                    None
                };
                return LoopOutcome::new(RunStatus::LimitReached, error, last_text, iterations);
            }
            return LoopOutcome::new(RunStatus::Completed, None, last_text, iterations);
        }

        // --- tools ------------------------------------------------------------
        let per_turn = spec.limits.max_tool_calls_per_turn.max(1);
        let skipped_by_cap = calls.len().saturating_sub(per_turn);
        // A turn is bounded in *bytes*, not only in call count: 32 parallel
        // calls at the 24 KB per-call cap is three context budgets in one
        // request, and a turn that cannot fit is a turn whose results get
        // evicted before the model sees them - which is what makes a model
        // re-read the same file forever. Half the budget per turn keeps every
        // call visible and leaves the other half for history.
        let turn_cap = context::turn_budget(spec.context_max_chars);
        let mut turn_chars = 0usize;
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

            // A read-only call whose exact arguments already returned the same
            // answer, with no mutation in between, cannot produce anything new:
            // invoking it again only re-bills the same bytes. Walk the model
            // forward instead of paying for the same page twice.
            let repeat_key = format!("{}\u{1}{}", call.name, call.arguments);
            let read_only = spec
                .tools
                .get(&call.name)
                .map(|tool| tool.class() == kc_core::tool::ToolClass::ReadOnly)
                .unwrap_or(false);
            let repeat_stale = read_only
                && repeats.get(&repeat_key).is_some_and(|(count, _, seen_at)| {
                    // The third identical answer carries the warning; the
                    // fourth is refused rather than paying for the same page
                    // again (measured: one review run re-read the same file
                    // 27 times while ignoring the warning).
                    *count >= REPEAT_NOTE_AFTER && *seen_at == mutations
                });
            if repeat_stale {
                let count = repeats.get(&repeat_key).map(|(c, _, _)| *c).unwrap_or(0);
                let note = format!(
                    "[blocked: this exact call has already returned the same result {count} times \
                     in this run, and nothing has been written since. Re-issuing it cannot produce \
                     new information. Use what you already have, or read a range you have not seen.]"
                );
                ctx.emit(Event::ToolCallFinished {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    ok: false,
                    output: note.clone(),
                    truncated: false,
                    duration_ms: 0,
                    // A blocked repeat is refused, so there is nothing to show.
                    images: Vec::new(),
                });
                ctx.notice(
                    "warn",
                    format!(
                        "blocked a repeated identical `{}` call ({count}x): nothing changed since",
                        call.name
                    ),
                );
                let message = Message::tool_result(call.id.clone(), call.name.clone(), note);
                if let Err(e) = persist(
                    &inner,
                    &spec.session_id,
                    &spec.run_id,
                    spec.agent_id.as_ref(),
                    &message,
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
                history.push(message);
                continue;
            }

            let remaining = turn_cap
                .saturating_sub(turn_chars)
                .max(TURN_RESULT_FLOOR_BYTES);
            let result = run_one_tool(&inner, &spec, &ctx, call, remaining).await;
            match result {
                Ok(message) => {
                    // Same call, same answer: say so instead of pretending it is
                    // new information.
                    let key = repeat_key;
                    let hash = content_hash(&message.content);
                    let count = match repeats.get_mut(&key) {
                        Some((count, previous, seen_at)) if *previous == hash => {
                            *count = count.saturating_add(1);
                            *seen_at = mutations;
                            *count
                        }
                        _ => {
                            repeats.insert(key, (1, hash, mutations));
                            1
                        }
                    };
                    // No note is appended to the result itself: adding text to a
                    // tool result is one of the shapes that pushes this model into
                    // a reasoning stall (see the request-building comment). The
                    // fourth identical call is refused outright instead.
                    if count >= REPEAT_NOTE_AFTER && read_only {
                        ctx.notice(
                            "warn",
                            format!(
                                "`{}` returned an identical result {count} times in this run",
                                call.name
                            ),
                        );
                    }
                    if !read_only {
                        mutations = mutations.saturating_add(1);
                    }
                    turn_chars = turn_chars.saturating_add(
                        message.content.len() + message.images.len() * context::IMAGE_CONTEXT_CHARS,
                    );
                    let persisted = persist(
                        &inner,
                        &spec.session_id,
                        &spec.run_id,
                        spec.agent_id.as_ref(),
                        &message,
                    )
                    .await;
                    match persisted {
                        Ok(id) => {
                            // Place the journal rows this call wrote on the
                            // transcript timeline: they were recorded *before* the
                            // write, so the row they belong to did not exist yet.
                            if let Some(journal) = &spec.journal {
                                journal.bind_message(id).await;
                            }
                        }
                        Err(e) => {
                            let message = format!("could not persist the tool message: {e}");
                            ctx.error("store", message.clone());
                            return LoopOutcome::new(
                                RunStatus::Failed,
                                Some(message),
                                last_text,
                                iterations,
                            );
                        }
                    }
                    history.push(message)
                }
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

        // Bound the in-memory working set: a long session must not keep (and
        // re-clone, every iteration) a transcript far past the request budget.
        // The dropped turns survive in the persisted transcript and in the
        // digest notice, so nothing is lost, only unbounded memory growth.
        context::compact_run_window(&mut history, spec.context_max_chars);
    }
}

/// The synthetic user turn that carries a run across an iteration-budget
/// boundary. It is persisted, so the transcript explains the jump, and it tells
/// the model exactly what to do next instead of leaving it to guess.
fn continuation_nudge(continues: u32, max_segments: u32, spec: &LoopSpec) -> String {
    let remaining = spec
        .deadline
        .saturating_duration_since(Instant::now())
        .as_secs();
    format!(
        "[runtime checkpoint] Iteration budget refreshed (segment {}/{max_segments}). \
This is not a new request: the task is still in progress and you have {remaining}s of \
wall clock left. Continue exactly where you stopped. Do not repeat work you already \
did, do not re-read files whose contents you already know, and do not restate your plan. \
If the remaining work is large, keep a short plan in a file so a later segment can pick \
it up. Stop only when the task is complete, verified, or genuinely blocked.",
        continues + 1
    )
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
    let policy = inner.deps.settings.approval_policy().await;
    let view = ApprovalPolicyView {
        mode: policy.mode.clone(),
        allow_commands: policy.allow_commands.clone(),
        auto_approve_paths: policy.auto_approve_paths.clone(),
    };

    // `read_only` is a hard policy, not a prompt: it denies every external tool
    // that is not declared read-only, even one whose server set
    // `require_approval = false`. That flag can skip a prompt; it cannot widen
    // what the policy allows.
    if view.mode == ApprovalMode::ReadOnly && tool.class() != kc_core::tool::ToolClass::ReadOnly {
        return Some(ToolOutcome::err(format!(
            "denied by policy: read_only mode does not allow `{}` ({:?} tool from {})",
            call.name,
            tool.class(),
            origin.label()
        )));
    }

    // `requires_approval` is the tool's declaration (default: every class except
    // ReadOnly). A read-only external tool needs no prompt.
    if !tool.requires_approval() {
        return None;
    }

    match view.mode {
        ApprovalMode::Auto | ApprovalMode::ReadOnly => None,
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

/// Execute a single tool call: emit start/finish, invoke, truncate.
///
/// `remaining` is what is left of the turn's byte budget; a result larger than
/// that is truncated here, before it is persisted, so the transcript and the
/// request always agree about every byte.
async fn run_one_tool(
    inner: &Arc<RuntimeInner>,
    spec: &LoopSpec,
    ctx: &EmitCtx,
    call: &ToolCall,
    remaining: usize,
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

    let max_bytes = spec.limits.max_tool_output_bytes.min(remaining);
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
                    workspace: Arc::clone(&spec.workspace),
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
                    supports_images: spec.model.supports_images,
                    checkpoints: spec
                        .journal
                        .clone()
                        .map(|j| j as Arc<dyn kc_core::JournalHandle>),
                };
                let request = kc_core::tool::ToolCallRequest {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments,
                };
                // A tool call must be bounded. The cancel token is only *offered*
                // to the tool through `ToolCtx`; a tool that ignores it (any
                // plugin or MCP tool, or a buggy built-in) would otherwise block
                // this loop forever. `Runtime::cancel` only fires the token and
                // returns, and `is_running` keeps reporting `true` for a
                // cancelled-but-unwinding run, so every later `submit` on this
                // session would be rejected with a 409 and the session would be
                // bricked for the life of the process. Racing the invocation
                // against cancellation and the run deadline turns that into an
                // ordinary failed tool result, which keeps the transcript valid
                // (the call still gets a result row) and lets the loop exit.
                let invoke =
                    std::panic::AssertUnwindSafe(tool.invoke(request, tool_ctx)).catch_unwind();
                tokio::pin!(invoke);
                tokio::select! {
                    biased;
                    _ = spec.cancel.cancelled() => {
                        // Fire-and-forget cancellation is not enough: a tool that
                        // *does* honour the token (every built-in, and the shell
                        // tool's process-group kill) needs a moment to unwind and
                        // report what it observed. Cutting it off here would drop
                        // the future mid-flight and lose that. So give it the same
                        // grace the subagent supervisor uses, and only synthesise a
                        // result for a tool that ignores the token entirely.
                        match tokio::time::timeout(TOOL_CANCEL_GRACE, &mut invoke).await {
                            Ok(Ok(outcome)) => outcome,
                            Ok(Err(payload)) => kc_core::tool::ToolOutcome::err(format!(
                                "tool `{}` panicked: {}",
                                call.name,
                                panic_message(payload.as_ref())
                            )),
                            Err(_) => kc_core::tool::ToolOutcome::err(format!(
                                "tool `{}` was cancelled before it finished",
                                call.name
                            )),
                        }
                    }
                    _ = tokio::time::sleep_until(spec.deadline) => {
                        kc_core::tool::ToolOutcome::err(format!(
                            "tool `{}` did not finish before the run deadline",
                            call.name
                        ))
                    }
                    result = &mut invoke => match result {
                        Ok(outcome) => outcome,
                        Err(payload) => kc_core::tool::ToolOutcome::err(format!(
                            "tool `{}` panicked: {}",
                            call.name,
                            panic_message(payload.as_ref())
                        )),
                    },
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
        // The same bytes the model gets, so the chat can show what it saw
        // without a second round-trip to the transcript endpoint.
        images: outcome.images.clone(),
    });
    inner
        .deps
        .hooks
        .on_tool_result(&spec.session_id.0, &call.name, outcome.ok)
        .await;

    let message = Message::tool_result(call.id.clone(), call.name.clone(), output)
        // Pixels travel beside the text: the store keeps them and `kc-provider`
        // re-emits them as a follow-up `user` message, because a `tool` message
        // cannot carry an image on the OpenAI-compatible wire format.
        .with_images(outcome.images);
    Ok(message)
}

/// Identical results after how many repeats earn a note in the tool result.
const REPEAT_NOTE_AFTER: u32 = 3;

/// Floor for one tool result when the turn is already over its share: enough to
/// see what came back, not enough to blow the budget.
const TURN_RESULT_FLOOR_BYTES: usize = 2_000;

fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Appended to a tool result when the model repeats a call it already made.
///
/// It does not block the call - a re-read after an edit legitimately returns new
/// content - but it stops the loop from being invisible: the model is told the
/// answer has not changed, which is the one piece of information a stuck model
/// does not otherwise get.
/// Notice emitted when the model stopped because it hit its output budget.
pub(crate) const TOKEN_LIMIT_NOTICE: &str =
    "response hit the model's max_tokens limit; the answer may be incomplete and its final text is truncated";

/// How long to keep reading after `finish_reason` for the trailing `usage`
/// block. Generous for a same-connection frame, short enough that a gateway
/// holding the stream open cannot stall the turn.
const USAGE_WINDOW: Duration = Duration::from_secs(3);

/// How long a tool gets to unwind after the run is cancelled.
/// A tool that honours the cancel token (every built-in, and the shell tool's
/// process-group kill) needs a moment to stop and report what it observed.
/// Dropping its future the instant the token fires would lose that report, so
/// the loop waits this long before synthesising a "cancelled" result. A tool
/// that ignores the token entirely is cut off here rather than blocking the
/// session forever. Matches the subagent supervisor's grace period.
const TOOL_CANCEL_GRACE: Duration = Duration::from_millis(500);

/// How long the `before_prompt` hook may take before the run continues without
/// its notes.
///
/// `Hooks` is a public trait and the plugin host fans out to every plugin, so a
/// hook that never returns would stall the loop on every iteration with no
/// deadline. The hook is advisory: losing its notes is cheaper than losing the
/// run, so a timeout is a warning, not a failure.
const HOOK_BUDGET: Duration = Duration::from_secs(5);

/// Consume the provider stream, emitting deltas live.
async fn consume(
    ctx: &EmitCtx,
    cancel: &CancellationToken,
    deadline: Instant,
    mut stream: ProviderStream,
) -> (String, Vec<ToolCall>, StreamEnd, Option<String>) {
    let mut text = String::new();
    let mut accumulator = kc_core::provider::ToolCallAccumulator::default();
    let mut end = StreamEnd::Complete;
    let mut finish_reason: Option<String> = None;
    // Rolling tails of both channels, so a stream that degenerates into
    // repeating one short block can be caught while it happens. A reasoning
    // model can loop in `reasoning_content` for hundreds of thousands of tokens
    // without ever writing a word of content, and that channel is discarded, so
    // nothing downstream can notice: it has to be caught here.
    let mut text_tail = String::new();
    let mut reason_tail = String::new();
    let mut degenerate: Option<Degenerate> = None;
    // Tool-call arguments stream too, and a runaway one is a loop with no text.
    let mut arguments_seen = String::new();
    // Once the provider says why it stopped, keep reading for a moment: the
    // `usage` block (tokens, and the prompt-cache counts) arrives in the chunk
    // *after* the one carrying `finish_reason`. The window is bounded so a
    // gateway that holds the SSE connection open cannot stall the turn.
    let mut usage_window: Option<Instant> = None;

    loop {
        let usage_deadline = async {
            match usage_window {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
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
            _ = usage_deadline => {
                // The provider finished and did not send a usage block.
                break;
            }
            item = stream.next() => {
                let Some(item) = item else { break };
                match item {
                    Ok(ProviderChunk::Text(delta)) => {
                        if !delta.is_empty() {
                            text.push_str(&delta);
                            ctx.emit(Event::TextDelta { delta: delta.clone() });
                            push_tail(&mut text_tail, &delta);
                            if let Some(sample) = repeated_phrase(&text_tail, TEXT_REPEATS) {
                                // The answer degenerated into the same block over
                                // and over. Keep what came before it, drop the
                                // loop, and end the turn: persisting the
                                // repetition would feed it back on the next
                                // request and make it the model's own habit.
                                text = drop_trailing_repetition(&text, &sample);
                                degenerate = Some(Degenerate::Text(sample));
                                break;
                            }
                        }
                    }
                    Ok(ProviderChunk::Reasoning(delta)) => {
                        if !delta.is_empty() {
                            ctx.emit(Event::ReasoningDelta { delta: delta.clone() });
                            push_tail(&mut reason_tail, &delta);
                            if let Some(sample) = repeated_phrase(&reason_tail, REASONING_REPEATS) {
                                // Reasoning is streamed and then discarded, so
                                // there is nothing to save here - only a turn to
                                // abandon. Re-issuing the identical request is
                                // the cheapest escape: the loop is a sampling
                                // accident, and the alternative is burning the
                                // whole output budget on it.
                                degenerate = Some(Degenerate::Reasoning(sample));
                                break;
                            }
                        }
                    }
                    Ok(ProviderChunk::ToolCallDelta { index, id, name, arguments_delta }) => {
                        if let Some(fragment) = &arguments_delta {
                            arguments_seen.push_str(fragment);
                            if arguments_seen.chars().count() > ARGUMENT_TAIL_CHARS {
                                // A tool call whose arguments grow past this is
                                // not a call any more; it is the same runaway
                                // generation with no text to catch it by.
                                degenerate = Some(Degenerate::Arguments(
                                    arguments_seen.chars().count(),
                                ));
                                break;
                            }
                        }
                        accumulator.apply(index, id, name, arguments_delta);
                    }
                    Ok(ProviderChunk::Usage { input_tokens, output_tokens, cached_tokens }) => {
                        // The prefix cache is the single biggest cost lever in a
                        // long run, and the only way to know it still works is
                        // to read it back from the provider on every call.
                        if let Some(cached) = cached_tokens {
                            let ratio = if input_tokens == 0 {
                                0.0
                            } else {
                                f64::from(cached) * 100.0 / f64::from(input_tokens)
                            };
                            tracing::debug!(
                                input_tokens,
                                cached_tokens = cached,
                                cache_hit_percent = format_args!("{ratio:.1}"),
                                "prompt cache"
                            );
                        }
                        ctx.emit(Event::Usage { input_tokens, output_tokens, cached_tokens });
                        // Recorded as well as announced: the event is live-only,
                        // so this row is what lets the session view answer "what
                        // has this cost" after a reload. Awaited on purpose - a
                        // detached write could land after a session delete and
                        // spuriously resurrect nothing, and the store call is a
                        // single local INSERT.
                        ctx.record_usage(input_tokens, output_tokens, cached_tokens).await;
                        // The usage block is the last thing the provider sends;
                        // do not wait out the rest of the window for it.
                        if usage_window.is_some() {
                            break;
                        }
                    }
                    Ok(ProviderChunk::Finish { reason }) => {
                        // Record it and keep reading: OpenAI-compatible endpoints
                        // send the `usage` block in a chunk *after* the one that
                        // carries `finish_reason`, so breaking here would throw
                        // away the token counts (and the prompt-cache numbers)
                        // of every real call. The window below bounds the wait.
                        finish_reason = Some(reason);
                        usage_window = Some(Instant::now() + USAGE_WINDOW);
                    }
                    Ok(ProviderChunk::Error { message, retryable }) => {
                        let suffix = if retryable { " (retryable)" } else { "" };
                        end = StreamEnd::Failed {
                            message: format!("provider stream error: {message}{suffix}"),
                            // The provider already decided whether this error
                            // kind is worth another attempt (rate limit,
                            // overloaded, 5xx ... versus a rejected request).
                            transient: retryable,
                        };
                        break;
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            end = StreamEnd::Cancelled;
                            break;
                        }
                        // Transport death mid-body: a decode error, a reset
                        // connection, a read timeout. Another attempt can land
                        // on a healthy channel; a configuration error cannot.
                        let transient = matches!(e, Error::Provider(_) | Error::Timeout(_));
                        end = StreamEnd::Failed {
                            message: format!("provider stream failed: {e}"),
                            transient,
                        };
                        break;
                    }
                }
            }
        }
    }

    // A clean EOF without any finish_reason is not success: the stream was cut
    // before the provider told us why it stopped. The body died mid-flight, so
    // it is exactly the failure class a restart is for.
    //
    // `"eof"` is the provider's own name for this case: `stream.rs` synthesises
    // `Finish { reason: "eof" }` when the body ends without a `finish_reason`.
    // It must be treated as incomplete here too, otherwise a proxy that cuts the
    // body at an event boundary produces a `completed` run with silently
    // truncated text persisted as a normal answer.
    let ended_without_reason = finish_reason
        .as_deref()
        .is_none_or(|reason| reason.eq_ignore_ascii_case("eof"));
    if matches!(end, StreamEnd::Complete) && ended_without_reason {
        end = StreamEnd::Failed {
            message: "provider stream ended without a finish_reason; the response is incomplete"
                .into(),
            transient: true,
        };
    }

    match degenerate {
        // Reasoning that loops is worth one more attempt with the same prompt:
        // nothing was persisted, and the alternative is paying for the loop.
        Some(Degenerate::Reasoning(sample)) => {
            ctx.notice(
                "warn",
                format!(
                    "the model's reasoning started repeating `{sample}`; abandoning this attempt"
                ),
            );
            end = StreamEnd::Failed {
                message: format!(
                    "the model's reasoning repeated `{sample}` over and over; the turn was abandoned"
                ),
                transient: true,
            };
        }
        // Text that loops is trimmed above; the rest of the answer stands.
        Some(Degenerate::Text(sample)) => {
            ctx.notice(
                "warn",
                format!("the answer started repeating `{sample}`; the repetition was dropped"),
            );
            end = StreamEnd::Complete;
            finish_reason = finish_reason.or(Some("degenerate".into()));
        }
        // A tool call whose arguments never end is the same runaway.
        Some(Degenerate::Arguments(chars)) => {
            ctx.notice(
                "warn",
                format!(
                    "a tool call's arguments grew past {chars} characters; the turn was abandoned"
                ),
            );
            end = StreamEnd::Failed {
                message: format!(
                    "a tool call's arguments grew past {chars} characters without ending; the turn was abandoned"
                ),
                transient: true,
            };
        }
        None => {}
    }

    (text, accumulator.finish(), end, finish_reason)
}

/// What kind of repetition a stream fell into. Carries the repeated sample so the
/// reason shown to the user names what the model was saying.
#[derive(Debug, Clone)]
enum Degenerate {
    Text(String),
    Reasoning(String),
    Arguments(usize),
}

/// How many times a phrase must repeat before a stream counts as degenerate.
///
/// Different per channel on purpose: reasoning is discarded, so cutting it is
/// free and 8 repetitions is enough to be sure; answer text is content, so the
/// bar is higher (a table or a list may repeat a line several times).
pub(crate) const REASONING_REPEATS: usize = 8;
pub(crate) const TEXT_REPEATS: usize = 10;
/// The phrase length compared. Short enough that a loop is caught in its first
/// few hundred characters, long enough that ordinary repeated words ("the", "and") do
/// not count.
const DEGENERATE_NGRAM: usize = 24;
/// How much of the recent stream is examined.
const DEGENERATE_TAIL_CHARS: usize = 4_096;
/// Tool-call arguments are capped as well: a runaway JSON blob is a loop that
/// carries no text at all.
const ARGUMENT_TAIL_CHARS: usize = 256_000;

fn push_tail(tail: &mut String, delta: &str) {
    tail.push_str(delta);
    if tail.chars().count() > DEGENERATE_TAIL_CHARS {
        let excess = tail.chars().count() - DEGENERATE_TAIL_CHARS;
        let byte_cut = tail
            .char_indices()
            .nth(excess)
            .map(|(at, _)| at)
            .unwrap_or(tail.len());
        tail.drain(..byte_cut);
    }
}

/// The phrase the tail is stuck on, if one repeats often enough.
///
/// Counting a rolling n-gram rather than matching an exact cycle is deliberate:
/// a stuck model varies the whitespace (`Let me write.` / `Let me write.\n\nGo.`),
/// so an exact-period detector sees "different" text every time while a reader
/// sees the same five words forever. A 24-character window that repeats 8+ times
/// inside the last 4k characters is a loop whatever the punctuation does.
pub(crate) fn repeated_phrase(tail: &str, repeats: usize) -> Option<String> {
    let chars: Vec<char> = tail.chars().collect();
    let k = DEGENERATE_NGRAM;
    if chars.len() < k * repeats {
        return None;
    }
    // Rolling hash of every k-gram, so the scan stays linear in the tail.
    const BASE: u64 = 131;
    let mut power: u64 = 1;
    for _ in 1..k {
        power = power.wrapping_mul(BASE);
    }
    let mut hash: u64 = 0;
    for &c in &chars[..k] {
        hash = hash.wrapping_mul(BASE).wrapping_add(c as u64);
    }
    let mut counts: std::collections::HashMap<u64, (usize, usize)> =
        std::collections::HashMap::new();
    let mut best: Option<(usize, usize)> = None; // (count, start of the last hit)
    let mut record = |hash: u64, start: usize| {
        let entry = counts.entry(hash).or_insert((0, start));
        entry.0 += 1;
        entry.1 = start;
        if entry.0 >= repeats && best.map(|(c, _)| entry.0 > c).unwrap_or(true) {
            best = Some((entry.0, start));
        }
    };
    record(hash, 0);
    for start in 1..=chars.len() - k {
        hash = hash
            .wrapping_sub((chars[start - 1] as u64).wrapping_mul(power))
            .wrapping_mul(BASE)
            .wrapping_add(chars[start + k - 1] as u64);
        record(hash, start);
    }
    let (_, at) = best?;
    let candidate: String = chars[at..at + k].iter().collect();
    // Verify against a real repeat (hash collisions would otherwise cut a good
    // answer): the window must appear `repeats` times as an exact substring.
    if tail.matches(&candidate).count() >= repeats {
        Some(condense(&candidate))
    } else {
        None
    }
}

/// Drop the repeated tail from an answer, keeping everything before the loop.
///
/// The sample is whitespace-condensed, so it cannot be matched byte-for-byte
/// against the raw text; a short anchor of it can. Only the tail is searched, so
/// a legitimate sentence earlier in the answer that happens to start the same way
/// is not cut.
fn drop_trailing_repetition(text: &str, sample: &str) -> String {
    // The sample is a fixed-width window, so it usually begins mid-sentence with
    // the punctuation that ended the *previous* one (`. Let me write. Go. Now.`).
    // Anchoring on that punctuation would cut the preceding sentence's full stop
    // along with the loop, turning "Nine findings follow." into "Nine findings
    // follow". Start the anchor at the first word character instead, so the cut
    // lands on the whitespace before the loop and the sentence before it stays
    // intact.
    let anchor: String = sample
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .take(12)
        .collect();
    if anchor.trim().is_empty() {
        return text.to_string();
    }
    const SEARCH_CHARS: usize = 2_048;
    let total = text.chars().count();
    let from_char = total.saturating_sub(SEARCH_CHARS);
    let from_byte = text
        .char_indices()
        .nth(from_char)
        .map(|(byte, _)| byte)
        .unwrap_or(0);
    match text[from_byte..].find(&anchor) {
        Some(at) => text[..from_byte + at].trim_end().to_string(),
        None => text.to_string(),
    }
}

/// One-line form of a sample, for a notice or a retry reason.
fn condense(sample: &str) -> String {
    let cleaned: String = sample
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let squeezed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if squeezed.chars().count() > 60 {
        format!("{}…", squeezed.chars().take(60).collect::<String>())
    } else {
        squeezed
    }
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
