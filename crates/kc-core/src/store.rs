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

/// A durable grouping owned by one project, containing project-local sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mission {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionTask {
    pub id: String,
    pub mission_id: String,
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    pub state: String,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub error: Option<String>,
}

/// The reviewed editable portion of a task, used as a compare-and-swap guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionTaskPlan {
    pub project_id: String,
    pub prompt: String,
    pub dependencies: Vec<String>,
}

/// An explicitly published, immutable mission record. Content and provenance are
/// assertions by the publisher, not verified by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionRecord {
    pub id: String,
    pub mission_id: String,
    pub request_id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub artifact_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionSession {
    pub project_id: String,
    pub session_id: String,
    pub request_id: String,
    pub prompt: String,
    pub created_at: i64,
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
    /// The Workbench result pane's preferred desktop width in pixels. `None`
    /// uses the frontend default; the HTTP preferences route owns validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_panel_width: Option<u32>,
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
    /// True for a control-plane chat. Those sessions belong to no project and
    /// live in the server's own directory, so a client that draws "the
    /// sessions of a project" must be able to tell them apart from one.
    #[serde(default)]
    pub control: bool,
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
    /// Reasoning content from models that separate thinking from output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
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

/// A chronological page of visible transcript messages.
#[derive(Debug, Clone)]
pub struct MessagePage {
    pub messages: Vec<StoredMessage>,
    pub has_more: bool,
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
            reasoning_content: self.reasoning_content.clone(),
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
    pub reasoning_content: Option<String>,
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
            reasoning_content: m.reasoning_content.clone(),
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

#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait Store: Send + Sync {
    /// Returns the existing record on an identical request-id replay.
    async fn publish_mission_record(
        &self,
        _record: MissionRecord,
    ) -> Result<(MissionRecord, bool)> {
        Err(crate::error::Error::Store(
            "mission records unsupported".into(),
        ))
    }
    async fn list_mission_records(
        &self,
        _mission_id: &str,
        _kind: &str,
    ) -> Result<Vec<MissionRecord>> {
        Err(crate::error::Error::Store(
            "mission records unsupported".into(),
        ))
    }
    /// Fetch only the selected artifacts, in caller order, within the handoff limits.
    async fn get_mission_artifacts(
        &self,
        _mission_id: &str,
        _ids: &[String],
    ) -> Result<Vec<MissionRecord>> {
        Err(crate::error::Error::Store(
            "mission artifacts unsupported".into(),
        ))
    }
    async fn create_mission_task(&self, _task: MissionTask) -> Result<MissionTask> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    async fn revise_mission_task(
        &self,
        _mission_id: &str,
        _task_id: &str,
        _project_id: Option<String>,
        _prompt: Option<String>,
        _dependencies: Option<Vec<String>>,
    ) -> Result<MissionTask> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    /// Atomically replan only a never-claimed task whose reviewed plan still matches.
    async fn replan_mission_task(
        &self,
        _mission_id: &str,
        _task_id: &str,
        _expected: MissionTaskPlan,
        _project_id: Option<String>,
        _prompt: Option<String>,
        _dependencies: Option<Vec<String>>,
    ) -> Result<MissionTask> {
        Err(crate::error::Error::Store(
            "mission replanning unsupported".into(),
        ))
    }
    async fn list_mission_tasks(&self, _mission_id: &str) -> Result<Vec<MissionTask>> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    async fn start_mission_tasks(&self, _mission_id: &str, _limit: u32) -> Result<()> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    async fn mission_schedule_state(&self, _mission_id: &str) -> Result<Option<String>> {
        Err(crate::error::Error::Store(
            "mission schedules unsupported".into(),
        ))
    }
    async fn control_mission_tasks(&self, _mission_id: &str, _action: &str) -> Result<()> {
        Err(crate::error::Error::Store(
            "mission controls unsupported".into(),
        ))
    }
    /// Atomically reserve a retry request and requeue one terminal task.
    async fn retry_mission_task(
        &self,
        _mission_id: &str,
        _task_id: &str,
        _request_id: &str,
        _project_id: Option<&str>,
    ) -> Result<bool> {
        Err(crate::error::Error::Store(
            "mission task retries unsupported".into(),
        ))
    }
    async fn claim_mission_task(
        &self,
        _mission_id: &str,
        _session_id: &str,
    ) -> Result<Option<MissionTask>> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    async fn finish_mission_task(
        &self,
        _id: &str,
        _state: &str,
        _run_id: Option<&str>,
        _error: Option<&str>,
    ) -> Result<()> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }
    async fn recover_mission_tasks(&self) -> Result<Vec<String>> {
        Err(crate::error::Error::Store(
            "mission tasks unsupported".into(),
        ))
    }

    /// Atomically account for a physical provider attempt, or refuse it.
    async fn admit_provider_attempt(&self, _session_id: &str) -> Result<()> {
        Err(crate::error::Error::Store(
            "provider attempt admission unsupported".into(),
        ))
    }
    async fn mission_attempt_budget(&self, _id: &str) -> Result<Option<(Option<u32>, u32)>> {
        Err(crate::error::Error::Store(
            "mission budgets unsupported".into(),
        ))
    }
    async fn create_mission(
        &self,
        _id: &str,
        _project_id: &str,
        _title: &str,
        _description: &str,
        _max_provider_attempts: Option<u32>,
    ) -> Result<Mission> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    async fn list_missions(&self, _project_id: &str) -> Result<Vec<Mission>> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    /// A stable newest-first page. The compatibility fallback is intentionally
    /// in memory; SQLite overrides it with SQL OFFSET/LIMIT.
    async fn list_mission_page(
        &self,
        project_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Mission>, bool)> {
        let mut missions = self.list_missions(project_id).await?;
        missions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let mut page = missions
            .into_iter()
            .skip(offset)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let has_more = page.len() > limit;
        page.truncate(limit);
        Ok((page, has_more))
    }
    async fn get_mission(&self, _id: &str) -> Result<Option<Mission>> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    async fn delete_mission(&self, _id: &str) -> Result<()> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    /// Atomically reserve a request and create its session. Returns false on replay.
    async fn create_mission_session(
        &self,
        _mission_id: &str,
        _request_id: &str,
        _session_id: &str,
        _project_id: &str,
        _title: &str,
        _workspace: &str,
        _prompt: &str,
    ) -> Result<bool> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    async fn list_mission_sessions(&self, _mission_id: &str) -> Result<Vec<MissionSession>> {
        Err(crate::error::Error::Store("missions unsupported".into()))
    }
    /// Find the mission whose reserved coordinator session is this session.
    async fn coordinator_mission(&self, _session_id: &str) -> Result<Option<Mission>> {
        Ok(None)
    }

    /// Reserve the singleton control chat and return (session, created).
    /// Create one durable control-plane session.
    ///
    /// Control sessions are server-owned: they have no project and each gets its
    /// own directory, so several can run at once without touching a project.
    async fn create_control_session(
        &self,
        _id: &str,
        _title: &str,
        _workspace: &str,
    ) -> Result<Session> {
        Err(crate::error::Error::Store(
            "control sessions unsupported".into(),
        ))
    }
    /// Every control session, newest activity first.
    async fn list_control_sessions(&self) -> Result<Vec<SessionSummary>> {
        Err(crate::error::Error::Store(
            "control sessions unsupported".into(),
        ))
    }
    /// Detach one control session from a project (legacy rows), rebinding its
    /// workspace to the server-owned directory.
    async fn detach_control_session(&self, _id: &str, _workspace: &str) -> Result<()> {
        Err(crate::error::Error::Store(
            "control sessions unsupported".into(),
        ))
    }
    async fn is_control_session(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }
    /// Atomically reserve a control-plane dispatch; false means identical replay.
    async fn reserve_control_dispatch(
        &self,
        _request_id: &str,
        _session_id: &str,
        _project_id: &str,
        _title: &str,
        _workspace: &str,
        _prompt: &str,
    ) -> Result<(String, bool)> {
        Err(crate::error::Error::Store(
            "control dispatch unsupported".into(),
        ))
    }

    async fn create_session(
        &self,
        id: &str,
        title: &str,
        workspace: &str,
        project_id: Option<&str>,
    ) -> Result<Session>;
    /// Total sessions and up to `limit` recovery candidates, interrupted first
    /// then recent, each ordered by update time and id descending. Call before
    /// marking runs or agents interrupted. Exclude agents if their snapshot failed.
    async fn recovery_sessions(
        &self,
        cutoff: i64,
        limit: usize,
        include_agents: bool,
    ) -> Result<(usize, Vec<String>)>;
    /// Lightweight snapshot for releasing claims of running and paused agents.
    async fn running_agent_ids(&self) -> Result<Vec<(String, String)>>;
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
    async fn list_sessions_for_project(&self, project_id: &str) -> Result<Vec<SessionSummary>>;
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
    async fn rename_session(&self, id: &str, title: &str) -> Result<()>;
    /// Rename only if the title still matches the version the caller generated
    /// from. False means the session vanished or another rename won.
    async fn rename_session_if_title(&self, id: &str, expected: &str, title: &str) -> Result<bool> {
        let Some(session) = self.get_session(id).await? else {
            return Ok(false);
        };
        if session.title != expected {
            return Ok(false);
        }
        self.rename_session(id, title).await?;
        Ok(true)
    }
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
    /// Save the project-scoped Workbench result width. The route validates the
    /// public bounds; stores persist the value without touching session state.
    async fn set_project_result_panel_width(&self, id: &str, width: u32) -> Result<()>;
    async fn delete_project(&self, id: &str) -> Result<()>;

    async fn append_message(&self, message: &NewMessage) -> Result<i64>;
    async fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<StoredMessage>>;
    /// Newest visible messages before an exclusive cursor, in ascending id order.
    /// An oversized newest message is returned alone to guarantee progress.
    async fn list_message_page(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        before_id: Option<i64>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<MessagePage> {
        // Compatibility fallback for stores without a native paged query. SQLite
        // overrides this to avoid loading the transcript into memory.
        let mut messages = self.list_messages(session_id, usize::MAX).await?;
        messages.retain(|m| {
            before_id.is_none_or(|id| m.id < id)
                && agent_id.is_none_or(|id| m.agent_id.as_deref() == Some(id))
        });
        let mut selected = Vec::new();
        let mut bytes = 0usize;
        let mut has_more = false;
        for message in messages.into_iter().rev() {
            let size = serde_json::to_vec(&message)
                .map_err(|e| crate::error::Error::Store(format!("encode message: {e}")))?
                .len();
            if selected.len() >= limit
                || (!selected.is_empty() && bytes.saturating_add(size) > max_bytes)
            {
                has_more = true;
                break;
            }
            bytes = bytes.saturating_add(size);
            selected.push(message);
        }
        selected.reverse();
        Ok(MessagePage {
            messages: selected,
            has_more,
        })
    }
    /// Parent conversation rows before an exclusive cursor, newest first. Excludes
    /// rewound, system and subagent rows. Stop before exceeding `max_bytes`, except
    /// that the newest row is returned alone if it is oversized. Zero limit is empty.
    /// Oversized tool content may be replaced with an explicit omission marker to
    /// leave room for its call and preceding context. Implementations must apply
    /// the byte bound before materializing row payloads.
    async fn list_history_page(
        &self,
        session_id: &str,
        before_id: Option<i64>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<StoredMessage>>;
    /// Tool names in the first `limit` visible messages after the cursor, without
    /// loading message bodies. Fail closed if scan payloads exceed `max_bytes`.
    async fn rewind_tool_names(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<String>>;

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

    /// Every bound journal row for messages newer than `after_id`, in the order
    /// the tools ran (ascending id). Unbound rows are omitted because only an
    /// active writer can bind them safely; rewind admission excludes such runs.
    async fn list_checkpoints(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<FileCheckpoint>>;

    /// List bound checkpoint metadata without materializing before-image blobs.
    async fn list_checkpoint_metadata(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<FileCheckpoint>>;

    /// Fetch a before-image only if it fits the remaining preview byte budget.
    /// An oversized blob must fail before its bytes are materialized.
    async fn checkpoint_content_bounded(&self, id: i64, max_bytes: usize)
        -> Result<Option<String>>;

    /// Fetch one checkpoint before-image. None means the row or its blob is missing.
    async fn checkpoint_content(&self, id: i64) -> Result<Option<String>>;

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

    /// Bind a tool result's checkpoint ids together. SQLite performs the whole
    /// batch in one transaction; compatibility errors leave ids pending in the
    /// journal so a later tool result can retry them.
    async fn bind_checkpoints(&self, ids: &[i64], message_id: i64) -> Result<usize> {
        for id in ids {
            self.set_checkpoint_message_id(*id, message_id).await?;
        }
        Ok(ids.len())
    }

    /// Timestamp of the newest rewind in a session, for the UI banner.
    async fn last_rewind_at(&self, session_id: &str) -> Result<Option<i64>>;

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()>;
    /// Change only status/updated_at while the persisted status is one of the
    /// expected values. False lets control operations preserve terminal state.
    async fn transition_agent_status(
        &self,
        id: &str,
        expected: &[&str],
        next: &str,
        updated_at: i64,
    ) -> Result<bool> {
        let Some(summary) = self.get_agent(id).await? else {
            return Ok(false);
        };
        if !expected.contains(&summary.agent.status.as_str()) {
            return Ok(false);
        }
        let mut agent = summary.agent;
        agent.status = next.to_string();
        agent.updated_at = updated_at;
        self.upsert_agent(&agent).await?;
        Ok(true)
    }
    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>>;
    /// Every agent across every session, newest first, joined with its session's
    /// title/workspace. `project_id` narrows the result to one project.
    ///
    /// This is what the Agents view uses: a subagent in a session the user is not
    /// currently looking at is still worth showing while it runs.
    async fn list_agents_all(&self, project_id: Option<&str>) -> Result<Vec<AgentSummary>>;
    /// Filter the same complete result set without changing its order or size
    /// contract. SQLite applies all three predicates before reading row bodies.
    async fn list_agents_filtered(
        &self,
        project_id: Option<&str>,
        session_id: Option<&str>,
        statuses: &[String],
    ) -> Result<Vec<AgentSummary>> {
        let mut agents = self.list_agents_all(project_id).await?;
        if let Some(session_id) = session_id {
            agents.retain(|summary| summary.agent.session_id == session_id);
        }
        if !statuses.is_empty() {
            agents.retain(|summary| statuses.contains(&summary.agent.status));
        }
        Ok(agents)
    }
    /// Every subagent of one session, newest `created_at` first, each with the
    /// session context (`session_title`, `workspace`, `project_id`) the Agents
    /// view groups by.
    ///
    /// Not scoped to a project: the control plane shows the agents of a control
    /// chat and of a project session in the same list, and a control chat has no
    /// project at all. The default is empty so a store that does not implement
    /// it still compiles.
    async fn list_agent_summaries_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<AgentSummary>> {
        let _ = session_id;
        Ok(Vec::new())
    }
    /// Per-session subagent counts: `(session_id, live, total)`, where live is a
    /// running **or paused** agent.
    ///
    /// One grouped query, for the control plane's per-chat badges: asking
    /// `list_agent_summaries_for_session` once per chat is an N+1 that grows with
    /// the number of chats, and the badge only needs two numbers.
    async fn agent_counts_by_session(&self) -> Result<Vec<(String, usize, usize)>> {
        Ok(Vec::new())
    }
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

    /// Persisted usage for all attributed mission sessions, including prior retries.
    /// None means that the store does not support this aggregation.
    async fn mission_usage(&self, _mission_id: &str) -> Result<Option<UsageTotals>> {
        Ok(None)
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
    /// Persist a patch as one unit. The compatibility fallback supports simple
    /// test stores; SQLite overrides it with a single transaction.
    async fn set_settings(&self, settings: &[(String, String)]) -> Result<()> {
        for (key, value) in settings {
            self.set_setting(key, value).await?;
        }
        Ok(())
    }
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
