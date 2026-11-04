//! Subagent spawning, delegation tools, concurrency limits and status reporting.
//!
//! The runtime registers a family of delegate tools into the tool view it
//! exposes. Every one of them runs a *nested* agent loop with:
//!
//! * a fresh message history (system prompt marked `is_subagent`, task as the
//!   user turn) persisted with the agent id so it never pollutes the parent
//!   conversation;
//! * iterations bounded by `settings.subagents.max_iterations` per segment and
//!   refreshed up to `settings.subagents.max_continues` times, with wall clock by
//!   `timeout_secs` (plus a hard backstop on top of the in-loop deadline);
//! * a tool set that excludes `spawn_agent` unless recursion is explicitly
//!   enabled *and* the depth budget allows it;
//! * approvals that keep the parent policy but carry the agent id;
//! * a concurrency limit of `settings.subagents.max_concurrent`.
//!
//! A subagent failure is reported as a failed tool result; it never fails the
//! parent run. Cancelling the parent cancels children through a child token.

// @kc-doc: subagents
// @kc-title: Delegate work to subagents
// @kc-summary: How spawn_agent, spawn_agents and the control tools work - blocking and background, sequential and parallel - and the limits they run under.
// @kc-tags: subagents, delegation, agents, parallel, background, spawn_agent, spawn_agents, send_message, long-running
// @kc-applies-to: tool:spawn_agent, tool:spawn_agents, tool:send_message, tool:wait_for_agents, tool:list_subagents, tool:agent_history, tool:pause_agents, tool:resume_agents, tool:kill_agents, tool:report_status, config:[subagents], config:subagents.max_concurrent, config:subagents.allow_background, config:subagents.max_background, config:subagents.wait_timeout_secs, config:subagents.max_iterations, config:subagents.max_continues, config:subagents.timeout_secs, env:KARUCODE_SUBAGENT_CONCURRENCY, env:KARUCODE_SUBAGENT_MAX_ITERATIONS, env:KARUCODE_SUBAGENT_MAX_CONTINUES, env:KARUCODE_SUBAGENT_TIMEOUT_SECS, env:KARUCODE_SUBAGENT_ALLOW_BACKGROUND, env:KARUCODE_SUBAGENT_MAX_BACKGROUND, env:KARUCODE_SUBAGENT_WAIT_TIMEOUT_SECS, env:KARUCODE_ALLOW_RECURSIVE_SUBAGENTS, route:GET /api/sessions/:id, route:GET /api/agents, route:GET /api/agents/:id, route:POST /api/agents/:id/pause, route:POST /api/agents/:id/resume, route:POST /api/agents/:id/cancel, event:agent_spawned, event:agent_progress, event:agent_paused, event:agent_message, event:agent_note, event:agent_finished, ui:AgentsTab
//
// The model can call `spawn_agent` with a self-contained task and a list of
// `owned_paths`. A subagent is a **nested, bounded agent loop**: it gets a fresh
// history (system prompt marked as subagent, the task as the user turn), its own
// run id and agent id, and the same tool set minus the delegate tools unless
// recursion is explicitly allowed.
//
// You do not configure a subagent per task: you write the task, and the agent
// decides. To steer it, ask for delegation explicitly - "use a subagent to map
// the call sites while you fix the parser".
//
// # Sequential and parallel
//
// | Call | Blocks? | Pattern |
// | --- | --- | --- |
// | `spawn_agent` | yes | one job, then use its answer |
// | `spawn_agent` + `background: true` | no | start it, keep working, collect later |
// | `spawn_agents` | yes | a fan-out that runs together and returns one batch |
// | `spawn_agents` + `background: true` | no | start a fan-out and keep working |
//
// The blocking forms are the cheap ones: the parent spends one tool call and gets
// the answer as the result. The background forms cost a second call
// (`wait_for_agents`) but let the parent do something useful meanwhile. A
// fan-out is the case that was previously impossible - three `spawn_agent` calls
// inside one turn execute in order, so they never actually overlapped on
// anything but provider latency.
//
// ```json
// {"name": "spawn_agents", "arguments": {"tasks": [
//   {"task": "map the call sites of parse()", "name": "mapper", "role": "mapper"},
//   {"task": "review the diff for panics", "name": "reviewer", "role": "reviewer"}
// ]}}
// ```
//
// # Talking to a live agent
//
// | Tool | What it does |
// | --- | --- |
// | `send_message` | deliver text to one or more agents (or a parent) as their next user turn |
// | `wait_for_agents` | block until named (or all) background agents settle, bounded by a timeout |
// | `list_subagents` | the live roster: status, iterations, unread mail, last reported note |
// | `agent_history` | one agent's transcript - the audit trail behind its summary |
// | `pause_agents` / `resume_agents` | park an agent between iterations, and let it go |
// | `kill_agents` | stop an agent and everything below it |
// | `report_status` | a subagent says what it is doing, for its parent and the roster |
//
// A message is delivered where a user turn belongs - at the top of the
// recipient's next iteration, before the request is built. It never interrupts
// the turn in flight, for the same reason a user steer does not: aborting a model
// mid-plan throws away work and can persist a tool call with no result. So
// `send_message` returns `queued`, not "they have read it".
//
// Pausing works the same way: the agent finishes its current turn, then parks.
// That is the only point where stopping is free. A paused agent still holds its
// subagent slot, and `kill_agents` still works on it - a paused agent is never
// unkillable.
//
// **Ownership.** An agent may pause, resume, kill and inspect the agents it
// spawned and everything *they* spawned. Messaging is open to everyone, including
// upwards to the parent: a child reporting a blocker is the case this exists for.
// Welcome but worth knowing: a child can therefore inject a turn into its
// parent's conversation. That is by design, and it is why the injected turn is
// labelled with who sent it.
//
// # Limits
//
// ```toml
// [subagents]
// max_concurrent = 3       # loops at once; 0 = no ceiling, KARUCODE_SUBAGENT_CONCURRENCY
// max_background = 8       # live background agents per session; 0 = no ceiling
// allow_background = true  # KARUCODE_SUBAGENT_ALLOW_BACKGROUND
// wait_timeout_secs = 600  # default bound on wait_for_agents
// max_iterations = 200     # turns per segment; KARUCODE_SUBAGENT_MAX_ITERATIONS
// max_continues = 60       # segment refreshes;   KARUCODE_SUBAGENT_MAX_CONTINUES
// allow_recursive = false  # KARUCODE_ALLOW_RECURSIVE_SUBAGENTS
// max_depth = 1            # 0 = no delegation; at most 255
// timeout_secs = 14400     # 4h wall clock;      KARUCODE_SUBAGENT_TIMEOUT_SECS
// ```
//
// Concurrency is enforced by a semaphore, iterations and wall clock per subagent,
// and a hard backstop on top of the in-loop deadline. Cancelling the parent cancels
// children through a child token. A spawn past a finite `max_concurrent` or
// `max_background` is **refused**, never queued: a model that says "go" and then
// sees nothing happen is worse than one that was told "no".
//
// # Unlimited subagents
//
// `max_concurrent = 0` and `max_background = 0` each mean **no ceiling**, which is
// how an install says "get out of the way": every spawn is granted a slot
// immediately, and a `spawn_agents` fan-out starts every child it was given. Both
// are needed for a wide fan-out, because the two limits apply on top of each
// other - a background batch reads `max_background` for its capacity check and
// `max_concurrent` for its slots. `max_depth = 255` removes the practical nesting
// limit (the field is a `u8`, so that is the deepest it can express).
//
// Nothing else has to change: `active` and `peak` are still counted, the roster and
// the stats stay honest, and a spawn is still bounded by `max_depth`,
// `timeout_secs` and the install's wall clock. What stops a runaway is cost, not a
// counter - every live loop is a provider stream and a transcript, so a model that
// fans out two hundred ways bills for two hundred loops. That is the trade the
// setting makes, and it is why the defaults are small.
//
// Zero does not mean the same thing for every field in this section:
// `max_depth = 0` forbids delegation outright and `max_continues = 0` restores the
// one-shot budget, so read each field's own rule rather than one rule for all.
//
// # A subagent runs as long as the task needs
//
// A delegated task is a whole piece of work, so a subagent uses the same
// checkpoint rule as a top-level run: when a segment's `max_iterations` is used
// up, the runtime appends a `[runtime checkpoint]` turn and keeps going, in the
// same transcript, up to `max_continues` refreshes. The effective ceiling is
// about `(max_continues + 1) * max_iterations` model turns (12,200 with the
// defaults), or `timeout_secs`, whichever comes first - whichever way a long
// task ends, it ends because time ran out, not because a counter did.
//
// Set `max_continues = 0` to restore the old one-shot behaviour: the subagent
// stops dead at `max_iterations` and reports what it has. `limits.auto_continue
// = false` is the install-wide off switch and wins over this section.
//
// The subagent is told its real budget in its system prompt (segment size,
// refreshes, wall clock), so it plans against the numbers it actually gets.
//
// # What it inherits and what it does not
//
// Inherits: workspace, approval policy, settings, the session's model pin (the
// call's own `model` argument still overrides just that child). Does not inherit:
// the parent's conversation, its tool results, or its claims. Transcripts are
// persisted with the agent id so the parent chat stays clean; the UI shows
// progress and the final result per agent.
//
// # Watching a subagent
//
// Open the **Agents** tab in the inspector. It lists agents for the whole project
// (not only the open session), so a review fan-out is visible while you work
// somewhere else. The status chips - Running, Paused, Done, Failed, Cancelled,
// Interrupted - carry counts and filter the list; switching to **This session**
// narrows it to the working session. A running agent has pause and stop buttons.
//
// With an unbounded `max_concurrent` a single fan-out can leave hundreds of rows
// here, so the list renders the first 60 - live agents first - and offers **Show
// all**. The count above the list and the filter chips always describe the whole
// set, not the rendered page.
//
// Click an agent to open its drill-in view: the task it was given, its model and
// parent, its last reported note, and its live transcript - assistant text,
// thinking, each tool call with status and output, commands with exit codes, and
// approvals it is waiting on. The transcript keeps streaming while the agent runs
// and is backfilled from SQLite when you open an agent that finished before the
// page loaded.
//
// The same data is available over HTTP: `GET /api/agents?project=<id>`,
// `GET /api/agents/:id`, and `POST /api/agents/:id/pause`, `/resume`, `/cancel`.
// All are documented in `docs/API.md`.
//
// # Failure handling
//
// A subagent failure is a failed tool result for the parent - it never fails the
// parent run. A timeout, a provider error and a cancelled task all come back with
// an explanatory message. A *background* agent has no such result to carry the
// failure, so it notifies and leaves it in its row and its summary.
//
// # Gotchas
//
// * **Git worktrees isolate subagent work** when the workspace is a git repository.
//   A worktree is created for each subagent and cleaned up when it completes,
//   preventing direct file conflicts with the parent and sibling agents. The
//   worktree is detached, so creating and removing one leaves no branch behind in
//   your repository. If the workspace is not a git repository or worktree creation
//   fails, subagents transparently reuse the shared workspace and `owned_paths`
//   provides advisory conflict reporting.
// * Recursion is off by default; enabling it without a depth budget turns one
//   question into an expensive fan-out.
// * Subagent runs cost tokens and time. Delegating a one-file read is slower than
//   reading the file.
// * Tool calls inside a turn execute in order, so a blocking fan-out still issues
//   its children one after another; `spawn_agents` is what makes them overlap.
// * `max_background` caps *live background* agents per session, when it is
//   finite. A blocking spawn is bounded by `max_concurrent` instead, because it
//   holds its parent's turn.
// * Setting `max_concurrent` to `0` does not make spawning free: it removes one
//   counter. `max_depth`, `timeout_secs`, the install's run clock and the bill for
//   every concurrent provider stream still apply.
// * A background agent outlives the turn that started it but not the run: when
//   the run ends its children are cancelled, and a restart marks them
//   `interrupted`.
// * A subagent runs on its session's pinned model, not the install-wide default:
//   pinning a session to a small model makes its subagents cheap too.
// * A subagent that keeps working without progress burns tokens until
//   `timeout_secs`. Keep it pointed at one task; split unrelated work into
//   separate subagents instead of widening one.
// * `report_status` is a claim, not a measurement. Nothing verifies a progress
//   number, so treat it as a hint about what to look at.
//
// Full reference: `docs/internal/subagents.md` (this page) and the
// `subagents` topic; `GET /api/sessions/:id` returns the session's `agents`
// array alongside its messages and runs.

