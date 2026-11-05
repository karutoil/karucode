//! Agent runtime: the multi-step loop, cancellation, approvals, subagents,
//! event fan-out and context budgeting.
//!
//! The runtime implements the agent loop purely against the traits in
//! [`kc_core`] (`Store`, `ProviderFactory`, `Settings`, `Tool`, `Hooks`). The
//! server wires the concrete implementations in; tests wire fakes in.
//!
//! # Built-in tools
//!
//! On construction the runtime installs its own delegate tool (`spawn_agent`)
//! into the tool *view* it exposes through [`Runtime::tools`]. The registry the
//! server passes in [`RuntimeDeps::tools`] is left untouched, and
//! [`Runtime::set_tools`] re-installs the built-ins on top of whatever the server
//! supplies, so polling `Runtime::tools()` is always the authoritative set to
//! show to the model. [`Runtime::builtin_tool_names`] lists them.

use kc_core::error::{Error, Result};
use kc_core::events::EventEnvelope;
use kc_core::hooks::Hooks;
use kc_core::ids::{AgentId, RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::provider::ProviderFactory;
use kc_core::queue::{QueuedKind, QueuedMessage, MAX_QUEUED_ATTACHMENTS};
use kc_core::settings::Settings;
use kc_core::store::{AgentStatus, Store};
use kc_core::tool::ToolRegistry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod agent;
pub mod approvals;
pub mod context;
pub mod events;
pub(crate) mod history;
pub(crate) mod hub;
mod notify;
mod restart;
pub mod rewind;
pub mod subagent;
pub(crate) mod worktree;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_support;

/// How many stored messages a new run loads as conversation history.
///
/// This is a *paging* bound, not a context bound: the character budget
/// (`limits.context_max_chars`) is what actually decides how much the model
/// sees. It used to be 400, which silently dropped the beginning of a long
/// session - exactly the failure a long-horizon harness must not have. The
/// value is now high enough that a day of work fits; a session that somehow
/// exceeds it still keeps its newest turns and the trim notice names the cut.
pub const HISTORY_LIMIT: usize = 20_000;

/// How old (in days) a session may be and still be repaired at startup.
///
/// Startup repair is not the only repair: `begin_run` calls `repair_history`
/// before every run, so a session that is skipped here is still normalised
/// before the model ever sees it. The startup pass exists only to keep the
/// *first* turn after a restart from replaying a broken exchange, and a session
/// nobody has touched in a month is not about to be the first turn. Bounding by
/// recency is what keeps boot from reading every row of every session ever
/// created: `HISTORY_LIMIT` rows per session times hundreds of sessions is
/// millions of rows parsed before the port is even open.
pub const RECOVERY_MAX_AGE_DAYS: i64 = 30;

/// Hard cap on how many sessions one startup pass repairs.
///
/// Recency alone is not a bound - a busy install can touch hundreds of sessions
/// inside the window - so the pass also stops after this many. Sessions past the
/// cap are repaired lazily on their first run, exactly like the old ones.
pub const RECOVERY_MAX_SESSIONS: usize = 64;

/// Environment escape hatch: set to `1`/`true` to skip the startup transcript
/// repair entirely. Interrupted runs and agents are still marked and their
/// claims still released; only the per-session transcript pass is skipped, and
/// every session is then repaired by its first run.
pub const SKIP_RECOVERY_ENV: &str = "KARUCODE_SKIP_RECOVERY";

/// Broadcast channel capacity: a slow SSE consumer lags and is skipped rather
/// than blocking the run.
pub const EVENT_CHANNEL_CAPACITY: usize = 2048;

pub struct RuntimeDeps {
    pub store: Arc<dyn Store>,
    pub providers: Arc<dyn ProviderFactory>,
    pub tools: Arc<ToolRegistry>,
    pub settings: Arc<dyn Settings>,
    pub hooks: Arc<dyn Hooks>,
    /// Resolves the workspace a run is confined to. A run uses the workspace of
    /// the session's project, so one process can serve several projects without
    /// sharing a mutable "current workspace".
    pub workspaces: Arc<dyn WorkspaceProvider>,
}

/// Resolve the workspace for a run. The server implements this over its project
/// registry; [`SingleWorkspace`] covers tests and single-directory embeddings.
pub trait WorkspaceProvider: Send + Sync {
    /// Resolve the workspace for a run.
    ///
    /// `None` means "the implicit workspace" (a session created before projects
    /// existed, or one the caller did not scope). A server started without a
    /// default workspace returns an error here instead, so a run cannot silently
    /// land somewhere nobody chose.
    fn workspace_for(&self, project_id: Option<&str>) -> Result<Arc<Workspace>>;

    /// Resolve the workspace for a session that has already been loaded.
    ///
    /// A session with a project uses that project's workspace. A session without
    /// one (the control chat) runs in its own recorded directory, which is how the
    /// central chat stays outside every project. The default delegates so a
    /// single-workspace provider keeps its old behaviour.
    fn workspace_for_session(
        &self,
        project_id: Option<&str>,
        _recorded: &str,
    ) -> Result<Arc<Workspace>> {
        self.workspace_for(project_id)
    }
}

/// One workspace for every run.
pub struct SingleWorkspace {
    workspace: Arc<Workspace>,
}

impl SingleWorkspace {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

impl WorkspaceProvider for SingleWorkspace {
    fn workspace_for(&self, _project_id: Option<&str>) -> Result<Arc<Workspace>> {
        Ok(Arc::clone(&self.workspace))
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunHandle {
    pub run_id: String,
    pub session_id: String,
}

/// What a human asked the runtime to do to one live agent.
///
/// A distinct type rather than a `&str` so the HTTP route, the runtime and any
/// future caller cannot disagree about the spelling, and so an unknown verb is a
/// parse error instead of a silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAction {
    Pause,
    Resume,
    Cancel,
}

impl AgentAction {
    /// Parse the path segment of `POST /api/agents/:id/<action>`.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "pause" => AgentAction::Pause,
            "resume" => AgentAction::Resume,
            "cancel" | "kill" | "stop" => AgentAction::Cancel,
            _ => return None,
        })
    }
}

/// What a rewind did, or what it would do.
///
/// The preview is returned alongside the outcomes so a caller never has to
/// re-derive "which files were involved" from the undo results: the two come from
/// the same journal rows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RewindOutcome {
    pub point: kc_core::store::RewindPoint,
    pub preview: kc_core::checkpoint::RewindPreview,
    /// One entry per journal row, in the order the undo ran (newest first).
    /// Empty when the rewind only hid the transcript.
    pub outcomes: Vec<kc_core::checkpoint::UndoOutcome>,
    /// The run started after the rewind, for the retry and note modes. `None`
    /// when the caller only rewound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveRun {
    pub run_id: String,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub started_at: i64,
    pub iterations: u32,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RecoveryReport {
    pub interrupted_runs: usize,
    pub interrupted_agents: usize,
    pub expired_approvals: usize,
    /// Sessions whose transcript repair failed. Recovery is best-effort per
    /// session: one unreadable or unwritable session must not stop the server
    /// from starting, and the next run in that session repairs it again.
    pub failed_sessions: usize,
    /// Sessions whose transcript was repaired at startup.
    pub repaired_sessions: usize,
    /// Sessions skipped at startup because they were not interrupted and are
    /// older than [`RECOVERY_MAX_AGE_DAYS`]. They are repaired lazily by the
    /// first run in the session, which always calls `repair_history`.
    pub skipped_sessions: usize,
    /// Path claims released for agents that recovery marked `interrupted`.
    pub released_claims: usize,
}

pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

pub(crate) struct RuntimeInner {
    pub deps: RuntimeDeps,
    pub events: broadcast::Sender<EventEnvelope>,
    /// The authoritative tool view: server tools plus runtime built-ins.
    pub tools: RwLock<Arc<ToolRegistry>>,
    /// In-flight runs, keyed by run id for top-level runs and by agent id for
    /// subagent loops.
    pub runs: Mutex<HashMap<String, Arc<RunState>>>,
    /// One fair async lock per session for operations that need a quiescent
    /// transcript: submit, rewind, and the indivisible rewind-and-retry pair.
    /// The map is locked only long enough to clone the session's lock.
    pub session_lifecycle: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub approvals: approvals::ApprovalRegistry,
    pub subagents: Arc<subagent::SubagentLimiter>,
    /// The live half of subagents: pause gates, mailboxes and a registry of the
    /// agents whose loops exist right now. See [`hub`].
    pub hub: Arc<hub::AgentHub>,
    /// Serializes sequence allocation with broadcast publication.
    pub seq: Mutex<u64>,
    /// Serializes queue snapshot construction with its publication. Without it,
    /// an older snapshot can be built, paused, and emitted after a newer one.
    pub queue_publication: Mutex<()>,
    pub events_emitted: AtomicU64,
    /// `allow_session` approvals, keyed by session + kind + target.
    pub session_allow: Mutex<HashSet<String>>,
    /// Steering messages accepted while a run was in flight, per session.
    ///
    /// A *steer* is a user message that arrives mid-run: "also rename the test",
    /// "stop using the mock, hit the real endpoint". It cannot start a second run
    /// (one run per session), and it must not interrupt the turn in flight -
    /// killing the model mid-tool-call throws away work and can leave a tool call
    /// without a result. So it is queued here and drained by the loop at the top
    /// of its next iteration, where it becomes an ordinary persisted `user`
    /// message in the conversation.
    ///
    /// Why a real message and not a trailing note: see the long comment at
    /// `agent::run_loop_inner` - any text appended *after* the last turn makes a
    /// reasoning model deliberate instead of act, so a steer is inserted where a
    /// user turn belongs, which costs a prompt-cache miss once and then behaves
    /// exactly like something the user typed.
    pub steers: Mutex<HashMap<String, VecDeque<QueuedSteer>>>,
    /// Follow-up prompts accepted while a run was in flight, per session.
    ///
    /// A *follow-up* is the other half of steering: it does not want to change the
    /// turn in progress, it wants the next one. "Run the tests when you are done",
    /// "then commit it". Queueing it here rather than rejecting the request means
    /// the user can type while the agent works and walk away.
    ///
    /// Unlike a steer, a follow-up starts a *new run* once the current one ends,
    /// which is why it is kept apart: a steer is folded into a conversation, a
    /// follow-up begins one.
    pub follow_ups: Mutex<HashMap<String, VecDeque<QueuedFollowUp>>>,
}

/// One accepted follow-up prompt, waiting for the current run to finish.
///
/// Both queues hold the same shape ([`QueuedMessage`]) so the composer can
/// render and recall them with one code path; the aliases are what the runtime
/// and the HTTP layer name each queue's members by.
pub type QueuedFollowUp = QueuedMessage;

/// One accepted steering message, waiting for the run loop to pick it up.
pub type QueuedSteer = QueuedMessage;

/// What the caller supplies when queueing a message.
///
/// `content` is what the model sees (the draft with one block per attachment,
/// rendered by the browser exactly as a plain send renders it), and `draft` plus
/// `attachments` are what a recall puts back on screen. They are passed together
/// because they must agree: a queued message whose content and chips disagreed
/// would recall into a draft that no longer produces the same request.
///
/// `From<String>`/`From<&str>` cover the common "just text" case, so a caller
/// with nothing attached does not have to spell the struct out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInput {
    pub content: String,
    /// The text the user typed, when it differs from `content` (attachments do
    /// that). `None` means "the content is the draft".
    pub draft: Option<String>,
    pub attachments: Vec<kc_core::queue::QueuedAttachment>,
}

impl QueuedInput {
    /// Text with nothing attached.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            draft: None,
            attachments: Vec::new(),
        }
    }

    /// Text plus the chips a recall should restore.
    pub fn with_attachments(
        content: impl Into<String>,
        draft: Option<String>,
        attachments: Vec<kc_core::queue::QueuedAttachment>,
    ) -> Self {
        Self {
            content: content.into(),
            draft,
            attachments,
        }
    }
}

impl From<String> for QueuedInput {
    fn from(content: String) -> Self {
        Self::text(content)
    }
}

impl From<&str> for QueuedInput {
    fn from(content: &str) -> Self {
        Self::text(content)
    }
}

/// How many steering messages one session may queue.
///
/// A queue that grows without bound would let a stuck client fill memory, and the
/// run loop drains it one turn at a time; past a couple of dozen the user is
/// typing a new prompt, not steering. `POST .../messages?mode=steer` reports the
/// documented 409 when this is reached.
pub const MAX_QUEUED_STEERS: usize = 32;

/// How many follow-up prompts one session may queue.
///
/// Each one is a whole run, so the useful ceiling is much lower than the steer
/// queue: three "and then..." instructions is a plan, thirty is a paste of
/// intentions that the user has not thought through. `POST .../mode=follow_up`
/// reports the documented 409 past this.
pub const MAX_QUEUED_FOLLOW_UPS: usize = 8;

/// Both queues of one session, read together.
#[derive(Debug, Clone, Default)]
pub struct QueuedSnapshot {
    pub steers: Vec<QueuedSteer>,
    pub follow_ups: Vec<QueuedFollowUp>,
}

impl QueuedSnapshot {
    pub fn is_empty(&self) -> bool {
        self.steers.is_empty() && self.follow_ups.is_empty()
    }
}

/// Remove the message with `queued_id` from one session's queue, if it is there.
///
/// The queue entry is dropped when it empties, so a settled session leaves no
/// empty map key behind (the maps outlive the sessions they are keyed by).
fn take_by_id(
    queues: &mut HashMap<String, VecDeque<QueuedMessage>>,
    session_id: &str,
    queued_id: &str,
) -> Option<QueuedMessage> {
    let queue = queues.get_mut(session_id)?;
    let index = queue.iter().position(|queued| queued.id == queued_id)?;
    let removed = queue.remove(index);
    if queue.is_empty() {
        queues.remove(session_id);
    }
    removed
}

