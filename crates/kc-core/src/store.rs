use crate::checkpoint::FileCheckpoint;
use crate::config::SessionModelSelection;
use crate::error::Result;
use crate::message::Message;
use crate::message::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    /// The process stopped while this run was active.
    Interrupted,
    LimitReached,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Interrupted => "interrupted",
            RunStatus::LimitReached => "limit_reached",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => RunStatus::Running,
            "completed" => RunStatus::Completed,
            "failed" => RunStatus::Failed,
            "cancelled" => RunStatus::Cancelled,
            "interrupted" => RunStatus::Interrupted,
            "limit_reached" => RunStatus::LimitReached,
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, RunStatus::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Running,
    /// The agent is alive but parked: a human paused it, or it is waiting for a
    /// message it was told to expect. It still holds its subagent slot, so a
    /// paused agent is a reason to spawn fewer, not more.
    Paused,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Paused => "paused",
            AgentStatus::Completed => "completed",
            AgentStatus::Failed => "failed",
            AgentStatus::Cancelled => "cancelled",
            AgentStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => AgentStatus::Running,
            "paused" => AgentStatus::Paused,
            "completed" => AgentStatus::Completed,
            "failed" => AgentStatus::Failed,
            "cancelled" => AgentStatus::Cancelled,
            "interrupted" => AgentStatus::Interrupted,
            _ => return None,
        })
    }

    /// Whether the agent's loop is still alive (running or parked), as opposed
    /// to settled. A paused agent is live: it can be resumed and keep spending.
    pub fn is_live(&self) -> bool {
        matches!(self, AgentStatus::Running | AgentStatus::Paused)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub workspace: String,
    /// Project this session belongs to. `None` for sessions created before
    /// projects existed; they run in the default workspace and are attached to
    /// a project at startup.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Per-session model selection. `None` follows the install-wide default,
    /// so several sessions can run different models at the same time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModelSelection>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A workspace the agent may work in, shown as one entry in the project rail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Canonical absolute directory path. Unique across projects.
    pub path: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Number of sessions in this project (computed by `list_projects`).
    #[serde(default)]
    pub session_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub workspace: String,
    #[serde(default)]
    pub project_id: Option<String>,
    /// Per-session model selection (`None` = install-wide default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModelSelection>,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_status: Option<String>,
    #[serde(default)]
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Images carried by this message (see [`crate::message::ImageAttachment`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::message::ImageAttachment>,
    /// When set, this message was hidden by a rewind at the recorded time.
    ///
    /// Rows are never deleted - the store is append-only and the resume and
    /// retention passes assume monotonic ids - so a rewind marks the rows it
    /// hides instead. [`Store::list_messages`] and friends filter them out, so a
    /// rewound message is invisible to the model and to the browser alike.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewound_at: Option<i64>,
    pub created_at: i64,
}

impl StoredMessage {
    pub fn to_message(&self) -> Message {
        Message {
            role: self.role.parse().unwrap_or(crate::message::Role::User),
            content: self.content.clone(),
            tool_calls: self.tool_calls.clone(),
            tool_call_id: self.tool_call_id.clone(),
            name: self.name.clone(),
            images: self.images.clone(),
        }
    }
}

/// Where a rewind starts and what it hides, resolved from a message id.
///
/// The point is always a *user* message: the message whose prompt the user wants
/// to revisit. [`newest_msg_id`] is the last message that survives, so
/// everything strictly after it (assistant turns, tool results, steers) is
/// hidden and the model's next request is exactly the conversation as it stood
/// right after that prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindPoint {
    pub session_id: String,
    /// The message the user picked, after normalisation (always a `user` row).
    pub user_message_id: i64,
    /// The prompt that message carried, re-sent verbatim by a retry.
    pub prompt: String,
    /// Newest message that survives the rewind.
    pub keep_through: i64,
    /// Newest message that survives when the prompt itself is replaced too.
    ///
    /// A retry re-sends the prompt and a note replaces it, and in both cases
    /// keeping the old prompt would show the model two user turns in a row -
    /// "fix the build", then "fix the build differently" - which reads as a
    /// correction of work that is no longer in the transcript. `0` when the
    /// prompt is the first message in the session.
    pub before_prompt: i64,
    /// Number of messages this rewind hides.
    pub messages_removed: usize,
}

impl RewindPoint {
    /// The marker a rewind would use for `keep_prompt`.
    pub fn cut(&self, keep_prompt: bool) -> i64 {
        if keep_prompt {
            self.keep_through
        } else {
            self.before_prompt
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewMessage {
    pub session_id: String,
    pub run_id: Option<String>,
    pub agent_id: Option<String>,
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    /// Images attached to this message (see [`crate::message::ImageAttachment`]).
    pub images: Vec<crate::message::ImageAttachment>,
}

impl NewMessage {
    pub fn from_message(session_id: &str, run_id: Option<&str>, m: &Message) -> Self {
        Self {
            session_id: session_id.to_string(),
            run_id: run_id.map(|s| s.to_string()),
            agent_id: None,
            role: m.role.as_str().to_string(),
            content: m.content.clone(),
            tool_calls: m.tool_calls.clone(),
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
            images: m.images.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub session_id: String,
    pub status: String,
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub session_id: String,
    pub parent_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    pub name: String,
    pub role: String,
    pub task: String,
    pub status: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub iterations: u32,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub owned_paths: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// True when the spawn detached: the tool call returned immediately and the
    /// agent kept working on its own.
    #[serde(default)]
    pub background: bool,
    /// The agent's own last statement of what it is doing, from `report_status`.
    /// Advisory: it is a claim, not a measurement.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The agent's claimed completion, 0..=1. `None` means it has not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
}

impl AgentRecord {
    /// Whether this agent's loop is still alive (running or paused).
    pub fn is_live(&self) -> bool {
        AgentStatus::parse(&self.status).is_some_and(|status| status.is_live())
    }
}

/// An agent plus the session context the Agents view needs to group it.
///
/// The agent fields are flattened into the same JSON object, so a client that
/// already understands `AgentRecord` can read an `AgentSummary` unchanged and
/// gets `session_title`, `workspace` and `project_id` on top.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    #[serde(flatten)]
    pub agent: AgentRecord,
    pub session_title: String,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub payload: serde_json::Value,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
}

/// One provider call's token accounting, as reported by the endpoint.
///
/// The runtime emits a `usage` event per model call and this is the row it
/// leaves behind, so "what did this session cost" survives a page reload. The
/// numbers are the provider's own: `cached_tokens` is `None` when the endpoint
/// does not report prefix-cache hits, which is *unknown*, not a 0% hit rate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub session_id: String,
    pub run_id: String,
    /// The subagent that made the call, `None` for the main agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Prompt tokens billed for this call.
    pub input_tokens: u32,
    /// Completion tokens produced for this call.
    pub output_tokens: u32,
    /// Prompt tokens the endpoint served from its prefix cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

/// Sum of every provider call in a scope, plus a count of the calls.
///
/// `cached_known_calls` is how many calls reported a cache figure at all: a
/// ratio computed over calls that did not is a lie, so the UI needs the
/// denominator as well as the numerator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    /// Calls that reported `cached_tokens` (at least one field known).
    pub cached_known_calls: u32,
    /// The newest call's prompt size: how full the context window is, as opposed
    /// to what the session has cost in total.
    pub last_input_tokens: u32,
}

impl UsageTotals {
    /// Fraction of prompt tokens served from the prefix cache, 0..1.
    ///
    /// `None` when no call reported a cache figure, so a caller can render
    /// "unknown" instead of a fabricated 0%. A reported `0` from an endpoint
    /// that does report the field is a real 0% and stays `Some(0.0)`.
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        if self.cached_known_calls == 0 || self.input_tokens == 0 {
            return None;
        }
        Some((self.cached_tokens as f64 / self.input_tokens as f64).clamp(0.0, 1.0))
    }
}

/// Counts describing a whole session, for the session information view.
///
/// Deliberately *counts and durations*, not a transcript: a session can hold
/// tens of thousands of messages, and the view that answers "how much work was
/// this" must not depend on shipping them to the browser.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStats {
    /// Every message in the session, subagent messages included.
    pub messages_total: u64,
    /// Messages the main agent's conversation holds (`agent_id` is NULL).
    pub messages_main: u64,
    /// Messages belonging to any subagent.
    pub messages_subagent: u64,
    /// Messages by role: `user`, `assistant`, `tool`, `system`.
    pub messages_by_role: BTreeMap<String, u64>,
    /// Main-agent messages by role, so "how many prompts did I send" is the
    /// conversation's answer rather than the sum of every subagent's.
    pub messages_main_by_role: BTreeMap<String, u64>,
    /// Every tool call the session made, by tool name.
    pub tool_calls: BTreeMap<String, u64>,
    /// Total tool calls (a call is one entry in an assistant message's list).
    pub tool_calls_total: u64,
    /// Runs by terminal status (`completed`, `failed`, `cancelled`, ...).
    pub runs_by_status: BTreeMap<String, u64>,
    /// Every run that ever started, including the one in flight.
    pub runs_total: u64,
    /// Sum of `finished_at - started_at` over runs that finished.
    ///
    /// This is the number the view calls "working time": wall clock during which
    /// a run was actually in flight, so idle gaps between runs are excluded.
    pub working_ms: u64,
}

/// What a retention pass deleted. Every field is a row count, so a caller can
/// log "pruned 1200 messages, 40 runs" without re-querying.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    /// Messages deleted because they were older than the retention window.
    pub messages: usize,
    /// Runs deleted because the session they belonged to no longer has any
    /// message left (a run with no transcript is dead weight).
    pub runs: usize,
    /// Agents deleted for the same reason as `runs`.
    pub agents: usize,
    /// Approvals deleted for the same reason as `runs`.
    pub approvals: usize,
}

impl PruneReport {
    /// Total rows removed by the pass.
    pub fn total(&self) -> usize {
        self.messages + self.runs + self.agents + self.approvals
    }

    /// True when the pass deleted nothing.
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// Result of a full `PRAGMA integrity_check`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityReport {
    /// True when SQLite reported `ok` for every page.
    pub ok: bool,
    /// Raw rows returned by the check; `["ok"]` for a healthy database.
    pub problems: Vec<String>,
    /// When the check ran (unix milliseconds).
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub path: String,
    pub session_id: String,
    pub agent_id: String,
    pub mode: String,
    pub created_at: i64,
}

/// A browser terminal: one PTY, its title and its position in the tab strip.
///
/// This row is what makes the terminal tab *global and durable*. The shell
/// process is a child of the server, so it cannot outlive a restart - but the
/// row can, and does: the tab strip, the titles, the working directory and the
/// last screenful of output are all here in SQLite, so opening karucode in a
/// different browser, after clearing site data, or after restarting the server
/// shows the same terminal tabs with the same names. Reattaching to a live
/// shell reconnects to the running process; reattaching to a reaped one starts
/// a fresh shell in the same directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSession {
    pub id: String,
    /// Shown in the tab strip. Renaming is a `PATCH`, and the default is
    /// derived from the working directory ("karucode-1", "~-2").
    pub title: String,
    /// Absolute directory the shell was started in. Kept even when the project
    /// is later deleted, so a reopen still describes where the shell lived.
    pub cwd: String,
    /// The project the terminal belonged to when it was created, if any. Only a
    /// label: a terminal is not confined, so this never restricts it.
    pub project_id: Option<String>,
    /// Manual ordering for the tab strip; ties break on `created_at`.
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// Last time a browser was attached, or the terminal was used. Drives the
    /// idle reaper.
    pub last_used_at: i64,
}

