use crate::config::ApprovalMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    ShellCommand,
    FileWrite,
    FileEdit,
    DeleteFile,
    NetworkTool,
    Plugin,
    Mcp,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub kind: ApprovalKind,
    pub title: String,
    pub detail: String,
    /// Machine-readable context: command, path, diff, etc.
    pub payload: serde_json::Value,
    pub session_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
    /// The request timed out or the run was cancelled.
    Expired,
}

impl ApprovalDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalDecision::AllowOnce => "allow_once",
            ApprovalDecision::AllowSession => "allow_session",
            ApprovalDecision::Deny => "deny",
            ApprovalDecision::Expired => "expired",
        }
    }
}

/// What a tool is allowed to do without asking, derived from the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone)]
pub struct ApprovalPolicyView {
    pub mode: ApprovalMode,
    pub allow_commands: Vec<String>,
    pub auto_approve_paths: Vec<String>,
}

impl ApprovalPolicyView {
    pub fn for_shell(&self, command: &str) -> AutoDecision {
        match self.mode {
            ApprovalMode::Auto => AutoDecision::Allow,
            ApprovalMode::ReadOnly => AutoDecision::Deny,
            ApprovalMode::Ask | ApprovalMode::AskCommands => {
                let cmd = command.trim_start();
                if self
                    .allow_commands
                    .iter()
                    .any(|p| !p.is_empty() && cmd.starts_with(p.as_str()))
                {
                    AutoDecision::Allow
                } else {
                    AutoDecision::Ask
                }
            }
        }
    }

    pub fn for_write(&self, path: &std::path::Path) -> AutoDecision {
        match self.mode {
            ApprovalMode::Auto => AutoDecision::Allow,
            ApprovalMode::ReadOnly => AutoDecision::Deny,
            ApprovalMode::AskCommands => AutoDecision::Allow,
            ApprovalMode::Ask => {
                let rel = path.to_string_lossy();
                if self
                    .auto_approve_paths
                    .iter()
                    .any(|p| !p.is_empty() && rel.starts_with(p.as_str()))
                {
                    AutoDecision::Allow
                } else {
                    AutoDecision::Ask
                }
            }
        }
    }

    pub fn for_external_tool(&self) -> AutoDecision {
        match self.mode {
            ApprovalMode::Auto => AutoDecision::Allow,
            ApprovalMode::ReadOnly => AutoDecision::Deny,
            _ => AutoDecision::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(mode: ApprovalMode, allow: &[&str]) -> ApprovalPolicyView {
        ApprovalPolicyView {
            mode,
            allow_commands: allow.iter().map(|s| s.to_string()).collect(),
            auto_approve_paths: vec![],
        }
    }

    #[test]
    fn read_only_denies_mutations_and_shell() {
        let v = view(ApprovalMode::ReadOnly, &[]);
        assert_eq!(v.for_shell("rm -rf /"), AutoDecision::Deny);
        assert_eq!(v.for_write(std::path::Path::new("a")), AutoDecision::Deny);
    }

    #[test]
    fn auto_allows() {
        let v = view(ApprovalMode::Auto, &[]);
        assert_eq!(v.for_shell("rm -rf /"), AutoDecision::Allow);
    }

    #[test]
    fn allowlist_prefixes_match() {
        let v = view(ApprovalMode::Ask, &["cargo test"]);
        assert_eq!(v.for_shell("cargo test --all"), AutoDecision::Allow);
        assert_eq!(v.for_shell("rm -rf /"), AutoDecision::Ask);
        assert_eq!(v.for_shell("cargo test; rm -rf /"), AutoDecision::Allow);
    }

    #[test]
    fn ask_commands_allows_workspace_writes() {
        let v = view(ApprovalMode::AskCommands, &[]);
        assert_eq!(v.for_write(std::path::Path::new("/w/a.txt")), AutoDecision::Allow);
        assert_eq!(v.for_shell("ls"), AutoDecision::Ask);
    }
}