pub(crate) struct RunState {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub agent_id: Option<AgentId>,
    pub cancel: CancellationToken,
    pub started_at: i64,
    pub iterations: AtomicU32,
    /// True for a user-submitted run (one per session at a time), false for a
    /// subagent loop.
    pub top_level: bool,
    /// False as soon as teardown begins. Queue admission checks this under the
    /// run-registry lock so no new steer can arrive after the final conversion
    /// of accepted-but-undrained steers into follow-ups.
    pub accepting_input: AtomicBool,
    /// Bitset of independently idempotent top-level finalization stages. The
    /// outer panic handler may resume finalization without repeating events or
    /// notifications that were already published.
    pub finalization: AtomicU8,
    /// The loop outcome is retained before finalization so an unexpected panic
    /// in a hook or notification cannot replace an already-recorded success
    /// with a contradictory status when the outer guard resumes the remaining
    /// stages.
    pub final_outcome: Mutex<Option<agent::LoopOutcome>>,
    /// Set when the human asked for this run to stop.
    ///
    /// `run_top_level` reads it to decide whether a `Cancelled` outcome is worth a
    /// notification: a cancellation you asked for needs no ping, one you did not
    /// ask for (a shutdown, a timeout) does. An `AtomicBool` rather than a flag on
    /// the token because `CancellationToken` deliberately does not carry a reason.
    pub cancelled_by_user: AtomicBool,
}

impl RunState {
    pub(crate) fn new(
        run_id: RunId,
        session_id: SessionId,
        agent_id: Option<AgentId>,
        cancel: CancellationToken,
        top_level: bool,
    ) -> Self {
        Self {
            run_id,
            session_id,
            agent_id,
            cancel,
            started_at: events::now_ms(),
            iterations: AtomicU32::new(0),
            top_level,
            accepting_input: AtomicBool::new(true),
            finalization: AtomicU8::new(0),
            final_outcome: Mutex::new(None),
            cancelled_by_user: AtomicBool::new(false),
        }
    }

    /// Whether the human was the one who stopped this run.
    pub(crate) fn was_cancelled_by_user(&self) -> bool {
        self.cancelled_by_user.load(Ordering::Relaxed)
    }

    /// Claim one idempotent finalization stage. A panic-safe second pass skips
    /// stages this bit already names.
    pub(crate) fn claim_finalization(&self, stage: u8) -> bool {
        self.finalization.fetch_or(stage, Ordering::AcqRel) & stage == 0
    }

    pub(crate) fn key(&self) -> String {
        match &self.agent_id {
            Some(a) => a.0.clone(),
            None => self.run_id.0.clone(),
        }
    }
}

impl Runtime {
    pub fn new(deps: RuntimeDeps) -> Arc<Self> {
        let inner = Arc::new(RuntimeInner {
            events: broadcast::channel(EVENT_CHANNEL_CAPACITY).0,
            tools: RwLock::new(Arc::new(ToolRegistry::new())),
            runs: Mutex::new(HashMap::new()),
            session_lifecycle: Mutex::new(HashMap::new()),
            approvals: approvals::ApprovalRegistry::new(),
            subagents: Arc::new(subagent::SubagentLimiter::new(Default::default())),
            hub: Arc::new(hub::AgentHub::new()),
            seq: Mutex::new(0),
            queue_publication: Mutex::new(()),
            events_emitted: AtomicU64::new(0),
            session_allow: Mutex::new(HashSet::new()),
            steers: Mutex::new(HashMap::new()),
            follow_ups: Mutex::new(HashMap::new()),
            deps,
        });
        let view = Arc::new(Self::tool_view_with_builtins(&inner, &inner.deps.tools));
        *inner.tools.write().expect("tools lock") = view;
        Arc::new(Self { inner })
    }

