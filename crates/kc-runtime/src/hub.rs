//! The live half of the subagent system: a registry, mailboxes and pause gates.
//!
//! A subagent used to be invisible to the runtime once it was spawned: the
//! `spawn_agent` tool awaited its own child and the only record of it was the
//! `agents` row in SQLite. That is enough for a blocking spawn and nothing else,
//! so this module owns the three pieces a **parallel** one needs:
//!
//! * [`AgentHub`] - one entry per live agent ([`LiveAgent`]) with its cancel
//!   token, its pause gate, its mailbox and the transcript it is writing. It is
//!   in memory, so it holds exactly the agents whose loops exist right now; the
//!   store keeps the history of all of them.
//! * A **mailbox** per agent. `send_message` pushes into it and the recipient's
//!   loop drains it at the top of its next iteration, where the text becomes an
//!   ordinary user turn. Nothing interrupts a turn in flight, for the same reason
//!   a steer does not: aborting a model mid-plan throws work away and can persist
//!   a tool call with no result.
//! * A **pause gate** per agent. A paused agent finishes the turn it is on - a
//!   tool in flight has a file handle or a process group attached - and then
//!   parks before its next model call. That is the only point where stopping is
//!   free.
//!
//! # What owns what
//!
//! An agent is *owned* by the agent (or top-level run) that spawned it, and
//! ownership is transitive: the owner, and every owner of the owner, may pause,
//! resume, kill and inspect it. Messaging is open *within one session*, including
//! between siblings and upwards to a parent, because a child reporting a blocker
//! to its parent is the case the whole feature exists for. An explicit id in a
//! different session is not a live target even though this registry is global.
//!
//! # What this deliberately does not do
//!
//! There is no scheduler and no work queue. A background agent starts immediately
//! (subject to `subagents.max_concurrent` and `max_background`), because a model
//! that says "go" and then watches nothing happen is worse than one that was told
//! "no". Limits are refused at spawn time, never quietly queued for later.

use kc_core::ids::AgentId;
use kc_core::store::{AgentRecord, AgentStatus};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// A message waiting for a recipient's next iteration.
#[derive(Debug, Clone)]
pub struct AgentMessage {
    /// Who sent it: `None` for the human (or the HTTP API on their behalf).
    pub from: Option<String>,
    /// Display name of the sender, so the recipient can say "from reviewer-3f2a"
    /// without a second lookup that could race the roster.
    pub from_name: String,
    pub content: String,
    pub sent_at: i64,
}

/// How many messages one agent may have queued before `send_message` refuses.
///
/// The recipient drains one per iteration, so this is really "how far ahead may a
/// parent plan". A few dozen is a plan; hundreds is a confused agent talking to
/// itself, and it would be delivered into a context budget that cannot hold it.
pub const MAX_MAILBOX: usize = 32;

/// The actual result of trying to enqueue a message.
///
/// A full mailbox is deliberately distinct from an accepted message at depth 32:
/// only `Queued` may produce a delivered/queued event or increment a tool's
/// delivery count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailboxDelivery {
    Queued {
        depth: usize,
    },
    Full {
        depth: usize,
    },
    /// Unknown, already removed, or not in the caller's session. These cases are
    /// intentionally indistinguishable to a model caller so an id cannot be used
    /// to inspect or inject into another session.
    NotLive,
}

/// One live agent: everything the runtime needs to steer it.
pub(crate) struct LiveAgent {
    pub id: AgentId,
    pub session_id: String,
    pub name: String,
    pub role: String,
    pub task: String,
    pub model: String,
    /// `None` for an agent spawned by a top-level run.
    pub parent_agent_id: Option<String>,
    pub run_id: String,
    /// The depth this agent runs at; its own children are one deeper.
    pub depth: u8,
    pub background: bool,
    pub started_at: i64,
    /// Cancels this agent's loop (and, transitively, its tools).
    pub cancel: CancellationToken,
    pub mailbox: Mutex<VecDeque<AgentMessage>>,
    /// Woken whenever the mailbox gains a message or the pause state changes.
    pub wake: Notify,
    /// Serializes the live pause flag with the caller's persisted status write.
    /// Every pause/resume path, including HTTP and a model tool, must hold this
    /// guard until its conditional store transition has finished.
    control: Arc<AsyncMutex<()>>,
    /// `true` while the agent should not start another iteration.
    pub paused: Mutex<bool>,
    /// A human (or a kill_agents call) asked for this to stop, as opposed to a
    /// parent being cancelled - the two are reported differently.
    pub killed: Mutex<bool>,
    /// Set to the agent's final status when its loop settles. A waiter polls it.
    pub settled: Mutex<Option<AgentStatus>>,
    /// Notified when `settled` becomes `Some`.
    pub done: Notify,
    /// The newest status the agent reported about itself.
    pub note: Mutex<String>,
    pub progress: Mutex<Option<f32>>,
    /// When the loop last started an iteration. `None` until it reports one, so a
    /// watcher can tell "starting up" from "idle for twenty minutes".
    pub last_seen_ms: std::sync::atomic::AtomicI64,
}