impl TerminalSession {
    /// The label the UI shows when a terminal has never been renamed.
    ///
    /// Derived from the directory's basename, because "karucode" says more than
    /// "Terminal 3" and the same install almost always has one directory per
    /// project.
    pub fn default_title(cwd: &str, position: i64) -> String {
        let base = std::path::Path::new(cwd)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| cwd.to_string());
        format!("{base}-{}", position.max(1))
    }
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn create_session(
        &self,
        id: &str,
        title: &str,
        workspace: &str,
        project_id: Option<&str>,
    ) -> Result<Session>;
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
    async fn list_sessions_for_project(&self, project_id: &str) -> Result<Vec<SessionSummary>>;
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
    async fn rename_session(&self, id: &str, title: &str) -> Result<()>;
    /// Set (`Some`) or clear (`None`) a session's per-session model selection.
    /// The next run in the session resolves its model from the new value.
    async fn set_session_model(
        &self,
        id: &str,
        model: Option<&SessionModelSelection>,
    ) -> Result<()>;
    async fn delete_session(&self, id: &str) -> Result<()>;
    async fn touch_session(&self, id: &str) -> Result<()>;
    /// Attach a session (typically one created before projects existed) to a
    /// project, and record the project's directory as the session workspace.
    async fn assign_session_project(&self, session_id: &str, project_id: &str) -> Result<()>;

    async fn create_project(&self, id: &str, name: &str, path: &str) -> Result<Project>;
    async fn list_projects(&self) -> Result<Vec<Project>>;
    async fn get_project(&self, id: &str) -> Result<Option<Project>>;
    async fn rename_project(&self, id: &str, name: &str) -> Result<()>;
    async fn delete_project(&self, id: &str) -> Result<()>;

    async fn append_message(&self, message: &NewMessage) -> Result<i64>;
    async fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<StoredMessage>>;
    async fn list_messages_after(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>>;

    async fn create_run(&self, id: &str, session_id: &str, model: &str, prompt: &str)
        -> Result<()>;
    async fn finish_run(&self, run_id: &str, status: RunStatus, error: Option<&str>) -> Result<()>;
    async fn mark_interrupted_runs(&self) -> Result<usize>;
    async fn list_runs(&self, session_id: &str, limit: usize) -> Result<Vec<RunRecord>>;

    // ---- rewind ----------------------------------------------------------
    //
    // A rewind hides the messages after a chosen user turn and (optionally)
    // undoes the file writes that ran in between. Messages are never deleted:
    // the store is append-only, and `repair_history`, retention and the browser
    // all read it as a monotonic log. Hiding is a column (`rewound_at`) that
    // every read filters on, so the model and the transcript agree by
    // construction rather than by two independent code paths.

    /// Resolve `message_id` to a [`RewindPoint`], normalising an assistant turn
    /// back to the user message that started it.
    ///
    /// `Err(NotFound)` when the message does not exist or belongs to another
    /// session. A message that is already rewound is *not* an error: rewinding
    /// the same point twice is a no-op that reports nothing left to hide, which
    /// is what a client retrying a request expects.
    async fn rewind_point(&self, session_id: &str, message_id: i64) -> Result<RewindPoint>;

    /// Hide every message in `session_id` newer than `keep_through`, returning how
    /// many rows were newly hidden. Idempotent: rows already hidden are not
    /// counted again.
    ///
    /// `keep_through` is the id of the *last surviving* message, not the count.
    async fn mark_messages_rewound(&self, session_id: &str, keep_through: i64) -> Result<usize>;

    /// Record the before-image of one file a mutating tool is about to change.
    async fn record_checkpoint(&self, checkpoint: &FileCheckpoint) -> Result<i64>;

    /// Every journal row for messages newer than `after_id`, in the order the
    /// tools ran (ascending id).
    async fn list_checkpoints(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<FileCheckpoint>>;

    /// Delete one journal row, once its undo has been applied.
    async fn delete_checkpoint(&self, id: i64) -> Result<()>;

    /// Attach a transcript position to a journal row written moments earlier.
    ///
    /// A tool records its before-image while its call is in flight, before the
    /// tool result row exists, so the row is written with `message_id = 0`. The
    /// run loop calls this once the result is persisted. That two-step write is
    /// the price of journaling *before* the change instead of after it, and it is
    /// worth paying: a journal written afterwards holds the new bytes.
    async fn set_checkpoint_message_id(&self, id: i64, message_id: i64) -> Result<()>;

    /// Timestamp of the newest rewind in a session, for the UI banner.
    async fn last_rewind_at(&self, session_id: &str) -> Result<Option<i64>>;

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()>;
    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>>;
    /// Every agent across every session, newest first, joined with its session's
    /// title/workspace. `project_id` narrows the result to one project.
    ///
    /// This is what the Agents view uses: a subagent in a session the user is not
    /// currently looking at is still worth showing while it runs.
    async fn list_agents_all(&self, project_id: Option<&str>) -> Result<Vec<AgentSummary>>;
    /// One agent by id, with its session context. `None` when it does not exist.
    async fn get_agent(&self, id: &str) -> Result<Option<AgentSummary>>;
    /// That agent's own transcript (messages carrying its agent id), oldest
    /// first. The parent session's messages are not included.
    async fn list_agent_messages(&self, agent_id: &str, limit: usize)
        -> Result<Vec<StoredMessage>>;
    /// How many messages each subagent of a session wrote, keyed by agent id.
    ///
    /// One grouped count instead of a `list_agent_messages` per agent: a session
    /// with twenty subagents would otherwise cost twenty queries just to show a
    /// column of numbers in the session view.
    ///
    /// The default is empty, so a store that does not implement it still
    /// compiles.
    async fn agent_message_counts(&self, session_id: &str) -> Result<BTreeMap<String, u64>> {
        let _ = session_id;
        Ok(BTreeMap::new())
    }
    async fn mark_interrupted_agents(&self) -> Result<usize>;

    // ---- token accounting -------------------------------------------------
    //
    // Every provider call reports its own token counts, and the session
    // information view needs them after the fact: "what has this session cost,
    // how much of it was cached, and how full is the window". The `usage` event
    // is live-only, so without a row per call a reload answers "unknown" - which
    // is why this is persisted rather than kept in the runtime.
    //
    // Both methods have a default implementation, so a store that does not care
    // about accounting (a test double, an embedding) keeps compiling and simply
    // reports no usage. Nothing here may fail a run: the runtime treats a
    // failure as a warning.

    /// Record one provider call's token counts.
    async fn record_usage(&self, record: &UsageRecord) -> Result<()> {
        let _ = record;
        Ok(())
    }

    /// Sum of every recorded call in a session, for the main agent and for each
    /// subagent separately. Agents absent from the map made no recorded call.
    async fn usage_totals(
        &self,
        session_id: &str,
    ) -> Result<BTreeMap<Option<String>, UsageTotals>> {
        let _ = session_id;
        Ok(BTreeMap::new())
    }

    /// Counts a summary view needs that a transcript page cannot give: the
    /// session's total messages by role, its tool calls by name, its runs by
    /// status and the wall-clock time it spent working.
    ///
    /// Answers about the **whole** session, not the newest `MESSAGE_LIMIT` rows
    /// the transcript endpoint returns, so a long session's "12 000 messages" is
    /// the real number rather than the page size.
    ///
    /// The default reports zeros, so a store that does not implement it still
    /// compiles and a client simply has nothing to show.
    async fn session_stats(&self, session_id: &str) -> Result<SessionStats> {
        let _ = session_id;
        Ok(SessionStats::default())
    }

    async fn create_approval(&self, record: &ApprovalRecord) -> Result<()>;
    async fn resolve_approval(&self, id: &str, decision: &str) -> Result<()>;
    async fn expire_pending_approvals(&self, run_id: &str, reason: &str) -> Result<usize>;
    async fn list_pending_approvals(&self, session_id: Option<&str>)
        -> Result<Vec<ApprovalRecord>>;

    async fn get_setting(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str) -> Result<()>;
    async fn all_settings(&self) -> Result<Vec<(String, String)>>;

    // ---- terminals -------------------------------------------------------
    //
    // A terminal row is a *named shell*, not the shell process: the process is a
    // child of the server and dies with it, while these rows survive so the tab
    // strip is the same in every browser and after every restart. See
    // [`TerminalSession`].

    /// Create a terminal row and return it. The caller has already resolved the
    /// id, the cwd and the title.
    async fn create_terminal(&self, terminal: &TerminalSession) -> Result<TerminalSession>;
    /// Every terminal, in tab-strip order (`position`, then `created_at`).
    async fn list_terminals(&self) -> Result<Vec<TerminalSession>>;
    async fn get_terminal(&self, id: &str) -> Result<Option<TerminalSession>>;
    /// Rename a terminal. `None` restores the derived default title.
    async fn rename_terminal(&self, id: &str, title: Option<&str>) -> Result<()>;
    /// Record that a browser attached (or typed), for the idle reaper.
    async fn touch_terminal(&self, id: &str) -> Result<()>;
    async fn delete_terminal(&self, id: &str) -> Result<()>;
    /// Every terminal id that exists, cheapest possible check for the runtime's
    /// reaper and for a websocket attach that names an unknown id.
    async fn terminal_ids(&self) -> Result<Vec<String>>;

    /// Advisory path ownership used to warn about overlapping subagent work.
    async fn claim_paths(
        &self,
        session_id: &str,
        agent_id: &str,
        paths: &[String],
        mode: &str,
    ) -> Result<Vec<ClaimRecord>>;
    async fn list_claims(&self, session_id: &str) -> Result<Vec<ClaimRecord>>;
    async fn release_claims(&self, agent_id: &str) -> Result<()>;

    // ---- retention, maintenance and diagnostics -------------------------
    //
    // The store is append-only by design, so without a retention pass the
    // database grows forever. The server is expected to call
    // `prune_messages_older_than` on a timer (see `Config::retention_days`,
    // where `0` means "keep everything") and `incremental_vacuum` afterwards to
    // hand the freed pages back to the filesystem.

    /// Delete messages older than `days`, keeping the newest
    /// [`Store::retention_keep_messages`] messages of every session so a session
    /// never loses its recent context. Runs, agents and approvals that belong to
    /// a session with no message left are deleted too.
    ///
    /// `days == 0` is a no-op: it means "keep everything".
    async fn prune_messages_older_than(&self, days: u64) -> Result<PruneReport>;

    /// How many messages per session a retention pass always keeps.
    fn retention_keep_messages(&self) -> usize;

    /// Rebuild the database file and reclaim all free pages. Expensive and
    /// blocking; run it only on explicit request or at startup.
    async fn vacuum(&self) -> Result<()>;

    /// Return free pages to the filesystem without rewriting the database.
    /// Cheap enough to run after every retention pass, but only effective when
    /// the database was created with `auto_vacuum=INCREMENTAL`.
    async fn incremental_vacuum(&self) -> Result<()>;

    /// Size of the database file in bytes (0 when it does not exist yet).
    async fn db_size_bytes(&self) -> Result<u64>;

    /// Full `PRAGMA integrity_check`. `open` already runs the cheap
    /// `quick_check`; this is the deep check for a maintenance endpoint.
    async fn integrity_check(&self) -> Result<IntegrityReport>;

    /// When the last integrity check ran (unix milliseconds), or `None` if the
    /// store has not checked itself yet.
    fn last_integrity_check(&self) -> Option<i64>;
}