use crate::agent::{LoopOutcome, LoopSpec};
use crate::approvals::RuntimeApprovalGate;
use crate::events::now_ms;
use crate::hub::{AgentMessage, ControlError, LiveAgent, MAX_MAILBOX};
use crate::RuntimeInner;
use async_trait::async_trait;
use futures_util::FutureExt;
use kc_core::error::Result;
use kc_core::events::Event;
use kc_core::ids::AgentId;
use kc_core::message::Message;
use kc_core::store::{AgentRecord, AgentStatus, NewMessage, RunStatus};
use kc_core::tool::{
    truncate_output, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolRegistry, ToolSpec,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Name of the built-in delegate tool.
pub const SPAWN_AGENT_TOOL: &str = "spawn_agent";
/// Fan-out: several independent tasks, started together.
pub const SPAWN_AGENTS_TOOL: &str = "spawn_agents";
/// Deliver a message to a live agent (or to this agent's parent).
pub const SEND_MESSAGE_TOOL: &str = "send_message";
/// Block until background agents settle.
pub const WAIT_FOR_AGENTS_TOOL: &str = "wait_for_agents";
/// The live roster.
pub const LIST_SUBAGENTS_TOOL: &str = "list_subagents";
/// One agent's transcript.
pub const AGENT_HISTORY_TOOL: &str = "agent_history";
pub const PAUSE_AGENTS_TOOL: &str = "pause_agents";
pub const RESUME_AGENTS_TOOL: &str = "resume_agents";
pub const KILL_AGENTS_TOOL: &str = "kill_agents";
/// A subagent reports what it is doing, for its parent and the roster.
pub const REPORT_STATUS_TOOL: &str = "report_status";

/// The delegate tools a child keeps at *every* depth, even with recursion off.
///
/// They are coordination, not multiplication: a child that cannot report status
/// is invisible to its parent, and one that cannot message is unable to say it is
/// blocked - which is the whole reason a hierarchy is worth having.
pub const COORDINATION_TOOLS: &[&str] = &[
    SEND_MESSAGE_TOOL,
    LIST_SUBAGENTS_TOOL,
    AGENT_HISTORY_TOOL,
    REPORT_STATUS_TOOL,
];

/// The delegate tools that only a child allowed to recurse may use: spawning more
/// work, and stopping or parking somebody else's.
pub const DELEGATION_ONLY_TOOLS: &[&str] = &[
    SPAWN_AGENT_TOOL,
    SPAWN_AGENTS_TOOL,
    WAIT_FOR_AGENTS_TOOL,
    PAUSE_AGENTS_TOOL,
    RESUME_AGENTS_TOOL,
    KILL_AGENTS_TOOL,
];

/// Every tool this module installs.
pub const DELEGATE_TOOLS: &[&str] = &[
    SPAWN_AGENT_TOOL,
    SPAWN_AGENTS_TOOL,
    SEND_MESSAGE_TOOL,
    WAIT_FOR_AGENTS_TOOL,
    LIST_SUBAGENTS_TOOL,
    AGENT_HISTORY_TOOL,
    PAUSE_AGENTS_TOOL,
    RESUME_AGENTS_TOOL,
    KILL_AGENTS_TOOL,
    REPORT_STATUS_TOOL,
];

/// How long a timed-out or cancelled subagent's tools get to observe the
/// cancellation token and kill their process groups before the loop future is
/// dropped. Dropping first would only kill the leader process (`kill_on_drop`),
/// leaving children behind.
const SUBAGENT_CANCEL_GRACE: Duration = Duration::from_millis(500);

/// How long a **background** child gets to record a run-end cancellation as
/// `cancelled` before its loop future is dropped and the status is written as
/// `interrupted` instead.
///
/// Longer than [`SUBAGENT_CANCEL_GRACE`] because this grace is on a *run's* exit
/// path rather than inside the child's own cleanup: a tool that honours the token
/// needs time to kill its process group and return, and declaring a live child
/// `interrupted` while it is still unwinding would make the roster lie in the
/// other direction.
pub(crate) const RUN_END_CHILD_GRACE: Duration = Duration::from_millis(1_000);

/// Upper bound for a subagent summary returned to the parent and stored on the
/// agent record.
pub const MAX_SUMMARY_BYTES: usize = 8_000;

/// Upper bound on `agent_history` output, so auditing one agent cannot eat a
/// parent's context in a single call.
const MAX_HISTORY_BYTES: usize = 24_000;

/// Counting limiter for concurrent subagents.
///
/// The limit is read from live settings on every acquire call, so a settings
/// change applies to the next spawn without restarting the runtime.
pub struct SubagentLimiter {
    pub config: kc_core::config::SubagentConfig,
    active: Mutex<usize>,
    peak: AtomicUsize,
    notify: Notify,
}

impl SubagentLimiter {
    pub fn new(config: kc_core::config::SubagentConfig) -> Self {
        Self {
            config,
            active: Mutex::new(0),
            peak: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    pub fn active(&self) -> usize {
        *self.active.lock().expect("subagent limiter")
    }

    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Take a slot only if one is free right now.
    ///
    /// A background spawn must never park its parent's turn, so it either gets a
    /// slot immediately or is refused with an explanation. Waiting would turn a
    /// tool call into a sleep the model cannot see or break out of.
    ///
    /// `max == 0` is the documented "no ceiling": the slot is always granted, and
    /// `active`/`peak` are still counted so the stats stay honest. Note this is
    /// the *only* meaning 0 can have here - a reader who expected 0 to mean
    /// "allow nothing" would turn every spawn into a refusal.
    pub fn try_acquire(self: &Arc<Self>, max: usize) -> Option<SubagentPermit> {
        let mut active = self.active.lock().expect("subagent limiter");
        if !slot_available(*active, max) {
            return None;
        }
        *active += 1;
        self.peak.fetch_max(*active, Ordering::Relaxed);
        Some(SubagentPermit {
            limiter: Arc::clone(self),
        })
    }

    /// Wait until a slot is free. Returns `None` if `cancel` fires first.
    pub async fn acquire(
        self: &Arc<Self>,
        max: usize,
        cancel: &CancellationToken,
    ) -> Option<SubagentPermit> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut active = self.active.lock().expect("subagent limiter");
                if slot_available(*active, max) {
                    *active += 1;
                    self.peak.fetch_max(*active, Ordering::Relaxed);
                    return Some(SubagentPermit {
                        limiter: Arc::clone(self),
                    });
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = notified => {}
            }
        }
    }
}

/// Is a slot free under a ceiling of `max`?
///
/// `0` is "no ceiling", which is why this is a named rule rather than an inline
/// `active < max`: the saturating form would make 0 mean "allow nothing", the
/// exact opposite of what the docs promise.
fn slot_available(active: usize, max: usize) -> bool {
    max == 0 || active < max
}

/// Holds one concurrency slot; released on drop, even on early return.
pub struct SubagentPermit {
    limiter: Arc<SubagentLimiter>,
}

impl Drop for SubagentPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.limiter.active.lock() {
            *active = active.saturating_sub(1);
        }
        self.limiter.notify.notify_waiters();
    }
}

/// Cancel every subagent loop a **top-level run** started, and wait (bounded) for
/// their statuses to be recorded.
///
/// This is the code behind the documented rule (`subagent.rs` module doc,
/// `docs/internal/subagents.md`): *a background agent outlives the turn that
/// started it but not the run*. Every exit path of a top-level run reaches it,
/// because completed, failed, cancelled and panicked all leave `run_top_level`.
///
/// `parent_run_id` is the top-level run's id, which every child it spawned carries
/// as its record's `parent_run_id` - including children of children, since that
/// field is inherited. The whole subtree is therefore reaped by one filter, which
/// is the correct scope: nothing a run started may outlive it.
///
/// A *detached* child is a legitimate feature while the run is alive - it keeps
/// working, the parent can `wait_for_agents` on it, and the composer stays usable
/// because `has_active_run` ignores it. Only the run's end reaps it.
///
/// The status the child records is its own: a loop that honours the token reports
/// `cancelled` (via [`crate::subagent`]'s `Exit::ParentCancelled` arm), and one
/// that ignores it keeps working - it still holds a cancelled token, so
/// [`Runtime::is_running`] already reports the session as idle and the next thing
/// that touches it will see a loop that is unwinding. This function does not
/// overwrite a status another writer owns.
pub(crate) async fn cancel_run_children(inner: &Arc<RuntimeInner>, parent_run_id: &str) {
    let reaped: Vec<(String, Arc<LiveAgent>)> = inner
        .hub
        .list_all()
        .into_iter()
        .filter(|agent| agent.run_id == parent_run_id)
        .map(|agent| (agent.id.0.clone(), agent))
        .collect();
    if reaped.is_empty() {
        return;
    }
    let ids: Vec<String> = reaped.iter().map(|(id, _)| id.clone()).collect();
    let session_id = reaped[0].1.session_id.clone();
    // `kill` marks each one first, so a loop that observes the token immediately
    // still knows a control request stopped it rather than a crash. The status it
    // writes then is `cancelled`, which is the truth: the run's end asked it to
    // stop.
    let killed = inner.hub.kill(&session_id, &ids, "run ended");
    tracing::info!(
        parent_run = %parent_run_id,
        agents = killed.len(),
        "cancelling subagents when the run ended"
    );

    // Wait (bounded, and all at once) for each loop to write its own final
    // status, so a client that reacts to the run's `RunFinished` by refetching the
    // roster already sees `cancelled`. Concurrent because a run may have spawned
    // several agents and a fan-out must cost one grace, not one per child. A loop
    // that ignores the token past the grace is left registered: it will record
    // itself when it unwinds, and inventing a status here would race that write
    // with a worse one.
    let waiting = reaped.into_iter().map(|(_, live)| async move {
        let _ = crate::hub::wait_for_settled(&live, RUN_END_CHILD_GRACE, &CancellationToken::new())
            .await;
    });
    futures_util::future::join_all(waiting).await;
}