    /// Names of the tools the runtime itself contributes to the registry view.
    ///
    /// The delegate family lives here rather than in `kc-tools` because every one
    /// of them needs the runtime's live agent registry, and the list is what
    /// `set_tools` re-installs on top of whatever the server supplies, so a
    /// reload cannot silently drop the ability to delegate.
    pub fn builtin_tool_names() -> &'static [&'static str] {
        subagent::DELEGATE_TOOLS
    }

    pub(crate) fn shared(&self) -> &Arc<RuntimeInner> {
        &self.inner
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.inner.events.subscribe()
    }

    /// The authoritative tool registry: server-registered tools plus the
    /// runtime built-ins.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.inner.tools.read().expect("tools lock").clone()
    }

    /// Replace the tool registry. Built-ins are re-installed on top of the new
    /// set, so `spawn_agent` cannot be accidentally dropped by a reload.
    pub fn set_tools(&self, tools: Arc<ToolRegistry>) {
        let view = Arc::new(Self::tool_view_with_builtins(&self.inner, &tools));
        *self.inner.tools.write().expect("tools lock") = view;
    }

    fn tool_view_with_builtins(inner: &Arc<RuntimeInner>, base: &ToolRegistry) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for name in base.names() {
            if Self::builtin_tool_names().contains(&name.as_str()) {
                continue;
            }
            if let Some(tool) = base.get(&name) {
                registry.register(tool);
            }
        }
        subagent::install_delegate_tools(inner, &mut registry);
        registry
    }

    /// Stamp with the runtime seq/ts, attach context and publish one event.
    ///
    /// Used by the server for session lifecycle events (created/renamed/deleted)
    /// so the SSE sequence stays monotonic across runtime and server events.
    pub fn emit(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
        agent_id: Option<&str>,
        event: kc_core::events::Event,
    ) -> EventEnvelope {
        self.inner.emit(session_id, run_id, agent_id, event)
    }

    /// Number of events published since process start.
    pub fn events_emitted(&self) -> u64 {
        self.inner.events_emitted.load(Ordering::Relaxed)
    }

    /// Forget every `allow_session` grant remembered for a session. Call this
    /// when a session is deleted so grants cannot be inherited by a new session
    /// that happens to reuse the id.
    pub fn clear_session_allows(&self, session_id: &str) {
        self.inner.clear_session_allows(session_id);
    }

    /// Startup recovery: leftover `running` runs and agents become
    /// `interrupted`, the path claims those agents held are released, and
    /// approvals still pending become `expired`. Returns counts.
    ///
    /// The transcript repair pass is bounded and best-effort by design:
    ///
    /// * only sessions a crash could have left mid-exchange are repaired at
    ///   startup - one with a `running` run or agent, or one updated inside
    ///   [`RECOVERY_MAX_AGE_DAYS`] - and at most [`RECOVERY_MAX_SESSIONS`] of
    ///   them. `HISTORY_LIMIT` rows per session is not a bound an install with
    ///   hundreds of long sessions can afford to read before the port is open;
    /// * a session whose repair fails is logged and counted, never propagated.
    ///   One corrupt row or a full disk must not stop the server from starting.
    ///
    /// Skipping a session is safe because `begin_run` repairs the transcript
    /// before every run, so a session that is not repaired here is still
    /// normalised before the model sees it. Only an unopenable store fails
    /// recovery.
    pub async fn recover(&self) -> Result<RecoveryReport> {
        let store = &self.inner.deps.store;
        let started = std::time::Instant::now();

        // Snapshot before rewriting statuses; failed agent snapshots remain best-effort.
        let (running_agents, include_agents) = match store.running_agent_ids().await {
            Ok(agents) => (agents, true),
            Err(e) => {
                tracing::warn!(error = %e, "startup recovery: could not list agents; interrupted agents' path claims will not be released");
                (Vec::new(), false)
            }
        };
        let skip_repair = skip_recovery_requested();
        let cutoff = events::now_ms() - RECOVERY_MAX_AGE_DAYS * 86_400_000;
        let (total_sessions, candidates) = store
            .recovery_sessions(
                cutoff,
                if skip_repair {
                    0
                } else {
                    RECOVERY_MAX_SESSIONS
                },
                include_agents,
            )
            .await?;

        let interrupted_runs = store.mark_interrupted_runs().await?;
        let interrupted_agents = store.mark_interrupted_agents().await?;

        // A dead agent's claims are not advisory history, they are a lie: every
        // later subagent touching the same path would be warned about an agent
        // that no longer exists, which trains the model and the operator to
        // ignore the one overlap signal `owned_paths` provides. Release them for
        // exactly the agents this pass marked interrupted.
        let mut released_claims = 0usize;
        for (_, agent_id) in &running_agents {
            match store.release_claims(agent_id).await {
                Ok(()) => released_claims += 1,
                Err(e) => tracing::warn!(
                    agent = %agent_id,
                    error = %e,
                    "startup recovery: could not release the path claims of an interrupted agent"
                ),
            }
        }

        // A crash can leave an assistant turn whose tool result was never
        // persisted. Repair the transcripts a restart could have broken, so the
        // first run after a restart does not replay an invalid exchange. The cap
        // is the one the resolved model actually gets, not a global constant: a
        // repaired window that is smaller than the model's window makes the
        // first turn of the next run compact work it could have kept.
        let limits = self.inner.deps.settings.limits().await;
        let model = self.inner.deps.settings.resolve_model(None).await;
        let max_chars = limits.context_chars_for(&model);

        let mut repaired_sessions = 0usize;
        let mut failed_sessions = 0usize;
        let skipped_sessions = total_sessions.saturating_sub(candidates.len());
        if skip_repair {
            tracing::info!(sessions = total_sessions, env = SKIP_RECOVERY_ENV,
                "startup recovery: transcript repair skipped; every session is repaired by its first run");
        } else {
            for session_id in candidates {
                match history::repair_history(store, &session_id, HISTORY_LIMIT, max_chars).await {
                    Ok(_) => repaired_sessions += 1,
                    Err(e) => {
                        failed_sessions += 1;
                        tracing::warn!(session = %session_id, error = %e,
                            "startup recovery: could not repair this session's transcript; its next run will repair it");
                    }
                }
            }
        }

        let pending = store.list_pending_approvals(None).await?;
        let mut run_ids: Vec<String> = Vec::new();
        for approval in &pending {
            if !run_ids.contains(&approval.run_id) {
                run_ids.push(approval.run_id.clone());
            }
        }
        let mut expired_approvals = 0usize;
        for run_id in run_ids {
            expired_approvals += store
                .expire_pending_approvals(&run_id, "server restarted")
                .await?;
        }

        let elapsed = started.elapsed();
        tracing::info!(
            repaired_sessions,
            failed_sessions,
            skipped_sessions,
            interrupted_runs,
            interrupted_agents,
            released_claims,
            expired_approvals,
            elapsed_ms = elapsed.as_millis() as u64,
            "startup recovery: repaired {repaired_sessions} session(s) in {}ms \
             ({failed_sessions} failed, {skipped_sessions} deferred to their first run)",
            elapsed.as_millis()
        );

        Ok(RecoveryReport {
            interrupted_runs,
            interrupted_agents,
            expired_approvals,
            failed_sessions,
            repaired_sessions,
            skipped_sessions,
            released_claims,
        })
    }

    /// Start a run for `prompt` in `session_id`.
    ///
    /// The run row and the user message are written before this returns, so the
    /// session view is consistent as soon as the caller got the handle. The
    /// model loop itself runs in a background task.
    pub async fn submit(self: &Arc<Self>, session_id: &str, prompt: String) -> Result<RunHandle> {
        agent::start_run(self, session_id, prompt).await
    }

    /// Hide everything after `message_id` and (optionally) undo the file changes
    /// made since it.
    ///
    /// `message_id` is normalised to the user prompt that owns it, so picking an
    /// assistant turn means "the request that produced this". With
    /// `keep_prompt` the prompt survives (the caller wants to edit it);
    /// otherwise it is hidden too, which is what a retry and a replacement note
    /// need so the model does not see two user turns in a row.
    ///
    /// Nothing is destructive in the database sense: the tail is marked
    /// (`rewound_at`) rather than deleted, so the transcript the model sees and
    /// the transcript the browser renders are the same list. With `revert` the
    /// file journal is replayed newest-first and each undone row is retired.
    ///
    /// Refused while a run is in flight: a running loop holds `history` in memory
    /// and would re-persist the messages the rewind just hid.
    pub async fn rewind(
        self: &Arc<Self>,
        session_id: &str,
        message_id: i64,
        revert: bool,
        keep_prompt: bool,
    ) -> Result<RewindOutcome> {
        let (point, preview, outcomes) =
            rewind::rewind(self, session_id, message_id, revert, keep_prompt).await?;
        Ok(RewindOutcome {
            point,
            preview,
            outcomes,
            run_id: None,
        })
    }

    /// Rewind, then start the run that replaces the hidden turn.
    ///
    /// The two are one operation on purpose. A caller that rewound and then
    /// failed to start the run would leave a session whose transcript ends at a
    /// prompt with no answer and no run - indistinguishable from a crash. Doing
    /// both here means the only outcomes are "both happened" or "neither did"
    /// (a rewind whose follow-up run is refused is reported with the preview, so
    /// the caller can say what state the session is in).
    ///
    /// `next_prompt` is what the new turn says: the original prompt for a retry,
    /// or the user's replacement note. The rewind always hides the original
    /// prompt, because two user turns in a row reads to the model as a correction
    /// of work that is no longer in the transcript.
    pub async fn rewind_and_run(
        self: &Arc<Self>,
        session_id: &str,
        message_id: i64,
        revert: bool,
        next_prompt: String,
    ) -> Result<RewindOutcome> {
        if next_prompt.trim().is_empty() {
            return Err(Error::BadRequest(
                "a retry or a note needs the prompt to send after the rewind".into(),
            ));
        }
        // One owned guard spans both halves. Calling the public `rewind` and
        // `submit` separately would drop the guard between them, allowing an
        // unrelated submission to start against the half-rewound transcript.
        let lifecycle = self.inner.lock_session_lifecycle(session_id).await;
        let (point, preview, outcomes) =
            rewind::rewind_quiescent(self, session_id, message_id, revert, false).await?;
        let handle =
            agent::start_run_with_lifecycle(self, session_id, next_prompt, lifecycle).await?;
        Ok(RewindOutcome {
            point,
            preview,
            outcomes,
            run_id: Some(handle.run_id),
        })
    }

    /// What [`Runtime::rewind`] would do, without doing it.
    pub async fn rewind_preview(
        &self,
        session_id: &str,
        message_id: i64,
    ) -> Result<(
        kc_core::store::RewindPoint,
        kc_core::checkpoint::RewindPreview,
    )> {
        let inner = self.shared();
        let session = inner
            .deps
            .store
            .get_session(session_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
        let workspace = inner
            .deps
            .workspaces
            .workspace_for(session.project_id.as_deref())?;
        rewind::Rewinder::new(inner.deps.store.clone(), workspace)
            .preview(session_id, message_id)
            .await
    }

    /// Cancel the run(s) for a session because the human asked.
    ///
    /// The `cancelled_by_user` flag is what keeps the resulting notification quiet:
    /// you pressed Escape, you know what happened. `shutdown()` deliberately does
    /// not set it, so a run stopped by the server going down still notifies.
    /// Returns `false` when nothing was running.
    pub fn cancel(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        let mut cancelled = false;
        for state in runs.values() {
            if state.session_id.0 == session_id {
                state.cancelled_by_user.store(true, Ordering::Relaxed);
                state.cancel.cancel();
                cancelled = true;
            }
        }
        cancelled
    }

    /// Accept a steering message for a running session.
    ///
    /// Queues `content` for the run loop to pick up at the top of its next
    /// iteration. Fails when nothing is running (a steer with no run has no
    /// meaning - the caller should `submit` instead) or when the queue is full.
    ///
    /// The message is *not* persisted here. It becomes a stored `user` message
    /// when the loop drains it, which keeps the transcript in the order the model
    /// saw it. If it arrives on the final no-tool response, teardown closes
    /// admission and converts it to a follow-up with the same id, so accepted input
    /// is either applied by this run or deliberately answered by the next one.
    ///
    /// Every queue mutation publishes [`Event::QueuedMessages`] with the whole
    /// queue, so a second tab (or the same tab after a reconnect) never shows a
    /// stale pending panel.
    pub fn steer(&self, session_id: &str, input: impl Into<QueuedInput>) -> Result<QueuedSteer> {
        self.queue(
            QueuedKind::Steer,
            session_id,
            input.into(),
            MAX_QUEUED_STEERS,
        )
    }

    /// Accept a follow-up prompt for a running session.
    ///
    /// Queued and submitted automatically when the current run ends. Rejected when
    /// nothing is running, because the caller should simply `submit` (that is what
    /// a follow-up with no run *is*), and when the queue is full.
    pub fn enqueue_follow_up(
        &self,
        session_id: &str,
        input: impl Into<QueuedInput>,
    ) -> Result<QueuedFollowUp> {
        self.queue(
            QueuedKind::FollowUp,
            session_id,
            input.into(),
            MAX_QUEUED_FOLLOW_UPS,
        )
    }

    fn queue(
        &self,
        kind: QueuedKind,
        session_id: &str,
        input: QueuedInput,
        capacity: usize,
    ) -> Result<QueuedMessage> {
        let trimmed = input.content.trim();
        let noun = match kind {
            QueuedKind::Steer => "a steering message",
            QueuedKind::FollowUp => "a follow-up message",
        };
        if trimmed.is_empty() {
            return Err(kc_core::Error::BadRequest(format!(
                "{noun} may not be empty"
            )));
        }
        if input.attachments.len() > MAX_QUEUED_ATTACHMENTS {
            return Err(kc_core::Error::BadRequest(format!(
                "a queued message may carry at most {MAX_QUEUED_ATTACHMENTS} attachments (got {})",
                input.attachments.len()
            )));
        }
        let id = match kind {
            QueuedKind::Steer => format!("steer_{}", uuid::Uuid::new_v4().simple()),
            QueuedKind::FollowUp => format!("followup_{}", uuid::Uuid::new_v4().simple()),
        };
        let queued = QueuedMessage {
            id,
            kind,
            content: trimmed.to_string(),
            // A draft identical to the content carries no information; leaving it
            // out is what makes "no attachments were attached" observable on the
            // wire instead of an empty string that has to be guessed at.
            draft: input
                .draft
                .map(|draft| draft.trim().to_string())
                .filter(|draft| *draft != trimmed),
            attachments: input.attachments,
            accepted_at: events::now_ms(),
        };
        {
            // Serialize admission with run deregistration. Otherwise teardown can
            // drain the queue between checking the run and inserting the message,
            // leaving an accepted follow-up stranded indefinitely. Lock order is
            // runs then queue; release both before publishing events.
            let runs = self.inner.runs.lock().expect("runs lock");
            if !runs.values().any(|s| {
                s.top_level
                    && s.session_id.0 == session_id
                    && !s.cancel.is_cancelled()
                    && s.accepting_input.load(Ordering::Acquire)
            }) {
                return Err(kc_core::Error::BadRequest(format!(
                    "session {session_id} has no run in flight; send a normal message instead"
                )));
            }
            let mut queues = match kind {
                QueuedKind::Steer => self.inner.steers.lock().expect("steers lock"),
                QueuedKind::FollowUp => self.inner.follow_ups.lock().expect("follow ups lock"),
            };
            let queue = queues.entry(session_id.to_string()).or_default();
            if queue.len() >= capacity {
                let what = match kind {
                    QueuedKind::Steer => "steering messages",
                    QueuedKind::FollowUp => "follow-ups",
                };
                return Err(kc_core::Error::BadRequest(format!(
                    "session {session_id} already has {capacity} {what} queued"
                )));
            }
            queue.push_back(queued.clone());
        }
        self.publish_queues(session_id);
        Ok(queued)
    }

    /// Steering messages still waiting for a running session, oldest first.
    pub fn queued_steers(&self, session_id: &str) -> Vec<QueuedSteer> {
        let queues = self.inner.steers.lock().expect("steers lock");
        queues
            .get(session_id)
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Follow-up prompts still waiting for a running session, oldest first.
    pub fn queued_follow_ups(&self, session_id: &str) -> Vec<QueuedFollowUp> {
        let queues = self.inner.follow_ups.lock().expect("follow ups lock");
        queues
            .get(session_id)
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Both queues for a session, oldest first.
    ///
    /// Read under one pass of both locks so the pair a caller reports is a pair
    /// that existed together - which is what the queued-messages event promises.
    pub fn queued_all(&self, session_id: &str) -> QueuedSnapshot {
        QueuedSnapshot {
            steers: self.queued_steers(session_id),
            follow_ups: self.queued_follow_ups(session_id),
        }
    }

    /// Take one queued message back out, by id.
    ///
    /// This is what "recall" means: the message never reaches the model, and the
    /// caller gets it back - content, draft and attachment chips - to put in the
    /// composer. An id that is not queued (already landed, already recalled, or
    /// belonging to another session) is [`kc_core::Error::NotFound`], so a browser
    /// that raced the run loop is told rather than silently succeeding.
    pub fn recall_queued(&self, session_id: &str, queued_id: &str) -> Result<QueuedMessage> {
        let recalled = {
            let mut steers = self.inner.steers.lock().expect("steers lock");
            take_by_id(&mut steers, session_id, queued_id)
        };
        let recalled = match recalled {
            Some(recalled) => recalled,
            None => {
                let mut follow_ups = self.inner.follow_ups.lock().expect("follow ups lock");
                match take_by_id(&mut follow_ups, session_id, queued_id) {
                    Some(recalled) => recalled,
                    None => {
                        return Err(kc_core::Error::NotFound(format!(
                            "no queued message `{queued_id}` for session {session_id}; \
                             it may already have been applied"
                        )))
                    }
                }
            }
        };
        self.publish_queues(session_id);
        Ok(recalled)
    }

    /// Drop anything still queued for a session (its run ended or it was deleted).
    pub fn clear_steers(&self, session_id: &str) {
        let removed = self
            .inner
            .steers
            .lock()
            .expect("steers lock")
            .remove(session_id)
            .is_some();
        if removed {
            self.publish_queues(session_id);
        }
    }

    /// Take the next follow-up for a session, if any.
    ///
    /// Taken one at a time, because each one becomes a run and a run has to
    /// finish before the next may start.
    pub fn take_follow_up(&self, session_id: &str) -> Option<QueuedFollowUp> {
        let next = {
            let mut queues = self.inner.follow_ups.lock().expect("follow ups lock");
            let next = queues.get_mut(session_id).and_then(VecDeque::pop_front);
            if queues.get(session_id).is_some_and(VecDeque::is_empty) {
                queues.remove(session_id);
            }
            next
        };
        if next.is_some() {
            self.publish_queues(session_id);
        }
        next
    }

    /// Put a taken follow-up back at the front of the queue.
    ///
    /// The counterpart of [`Runtime::take_follow_up`] for the one case where a
    /// taken entry must not be spent: the run it was taken for could not start.
    /// The client was told `202` with this id, so dropping it would silently lose
    /// a message it can no longer recall - putting it back keeps both promises
    /// (the id stays findable, and the next free run picks it up).
    pub fn requeue_follow_up(&self, session_id: &str, follow_up: QueuedFollowUp) {
        if follow_up.kind != QueuedKind::FollowUp {
            // Not a follow-up: leave the other queue alone rather than putting
            // a steering message where a run is expected.
            return;
        }
        {
            let mut queues = self.inner.follow_ups.lock().expect("follow ups lock");
            let queue = queues.entry(session_id.to_string()).or_default();
            // Front, not back: it was the oldest entry, and the user's order is
            // what a queue promises.
            queue.push_front(follow_up);
        }
        self.publish_queues(session_id);
    }

    /// Is a **user run** registered for this session, cancelled or not?
    ///
    /// Exactly the predicate `start_run` refuses on, which is what makes it
    /// usable as the "would a new run be accepted right now" test: unlike
    /// [`Runtime::has_active_run`] it does not ignore a run that is unwinding, so
    /// a follow-up chain that waits on this one cannot spin against a refusal.
    pub(crate) fn top_level_run_registered(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        runs.values()
            .any(|s| s.top_level && s.session_id.0 == session_id)
    }

    /// Close queue admission, convert any final-turn steers into follow-ups and
    /// remove the run as one registry critical section. Queue admission takes
    /// the same registry lock, so an accepted message cannot be stranded between
    /// the conversion and deregistration.
    pub(crate) fn deregister_top_level(&self, key: &str, state: &Arc<RunState>) -> usize {
        let session_id = state.session_id.0.clone();
        let converted = {
            let mut runs = self.inner.runs.lock().expect("runs lock");
            state.accepting_input.store(false, Ordering::Release);
            let mut steers = self.inner.steers.lock().expect("steers lock");
            let pending = steers.remove(&session_id).unwrap_or_default();
            let converted = pending.len();
            if converted > 0 {
                let mut follow_ups = self.inner.follow_ups.lock().expect("follow ups lock");
                let queue = follow_ups.entry(session_id.clone()).or_default();
                // Preserve the ids clients were given, and put these older
                // requests ahead of already queued follow-ups.
                for mut steer in pending.into_iter().rev() {
                    steer.kind = QueuedKind::FollowUp;
                    queue.push_front(steer);
                }
            }
            runs.remove(key);
            converted
        };
        if converted > 0 {
            self.publish_queues(&session_id);
            self.emit(
                Some(&session_id),
                None,
                None,
                kc_core::events::Event::Notice {
                    level: "info".into(),
                    message: format!(
                        "The run ended before {converted} steering message(s) could be applied; they will run as follow-ups."
                    ),
                },
            );
        }
        converted
    }

    /// Drop every follow-up queued for a session (it was deleted).
    pub fn clear_follow_ups(&self, session_id: &str) {
        let removed = self
            .inner
            .follow_ups
            .lock()
            .expect("follow ups lock")
            .remove(session_id)
            .is_some();
        if removed {
            self.publish_queues(session_id);
        }
    }

    /// Tell every client what this session's queues hold now.
    ///
    /// Called after each mutation. The whole pair is sent rather than a delta:
    /// the frames are small (a queue holds tens of short strings) and a client
    /// that lost one then resynchronises on the next instead of drifting.
    fn publish_queues(&self, session_id: &str) {
        // Keep the read and event construction in one publication critical
        // section. The event emitter separately serializes sequence numbers;
        // that alone would not stop an older queue snapshot following a newer.
        let _publication = self
            .inner
            .queue_publication
            .lock()
            .expect("queue publication lock");
        let snapshot = self.queued_all(session_id);
        self.emit(
            Some(session_id),
            None,
            None,
            kc_core::events::Event::QueuedMessages {
                session_id: session_id.to_string(),
                steers: snapshot.steers,
                follow_ups: snapshot.follow_ups,
            },
        );
    }

    /// Every in-flight loop: user runs (`agent_id: None`) and nested subagent
    /// loops (`agent_id: Some`), sorted by session and run id.
    pub fn active_runs(&self) -> Vec<ActiveRun> {
        let runs = self.inner.runs.lock().expect("runs lock");
        let mut out: Vec<ActiveRun> = runs
            .values()
            .map(|state| ActiveRun {
                run_id: state.run_id.0.clone(),
                session_id: state.session_id.0.clone(),
                agent_id: state.agent_id.as_ref().map(|a| a.0.clone()),
                started_at: state.started_at,
                iterations: state.iterations.load(Ordering::Relaxed),
            })
            .collect();
        out.sort_by(|a, b| {
            (a.session_id.as_str(), a.run_id.as_str())
                .cmp(&(b.session_id.as_str(), b.run_id.as_str()))
        });
        out
    }

    /// True while a **user run** for this session is in flight.
    ///
    /// This is the question the composer and `POST .../messages` ask: "would a
    /// new prompt be refused right now?". A detached subagent is deliberately not
    /// counted - it outlives the turn that started it, and a session that looked
    /// busy for as long as a background agent ran would silently refuse every
    /// prompt the user typed in the meantime. Use [`Runtime::is_running`] for
    /// "is anything at all still writing in here" (rewind, deletion).
    pub fn has_active_run(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        runs.values()
            .any(|s| s.top_level && s.session_id.0 == session_id && !s.cancel.is_cancelled())
    }

    /// True while *any* loop for this session is registered and has not been
    /// cancelled: a user run, or a subagent it started (including a detached one).
    ///
    /// Cancelled-but-unwinding runs still block a new `submit`, which is why
    /// `submit` does its own check on the run map.
    pub fn is_running(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        runs.values()
            .any(|s| s.session_id.0 == session_id && !s.cancel.is_cancelled())
    }

    /// True until every loop has actually deregistered. Unlike `is_running`,
    /// this deliberately includes a cancelled-but-unwinding loop: rewind must
    /// wait for its final tool results and terminal writes to finish.
    pub(crate) fn has_registered_runs(&self, session_id: &str) -> bool {
        let runs = self.inner.runs.lock().expect("runs lock");
        runs.values().any(|s| s.session_id.0 == session_id)
    }

    /// Resolve a pending approval. Unknown, already resolved or expired ids are
    /// a 404-style [`Error::NotFound`].
    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        decision: kc_core::approval::ApprovalDecision,
    ) -> Result<()> {
        approvals::resolve(Arc::clone(&self.inner), approval_id, decision).await
    }

    /// Pause, resume or cancel one live agent on a human's behalf.
    ///
    /// This is the runtime half of `POST /api/agents/:id/{pause,resume,cancel}`.
    /// A human is not an agent, so ownership does not apply: the person operating
    /// the install may stop anything running in it. The `by` value is recorded on
    /// the event so the roster can say who did it.
    ///
    /// The status returned is what the agent *is*, not what was asked for: a
    /// pause on an agent that is already unwinding reports `cancelled` rather
    /// than a pause that will never land. An unknown or settled id is
    /// [`Error::NotFound`], so a browser that raced the finish is told instead of
    /// silently succeeding.
    pub async fn control_agent(&self, agent_id: &str, action: AgentAction) -> Result<AgentStatus> {
        let live = self.inner.hub.get(agent_id).ok_or_else(|| {
            Error::NotFound(format!(
                "agent {agent_id} is not running; it may have finished already"
            ))
        })?;
        let session_id = live.session_id.clone();
        let status = match action {
            AgentAction::Pause => match self.inner.hub.pause(agent_id, "user") {
                Ok(status) => status,
                Err(e) => return Err(Error::BadRequest(e.message())),
            },
            AgentAction::Resume => match self.inner.hub.resume(agent_id, "user") {
                Ok(status) => status,
                Err(e) => return Err(Error::BadRequest(e.message())),
            },
            AgentAction::Cancel => {
                // The loop writes its own final status a moment from now, so the
                // honest answer here is what was asked for: cancelled. Reporting
                // the pre-cancel status would be a lie, and reporting nothing
                // would make a stop button look broken.
                self.inner
                    .hub
                    .kill(&session_id, &[agent_id.to_string()], "user");
                AgentStatus::Cancelled
            }
        };
        if !matches!(action, AgentAction::Cancel) {
            let expected = match action {
                AgentAction::Pause => AgentStatus::Running.as_str(),
                AgentAction::Resume => AgentStatus::Paused.as_str(),
                AgentAction::Cancel => unreachable!(),
            };
            match self
                .inner
                .deps
                .store
                .transition_agent_status(agent_id, &[expected], status.as_str(), events::now_ms())
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    // A terminal full-row write won while the live flag was
                    // being changed. Report the state that actually persists;
                    // never publish a pause/resume success that did not land.
                    let summary = self.inner.deps.store.get_agent(agent_id).await?;
                    let Some(summary) = summary else {
                        return Err(Error::NotFound(format!(
                            "agent {agent_id} disappeared while it was being controlled"
                        )));
                    };
                    return AgentStatus::parse(&summary.agent.status).ok_or_else(|| {
                        Error::Store(format!(
                            "agent {agent_id} has unknown status {:?}",
                            summary.agent.status
                        ))
                    });
                }
                Err(e) => {
                    // Preserve the existing live-control contract on a store
                    // outage: the pause flag is set and the caller gets the
                    // actual live state, with the persistence failure logged.
                    tracing::warn!(agent = %agent_id, error = %e, "could not persist an agent status");
                }
            }
        }
        self.inner.emit(
            Some(&session_id),
            None,
            Some(agent_id),
            kc_core::events::Event::AgentPaused {
                agent_id: agent_id.to_string(),
                paused: matches!(action, AgentAction::Pause),
                by: "user".to_string(),
            },
        );
        Ok(status)
    }

    /// Live agents in one session, for a caller that wants the roster without a
    /// round trip to the store (the store has the history; this has the truth
    /// about what is running right now).
    pub fn live_agent_ids(&self, session_id: &str) -> Vec<String> {
        self.inner
            .hub
            .list(session_id)
            .iter()
            .map(|agent| agent.id.0.clone())
            .collect()
    }

    /// How many agents are live in a session right now.
    pub fn live_agent_count(&self, session_id: &str) -> usize {
        self.inner.hub.live_count(session_id)
    }

    /// The highest number of subagent loops that have run at the same time since
    /// this runtime started.
    ///
    /// A high-water mark, not a current count: it is what makes "did these
    /// actually overlap" observable from a test, a metrics line or a report,
    /// without having to sample a moving number.
    pub fn subagents_peak(&self) -> usize {
        self.inner.subagents.peak()
    }

    /// Subagent loops running right now.
    pub fn subagents_active(&self) -> usize {
        self.inner.subagents.active()
    }

    /// Cancel every run and wait briefly for the loops to unwind.
    pub async fn shutdown(&self) {
        {
            let runs = self.inner.runs.lock().expect("runs lock");
            for state in runs.values() {
                state.cancel.cancel();
            }
        }
        // Wake gates parked on an approval: their cancel token will also fire,
        // but this makes the unwinding immediate and releases every waiter.
        self.inner.approvals.cancel_all();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let empty = self.inner.runs.lock().map(|r| r.is_empty()).unwrap_or(true);
            if empty {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tracing::warn!("runtime shutdown: some runs did not stop within 5s");
    }
}