impl LiveAgent {
    fn new(record: &AgentRecord, run_id: String, depth: u8, cancel: CancellationToken) -> Self {
        Self {
            id: AgentId(record.id.clone()),
            session_id: record.session_id.clone(),
            name: record.name.clone(),
            role: record.role.clone(),
            task: record.task.clone(),
            model: record.model.clone(),
            parent_agent_id: record.parent_agent_id.clone(),
            run_id,
            depth,
            background: record.background,
            started_at: record.created_at,
            cancel,
            mailbox: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            control: Arc::new(AsyncMutex::new(())),
            paused: Mutex::new(false),
            killed: Mutex::new(false),
            settled: Mutex::new(None),
            done: Notify::new(),
            note: Mutex::new(record.note.clone()),
            progress: Mutex::new(record.progress),
            last_seen_ms: std::sync::atomic::AtomicI64::new(record.created_at),
        }
    }

    /// Whether a pause request has landed but the loop has not parked yet.
    pub fn is_paused(&self) -> bool {
        *self.paused.lock().expect("paused lock")
    }

    fn set_paused(&self, paused: bool) {
        *self.paused.lock().expect("paused lock") = paused;
        // A resume must reach a loop parked in `wait_while_paused` even though no
        // message arrived; a pause needs no wake, because the loop checks the
        // flag on its own way into the next iteration.
        self.wake.notify_waiters();
    }

    /// Take everything waiting, oldest first.
    pub fn take_mail(&self) -> Vec<AgentMessage> {
        let mut queued = self.mailbox.lock().expect("mailbox lock");
        queued.drain(..).collect()
    }

    pub fn queued_mail(&self) -> usize {
        self.mailbox.lock().expect("mailbox lock").len()
    }
}

/// An exclusive pause/resume operation on one agent.
///
/// The guard is async because the caller must keep it across the store await. A
/// second control operation cannot change either the live flag or the persisted
/// status's expected predecessor until the first operation has published its
/// result.
pub(crate) struct AgentControlGuard {
    live: Arc<LiveAgent>,
    _lock: OwnedMutexGuard<()>,
}

impl AgentControlGuard {
    pub fn live(&self) -> &Arc<LiveAgent> {
        &self.live
    }

    /// Pause `id`. Returns the status it was in, for the event and the caller.
    pub fn pause(&self, by: &str) -> Result<AgentStatus, ControlError> {
        let id = &self.live.id.0;
        if self.live.cancel.is_cancelled() {
            return Err(ControlError::NotLive {
                agent: id.clone(),
                status: AgentStatus::Cancelled.as_str().to_string(),
            });
        }
        let mut paused = self.live.paused.lock().expect("paused lock");
        if *paused {
            // Idempotent: a second pause is a no-op, not an error. A parent that
            // pauses a subtree twice should not have to care which call landed.
            return Ok(AgentStatus::Paused);
        }
        *paused = true;
        drop(paused);
        tracing::debug!(agent = %id, by, "paused a subagent");
        Ok(AgentStatus::Paused)
    }

    /// Lift a pause and wake a parked loop.
    pub fn resume(&self, by: &str) -> Result<AgentStatus, ControlError> {
        let id = &self.live.id.0;
        if self.live.cancel.is_cancelled() {
            return Ok(AgentStatus::Cancelled);
        }
        self.live.set_paused(false);
        tracing::debug!(agent = %id, by, "resumed a subagent");
        Ok(AgentStatus::Running)
    }
}

