//! First-party coding tools.
//!
//! File tools are confined to a [`kc_core::paths::Workspace`]. `run_command` is
//! explicitly *not* sandboxed: it spawns a real process as the server user, in
//! its own process group so cancellation can kill the whole tree.

use async_trait::async_trait;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec};
use serde_json::json;
use std::sync::Arc;

pub mod command;
pub mod file_ops;
pub mod git;
pub mod search;

fn spec(name: &str, description: &str, parameters: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
        origin: kc_core::tool::ToolOrigin::Builtin,
    }
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "read_file",
            "Read a UTF-8 text file inside the workspace. Returns numbered lines. Use offset/limit for large files.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path"},
                    "offset": {"type": "integer", "description": "1-based first line"},
                    "limit": {"type": "integer", "description": "Max lines to return"}
                },
                "required": ["path"]
            }),
        )
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: read_file")
    }
}

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        spec(
            "list_dir",
            "List a directory inside the workspace. Respects .gitignore-style ignores and marks directories.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "depth": {"type": "integer", "description": "Max depth (default 1, max 3)"}
                }
            }),
        )
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: list_dir")
    }
}

pub struct SearchFiles;

#[async_trait]
impl Tool for SearchFiles {
    fn spec(&self) -> ToolSpec {
        spec(
            "search_files",
            "Search file contents with ripgrep (fallback: built-in regex walker). Returns path:line: text matches.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string", "description": "Optional glob filter, e.g. *.rs"},
                    "max_results": {"type": "integer"},
                    "case_sensitive": {"type": "boolean"}
                },
                "required": ["pattern"]
            }),
        )
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: search_files")
    }
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "write_file",
            "Create or overwrite a file with the given content. Parent directories are created. Requires approval per policy.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        )
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: write_file")
    }
}

pub struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "edit_file",
            "Replace an exact string in an existing file. Fails loudly if the old string is missing or ambiguous.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        )
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: edit_file")
    }
}

pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn spec(&self) -> ToolSpec {
        spec(
            "run_command",
            "Run a shell command in the workspace, streaming stdout/stderr. Not sandboxed: runs as the server user. Requires approval per policy.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "cwd": {"type": "string", "description": "Workspace-relative working directory"},
                    "timeout_secs": {"type": "integer"}
                },
                "required": ["command"]
            }),
        )
    }
    fn class(&self) -> ToolClass {
        ToolClass::Shell
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: run_command")
    }
}

pub struct GitStatus;

#[async_trait]
impl Tool for GitStatus {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_status",
            "Show git branch and working-tree status for the workspace (or a subdirectory).",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}}
            }),
        )
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: git_status")
    }
}

pub struct GitDiff;

#[async_trait]
impl Tool for GitDiff {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_diff",
            "Show the staged or unstaged diff for a path, bounded in size.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "staged": {"type": "boolean"},
                    "base": {"type": "string", "description": "Optional git ref to diff against"}
                }
            }),
        )
    }

    async fn invoke(&self, _request: ToolCallRequest, _ctx: ToolCtx) -> ToolOutcome {
        todo!("kc-tools: git_diff")
    }
}

/// All first-party tools, ready to register.
pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFile),
        Arc::new(ListDir),
        Arc::new(SearchFiles),
        Arc::new(WriteFile),
        Arc::new(EditFile),
        Arc::new(RunCommand),
        Arc::new(GitStatus),
        Arc::new(GitDiff),
    ]
}