impl RuntimeInner {
    pub(crate) async fn lock_session_lifecycle(
        &self,
        session_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .session_lifecycle
                .lock()
                .expect("session lifecycle lock");
            Arc::clone(
                locks
                    .entry(session_id.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        lock.lock_owned().await
    }

    pub(crate) fn tool_view(&self) -> Arc<ToolRegistry> {
        self.tools.read().expect("tools lock").clone()
    }

    /// Take everything queued for a session. Called by the run loop.
    ///
    /// Emits [`kc_core::events::Event::QueuedMessages`] when it took anything, so
    /// the pending panel empties the moment the loop folds the messages in rather
    /// than lingering until the next queue mutation.
    pub(crate) fn drain_steers(&self, session_id: &str) -> Vec<QueuedSteer> {
        let drained: Vec<QueuedSteer> = {
            let mut queues = self.steers.lock().expect("steers lock");
            let drained: Vec<QueuedSteer> = queues
                .get_mut(session_id)
                .map(|queue| queue.drain(..).collect())
                .unwrap_or_default();
            if queues.get(session_id).is_some_and(VecDeque::is_empty) {
                queues.remove(session_id);
            }
            drained
        };
        if !drained.is_empty() {
            let _publication = self
                .queue_publication
                .lock()
                .expect("queue publication lock");
            let follow_ups = self
                .follow_ups
                .lock()
                .expect("follow ups lock")
                .get(session_id)
                .map(|queue| queue.iter().cloned().collect())
                .unwrap_or_default();
            self.emit(
                Some(session_id),
                None,
                None,
                kc_core::events::Event::QueuedMessages {
                    session_id: session_id.to_string(),
                    steers: Vec::new(),
                    follow_ups,
                },
            );
        }
        drained
    }
}

/// True when [`SKIP_RECOVERY_ENV`] asks startup recovery to skip the transcript
/// repair pass. Unset, empty, `0` and `false` all mean "do the repair".
fn skip_recovery_requested() -> bool {
    parse_skip_recovery(std::env::var(SKIP_RECOVERY_ENV).ok().as_deref())
}

/// The parsing half of [`skip_recovery_requested`], split out so it can be
/// tested without mutating the process environment (which parallel tests share).
fn parse_skip_recovery(value: Option<&str>) -> bool {
    match value {
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            !(value.is_empty() || value == "0" || value == "false" || value == "no")
        }
        None => false,
    }
}

/// Startup-recovery behaviour: best-effort repair, bounded work, claim release.
///
/// These live here rather than in `tests.rs` because they exercise `recover()`
/// itself and the recovery policy constants in this module.
#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::history::MISSING_TOOL_RESULT;
    use crate::tests_support::{new_message, HarnessBuilder, Script};
    use kc_core::message::ToolCall;
    use kc_core::store::{AgentRecord, Store};