/// Install every delegate tool into `registry`.
pub(crate) fn install_delegate_tools(inner: &Arc<RuntimeInner>, registry: &mut ToolRegistry) {
    let weak = Arc::downgrade(inner);
    for tool in [
        Arc::new(SpawnAgentTool {
            inner: weak.clone(),
            fan_out: false,
        }) as Arc<dyn Tool>,
        Arc::new(SpawnAgentTool {
            inner: weak.clone(),
            fan_out: true,
        }),
        Arc::new(SendMessageTool {
            inner: weak.clone(),
        }),
        Arc::new(WaitForAgentsTool {
            inner: weak.clone(),
        }),
        Arc::new(ListSubagentsTool {
            inner: weak.clone(),
        }),
        Arc::new(AgentHistoryTool {
            inner: weak.clone(),
        }),
        Arc::new(PauseAgentsTool {
            inner: weak.clone(),
            resume: false,
        }),
        Arc::new(PauseAgentsTool {
            inner: weak.clone(),
            resume: true,
        }),
        Arc::new(KillAgentsTool {
            inner: weak.clone(),
        }),
        Arc::new(ReportStatusTool { inner: weak }),
    ] {
        registry.register(tool);
    }
}

// ---------------------------------------------------------------------------
// spawn
// ---------------------------------------------------------------------------

/// One task inside a `spawn_agent` or `spawn_agents` call.
#[derive(Debug, Clone)]
struct SpawnRequest {
    task: String,
    name: Option<String>,
    role: String,
    owned_paths: Vec<String>,
    model: Option<String>,
}

impl SpawnRequest {
    /// How a refusal refers to this task: its name if it has one, else its
    /// position in the batch - deliberately never the instruction text. A refusal
    /// becomes a tool result, and echoing the caller's own words back into the
    /// transcript is noise the caller already has.
    fn label(&self, index: usize) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None => format!("task {}", index + 1),
        }
    }
}

/// What the caller resolved out of the tool arguments.
#[derive(Debug, Clone)]
struct SpawnOptions {
    background: bool,
    /// Wait for a background fan-out before returning, so the whole batch is one
    /// result. Only meaningful with `background: true`.
    wait: bool,
}

pub(crate) struct SpawnAgentTool {
    inner: Weak<RuntimeInner>,
    /// `true` for `spawn_agents`, which starts several children at once.
    fan_out: bool,
}

fn spawn_spec(fan_out: bool) -> ToolSpec {
    if fan_out {
        ToolSpec {
            name: SPAWN_AGENTS_TOOL.to_string(),
            description: "Spawn several subagents that work **at the same time** on independent \
                          self-contained tasks in the same workspace, then return their summaries \
                          as one batch. Use this instead of several `spawn_agent` calls when the \
                          tasks do not depend on each other: separate `spawn_agent` calls inside \
                          one turn run one after another. Set `background: true` to return \
                          immediately and collect the results with `wait_for_agents`."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One independent task per subagent. They run concurrently.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "task": {
                                    "type": "string",
                                    "description": "Self-contained instruction: what to inspect or change, and what to report back."
                                },
                                "name": { "type": "string", "description": "Short display name, e.g. `reviewer`." },
                                "role": { "type": "string", "description": "Role label shown in the UI." },
                                "owned_paths": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Workspace paths this subagent intends to modify."
                                },
                                "model": { "type": "string", "description": "Optional model override for this subagent." }
                            },
                            "required": ["task"],
                            "additionalProperties": false
                        }
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Start them and return immediately; collect with `wait_for_agents`."
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "With `background: true`, still wait for the whole batch and return its summaries."
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    } else {
        ToolSpec {
            name: SPAWN_AGENT_TOOL.to_string(),
            description: "Spawn a focused subagent that works on one self-contained task in the \
                          same workspace and reports back a summary. Use it to parallelise \
                          independent investigations or reviews. The subagent gets its own \
                          conversation, cannot see this conversation, and cannot ask you \
                          questions. Pass `background: true` to return immediately and keep \
                          working while it runs (and to run several at once)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Self-contained instruction: what to inspect or change, and what to report back."
                    },
                    "name": { "type": "string", "description": "Short display name, e.g. `reviewer`." },
                    "role": { "type": "string", "description": "Role label shown in the UI, e.g. `reviewer`." },
                    "owned_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace paths this subagent intends to modify; used to warn about overlaps."
                    },
                    "model": { "type": "string", "description": "Optional model override for this subagent." },
                    "background": {
                        "type": "boolean",
                        "description": "Return immediately instead of waiting for the subagent; collect the result with `wait_for_agents`."
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn spec(&self) -> ToolSpec {
        spawn_spec(self.fan_out)
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down; cannot spawn a subagent");
        };
        let result = if self.fan_out {
            run_fan_out(&inner, &request, &ctx).await
        } else {
            run_single_spawn(&inner, &request, &ctx).await
        };
        match result {
            Ok(outcome) => outcome,
            Err(e) => ToolOutcome::err(format!("spawn_agent failed: {e}")),
        }
    }
}

/// Parse one task object out of a `tasks` entry.
fn parse_task(value: &serde_json::Value) -> Result<SpawnRequest> {
    let obj = value.as_object().ok_or_else(|| {
        kc_core::Error::BadRequest("each entry of `tasks` must be an object".into())
    })?;
    let task = obj
        .get("task")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            kc_core::Error::BadRequest("each entry of `tasks` needs a non-empty `task`".into())
        })?
        .to_string();
    let owned_paths: Vec<String> = match obj.get("owned_paths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest("`owned_paths` must be an array of strings".into())
        })?,
    };
    let text = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(SpawnRequest {
        task,
        name: text("name"),
        role: text("role").unwrap_or_else(|| "worker".to_string()),
        owned_paths,
        model: text("model"),
    })
}

fn parse_spawn_options(request: &ToolCallRequest) -> Result<SpawnOptions> {
    let flag = |key: &str| match request.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(kc_core::Error::BadRequest(format!(
            "`{key}` must be a boolean"
        ))),
    };
    Ok(SpawnOptions {
        background: flag("background")?,
        wait: flag("wait")?,
    })
}

async fn run_single_spawn(
    inner: &Arc<RuntimeInner>,
    request: &ToolCallRequest,
    ctx: &ToolCtx,
) -> Result<ToolOutcome> {
    let task = request
        .opt_str("task")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| kc_core::Error::BadRequest("missing non-empty `task` argument".into()))?;
    let role = request
        .opt_str("role")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "worker".to_string());
    let owned_paths: Vec<String> = match request.arguments.get("owned_paths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest("`owned_paths` must be an array of strings".into())
        })?,
    };
    let spawn = SpawnRequest {
        task,
        name: request
            .opt_str("name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        role,
        owned_paths,
        model: request
            .opt_str("model")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };
    let options = parse_spawn_options(request)?;
    spawn_batch(inner, ctx, vec![spawn], &options, false).await
}

async fn run_fan_out(
    inner: &Arc<RuntimeInner>,
    request: &ToolCallRequest,
    ctx: &ToolCtx,
) -> Result<ToolOutcome> {
    let raw =
        request.arguments.get("tasks").cloned().ok_or_else(|| {
            kc_core::Error::BadRequest("`spawn_agents` needs a `tasks` array".into())
        })?;
    let items = raw
        .as_array()
        .ok_or_else(|| kc_core::Error::BadRequest("`tasks` must be an array".into()))?;
    if items.is_empty() {
        return Ok(ToolOutcome::err(
            "`tasks` is empty; call `spawn_agent` for a single task, or name the work to fan out",
        ));
    }
    let mut tasks = Vec::with_capacity(items.len());
    for item in items {
        match parse_task(item) {
            Ok(task) => tasks.push(task),
            Err(e) => return Ok(ToolOutcome::err(format!("invalid `tasks` entry: {e}"))),
        }
    }
    let options = parse_spawn_options(request)?;
    spawn_batch(inner, ctx, tasks, &options, true).await
}