/// Every live agent in one runtime.
#[derive(Default)]
pub(crate) struct AgentHub {
    agents: Mutex<HashMap<String, Arc<LiveAgent>>>,
    /// The status each agent settled with, kept after its entry is removed so a
    /// `wait_for_agents` that raced the removal still gets a real answer instead
    /// of "unknown agent". Bounded by the prune below.
    finished: Mutex<HashMap<String, AgentStatus>>,
    /// Mail addressed to a *top-level run*, keyed by session and exact run id.
    ///
    /// An entry's existence is also its liveness registration. Registration is
    /// explicit; delivery never creates an entry, so a late child cannot recreate
    /// a retired mailbox after cleanup. An empty active entry remains until clear.
    /// The run loop drains mail where it drains user steers: a message to a loop
    /// is an ordinary user turn at the top of an iteration, never an interruption.
    run_mail: Mutex<HashMap<(String, String), VecDeque<AgentMessage>>>,
}

/// How many settled agent statuses are remembered for a late waiter.
const FINISHED_MEMORY: usize = 256;

/// Why a control request was refused. Rendered as the tool result, so the message
/// has to name the agent and the reason a model can act on.
#[derive(Debug)]
pub enum ControlError {
    Unknown(String),
    NotOwned { agent: String, owner: String },
    NotLive { agent: String, status: String },
}

impl ControlError {
    pub fn message(&self) -> String {
        match self {
            ControlError::Unknown(id) => {
                format!("no agent `{id}` exists in this session")
            }
            ControlError::NotOwned { agent, owner } => format!(
                "agent `{agent}` is not yours to control; it belongs to {owner} (you can message it, but not stop it)"
            ),
            ControlError::NotLive { agent, status } => {
                format!("agent `{agent}` is not running (status: {status})")
            }
        }
    }
}