    /// An assistant turn that asked for a tool and never got a result: exactly
    /// what a crash mid-exchange leaves behind, and what repair must synthesize.
    async fn broken_exchange(store: &FakeStoreHandle, session: &str, call_id: &str) {
        let mut assistant = new_message(session, "assistant", "");
        assistant.tool_calls = vec![ToolCall {
            id: call_id.to_string(),
            name: "shell".into(),
            arguments: "{}".into(),
        }];
        store.append_message(&assistant).await.expect("append");
    }

    type FakeStoreHandle = Arc<crate::tests_support::FakeStore>;

    fn agent(id: &str, session: &str, status: &str) -> AgentRecord {
        AgentRecord {
            id: id.to_string(),
            session_id: session.to_string(),
            parent_run_id: "r_1".into(),
            parent_agent_id: None,
            name: id.to_string(),
            role: "worker".into(),
            task: "task".into(),
            status: status.to_string(),
            model: "fake-model".into(),
            iterations: 1,
            summary: String::new(),
            error: None,
            owned_paths: Vec::new(),
            created_at: 0,
            updated_at: 0,
            finished_at: None,
            background: false,
            note: String::new(),
            progress: None,
        }
    }

    /// One session's repair fails (its synthesized tool result cannot be
    /// persisted); the pass must log and count it, keep going, and still return
    /// `Ok` so the server starts.
    #[tokio::test]
    async fn recover_continues_past_a_session_whose_repair_fails() {
        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let store: FakeStoreHandle = h.store.clone();

        // Session A needs a synthesized result, so its repair must append.
        broken_exchange(&store, h.session(), "call_a").await;

        // Session B also needs repair, but the synthesized row is already on
        // disk, so its repair succeeds without appending anything.
        let other = "s_other";
        store
            .create_session(other, "other", "/tmp", None)
            .await
            .expect("create session");
        broken_exchange(&store, other, "call_b").await;
        let mut persisted = new_message(other, "tool", MISSING_TOOL_RESULT);
        persisted.tool_call_id = Some("call_b".into());
        persisted.name = Some("shell".into());
        store.append_message(&persisted).await.expect("append");

        // Every append now fails: only session A's repair needs one.
        store.fail_appends(true);
        let report = h.runtime.recover().await.expect("recovery must not abort");

        assert_eq!(report.failed_sessions, 1, "the failing session is counted");
        assert_eq!(
            report.repaired_sessions, 1,
            "the other session still recovers"
        );
        assert_eq!(report.skipped_sessions, 0);

        // The failure is not sticky: once the store accepts writes again the
        // next startup repairs the session it could not repair before.
        store.fail_appends(false);
        let again = h.runtime.recover().await.expect("recover again");
        assert_eq!(again.failed_sessions, 0);
        assert_eq!(again.repaired_sessions, 2);
        assert!(
            store
                .messages()
                .iter()
                .any(|m| m.role == "tool" && m.content == MISSING_TOOL_RESULT),
            "the retried repair must persist the synthesized result"
        );
    }

