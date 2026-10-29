use crate::config::SessionModelSelection;
use crate::error::Result;
use crate::message::Message;
use crate::message::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Completed => "completed",
            AgentStatus::Failed => "failed",
            AgentStatus::Cancelled => "cancelled",
            AgentStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => AgentStatus::Running,
            "completed" => AgentStatus::Completed,
            "failed" => AgentStatus::Failed,
            "cancelled" => AgentStatus::Cancelled,
            "interrupted" => AgentStatus::Interrupted,
            _ => return None,
        })
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub path: String,
    pub session_id: String,
    pub agent_id: String,
    pub mode: String,
    pub created_at: i64,
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

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()>;
    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>>;
    async fn mark_interrupted_agents(&self) -> Result<usize>;

    async fn create_approval(&self, record: &ApprovalRecord) -> Result<()>;
    async fn resolve_approval(&self, id: &str, decision: &str) -> Result<()>;
    async fn expire_pending_approvals(&self, run_id: &str, reason: &str) -> Result<usize>;
    async fn list_pending_approvals(&self, session_id: Option<&str>)
        -> Result<Vec<ApprovalRecord>>;

    async fn get_setting(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str) -> Result<()>;
    async fn all_settings(&self) -> Result<Vec<(String, String)>>;

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
}