impl AgentHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an agent's live entry. The record must already be persisted.
    pub fn insert(
        &self,
        record: &AgentRecord,
        run_id: String,
        depth: u8,
        cancel: CancellationToken,
    ) -> Arc<LiveAgent> {
        let live = Arc::new(LiveAgent::new(record, run_id, depth, cancel));
        self.agents
            .lock()
            .expect("agents lock")
            .insert(record.id.clone(), Arc::clone(&live));
        live
    }

    pub fn get(&self, id: &str) -> Option<Arc<LiveAgent>> {
        self.agents.lock().expect("agents lock").get(id).cloned()
    }

    /// Serialize a pause/resume operation and its caller's store transition.
    ///
    /// Revalidate after waiting: an agent may finish while its predecessor holds
    /// the guard, in which case controlling the removed `Arc` would be a phantom
    /// success. Cancellation deliberately does not use this guard; a slow store
    /// must never delay a stop request.
    pub async fn control(&self, id: &str) -> Result<AgentControlGuard, ControlError> {
        let live = self
            .get(id)
            .ok_or_else(|| ControlError::Unknown(id.to_string()))?;
        let lock = Arc::clone(&live.control).lock_owned().await;
        let still_live = self.get(id).is_some_and(|current| {
            Arc::ptr_eq(&current, &live) && live.settled.lock().expect("settled lock").is_none()
        });
        if !still_live {
            return Err(ControlError::Unknown(id.to_string()));
        }
        Ok(AgentControlGuard { live, _lock: lock })
    }

    /// Remove an agent's live entry, remembering how it settled.
    pub fn finish(&self, id: &str, status: AgentStatus) {
        {
            let mut finished = self.finished.lock().expect("finished lock");
            if finished.len() >= FINISHED_MEMORY {
                // The map is only a courtesy for a waiter that lost the race, so
                // dropping the whole table is fine and cheaper than tracking LRU
                // order. A waiter that misses still sees the store's row.
                finished.clear();
            }
            finished.insert(id.to_string(), status);
        }
        self.agents.lock().expect("agents lock").remove(id);
    }

    /// Test access to the remembered settlement status. Production named waits
    /// use the owned persisted row so they can return its real summary as well.
    /// Checks the live map first: a running agent is never mistaken for settled.
    #[cfg(test)]
    pub fn settled_status(&self, id: &str) -> Option<AgentStatus> {
        if let Some(live) = self.get(id) {
            return *live.settled.lock().expect("settled lock");
        }
        self.finished
            .lock()
            .expect("finished lock")
            .get(id)
            .copied()
    }

    /// Mark a live entry's own `settled` (called just before [`Self::finish`]).
    ///
    /// Separate from [`Self::finish`] so the ordering is explicit: a waiter parked
    /// on this agent has to see the real status *before* the entry disappears.
    pub fn mark_settled(live: &LiveAgent, status: AgentStatus) {
        *live.settled.lock().expect("settled lock") = Some(status);
        live.done.notify_waiters();
    }

    /// Every live agent in a session, newest first.
    pub fn list(&self, session_id: &str) -> Vec<Arc<LiveAgent>> {
        self.list_all()
            .into_iter()
            .filter(|agent| agent.session_id == session_id)
            .collect()
    }

    /// Every live agent in the runtime, newest first.
    ///
    /// A run-end reap is scoped by `run_id`, not by session: a child carries the
    /// run that spawned it, so one filter finds the whole subtree without also
    /// touching a sibling session that happens to share the id space.
    pub fn list_all(&self) -> Vec<Arc<LiveAgent>> {
        let agents = self.agents.lock().expect("agents lock");
        let mut out: Vec<Arc<LiveAgent>> = agents.values().cloned().collect();
        out.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.id.0.cmp(&b.id.0))
        });
        out
    }

    /// Live agents spawned by `owner`, at any depth below it.
    ///
    /// Ownership is transitive, so this is the subtree below `owner` - which is
    /// what `kill_agents` with no arguments means.
    pub fn descendants_of(&self, session_id: &str, owner: Option<&str>) -> Vec<Arc<LiveAgent>> {
        let all = self.list(session_id);
        let Some(owner) = owner else {
            return all;
        };
        let mut subtree: std::collections::HashSet<String> = std::collections::HashSet::new();
        subtree.insert(owner.to_string());
        // One pass per depth level: the map is small (tens of agents) and this
        // avoids sorting by depth just to walk a tree.
        loop {
            let before = subtree.len();
            for agent in &all {
                if let Some(parent) = &agent.parent_agent_id {
                    if subtree.contains(parent) {
                        subtree.insert(agent.id.0.clone());
                    }
                }
            }
            if subtree.len() == before {
                break;
            }
        }
        all.into_iter()
            .filter(|agent| subtree.contains(&agent.id.0) && agent.id.0 != owner)
            .collect()
    }

    /// How many live agents a session currently holds.
    pub fn live_count(&self, session_id: &str) -> usize {
        let agents = self.agents.lock().expect("agents lock");
        agents
            .values()
            .filter(|agent| agent.session_id == session_id)
            .count()
    }

    /// How many *background* agents a session currently holds.
    pub fn background_count(&self, session_id: &str) -> usize {
        let agents = self.agents.lock().expect("agents lock");
        agents
            .values()
            .filter(|agent| agent.session_id == session_id && agent.background)
            .count()
    }

    /// Queue one message for a live agent in the sender's own session.
    ///
    /// The hub is global, so the session check belongs here immediately before
    /// the mailbox write rather than in a distant tool-level precheck. `Full`
    /// means the message was dropped, including when `depth == MAX_MAILBOX`.
    pub fn deliver_in_session(
        &self,
        session_id: &str,
        to: &str,
        message: AgentMessage,
    ) -> MailboxDelivery {
        let Some(live) = self.get(to) else {
            return MailboxDelivery::NotLive;
        };
        if live.session_id != session_id || live.settled.lock().expect("settled lock").is_some() {
            return MailboxDelivery::NotLive;
        }
        let depth = {
            let mut queue = live.mailbox.lock().expect("mailbox lock");
            if queue.len() >= MAX_MAILBOX {
                return MailboxDelivery::Full { depth: queue.len() };
            }
            queue.push_back(message);
            queue.len()
        };
        live.wake.notify_waiters();
        MailboxDelivery::Queued { depth }
    }

    /// Cancel `id` and everything below it. Returns the agents that were asked to
    /// stop, so the caller can report a count rather than a boolean.
    pub fn kill(&self, session_id: &str, ids: &[String], by: &str) -> Vec<String> {
        let mut killed = Vec::new();
        for id in ids {
            let Some(live) = self.get(id) else { continue };
            // Mark before cancelling: the loop reads `killed` when it builds its
            // final status, and it may observe the token immediately.
            *live.killed.lock().expect("killed lock") = true;
            live.cancel.cancel();
            // A killed agent may be parked on the pause gate; the token alone
            // would leave it there until something else woke it.
            live.wake.notify_waiters();
            killed.push(id.clone());
            tracing::debug!(agent = %id, by, "killed a subagent");
        }
        let _ = session_id;
        killed
    }

    /// Record a self-reported status and wake nobody (there is nothing to wake:
    /// a note is not a control signal).
    pub fn note(&self, id: &str, note: String, progress: Option<f32>) {
        if let Some(live) = self.get(id) {
            *live.note.lock().expect("note lock") = note;
            *live.progress.lock().expect("progress lock") = progress;
        }
    }

    /// Register the mailbox for one newly admitted top-level run.
    ///
    /// Only this lifecycle operation creates a generation. `deliver_to_run` may
    /// fill it and `take_run_mail` may empty it, but neither can resurrect it after
    /// `clear_run_mail` has retired the run.
    pub fn register_run_mail(&self, session_id: &str, run_id: &str) {
        let key = (session_id.to_string(), run_id.to_string());
        let previous = self
            .run_mail
            .lock()
            .expect("run mail lock")
            .insert(key, VecDeque::new());
        debug_assert!(previous.is_none(), "run mailbox registered twice");
    }

    /// Queue one message for the sender's exact top-level run generation.
    ///
    /// A late unwinding child gets `NotLive` after its parent run is retired. It
    /// cannot create an ownerless queue that a later run in the session would
    /// inherit. Normal same-run parent delivery and the 32/33 bound are unchanged.
    pub fn deliver_to_run(
        &self,
        session_id: &str,
        run_id: &str,
        message: AgentMessage,
    ) -> MailboxDelivery {
        let key = (session_id.to_string(), run_id.to_string());
        let mut mail = self.run_mail.lock().expect("run mail lock");
        let Some(queue) = mail.get_mut(&key) else {
            return MailboxDelivery::NotLive;
        };
        if queue.len() >= MAX_MAILBOX {
            return MailboxDelivery::Full { depth: queue.len() };
        }
        queue.push_back(message);
        MailboxDelivery::Queued { depth: queue.len() }
    }

    /// Take everything addressed to one exact run generation, oldest first.
    ///
    /// Keep the empty entry: it is the active-generation registration, not garbage
    /// for a late sender to interpret as an idle session queue.
    pub fn take_run_mail(&self, session_id: &str, run_id: &str) -> Vec<AgentMessage> {
        let key = (session_id.to_string(), run_id.to_string());
        self.run_mail
            .lock()
            .expect("run mail lock")
            .get_mut(&key)
            .map(|queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    /// Retire and drop exactly one run generation's mailbox.
    ///
    /// Cleanup is idempotent and cannot clear the successor's queue, even when an
    /// outer panic guard repeats finalization for an older run.
    pub fn clear_run_mail(&self, session_id: &str, run_id: &str) {
        self.run_mail
            .lock()
            .expect("run mail lock")
            .remove(&(session_id.to_string(), run_id.to_string()));
    }
}

/// The pause gate one loop checks before each iteration.
///
/// It is deliberately a plain struct with a loop rather than a semaphore: the
/// condition a park waits on is "not paused", which a resume sets directly, and a
/// paused agent has to stay visible in the roster the whole time.
pub(crate) async fn wait_while_paused(
    live: &LiveAgent,
    cancel: &CancellationToken,
    deadline: Instant,
) -> PauseExit {
    loop {
        if cancel.is_cancelled() {
            return PauseExit::Cancelled;
        }
        if !live.is_paused() {
            return PauseExit::Running;
        }
        let now = Instant::now();
        if now >= deadline {
            return PauseExit::Deadline;
        }
        // Registration before the check above would be racy; re-check and then
        // wait on a notified future created first, so a resume between the check
        // and the await cannot be missed.
        let notified = live.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !live.is_paused() {
            return PauseExit::Running;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return PauseExit::Cancelled,
            _ = tokio::time::sleep_until(deadline) => return PauseExit::Deadline,
            _ = notified => {}
        }
    }
}

/// Why a parked loop woke up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseExit {
    Running,
    Cancelled,
    Deadline,
}