/// Start every task in `tasks`, then either wait for all of them or return their
/// ids.
///
/// The children are started in order but they *run* together: each spawn inserts
/// its live entry and its permit before the batch waits, and the batch waits on
/// the whole set at the end. That is the difference between this and several
/// `spawn_agent` calls, which the loop executes one after the other.
async fn spawn_batch(
    inner: &Arc<RuntimeInner>,
    ctx: &ToolCtx,
    tasks: Vec<SpawnRequest>,
    options: &SpawnOptions,
    batch: bool,
) -> Result<ToolOutcome> {
    let cfg = ctx.settings.subagent_config().await;
    let child_depth = ctx.depth.saturating_add(1);
    if ctx.depth > 0 && !cfg.allow_recursive {
        return Ok(ToolOutcome::err(
            "recursive subagents are disabled (settings.subagents.allow_recursive = false)",
        ));
    }
    if child_depth > cfg.max_depth {
        return Ok(ToolOutcome::err(format!(
            "subagent depth limit reached (settings.subagents.max_depth = {})",
            cfg.max_depth
        )));
    }
    if options.background && !cfg.allow_background {
        return Ok(ToolOutcome::err(
            "background subagents are disabled (settings.subagents.allow_background = false); \
             spawn without `background` and the call waits for the result",
        ));
    }

    let store = inner.deps.store.clone();
    let session_id = ctx.approvals.session_id.clone();

    // Capacity is checked for the whole batch before anything is registered: a
    // fan-out that started half its children and then refused the rest would
    // leave the parent holding ids it did not ask for and cannot easily attribute.
    // `max_background = 0` is "no ceiling", so the check is skipped rather than
    // compared against zero - which would refuse every background spawn.
    let live_background = inner.hub.background_count(&session_id.0);
    if options.background && cfg.max_background > 0 {
        let would_be = live_background.saturating_add(tasks.len());
        if would_be > cfg.max_background {
            return Ok(ToolOutcome::err(format!(
                "refused: {live_background} background agents are already live and this call starts \
                 {} more, over settings.subagents.max_background = {} (wait for one to finish, or \
                 raise the limit)",
                tasks.len(),
                cfg.max_background
            )));
        }
    }

    // The session's model pin decides the child's model; an explicit `model`
    // argument still overrides it for just that child. A missing session is not
    // fatal: fall back to the default model.
    let session_selection = match store.get_session(&session_id.0).await {
        Ok(session) => session.and_then(|session| session.model),
        Err(e) => {
            tracing::warn!(
                session = %session_id.0,
                error = %e,
                "could not read the session model selection; using the default model"
            );
            None
        }
    };
    let base_model = inner
        .deps
        .settings
        .resolve_model(session_selection.as_ref())
        .await;
    let limits = inner.deps.settings.limits().await;
    let policy = inner.deps.settings.approval_policy().await;

    let mut started: Vec<Arc<LiveAgent>> = Vec::with_capacity(tasks.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();

    for (index, spawn) in tasks.iter().enumerate() {
        // A blocking spawn holds its parent's turn, so `max_concurrent` is its
        // real bound and a wait for a slot is honest. A background spawn must
        // never park the parent, so it takes a slot only if one is free right now
        // and is otherwise refused.
        let permit = if options.background {
            match inner.subagents.try_acquire(cfg.max_concurrent) {
                Some(permit) => permit,
                None => {
                    refusals.push(format!(
                        "{}: settings.subagents.max_concurrent = {} subagents are already running",
                        spawn.label(index),
                        cfg.max_concurrent
                    ));
                    continue;
                }
            }
        } else {
            match inner
                .subagents
                .acquire(cfg.max_concurrent, &ctx.cancel)
                .await
            {
                Some(permit) => permit,
                None => {
                    return Ok(ToolOutcome::err(
                        "cancelled while waiting for a free subagent slot",
                    ))
                }
            }
        };

        let mut model = base_model.clone();
        if let Some(override_model) = &spawn.model {
            model.model = override_model.clone();
        }

        match prepare_child(
            inner,
            ctx,
            spawn,
            model,
            &limits,
            &policy,
            child_depth,
            &cfg,
            options.background,
        )
        .await
        {
            Ok(prepared) => {
                warnings.extend(prepared.warnings.clone());
                let live = start_child(
                    inner,
                    prepared,
                    permit,
                    ctx.cancel.child_token(),
                    cfg.clone(),
                )
                .await;
                started.push(live);
            }
            Err(e) => refusals.push(format!("{}: {e}", spawn.label(index))),
        }
    }

    if started.is_empty() {
        return Ok(ToolOutcome {
            ok: false,
            content: format!(
                "no subagent could be started:\n{}",
                refusals
                    .iter()
                    .map(|r| format!("- {r}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            truncated: false,
            meta: Some(serde_json::json!({ "agents": [], "refused": refusals })),
            images: Vec::new(),
        });
    }

    // A blocking call - and a `wait: true` background batch - collects the whole
    // set and answers with their summaries. Otherwise the parent gets the ids and
    // its turn back.
    let collect = !options.background || options.wait;
    if !collect {
        let ids: Vec<String> = started.iter().map(|a| a.id.0.clone()).collect();
        let mut content = format!(
            "started {} background subagent{}:\n",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        );
        for agent in &started {
            content.push_str(&format!(
                "- `{}` {}{}\n",
                agent.id.0,
                agent.name,
                if agent.task.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", first_line(&agent.task))
                }
            ));
        }
        content.push_str(
            "\nThey keep working while you do. Collect them with \
             `wait_for_agents`, check on them with `list_subagents`, talk to them with \
             `send_message`, or stop them with `kill_agents`.",
        );
        if !warnings.is_empty() {
            content.push('\n');
            for warning in &warnings {
                content.push_str(&format!("\n- warning: {warning}"));
            }
        }
        let (content, cut) = truncate_output(&content, MAX_SUMMARY_BYTES);
        return Ok(ToolOutcome {
            ok: true,
            content,
            truncated: cut,
            meta: Some(serde_json::json!({
                "agents": ids,
                "background": true,
                "refused": refusals,
                "warnings": warnings,
            })),
            images: Vec::new(),
        });
    }

    // Wait for the whole set. A parent that is cancelled stops waiting, and the
    // children are cancelled with it by their own token.
    let mut results: Vec<(Arc<LiveAgent>, Option<AgentStatus>)> = Vec::with_capacity(started.len());
    for agent in &started {
        let timeout = Duration::from_secs(cfg.timeout_secs.max(1) + 1);
        let status = crate::hub::wait_for_settled(agent, timeout, &ctx.cancel).await;
        results.push((Arc::clone(agent), status.ok()));
    }

    let mut content = String::new();
    let mut ok = true;
    let mut meta_agents = Vec::new();
    for (agent, status) in &results {
        let record = store.get_agent(&agent.id.0).await.ok().flatten();
        let status_text = status
            .map(|s| s.as_str().to_string())
            .or_else(|| record.as_ref().map(|r| r.agent.status.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let summary = record
            .as_ref()
            .map(|r| r.agent.summary.clone())
            .unwrap_or_default();
        let error = record.as_ref().and_then(|r| r.agent.error.clone());
        let iterations = record.as_ref().map(|r| r.agent.iterations).unwrap_or(0);
        if status_text != AgentStatus::Completed.as_str() {
            ok = false;
        }
        content.push_str(&format!(
            "subagent `{}` ({}, {}) — {status_text}\niterations: {iterations}\n",
            agent.id.0, agent.name, agent.role
        ));
        if let Some(error) = &error {
            content.push_str(&format!("error: {error}\n"));
        }
        content.push('\n');
        content.push_str(if summary.trim().is_empty() {
            "(no summary produced)"
        } else {
            &summary
        });
        content.push_str("\n\n");
        meta_agents.push(serde_json::json!({
            "agent_id": agent.id.0,
            "name": agent.name,
            "status": status_text,
            "iterations": iterations,
            "error": error,
        }));
    }
    if !warnings.is_empty() {
        content.push_str("warnings:\n");
        for warning in &warnings {
            content.push_str(&format!("- {warning}\n"));
        }
    }
    if !refusals.is_empty() {
        content.push_str("refused:\n");
        for refusal in &refusals {
            content.push_str(&format!("- {refusal}\n"));
        }
    }
    let (content, cut) = truncate_output(&content, MAX_SUMMARY_BYTES);
    let _ = batch;
    Ok(ToolOutcome {
        ok: ok && refusals.is_empty(),
        content,
        truncated: cut,
        meta: Some(serde_json::json!({
            "agents": meta_agents,
            "refused": refusals,
            "warnings": warnings,
            "background": false,
        })),
        images: Vec::new(),
    })
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(120).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Everything a child loop needs that does not depend on it being started yet.
struct PreparedChild {
    record: AgentRecord,
    child_depth: u8,
    model: kc_core::config::ModelConfig,
    limits: kc_core::config::Limits,
    approval_mode: String,
    /// The conversation the child starts from (its task, already persisted).
    history: Vec<Message>,
    journal: Arc<crate::rewind::StoreJournal>,
    /// The parent's workspace: a child works in the project it was spawned from,
    /// never anywhere else.
    workspace: Arc<kc_core::paths::Workspace>,
    /// Path-claim warnings, reported with the result.
    warnings: Vec<String>,
    task: String,
}

/// Register an agent and everything around it, without starting its loop.
///
/// Kept apart from [`start_child`] so the blocking and background paths cannot
/// disagree about what a subagent *is*: same claims, same transcript, same depth,
/// same budget. The only difference between them is who awaits the loop.
#[allow(clippy::too_many_arguments)]
async fn prepare_child(
    inner: &Arc<RuntimeInner>,
    ctx: &ToolCtx,
    spawn: &SpawnRequest,
    model: kc_core::config::ModelConfig,
    limits: &kc_core::config::Limits,
    policy: &kc_core::config::ApprovalPolicy,
    child_depth: u8,
    cfg: &kc_core::config::SubagentConfig,
    background: bool,
) -> Result<PreparedChild> {
    let agent_id = AgentId::new();
    let short_id: String = {
        let tail: String = agent_id.0.chars().rev().take(8).collect();
        tail.chars().rev().collect()
    };
    let name = spawn
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-{short_id}", spawn.role));
    let store = inner.deps.store.clone();
    let session_id = ctx.approvals.session_id.clone();
    let parent_run_id = ctx.approvals.run_id.clone();

    let mut record = AgentRecord {
        id: agent_id.0.clone(),
        session_id: session_id.0.clone(),
        parent_run_id: parent_run_id.0.clone(),
        parent_agent_id: ctx.approvals.agent_id.as_ref().map(|a| a.0.clone()),
        name: name.clone(),
        role: spawn.role.clone(),
        task: spawn.task.clone(),
        status: AgentStatus::Running.as_str().to_string(),
        model: model.model.clone(),
        iterations: 0,
        summary: String::new(),
        error: None,
        owned_paths: spawn.owned_paths.clone(),
        created_at: now_ms(),
        updated_at: now_ms(),
        finished_at: None,
        background,
        note: String::new(),
        progress: None,
    };
    store.upsert_agent(&record).await?;
    ctx.emit(Event::AgentSpawned {
        agent_id: agent_id.0.clone(),
        name: name.clone(),
        role: spawn.role.clone(),
        task: spawn.task.clone(),
        model: model.model.clone(),
        parent_run_id: parent_run_id.0.clone(),
        background,
    });

    // --- claims ---------------------------------------------------------------
    let mut warnings: Vec<String> = Vec::new();
    if !spawn.owned_paths.is_empty() {
        // `claim_paths` records the claims and returns the *overlapping* claims
        // held by other agents (already normalised and ancestor-aware), so use
        // that result directly instead of re-deriving conflicts here.
        match store
            .claim_paths(&session_id.0, &agent_id.0, &spawn.owned_paths, "write")
            .await
        {
            Ok(conflicts) => {
                for claim in conflicts {
                    warnings.push(format!(
                        "path `{}` is also claimed by agent {}",
                        claim.path, claim.agent_id
                    ));
                }
            }
            Err(e) => warnings.push(format!("could not record path claims: {e}")),
        }
    }

    // --- child conversation ---------------------------------------------------
    let mut task_message = NewMessage::from_message(
        &session_id.0,
        Some(&parent_run_id.0),
        &Message::user(spawn.task.clone()),
    );
    task_message.agent_id = Some(agent_id.0.clone());
    if let Err(e) = store.append_message(&task_message).await {
        // Claims were recorded before the task message; release them on this
        // early return instead of leaking them for the rest of the session.
        if let Err(release) = store.release_claims(&agent_id.0).await {
            tracing::warn!(agent = %agent_id.0, error = %release, "could not release path claims");
        }
        record.status = AgentStatus::Failed.as_str().to_string();
        record.error = Some(format!("could not persist the subagent task: {e}"));
        record.updated_at = now_ms();
        record.finished_at = Some(now_ms());
        if store.upsert_agent(&record).await.is_ok() {
            ctx.emit(Event::AgentFinished {
                agent_id: agent_id.0.clone(),
                status: AgentStatus::Failed.as_str().to_string(),
                summary: String::new(),
                error: record.error.clone(),
            });
        }
        return Err(e);
    }

    let journal = crate::rewind::StoreJournal::new(
        inner.deps.store.clone(),
        session_id.clone(),
        parent_run_id.clone(),
        Some(agent_id.clone()),
        Arc::clone(&ctx.workspace),
    );

    let _ = cfg;
    Ok(PreparedChild {
        record,
        child_depth,
        model,
        limits: limits.clone(),
        approval_mode: policy.mode.as_str().to_string(),
        history: vec![Message::user(spawn.task.clone())],
        journal,
        workspace: Arc::clone(&ctx.workspace),
        warnings,
        task: spawn.task.clone(),
    })
}

/// Start a prepared child's loop in the background and return its live entry.
///
/// The permit travels into the task so the concurrency slot is held for exactly
/// as long as the loop runs, however it ends. `cfg` is read live by the caller:
/// the limiter's own copy is the construction-time default and would ignore a
/// saved settings change.
async fn start_child(
    inner: &Arc<RuntimeInner>,
    prepared: PreparedChild,
    permit: SubagentPermit,
    cancel: CancellationToken,
    cfg: kc_core::config::SubagentConfig,
) -> Arc<LiveAgent> {
    let PreparedChild {
        record,
        child_depth,
        model,
        limits,
        approval_mode,
        history,
        journal,
        workspace,
        warnings,
        task,
    } = prepared;
    let agent_id = AgentId(record.id.clone());
    let session_id = crate::SessionId(record.session_id.clone());
    let parent_run_id = crate::RunId(record.parent_run_id.clone());

    let child_state = Arc::new(crate::RunState::new(
        parent_run_id.clone(),
        session_id.clone(),
        Some(agent_id.clone()),
        cancel.clone(),
        false,
    ));

    let live = inner.hub.insert(
        &record,
        parent_run_id.0.clone(),
        child_depth,
        cancel.clone(),
    );
    inner
        .runs
        .lock()
        .expect("runs lock")
        .insert(child_state.key(), Arc::clone(&child_state));

    let gate = RuntimeApprovalGate::new(
        Arc::clone(inner),
        session_id.clone(),
        parent_run_id.clone(),
        Some(agent_id.clone()),
        cancel.clone(),
        child_depth,
    );
    let approval_scope = gate.scope();
    let gate: Arc<dyn kc_core::tool::ApprovalGate> = Arc::new(gate);

    let spec = LoopSpec {
        session_id: session_id.clone(),
        run_id: parent_run_id.clone(),
        agent_id: Some(agent_id.clone()),
        depth: child_depth,
        is_subagent: true,
        budget: kc_core::prompt::Budget::for_subagent(&limits, &cfg),
        deadline: Instant::now() + Duration::from_secs(cfg.timeout_secs.max(1)),
        context_max_chars: limits.context_chars_for(&model),
        model,
        limits,
        approval_mode,
        gate,
        approval_scope,
        tools: Arc::new(child_tool_registry(inner, child_depth, &cfg)),
        cancel: cancel.clone(),
        // Inherited from the parent's tool context: a subagent works in the
        // same project as the run that spawned it.
        workspace,
        // A subagent changes the same files as its parent, so its writes go into
        // the same journal - with its own `agent_id`, which is what lets a rewind
        // name the delegated turn a change came from.
        journal: Some(journal),
        history,
        run_state: Some(Arc::clone(&child_state)),
        hub_entry: Some(Arc::clone(&live)),
    };

    let spawned_inner = Arc::clone(inner);
    let spawned_live = Arc::clone(&live);
    let spawned_record = record;
    let warnings_for_task = warnings;
    tokio::spawn(async move {
        run_child_loop(
            spawned_inner,
            spawned_live,
            spawned_record,
            spec,
            _permit_holder(permit),
            warnings_for_task,
            task,
        )
        .await;
    });
    live
}

/// Keep a permit alive for the length of the task without naming it in a
/// signature. A `let _ = permit` inside the async block would be dropped at the
/// first suspension point by an optimising compiler that sees it unused, so the
/// permit is moved into an opaque holder instead.
fn _permit_holder(permit: SubagentPermit) -> SubagentPermit {
    permit
}

/// Drive one child's loop, then persist and report how it ended.
async fn run_child_loop(
    inner: Arc<RuntimeInner>,
    live: Arc<LiveAgent>,
    record: AgentRecord,
    mut spec: LoopSpec,
    _permit: SubagentPermit,
    mut warnings: Vec<String>,
    _task: String,
) {
    let agent_id = live.id.clone();
    let session_id = live.session_id.clone();
    let timeout_secs = {
        let cfg = inner.deps.settings.subagent_config().await;
        cfg.timeout_secs.max(1)
    };

    // Try to create a git worktree for this subagent if the repository supports it.
    // If the workspace is not a git repo or worktree creation fails, we fall back
    // to using the main workspace path.
    let worktree =
        match crate::worktree::WorktreeGuard::try_create(spec.workspace.root(), &agent_id).await {
            Ok(wt) => {
                if wt.is_active() {
                    tracing::debug!(
                        agent_id = %agent_id.0,
                        worktree_path = %wt.path().display(),
                        "subagent using git worktree"
                    );
                }
                wt
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = %agent_id.0,
                    error = %e,
                    "worktree creation error, proceeding with main workspace"
                );
                crate::worktree::WorktreeGuard::try_create(spec.workspace.root(), &agent_id)
                    .await
                    .unwrap_or_else(|_| {
                        // Last resort: create an inactive guard that points to the main workspace
                        crate::worktree::WorktreeGuard::fallback(spec.workspace.root())
                    })
            }
        };

    // Update the spec's workspace to point to the worktree (or main workspace if fallback)
    if worktree.is_active() || worktree.path() != spec.workspace.root() {
        spec.workspace = Arc::new(
            kc_core::paths::Workspace::new(worktree.path())
                .unwrap_or_else(|_| (*spec.workspace).clone()),
        );
    }

    let guarded = std::panic::AssertUnwindSafe(crate::agent::run_loop(Arc::clone(&inner), spec))
        .catch_unwind();
    tokio::pin!(guarded);
    let hard_deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(hard_deadline);

    enum Exit {
        Completed(LoopOutcome),
        TimedOut,
        ParentCancelled,
    }

    let exit = tokio::select! {
        biased;
        _ = live.cancel.cancelled() => Exit::ParentCancelled,
        _ = &mut hard_deadline => Exit::TimedOut,
        result = guarded.as_mut() => Exit::Completed(match result {
            Ok(outcome) => outcome,
            Err(payload) => LoopOutcome {
                status: RunStatus::Failed,
                error: Some(format!(
                    "subagent panicked: {}",
                    crate::agent::panic_message(payload.as_ref())
                )),
                text: String::new(),
                iterations: 0,
            },
        }),
    };

    let timed_out = matches!(exit, Exit::TimedOut);
    let killed = *live.killed.lock().expect("killed lock");
    let outcome = match exit {
        Exit::Completed(outcome) => outcome,
        Exit::TimedOut | Exit::ParentCancelled => {
            // Fire the child token, then keep polling the loop for a short grace
            // period: in-flight tools select on the token, kill their process
            // groups and return. Only then is the future dropped.
            live.cancel.cancel();
            let _ = tokio::time::timeout(SUBAGENT_CANCEL_GRACE, guarded.as_mut()).await;

            let iterations = inner
                .runs
                .lock()
                .expect("runs lock")
                .get(&agent_id.0)
                .map(|state| state.iterations.load(Ordering::Relaxed))
                .unwrap_or(0);
            if timed_out {
                LoopOutcome {
                    status: RunStatus::Failed,
                    error: Some(format!(
                        "subagent timed out after {timeout_secs}s (settings.subagents.timeout_secs)"
                    )),
                    text: String::new(),
                    iterations,
                }
            } else {
                LoopOutcome {
                    status: RunStatus::Cancelled,
                    error: None,
                    text: String::new(),
                    iterations,
                }
            }
        }
    };
    inner.runs.lock().expect("runs lock").remove(&agent_id.0);

    // --- finish ---------------------------------------------------------------
    // The worktree guard is dropped here, cleaning up the worktree if it was active.
    drop(worktree);

    let (summary_raw, _) = truncate_output(&outcome.text, MAX_SUMMARY_BYTES);
    let summary = if summary_raw.trim().is_empty() {
        outcome.error.clone().unwrap_or_default()
    } else {
        summary_raw
    };
    let agent_status = match outcome.status {
        RunStatus::Completed => AgentStatus::Completed,
        // A kill is a cancellation the parent asked for, so say so plainly rather
        // than reporting a generic stop.
        RunStatus::Cancelled => AgentStatus::Cancelled,
        RunStatus::Failed => AgentStatus::Failed,
        RunStatus::LimitReached => AgentStatus::Failed,
        RunStatus::Interrupted => AgentStatus::Interrupted,
        RunStatus::Running => AgentStatus::Running,
    };
    let error = match outcome.status {
        RunStatus::LimitReached => Some(
            outcome
                .error
                .clone()
                .unwrap_or_else(|| "subagent stopped at its iteration or time limit".to_string()),
        ),
        _ => outcome.error.clone(),
    };

    let store = inner.deps.store.clone();
    let mut final_record = record;
    final_record.status = agent_status.as_str().to_string();
    final_record.iterations = outcome.iterations;
    final_record.summary = summary.clone();
    final_record.error = error.clone();
    final_record.note = live.note.lock().expect("note lock").clone();
    final_record.progress = *live.progress.lock().expect("progress lock");
    final_record.updated_at = now_ms();
    final_record.finished_at = Some(now_ms());
    if let Err(e) = store.upsert_agent(&final_record).await {
        warnings.push(format!("could not update the agent record: {e}"));
    }
    if let Err(e) = store.release_claims(&agent_id.0).await {
        warnings.push(format!("could not release path claims: {e}"));
    }
    for warning in &warnings {
        tracing::warn!(agent = %agent_id.0, warning, "subagent finished with a warning");
    }

    let ctx = crate::events::EmitCtx::new(
        Arc::clone(&inner),
        session_id.clone(),
        live.run_id.clone(),
        Some(agent_id.0.clone()),
    );
    ctx.emit(Event::AgentFinished {
        agent_id: agent_id.0.clone(),
        status: agent_status.as_str().to_string(),
        summary: summary.clone(),
        error: error.clone(),
    });

    // Mark the live entry settled before removing it: a `wait_for_agents` that is
    // parked on it right now must observe the real status, not "unknown agent".
    crate::hub::AgentHub::mark_settled(&live, agent_status);
    inner.hub.finish(&agent_id.0, agent_status);

    // Only a failure is worth interrupting a human for: a subagent that succeeded
    // is reported by its parent run's own completion. A *background* agent is the
    // exception - nobody is waiting on the tool result that used to carry the
    // failure, so the failure itself has to be the signal.
    if agent_status == AgentStatus::Failed || agent_status == AgentStatus::Interrupted {
        crate::notify::agent_failed(
            &inner,
            &session_id,
            &ctx.sink(),
            &agent_id.0,
            &final_record.name,
            error.as_deref(),
        )
        .await;
    }
    let _ = killed;
}

/// Parent tool set minus the delegate tools, unless recursion is allowed at this
/// depth (in which case they stay available).
fn child_tool_registry(
    inner: &Arc<RuntimeInner>,
    depth: u8,
    cfg: &kc_core::config::SubagentConfig,
) -> ToolRegistry {
    let can_delegate = cfg.allow_recursive && depth < cfg.max_depth;
    let parent = inner.tool_view();
    let mut registry = ToolRegistry::new();
    for name in parent.names() {
        if !can_delegate && DELEGATION_ONLY_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if let Some(tool) = parent.get(&name) {
            registry.register(tool);
        }
    }
    // `send_message`, `report_status`, `list_subagents` and `agent_history` stay
    // available at every depth, even with recursion off: a child that can only
    // work cannot report that it is blocked, and a parent cannot steer a child it
    // cannot see. What recursion buys is the ability to spawn and to *control*
    // agents - the tools that can multiply work or stop somebody else's.
    for name in COORDINATION_TOOLS {
        if registry.get(name).is_none() {
            let mut fresh = ToolRegistry::new();
            install_delegate_tools(inner, &mut fresh);
            if let Some(tool) = fresh.get(name) {
                registry.register(tool);
            }
        }
    }
    if can_delegate {
        // The parent's view already carries these; if it somehow does not (an
        // embedder that installed its own registry), install ours so a recursive
        // child can still delegate.
        for name in DELEGATION_ONLY_TOOLS {
            if registry.get(name).is_none() {
                let mut fresh = ToolRegistry::new();
                install_delegate_tools(inner, &mut fresh);
                if let Some(tool) = fresh.get(name) {
                    registry.register(tool);
                }
            }
        }
    }
    registry
}

// ---------------------------------------------------------------------------
// control tools
// ---------------------------------------------------------------------------

/// Resolve which live agents a control call addresses.
///
/// `agents` names ids explicitly; an empty list means "everything I started",
/// which is the subtree below the caller (or every live agent in the session for
/// a top-level run). Ids the caller does not own are refused with
/// [`ControlError::NotOwned`] rather than silently skipped.
fn resolve_targets(
    inner: &Arc<RuntimeInner>,
    session_id: &str,
    caller: Option<&str>,
    requested: &[String],
) -> std::result::Result<Vec<Arc<LiveAgent>>, ControlError> {
    if requested.is_empty() {
        return Ok(inner.hub.descendants_of(session_id, caller));
    }
    let owned: std::collections::HashSet<String> = inner
        .hub
        .descendants_of(session_id, caller)
        .iter()
        .map(|agent| agent.id.0.clone())
        .collect();
    let mut out = Vec::with_capacity(requested.len());
    for id in requested {
        let live = inner
            .hub
            .get(id)
            .ok_or_else(|| ControlError::Unknown(id.clone()))?;
        if !owned.contains(id) {
            return Err(ControlError::NotOwned {
                agent: id.clone(),
                owner: live
                    .parent_agent_id
                    .clone()
                    .map(|parent| format!("agent `{parent}`"))
                    .unwrap_or_else(|| "the main agent".to_string()),
            });
        }
        out.push(live);
    }
    Ok(out)
}

fn parse_agent_ids(request: &ToolCallRequest, key: &str) -> Result<Vec<String>> {
    match request.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest(format!("`{key}` must be an array of strings"))
        }),
    }
}

fn caller_id(ctx: &ToolCtx) -> Option<String> {
    ctx.approvals.agent_id.as_ref().map(|id| id.0.clone())
}

fn caller_label(ctx: &ToolCtx) -> String {
    match &ctx.approvals.agent_id {
        Some(id) => format!("agent `{}`", id.0),
        None => "the main agent".to_string(),
    }
}

pub(crate) struct SendMessageTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for SendMessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: SEND_MESSAGE_TOOL.to_string(),
            description: "Deliver a message to one or more other agents - a subagent you spawned, \
                          a sibling, or your own parent - as their next turn. It does not interrupt \
                          them mid-turn: it is queued and they read it at the top of their next \
                          iteration. Omit `agents` to message your parent."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "What to say. Be specific: it becomes a user turn in their conversation."
                    },
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids to message. Omit (or leave empty) to message your parent."
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let message = match request
            .opt_str("message")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(message) => message.to_string(),
            None => return ToolOutcome::err("`message` may not be empty"),
        };
        let targets = match parse_agent_ids(&request, "agents") {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let from = caller_id(&ctx);
        let from_name = match &from {
            Some(id) => inner
                .hub
                .get(id)
                .map(|live| live.name.clone())
                .unwrap_or_else(|| id.clone()),
            None => "the main agent".to_string(),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        let envelope = |content: String| AgentMessage {
            from: from.clone(),
            from_name: from_name.clone(),
            content,
            sent_at: now_ms(),
        };

        let mut lines = Vec::new();
        let mut delivered = 0usize;

        if targets.is_empty() {
            // No explicit target means "my parent". For a top-level run that is
            // nobody, which is a mistake worth naming rather than silently
            // accepting.
            let parent = from.as_deref().and_then(|id| {
                inner
                    .hub
                    .get(id)
                    .and_then(|live| live.parent_agent_id.clone())
            });
            match parent {
                Some(parent) => match inner.hub.deliver(&parent, envelope(message.clone())) {
                    Some(depth) if depth <= MAX_MAILBOX => {
                        delivered += 1;
                        ctx.emit(Event::AgentMessage {
                            from_agent_id: from.clone(),
                            to: parent.clone(),
                            state: "queued".to_string(),
                        });
                        lines.push(format!(
                            "queued for your parent `{parent}` ({depth} per iteration)"
                        ));
                    }
                    _ => lines.push(format!(
                        "could not deliver to `{parent}`: that agent is no longer running"
                    )),
                },
                None => {
                    // Not spawned by an agent, so the parent is the session's run.
                    let depth = inner
                        .hub
                        .deliver_to_run(&session_id, envelope(message.clone()));
                    if depth >= MAX_MAILBOX {
                        lines.push(format!(
                            "the parent's mailbox is full ({MAX_MAILBOX} messages); it will drain on its next turn"
                        ));
                    } else {
                        delivered += 1;
                        ctx.emit(Event::AgentMessage {
                            from_agent_id: from.clone(),
                            to: format!("run:{}", ctx.approvals.run_id.0),
                            state: "queued".to_string(),
                        });
                        lines.push("queued for the main agent's run".to_string());
                    }
                }
            }
        } else {
            for target in &targets {
                match inner.hub.deliver(target, envelope(message.clone())) {
                    Some(depth) if depth <= MAX_MAILBOX => {
                        delivered += 1;
                        ctx.emit(Event::AgentMessage {
                            from_agent_id: from.clone(),
                            to: target.clone(),
                            state: "queued".to_string(),
                        });
                        lines.push(format!("queued for `{target}`"));
                    }
                    Some(_) => lines.push(format!(
                        "`{target}` already has {MAX_MAILBOX} unread messages; nothing was queued"
                    )),
                    None => lines.push(format!("`{target}` is not a live agent any more")),
                }
            }
        }

        let content = format!(
            "delivered to {delivered} agent{}:\n{}",
            if delivered == 1 { "" } else { "s" },
            lines.join("\n")
        );
        ToolOutcome {
            ok: delivered > 0,
            content,
            truncated: false,
            meta: Some(serde_json::json!({ "delivered": delivered })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct WaitForAgentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for WaitForAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: WAIT_FOR_AGENTS_TOOL.to_string(),
            description: "Wait until background subagents finish, then return their summaries. \
                          Omit `agents` to wait for every subagent you started, including ones \
                          they started. Bounded by `timeout_secs`: a wait that runs out reports \
                          which agents are still running instead of hanging."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids to wait for. Omit to wait for everything you started."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "How long to wait at most. Defaults to subagents.wait_timeout_secs."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let cfg = ctx.settings.subagent_config().await;
        let timeout = Duration::from_secs(
            request
                .opt_u64("timeout_secs")
                .unwrap_or(cfg.wait_timeout_secs)
                .clamp(1, cfg.timeout_secs.max(1) + 1),
        );

        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.message()),
        };
        if targets.is_empty() {
            // A named id that already finished is not "unknown": the answer the
            // caller wants is the status it settled with, which the hub kept for
            // exactly this race.
            let mut already: Vec<String> = requested
                .iter()
                .filter_map(|id| {
                    inner
                        .hub
                        .settled_status(id)
                        .map(|status| format!("`{id}` — {}", status.as_str()))
                })
                .collect();
            already.sort();
            if already.is_empty() {
                return ToolOutcome::ok(format!(
                    "{} has no live subagents to wait for",
                    caller_label(&ctx)
                ));
            }
            return ToolOutcome::ok(format!(
                "nothing to wait for; already finished:\n{}",
                already.join("\n")
            ));
        }

        let deadline = Instant::now() + timeout;
        let mut finished: Vec<(String, AgentStatus)> = Vec::new();
        let mut still_running: Vec<String> = Vec::new();
        for agent in &targets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let status = crate::hub::wait_for_settled(agent, remaining, &ctx.cancel).await;
            match status {
                Ok(status) => finished.push((agent.id.0.clone(), status)),
                Err(Some(status)) => finished.push((agent.id.0.clone(), status)),
                Err(None) => still_running.push(agent.id.0.clone()),
            }
            if ctx.cancel.is_cancelled() {
                break;
            }
        }

        let store = inner.deps.store.clone();
        let mut content = String::new();
        if !finished.is_empty() {
            content.push_str(&format!(
                "{} agent{} finished:\n",
                finished.len(),
                if finished.len() == 1 { "" } else { "s" }
            ));
            for (id, status) in &finished {
                let record = store.get_agent(id).await.ok().flatten();
                content.push_str(&format!("\n`{id}` — {}\n", status.as_str()));
                if let Some(record) = record {
                    if let Some(error) = &record.agent.error {
                        content.push_str(&format!("error: {error}\n"));
                    }
                    content.push_str(if record.agent.summary.trim().is_empty() {
                        "(no summary produced)\n"
                    } else {
                        &record.agent.summary
                    });
                    content.push('\n');
                }
            }
        }
        if !still_running.is_empty() {
            content.push_str(&format!(
                "\nstill running after {}s: {}\n\nThey keep working; wait again, or message them.",
                timeout.as_secs(),
                still_running.join(", ")
            ));
        }
        let (content, cut) = truncate_output(&content, MAX_SUMMARY_BYTES);
        ToolOutcome {
            ok: still_running.is_empty(),
            content,
            truncated: cut,
            meta: Some(serde_json::json!({
                "finished": finished
                    .iter()
                    .map(|(id, status)| serde_json::json!({ "agent_id": id, "status": status.as_str() }))
                    .collect::<Vec<_>>(),
                "still_running": still_running,
            })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct ListSubagentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for ListSubagentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: LIST_SUBAGENTS_TOOL.to_string(),
            description: "List the live subagents you can see - their ids, status, iterations, \
                          unread messages and last reported note - plus the ones that already \
                          finished. Use it to decide whether to wait, message, or stop something."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "all": {
                        "type": "boolean",
                        "description": "Include agents in this session that you did not start."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let all = request.opt_bool("all").unwrap_or(false);
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let live = if all {
            inner.hub.list(&session_id)
        } else {
            inner.hub.descendants_of(&session_id, caller.as_deref())
        };

        let mut content = String::new();
        let mut meta = Vec::new();
        if live.is_empty() {
            content.push_str("no live subagents");
        } else {
            content.push_str(&format!("{} live subagent(s):\n", live.len()));
            for agent in &live {
                let paused = agent.is_paused();
                let status = if paused { "paused" } else { "running" };
                let note = agent.note.lock().expect("note lock").clone();
                let progress = *agent.progress.lock().expect("progress lock");
                content.push_str(&format!(
                    "\n- `{}` {} ({}) — {status}, depth {}, {}, {} unread",
                    agent.id.0,
                    agent.name,
                    agent.role,
                    agent.depth,
                    if agent.model.is_empty() {
                        "default model".to_string()
                    } else {
                        agent.model.clone()
                    },
                    agent.queued_mail()
                ));
                if let Some(progress) = progress {
                    content.push_str(&format!(", {:.0}% according to itself", progress * 100.0));
                }
                content.push('\n');
                content.push_str(&format!("  task: {}\n", first_line(&agent.task)));
                if !note.trim().is_empty() {
                    content.push_str(&format!("  says: {note}\n"));
                }
                meta.push(serde_json::json!({
                    "agent_id": agent.id.0,
                    "name": agent.name,
                    "role": agent.role,
                    "status": status,
                    "model": agent.model,
                    "depth": agent.depth,
                    "background": agent.background,
                    "unread": agent.queued_mail(),
                    "note": note,
                    "progress": progress,
                    "parent_agent_id": agent.parent_agent_id,
                }));
            }
        }

        // Recently finished agents are half of what a caller wants to know
        // ("did the one I started an hour ago ever finish?"), and they are cheap:
        // the rows are already there.
        if let Ok(agents) = inner.deps.store.list_agents(&session_id).await {
            let mut finished: Vec<&AgentRecord> = agents
                .iter()
                .filter(|agent| !agent.is_live())
                .filter(|agent| all || caller.is_none() || agent.parent_agent_id == caller)
                .collect();
            finished.sort_by_key(|agent| std::cmp::Reverse(agent.updated_at));
            finished.truncate(10);
            if !finished.is_empty() {
                content.push_str("\n\nrecently finished:\n");
                for agent in finished {
                    content.push_str(&format!(
                        "- `{}` {} — {}{}\n",
                        agent.id,
                        agent.name,
                        agent.status,
                        if agent.error.is_some() {
                            " (with an error)"
                        } else {
                            ""
                        }
                    ));
                }
            }
        }

        let (content, cut) = truncate_output(&content, MAX_SUMMARY_BYTES);
        ToolOutcome {
            ok: true,
            content,
            truncated: cut,
            meta: Some(serde_json::json!({ "agents": meta })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct AgentHistoryTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for AgentHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: AGENT_HISTORY_TOOL.to_string(),
            description: "Read one agent's transcript: its task, what it did, the tools it called \
                          and the result it produced. This is the audit trail behind a summary."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "The agent whose transcript to read." },
                    "before": { "type": "integer", "description": "Exclusive message id cursor for older messages." },
                    "messages": {
                        "type": "integer",
                        "description": "How many of the newest messages to read (default 60)."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let Some(agent_id) = request
            .opt_str("agent_id")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return ToolOutcome::err("`agent_id` is required");
        };
        // The roster is per session, so reading is too: an id from another
        // session would otherwise be a way to read work this agent cannot see.
        match inner.deps.store.get_agent(agent_id).await {
            Ok(Some(agent)) if agent.agent.session_id == ctx.approvals.session_id.0 => {}
            Ok(Some(_)) => {
                return ToolOutcome::err(format!(
                    "agent `{agent_id}` belongs to another session; only this session's agents are visible"
                ))
            }
            Ok(None) => return ToolOutcome::err(format!("no agent `{agent_id}` exists")),
            Err(e) => return ToolOutcome::err(format!("could not look up `{agent_id}`: {e}")),
        }
        let limit = request.opt_u64("messages").unwrap_or(60).clamp(1, 500) as usize;
        let before = match request.arguments.get("before") {
            None => None,
            Some(value) => match value.as_i64().filter(|id| *id > 0) {
                Some(id) => Some(id),
                None => return ToolOutcome::err("`before` must be a positive message id"),
            },
        };
        let page = match inner
            .deps
            .store
            .list_message_page(
                &ctx.approvals.session_id.0,
                Some(agent_id),
                before,
                limit,
                MAX_HISTORY_BYTES,
            )
            .await
        {
            Ok(page) => page,
            Err(e) => return ToolOutcome::err(format!("could not read the transcript: {e}")),
        };
        let messages = page.messages;
        if messages.is_empty() {
            return ToolOutcome::ok(format!(
                "agent `{agent_id}` has no stored transcript on this page"
            ));
        }
        // Put pagination information first: the runtime may truncate tool output
        // again to a smaller configured limit before the model sees it.
        let header = if page.has_more {
            format!(
                "Older messages available: call agent_history with agent_id={agent_id:?} and before={}.\n\n",
                messages[0].id
            )
        } else {
            "No older messages on this transcript page.\n\n".to_string()
        };
        let mut body = String::new();
        for message in &messages {
            let role = message.role.as_str();
            body.push_str(&format!("[{role}]\n{}\n\n", message.content));
            for call in &message.tool_calls {
                body.push_str(&format!("  -> {}({})\n", call.name, call.arguments));
            }
        }
        let (body, cut) = truncate_output(&body, MAX_HISTORY_BYTES.saturating_sub(header.len()));
        let content = format!("{header}{body}");
        ToolOutcome {
            ok: true,
            content,
            truncated: cut || page.has_more,
            meta: Some(serde_json::json!({
                "agent_id": agent_id,
                "messages": messages.len(),
                "messages_truncated": page.has_more,
                "messages_before": if page.has_more { messages.first().map(|m| m.id) } else { None },
            })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct PauseAgentsTool {
    inner: Weak<RuntimeInner>,
    /// `true` for `resume_agents`; the two share every argument and every check.
    resume: bool,
}

#[async_trait]
impl Tool for PauseAgentsTool {
    fn spec(&self) -> ToolSpec {
        let (name, verb, description) = if self.resume {
            (
                RESUME_AGENTS_TOOL,
                "Resume",
                "Let paused subagents continue. They pick up at the next iteration, with their \
                 conversation intact.",
            )
        } else {
            (
                PAUSE_AGENTS_TOOL,
                "Pause",
                "Pause subagents so they stop starting new turns. The turn in flight finishes \
                 first - that is the only point where stopping is free - and the agent then parks \
                 until you resume it, message it, or kill it. A paused agent still holds its slot.",
            )
        };
        ToolSpec {
            name: name.to_string(),
            description: format!(
                "{verb} subagents. {description} Omit `agents` to apply to every subagent you \
                 started, including ones they started."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids. Omit to apply to everything you started."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.message()),
        };
        let verb = if self.resume { "resumed" } else { "paused" };
        if targets.is_empty() {
            return ToolOutcome::ok(format!(
                "nothing to {verb}: {} has no live subagents",
                caller_label(&ctx)
            ));
        }
        let by = caller
            .clone()
            .unwrap_or_else(|| "the main agent".to_string());
        let mut lines = Vec::new();
        for agent in &targets {
            let result = if self.resume {
                inner.hub.resume(&agent.id.0, &by)
            } else {
                inner.hub.pause(&agent.id.0, &by)
            };
            match result {
                Ok(status) => {
                    ctx.emit(Event::AgentPaused {
                        agent_id: agent.id.0.clone(),
                        paused: !self.resume,
                        by: by.clone(),
                    });
                    if let Err(e) = mark_paused(&inner, agent, !self.resume, status).await {
                        lines.push(format!("`{}`: {verb} but {e}", agent.id.0));
                    } else {
                        lines.push(format!("`{}` {verb}", agent.id.0));
                    }
                }
                Err(e) => lines.push(format!("`{}`: {}", agent.id.0, e.message())),
            }
        }
        ToolOutcome {
            ok: true,
            content: lines.join("\n"),
            truncated: false,
            meta: Some(serde_json::json!({ "agents": targets.len(), "resumed": self.resume })),
            images: Vec::new(),
        }
    }
}

/// Persist the visible half of a pause: the row's status, so a reload shows what
/// the roster showed live. A failure is reported to the caller but does not undo
/// the pause - the loop really is parked either way.
async fn mark_paused(
    inner: &Arc<RuntimeInner>,
    agent: &Arc<LiveAgent>,
    paused: bool,
    status: AgentStatus,
) -> Result<()> {
    let store = inner.deps.store.clone();
    let mut record = match store.get_agent(&agent.id.0).await? {
        Some(summary) => summary.agent,
        None => return Ok(()),
    };
    record.status = if paused {
        AgentStatus::Paused.as_str().to_string()
    } else {
        // A resumed agent that was killed in between stays cancelled; anything
        // else is running again.
        match status {
            AgentStatus::Cancelled => AgentStatus::Cancelled.as_str().to_string(),
            _ => AgentStatus::Running.as_str().to_string(),
        }
    };
    record.updated_at = now_ms();
    store.upsert_agent(&record).await
}

pub(crate) struct KillAgentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for KillAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: KILL_AGENTS_TOOL.to_string(),
            description: "Stop subagents and everything they started. Their work is not lost: the \
                          transcript and any summary produced so far stay readable, and their \
                          status becomes `cancelled`. Omit `agents` to stop every subagent you \
                          started."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids. Omit to stop everything you started."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.message()),
        };
        if targets.is_empty() {
            return ToolOutcome::ok(format!(
                "nothing to kill: {} has no live subagents",
                caller_label(&ctx)
            ));
        }
        // Include each target's own subtree: killing a parent that spawned others
        // must not orphan children that outlive it with nobody to collect them.
        let mut ids: Vec<String> = Vec::new();
        for agent in &targets {
            ids.push(agent.id.0.clone());
            for child in inner.hub.descendants_of(&session_id, Some(&agent.id.0)) {
                ids.push(child.id.0.clone());
            }
        }
        ids.sort();
        ids.dedup();
        let by = caller
            .clone()
            .unwrap_or_else(|| "the main agent".to_string());
        let killed = inner.hub.kill(&session_id, &ids, &by);
        ToolOutcome {
            ok: !killed.is_empty(),
            content: format!(
                "asked {} agent{} to stop: {}\n\nThey finish unwinding in a moment; their status \
                 becomes `cancelled` and their transcripts stay readable.",
                killed.len(),
                if killed.len() == 1 { "" } else { "s" },
                killed.join(", ")
            ),
            truncated: false,
            meta: Some(serde_json::json!({ "killed": killed })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct ReportStatusTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for ReportStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: REPORT_STATUS_TOOL.to_string(),
            description:
                "Say what you are doing and how far along you are, so your parent and the \
                          Agents view can see it without reading your whole transcript. Advisory: \
                          nothing verifies it, so report progress honestly or omit it."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note": {
                        "type": "string",
                        "description": "One line: what you are on now, or what is blocking you."
                    },
                    "progress": {
                        "type": "number",
                        "description": "Fraction complete, 0..1. Omit when you cannot honestly say."
                    }
                },
                "required": ["note"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let Some(agent_id) = caller_id(&ctx) else {
            return ToolOutcome::err(
                "`report_status` is for a subagent; the main agent reports by answering",
            );
        };
        let Some(note) = request
            .opt_str("note")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return ToolOutcome::err("`note` may not be empty");
        };
        let note = note.to_string();
        let progress = match request.arguments.get("progress") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => match value.as_f64() {
                Some(value) if (0.0..=1.0).contains(&value) => Some(value as f32),
                _ => {
                    return ToolOutcome::err("`progress` must be a number between 0 and 1");
                }
            },
        };

        inner.hub.note(&agent_id, note.clone(), progress);

        // Persist it too: the roster is live, but a note is exactly the kind of
        // thing a human wants to find later on a finished agent's row.
        if let Ok(Some(summary)) = inner.deps.store.get_agent(&agent_id).await {
            let mut record = summary.agent;
            record.note = note.clone();
            record.progress = progress;
            record.updated_at = now_ms();
            if let Err(e) = inner.deps.store.upsert_agent(&record).await {
                tracing::warn!(agent = %agent_id, error = %e, "could not persist a status note");
            }
        }

        ctx.emit(Event::AgentNote {
            agent_id: agent_id.clone(),
            note: note.clone(),
            progress,
        });

        ToolOutcome {
            ok: true,
            content: match progress {
                Some(progress) => {
                    format!("recorded: {note} ({:.0}% complete)", progress * 100.0)
                }
                None => format!("recorded: {note}"),
            },
            truncated: false,
            meta: Some(serde_json::json!({ "note": note, "progress": progress })),
            images: Vec::new(),
        }
    }
}

/// Convenience for embedders that build their own registry: install the
/// runtime built-ins into `registry`.
pub fn install_builtins(registry: &mut ToolRegistry, runtime: &Arc<crate::Runtime>) {
    install_delegate_tools(runtime.shared(), registry);
}

/// The wall-clock budget of one child, shared with the roster so a stale agent is
/// visible rather than silent.
pub fn child_timeout_secs(cfg: &kc_core::config::SubagentConfig) -> u64 {
    cfg.timeout_secs.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_counts_and_releases() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let a = limiter.acquire(1, &cancel).await.unwrap();
            assert_eq!(limiter.active(), 1);
            assert_eq!(limiter.peak(), 1);
            let blocked = Arc::clone(&limiter);
            let c2 = cancel.clone();
            let waiter = tokio::spawn(async move {
                let permit = blocked.acquire(1, &c2).await;
                permit.is_some()
            });
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!waiter.is_finished());
            drop(a);
            assert_eq!(limiter.active(), 0);
            assert!(waiter.await.unwrap());
            assert_eq!(limiter.peak(), 1);
        });
    }

    #[test]
    fn limiter_gives_up_on_cancel() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let _held = limiter.acquire(1, &cancel).await.unwrap();
            let c2 = cancel.clone();
            let blocked = Arc::clone(&limiter);
            let waiter = tokio::spawn(async move { blocked.acquire(1, &c2).await.is_some() });
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
            assert!(!waiter.await.unwrap());
        });
    }

    #[test]
    fn try_acquire_never_waits() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let first = limiter.try_acquire(1).expect("a free slot");
        assert!(
            limiter.try_acquire(1).is_none(),
            "a background spawn must be refused, not parked"
        );
        drop(first);
        assert!(limiter.try_acquire(1).is_some());
    }

    /// `0` is the "no ceiling" spelling, and the two paths that read it have to
    /// agree: a background spawn (which must never park its parent) and a
    /// blocking one (which may). Reading 0 as "allow nothing" here would turn the
    /// widest setting an operator can choose into a harness that refuses every
    /// spawn - the exact inverse of the documented contract.
    #[test]
    fn a_zero_ceiling_grants_every_slot() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let mut permits = Vec::new();
        for _ in 0..500 {
            permits.push(
                limiter
                    .try_acquire(0)
                    .expect("no ceiling means a slot is always free"),
            );
        }
        assert_eq!(limiter.active(), 500, "slots are still counted honestly");
        assert_eq!(limiter.peak(), 500);

        // The blocking path agrees with the immediate one, and does not depend on
        // a permit being released first.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let cancel = CancellationToken::new();
        rt.block_on(async {
            let another = limiter
                .acquire(0, &cancel)
                .await
                .expect("the unlimited acquire must not block");
            drop(another);
        });

        drop(permits);
        assert_eq!(limiter.active(), 0, "every permit is released on drop");
    }

    #[test]
    fn spec_is_a_delegate_tool_with_required_task() {
        struct Dummy;
        #[async_trait]
        impl Tool for Dummy {
            fn spec(&self) -> ToolSpec {
                spawn_spec(false)
            }
            async fn invoke(&self, _r: ToolCallRequest, _c: ToolCtx) -> ToolOutcome {
                ToolOutcome::ok("")
            }
        }
        let spec = Dummy.spec();
        assert_eq!(spec.name, SPAWN_AGENT_TOOL);
        assert_eq!(Dummy.class(), ToolClass::ReadOnly); // default class, not the spec's
        assert_eq!(spec.origin, kc_core::tool::ToolOrigin::Builtin);
        assert_eq!(spec.parameters["required"], serde_json::json!(["task"]));
        assert!(spec.description.contains("subagent"));
        // The background switch has to be advertised, or no model will use it.
        assert!(spec.parameters["properties"]["background"].is_object());
    }

    #[test]
    fn the_fan_out_spec_takes_a_batch() {
        let spec = spawn_spec(true);
        assert_eq!(spec.name, SPAWN_AGENTS_TOOL);
        assert_eq!(spec.parameters["required"], serde_json::json!(["tasks"]));
        assert!(spec.description.contains("at the same time"));
        assert!(spec.parameters["properties"]["tasks"]["items"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|v| v == "task")));
    }

    #[test]
    fn a_task_entry_is_parsed_or_refused() {
        let ok = parse_task(&serde_json::json!({
            "task": "  review the diff  ",
            "name": "reviewer",
            "owned_paths": ["src/lib.rs"]
        }))
        .expect("valid task");
        assert_eq!(ok.task, "review the diff");
        assert_eq!(ok.name.as_deref(), Some("reviewer"));
        assert_eq!(ok.role, "worker");
        assert_eq!(ok.owned_paths, vec!["src/lib.rs".to_string()]);

        assert!(parse_task(&serde_json::json!({ "task": "   " })).is_err());
        assert!(parse_task(&serde_json::json!("nope")).is_err());
        assert!(parse_task(&serde_json::json!({ "task": "x", "owned_paths": "nope" })).is_err());
    }

    #[test]
    fn background_flags_must_be_booleans() {
        let request = |value: serde_json::Value| ToolCallRequest {
            call_id: "c".into(),
            name: SPAWN_AGENT_TOOL.into(),
            arguments: value,
        };
        let options = parse_spawn_options(&request(serde_json::json!({ "task": "x" }))).unwrap();
        assert!(!options.background && !options.wait);
        let options =
            parse_spawn_options(&request(serde_json::json!({ "background": true }))).unwrap();
        assert!(options.background);
        assert!(parse_spawn_options(&request(serde_json::json!({ "background": "yes" }))).is_err());
    }
}
