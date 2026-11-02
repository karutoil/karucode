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
//! resume, kill and inspect it. Messaging is open - any agent may message any
//! other, including its parent - because a child reporting a blocker to its
//! parent is the case the whole feature exists for.
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
use tokio::sync::Notify;
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

/// Every live agent in one runtime.
#[derive(Default)]
pub(crate) struct AgentHub {
    agents: Mutex<HashMap<String, Arc<LiveAgent>>>,
    /// The status each agent settled with, kept after its entry is removed so a
    /// `wait_for_agents` that raced the removal still gets a real answer instead
    /// of "unknown agent". Bounded by the prune below.
    finished: Mutex<HashMap<String, AgentStatus>>,
    /// Mail addressed to a *top-level run*, per session.
    ///
    /// A child's "message my parent" means a run, not an agent, when it was
    /// spawned by the main agent - so this is the other half of the mailbox
    /// system, not a special case bolted on. The run loop drains it in the same
    /// place it drains user steers, which keeps one rule: a message to a loop is
    /// an ordinary user turn at the top of an iteration, never an interruption.
    run_mail: Mutex<HashMap<String, VecDeque<AgentMessage>>>,
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

    /// How an agent settled, if it has. Checks the live map first: an agent that
    /// is still running is `None` even if a previous agent had the same id.
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

    /// Queue one message for `to`. Returns the recipient's queue depth after it,
    /// or `None` when the recipient is not live.
    pub fn deliver(&self, to: &str, message: AgentMessage) -> Option<usize> {
        let live = self.get(to)?;
        let depth = {
            let mut queue = live.mailbox.lock().expect("mailbox lock");
            if queue.len() >= MAX_MAILBOX {
                return Some(queue.len());
            }
            queue.push_back(message);
            queue.len()
        };
        live.wake.notify_waiters();
        Some(depth)
    }

    /// Pause `id`. Returns the status it was in, for the event and the caller.
    pub fn pause(&self, id: &str, by: &str) -> Result<AgentStatus, ControlError> {
        let live = self
            .get(id)
            .ok_or_else(|| ControlError::Unknown(id.to_string()))?;
        if live.cancel.is_cancelled() {
            return Err(ControlError::NotLive {
                agent: id.to_string(),
                status: AgentStatus::Cancelled.as_str().to_string(),
            });
        }
        let mut paused = live.paused.lock().expect("paused lock");
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
    pub fn resume(&self, id: &str, by: &str) -> Result<AgentStatus, ControlError> {
        let live = self
            .get(id)
            .ok_or_else(|| ControlError::Unknown(id.to_string()))?;
        if live.cancel.is_cancelled() {
            return Ok(AgentStatus::Cancelled);
        }
        live.set_paused(false);
        tracing::debug!(agent = %id, by, "resumed a subagent");
        Ok(AgentStatus::Running)
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

    /// Queue one message for a session's top-level run.
    ///
    /// Accepted whether or not the run is live: a child that finishes after its
    /// parent's run has ended still gets a truthful "queued", and the message is
    /// dropped with the session's queue rather than leaking into a later run.
    /// Returns the queue depth.
    pub fn deliver_to_run(&self, session_id: &str, message: AgentMessage) -> usize {
        let mut mail = self.run_mail.lock().expect("run mail lock");
        let queue = mail.entry(session_id.to_string()).or_default();
        if queue.len() >= MAX_MAILBOX {
            return queue.len();
        }
        queue.push_back(message);
        queue.len()
    }

    /// Take everything addressed to a session's run, oldest first.
    pub fn take_run_mail(&self, session_id: &str) -> Vec<AgentMessage> {
        let mut mail = self.run_mail.lock().expect("run mail lock");
        let drained: Vec<AgentMessage> = mail
            .get_mut(session_id)
            .map(|queue| queue.drain(..).collect())
            .unwrap_or_default();
        if mail.get(session_id).is_some_and(VecDeque::is_empty) {
            mail.remove(session_id);
        }
        drained
    }

    /// Drop a session's run mailbox (its run ended or the session was deleted).
    pub fn clear_run_mail(&self, session_id: &str) {
        self.run_mail
            .lock()
            .expect("run mail lock")
            .remove(session_id);
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
        assert_eq!(hub.deliver("a_1", message.clone()), Some(1));
        assert_eq!(hub.deliver("a_1", message), Some(2));
        assert_eq!(hub.get("a_1").unwrap().queued_mail(), 2);
        assert_eq!(hub.get("a_1").unwrap().take_mail().len(), 2);
        assert_eq!(hub.get("a_1").unwrap().queued_mail(), 0);
        // An unknown recipient is not an error here; the tool reports it.
        assert_eq!(
            hub.deliver(
                "nope",
                AgentMessage {
                    from: None,
                    from_name: String::new(),
                    content: String::new(),
                    sent_at: 0,
                }
            ),
            None
        );
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

        hub.pause("a_1", "user").unwrap();
        assert!(live.is_paused());
        // A second pause is idempotent.
        assert!(hub.pause("a_1", "user").is_ok());

        let waiting = {
            let live = Arc::clone(&live);
            let cancel = cancel.clone();
            tokio::spawn(async move { wait_while_paused(&live, &cancel, far).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished(), "a paused loop must park");

        hub.resume("a_1", "user").unwrap();
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
        hub.pause("a_1", "user").unwrap();
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