/// Wait until `id` settles or `timeout` passes. `Ok(status)` when it settled,
/// `Err(None)` on timeout, `Err(Some(status))` when the wait itself was
/// interrupted by the waiter's own cancellation.
pub(crate) async fn wait_for_settled(
    live: &Arc<LiveAgent>,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<AgentStatus, Option<AgentStatus>> {
    if let Some(status) = *live.settled.lock().expect("settled lock") {
        return Ok(status);
    }
    let deadline = Instant::now() + timeout;
    loop {
        let notified = live.done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(status) = *live.settled.lock().expect("settled lock") {
            return Ok(status);
        }
        if cancel.is_cancelled() {
            return Err(*live.settled.lock().expect("settled lock"));
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(*live.settled.lock().expect("settled lock")),
            _ = tokio::time::sleep_until(deadline) => return Err(None),
            _ = notified => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::store::AgentRecord;

    fn record(id: &str, parent: Option<&str>, background: bool) -> AgentRecord {
        AgentRecord {
            id: id.to_string(),
            session_id: "s_1".to_string(),
            parent_run_id: "r_1".to_string(),
            parent_agent_id: parent.map(str::to_string),
            name: id.to_string(),
            role: "worker".to_string(),
            task: "task".to_string(),
            status: "running".to_string(),
            model: "m".to_string(),
            iterations: 0,
            summary: String::new(),
            error: None,
            owned_paths: Vec::new(),
            created_at: crate::events::now_ms(),
            updated_at: crate::events::now_ms(),
            finished_at: None,
            background,
            note: String::new(),
            progress: None,
        }
    }

    #[test]
    fn descendants_are_transitive_and_exclude_the_owner() {
        let hub = AgentHub::new();
        hub.insert(
            &record("a_root", None, false),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        hub.insert(
            &record("a_child", Some("a_root"), false),
            "r_1".into(),
            2,
            CancellationToken::new(),
        );
        hub.insert(
            &record("a_grandchild", Some("a_child"), false),
            "r_1".into(),
            3,
            CancellationToken::new(),
        );
        hub.insert(
            &record("a_other", None, false),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );

        let mut ids: Vec<String> = hub
            .descendants_of("s_1", Some("a_root"))
            .iter()
            .map(|agent| agent.id.0.clone())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["a_child", "a_grandchild"]);

        // No owner means every live agent in the session, which is what a
        // top-level run's "kill everything I started" has to mean.
        assert_eq!(hub.descendants_of("s_1", None).len(), 4);
    }

    #[test]
    fn a_delivered_message_survives_until_it_is_taken() {
        let hub = AgentHub::new();
        hub.insert(
            &record("a_1", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        let message = AgentMessage {
            from: None,
            from_name: "parent".into(),
            content: "look at the parser".into(),
            sent_at: 0,
        };
        assert_eq!(
            hub.deliver_in_session("s_1", "a_1", message.clone()),
            MailboxDelivery::Queued { depth: 1 }
        );
        assert_eq!(
            hub.deliver_in_session("s_1", "a_1", message),
            MailboxDelivery::Queued { depth: 2 }
        );
        assert_eq!(hub.get("a_1").unwrap().queued_mail(), 2);
        assert_eq!(hub.get("a_1").unwrap().take_mail().len(), 2);
        assert_eq!(hub.get("a_1").unwrap().queued_mail(), 0);
        // An unknown recipient is not an error here; the tool reports it.
        assert_eq!(
            hub.deliver_in_session(
                "s_1",
                "nope",
                AgentMessage {
                    from: None,
                    from_name: String::new(),
                    content: String::new(),
                    sent_at: 0,
                }
            ),
            MailboxDelivery::NotLive
        );
    }

    #[test]
    fn mailbox_depth_32_is_accepted_and_33_is_typed_full() {
        fn message(content: String) -> AgentMessage {
            AgentMessage {
                from: Some("sender".into()),
                from_name: "sender".into(),
                content,
                sent_at: 0,
            }
        }

        let hub = AgentHub::new();
        let live = hub.insert(
            &record("a_full", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        for index in 0..MAX_MAILBOX {
            assert_eq!(
                hub.deliver_in_session("s_1", "a_full", message(format!("agent-{index}"))),
                MailboxDelivery::Queued { depth: index + 1 }
            );
        }
        assert_eq!(
            hub.deliver_in_session("s_1", "a_full", message("dropped-agent-33".into())),
            MailboxDelivery::Full { depth: MAX_MAILBOX }
        );
        let queued = live.take_mail();
        assert_eq!(queued.len(), MAX_MAILBOX);
        assert!(queued.iter().all(|mail| mail.content != "dropped-agent-33"));

        // A registered top-level run has the same 32/33 boundary and typed-full
        // rejection as a live agent mailbox.
        hub.register_run_mail("s_1", "r_1");
        for index in 0..MAX_MAILBOX {
            assert_eq!(
                hub.deliver_to_run("s_1", "r_1", message(format!("run-{index}"))),
                MailboxDelivery::Queued { depth: index + 1 }
            );
        }
        assert_eq!(
            hub.deliver_to_run("s_1", "r_1", message("dropped-run-33".into())),
            MailboxDelivery::Full { depth: MAX_MAILBOX }
        );
        let queued = hub.take_run_mail("s_1", "r_1");
        assert_eq!(queued.len(), MAX_MAILBOX);
        assert!(queued.iter().all(|mail| mail.content != "dropped-run-33"));
    }

    #[test]
    fn late_mail_after_run_cleanup_cannot_recreate_a_successors_queue() {
        fn mail(content: &str) -> AgentMessage {
            AgentMessage {
                from: Some("a_old".into()),
                from_name: "old child".into(),
                content: content.to_string(),
                sent_at: 0,
            }
        }

        let hub = AgentHub::new();
        hub.register_run_mail("s_1", "r_old");
        assert_eq!(
            hub.deliver_to_run("s_1", "r_old", mail("delivered before retirement")),
            MailboxDelivery::Queued { depth: 1 }
        );
        hub.clear_run_mail("s_1", "r_old");
        assert!(hub.take_run_mail("s_1", "r_old").is_empty());

        // Retirement can win before delivery. This is the late unwinding child:
        // it is rejected and, crucially, no fresh ownerless entry is inserted.
        assert_eq!(
            hub.deliver_to_run("s_1", "r_old", mail("retired instruction")),
            MailboxDelivery::NotLive
        );
        hub.register_run_mail("s_1", "r_new");
        assert_eq!(
            hub.deliver_to_run("s_1", "r_old", mail("late after successor start")),
            MailboxDelivery::NotLive
        );
        assert!(hub.take_run_mail("s_1", "r_new").is_empty());

        // The inverse lock order is exact too: old-run cleanup after successor
        // registration/delivery must not drain the successor's legitimate mail.
        assert_eq!(
            hub.deliver_to_run("s_1", "r_new", mail("current parent instruction")),
            MailboxDelivery::Queued { depth: 1 }
        );
        hub.clear_run_mail("s_1", "r_old");
        let current = hub.take_run_mail("s_1", "r_new");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].content, "current parent instruction");
        assert_eq!(
            hub.deliver_to_run("s_1", "r_new", mail("same run remains active")),
            MailboxDelivery::Queued { depth: 1 },
            "draining an active run must not accidentally retire it"
        );
        hub.clear_run_mail("s_1", "r_new");
        assert_eq!(
            hub.deliver_to_run("s_1", "r_new", mail("settled current run")),
            MailboxDelivery::NotLive
        );
    }

    #[test]
    fn a_global_hub_never_delivers_to_another_sessions_agent() {
        let hub = AgentHub::new();
        let mut foreign = record("a_foreign", None, true);
        foreign.session_id = "s_other".into();
        let live = hub.insert(&foreign, "r_other".into(), 1, CancellationToken::new());
        assert_eq!(
            hub.deliver_in_session(
                "s_1",
                "a_foreign",
                AgentMessage {
                    from: Some("a_sender".into()),
                    from_name: "sender".into(),
                    content: "cross-session prompt".into(),
                    sent_at: 0,
                },
            ),
            MailboxDelivery::NotLive
        );
        assert_eq!(live.queued_mail(), 0);
    }

    #[tokio::test]
    async fn control_guard_excludes_a_second_pause_or_resume() {
        let hub = Arc::new(AgentHub::new());
        hub.insert(
            &record("a_control", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        let first = hub.control("a_control").await.unwrap();
        let second = {
            let hub = Arc::clone(&hub);
            tokio::spawn(async move { hub.control("a_control").await.unwrap() })
        };
        tokio::task::yield_now().await;
        assert!(!second.is_finished());
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("the waiting control operation should acquire the guard")
            .unwrap();
        second.resume("test").unwrap();
    }

    #[test]
    fn a_finished_agent_still_reports_how_it_settled() {
        let hub = AgentHub::new();
        let live = hub.insert(
            &record("a_1", None, false),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        assert_eq!(hub.settled_status("a_1"), None);
        AgentHub::mark_settled(&live, AgentStatus::Completed);
        hub.finish("a_1", AgentStatus::Completed);
        assert!(hub.get("a_1").is_none());
        assert_eq!(hub.settled_status("a_1"), Some(AgentStatus::Completed));
        // An id nobody ever saw is unknown, not "done".
        assert_eq!(hub.settled_status("a_none"), None);
    }

    #[test]
    fn counts_separate_background_from_blocking() {
        let hub = AgentHub::new();
        hub.insert(
            &record("a_bg", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        hub.insert(
            &record("a_fg", None, false),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        assert_eq!(hub.live_count("s_1"), 2);
        assert_eq!(hub.background_count("s_1"), 1);
        assert_eq!(hub.live_count("s_other"), 0);
    }

    #[tokio::test]
    async fn pause_parks_and_resume_releases_a_waiter() {
        let hub = AgentHub::new();
        let live = hub.insert(
            &record("a_1", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        let cancel = CancellationToken::new();
        let far = Instant::now() + Duration::from_secs(30);

        hub.control("a_1").await.unwrap().pause("user").unwrap();
        assert!(live.is_paused());
        // A second pause is idempotent.
        assert!(hub.control("a_1").await.unwrap().pause("user").is_ok());

        let waiting = {
            let live = Arc::clone(&live);
            let cancel = cancel.clone();
            tokio::spawn(async move { wait_while_paused(&live, &cancel, far).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished(), "a paused loop must park");

        hub.control("a_1").await.unwrap().resume("user").unwrap();
        assert_eq!(waiting.await.unwrap(), PauseExit::Running);
    }

    #[tokio::test]
    async fn kill_wakes_a_paused_loop() {
        let hub = AgentHub::new();
        hub.insert(
            &record("a_1", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        let live = hub.get("a_1").unwrap();
        hub.control("a_1").await.unwrap().pause("user").unwrap();
        let cancel = live.cancel.clone();
        let far = Instant::now() + Duration::from_secs(30);
        let waiting = {
            let live = Arc::clone(&live);
            let cancel = cancel.clone();
            tokio::spawn(async move { wait_while_paused(&live, &cancel, far).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(hub.kill("s_1", &["a_1".to_string()], "user").len() == 1);
        assert_eq!(waiting.await.unwrap(), PauseExit::Cancelled);
    }

    #[tokio::test]
    async fn waiting_for_a_settled_agent_reports_both_outcomes() {
        let hub = AgentHub::new();
        let live = hub.insert(
            &record("a_1", None, true),
            "r_1".into(),
            1,
            CancellationToken::new(),
        );
        let cancel = CancellationToken::new();

        // Times out while the agent is still working.
        let slow = wait_for_settled(&live, Duration::from_millis(20), &cancel).await;
        assert_eq!(slow, Err(None));

        let settled = {
            let live = Arc::clone(&live);
            let hub_ptr = Arc::new(());
            let _ = hub_ptr;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                AgentHub::mark_settled(&live, AgentStatus::Completed);
            })
        };
        let result = wait_for_settled(&live, Duration::from_secs(2), &cancel).await;
        assert_eq!(result, Ok(AgentStatus::Completed));
        settled.await.unwrap();
    }
}