    /// A crash leaves a subagent's claims behind. Recovery must release them for
    /// the agents it marks interrupted - and only for those.
    #[tokio::test]
    async fn recover_releases_claims_of_interrupted_agents_only() {
        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let store: FakeStoreHandle = h.store.clone();
        let session = h.session();

        store
            .upsert_agent(&agent("a_dead", session, "running"))
            .await
            .expect("upsert");
        store
            .upsert_agent(&agent("a_done", session, "completed"))
            .await
            .expect("upsert");
        store
            .claim_paths(session, "a_dead", &["src/lib.rs".into()], "write")
            .await
            .expect("claim");
        store
            .claim_paths(session, "a_done", &["src/other.rs".into()], "write")
            .await
            .expect("claim");

        let report = h.runtime.recover().await.expect("recover");
        assert_eq!(report.interrupted_agents, 1);
        assert_eq!(report.released_claims, 1);

        let claims = store.claims();
        assert!(
            !claims.iter().any(|c| c.agent_id == "a_dead"),
            "a dead agent's claims must not outlive it: {claims:?}"
        );
        assert!(
            claims.iter().any(|c| c.agent_id == "a_done"),
            "a finished agent's claims are not recovery's business: {claims:?}"
        );
    }

    /// The startup pass is capped, so a large install cannot spend boot reading
    /// every row of every session. The rest are repaired by their first run.
    #[tokio::test]
    async fn recover_caps_how_many_sessions_it_repairs() {
        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let store: FakeStoreHandle = h.store.clone();

        // The harness already made one session; add enough to exceed the cap.
        for i in 0..RECOVERY_MAX_SESSIONS {
            store
                .create_session(&format!("s_cap_{i}"), "cap", "/tmp", None)
                .await
                .expect("create session");
        }
        let total = RECOVERY_MAX_SESSIONS + 1;

        let report = h.runtime.recover().await.expect("recover");
        assert_eq!(report.repaired_sessions, RECOVERY_MAX_SESSIONS);
        assert_eq!(report.skipped_sessions, total - RECOVERY_MAX_SESSIONS);
        assert_eq!(report.failed_sessions, 0);
    }

    #[test]
    fn skip_recovery_env_parsing() {
        for on in ["1", "true", "TRUE", "yes", " on "] {
            assert!(parse_skip_recovery(Some(on)), "{on:?} must skip recovery");
        }
        for off in ["", "0", "false", "no", "  "] {
            assert!(!parse_skip_recovery(Some(off)), "{off:?} must not skip");
        }
        assert!(!parse_skip_recovery(None), "unset must not skip recovery");
    }
}
