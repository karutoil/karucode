//! First-party coding tools.
//!
//! File tools are confined to a [`kc_core::paths::Workspace`]. `run_command` is
//! explicitly *not* sandboxed: it spawns a real process as the server user, in
//! its own process group so cancellation can kill the whole tree.

use async_trait::async_trait;
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalPolicyView, AutoDecision};
use kc_core::config::ApprovalPolicy;
use kc_core::tool::{
    truncate_output, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod command;
pub mod file_ops;
pub mod git;
pub mod search;

#[cfg(test)]
mod tests;

fn spec(name: &str, description: &str, parameters: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
        origin: kc_core::tool::ToolOrigin::Builtin,
    }
}

/// Turn the persisted approval policy into the view the core uses for automatic
/// decisions.
fn policy_view(policy: &ApprovalPolicy) -> ApprovalPolicyView {
    ApprovalPolicyView {
        mode: policy.mode.clone(),
        allow_commands: policy.allow_commands.clone(),
        auto_approve_paths: policy.auto_approve_paths.clone(),
    }
}

/// Policy decision for a tool class without contacting the user.
///
/// `target` is the shell command for [`ToolClass::Shell`] and the
/// workspace-relative path for [`ToolClass::Mutating`].
pub fn auto_decision(policy: &ApprovalPolicy, class: ToolClass, target: &str) -> AutoDecision {
    let view = policy_view(policy);
    match class {
        ToolClass::ReadOnly => AutoDecision::Allow,
        ToolClass::Shell => view.for_shell(target),
        ToolClass::Mutating => view.for_write(&PathBuf::from(target)),
        ToolClass::Delegate | ToolClass::External => view.for_external_tool(),
    }
}

/// Resolve approval for one action. `Err` carries the ready-made refusal so the
/// caller can return it unchanged.
async fn gate(
    ctx: &ToolCtx,
    kind: ApprovalKind,
    decision: AutoDecision,
    action: &str,
    title: String,
    detail: String,
    payload: serde_json::Value,
) -> std::result::Result<ApprovalDecision, ToolOutcome> {
    match decision {
        AutoDecision::Allow => Ok(ApprovalDecision::AllowOnce),
        // Read-only mode never asks: asking would imply the answer could be yes.
        AutoDecision::Deny => Err(ToolOutcome::err(format!(
            "{action} is refused: the approval policy is `read_only`, so only read-only tools are allowed. \
             Switch approvals.mode to `ask`, `ask_commands` or `auto` to permit it."
        ))),
        AutoDecision::Ask => {
            let decision = ctx.approvals.ask(kind, title, detail, payload).await;
            if decision.is_allowed() {
                Ok(decision)
            } else {
                Err(ToolOutcome::err(format!(
                    "denied by user ({}) for {action}",
                    decision.as_str()
                )))
            }
        }
    }
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "read_file",
            "Read a UTF-8 text file inside the workspace. Returns numbered lines with a \
             `path — lines A-B of N (bytes)` header; use offset/limit for large files \
             (default 400 lines, max 2000). Binary files are refused. Output is capped by the \
             configured tool-output limit.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path"},
                    "offset": {"type": "integer", "description": "1-based first line to return"},
                    "limit": {"type": "integer", "description": "Max lines to return (default 400, max 2000)"}
                },
                "required": ["path"]
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = match request.arg_str("path") {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let offset = request.opt_u64("offset");
        let limit = request.opt_u64("limit");
        let limits = ctx.limits().await;

        match file_ops::read_file(
            &ctx.workspace,
            path,
            offset,
            limit,
            limits.max_file_read_bytes,
        ) {
            Ok(rendered) => {
                let (text, cut) = truncate_output(&rendered.text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text)
                    .truncated(cut || rendered.truncated)
                    .with_meta(rendered.meta)
            }
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        spec(
            "list_dir",
            "List a directory inside the workspace. Respects .gitignore rules, skips .git and \
             node_modules, lists directories first (with a trailing '/') and files with their size. \
             Capped at 500 entries.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative directory (default '.')"},
                    "depth": {"type": "integer", "description": "Max depth, 1-3 (default 1)"}
                }
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = request.opt_str("path").unwrap_or(".");
        let depth = request.opt_u64("depth").unwrap_or(1) as u32;
        let limits = ctx.limits().await;
        match file_ops::list_dir(&ctx.workspace, path, depth, file_ops::MAX_LIST_ENTRIES) {
            Ok(rendered) => {
                let (text, cut) = truncate_output(&rendered.text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text)
                    .truncated(cut || rendered.truncated)
                    .with_meta(rendered.meta)
            }
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

pub struct SearchFiles;

#[async_trait]
impl Tool for SearchFiles {
    fn spec(&self) -> ToolSpec {
        spec(
            "search_files",
            "Search file contents. Uses ripgrep when installed and falls back to a built-in regex \
             walker (both honour .gitignore). Returns `path:line: text` matches and reports which \
             engine ran; results are bounded by the configured search limit.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regex (ripgrep syntax)"},
                    "path": {"type": "string", "description": "Workspace-relative file or directory (default '.')"},
                    "glob": {"type": "string", "description": "Optional glob filter, e.g. *.rs"},
                    "max_results": {"type": "integer", "description": "Cap on returned matches"},
                    "case_sensitive": {"type": "boolean", "description": "Default true"}
                },
                "required": ["pattern"]
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let pattern = match request.arg_str("pattern") {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if pattern.is_empty() {
            return ToolOutcome::err("pattern may not be empty");
        }
        let limits = ctx.limits().await;
        let requested = request
            .opt_u64("max_results")
            .unwrap_or(limits.search_max_results as u64);
        let max_results = requested.min(limits.search_max_results as u64).max(1) as usize;
        let query = search::SearchQuery {
            pattern: pattern.to_string(),
            path: request.opt_str("path").unwrap_or(".").to_string(),
            glob: request.opt_str("glob").map(|g| g.to_string()),
            case_insensitive: !request.opt_bool("case_sensitive").unwrap_or(true),
            max_results,
        };

        match search::search(&ctx.workspace, &query, &ctx.cancel).await {
            Ok(result) => {
                let mut text = format!(
                    "{} match(es) for pattern {:?} in `{}` (engine: {}, cap {})\n\n{}",
                    result.matches.len(),
                    query.pattern,
                    query.path,
                    result.engine.label(),
                    max_results,
                    result.body()
                );
                if result.hit_limit {
                    text.push_str(&format!(
                        "\n\n[hit the {max_results}-match cap; narrow the pattern, add a glob or \
                         raise limits.search_max_results]"
                    ));
                }
                if let Some(warning) = &result.warning {
                    text.push_str(&format!("\n\n[warning: {warning}]"));
                }
                let (text, cut) = truncate_output(&text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text)
                    .truncated(cut || result.hit_limit)
                    .with_meta(json!({
                        "engine": result.engine.label(),
                        "matches": result.matches.len(),
                        "hit_limit": result.hit_limit,
                        "pattern": query.pattern,
                        "path": query.path,
                    }))
            }
            Err(kc_core::Error::Cancelled) => ToolOutcome::err("search cancelled"),
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "write_file",
            "Create or overwrite a file with the given content (max 2 MiB). Parent directories are \
             created and the write is atomic. Requires approval according to the approval policy; \
             refused in read_only mode.",
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

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = match request.arg_str("path") {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let content = match request.arg_str("content") {
            Ok(c) => c,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if content.len() > file_ops::MAX_FILE_WRITE_BYTES {
            return ToolOutcome::err(format!(
                "refusing to write {} bytes to `{path}`: write_file is capped at {} MiB per call. \
                 Split the content or generate it with a command instead.",
                content.len(),
                file_ops::MAX_FILE_WRITE_BYTES / (1024 * 1024)
            ));
        }

        let abs = match ctx.workspace.resolve_for_write(path) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let rel = ctx.workspace.relative(&abs);
        let overwrite = std::fs::symlink_metadata(&abs).is_ok();
        let bytes = content.len();
        let lines = content.lines().count();

        let policy = ctx.settings.approval_policy().await;
        let decision = auto_decision(&policy, ToolClass::Mutating, &rel);
        let detail = if overwrite {
            format!("overwrite existing file, {bytes} bytes")
        } else {
            format!("create new file, {bytes} bytes")
        };
        if let Err(refusal) = gate(
            &ctx,
            ApprovalKind::FileWrite,
            decision,
            &format!("write_file `{rel}`"),
            format!("Write {rel}"),
            detail,
            json!({"path": rel, "bytes": bytes, "overwrite": overwrite}),
        )
        .await
        {
            return refusal;
        }

        if let Err(e) = file_ops::write_atomic(&abs, content.as_bytes()) {
            return ToolOutcome::err(e.to_string());
        }

        ToolOutcome::ok(format!(
            "wrote {bytes} bytes ({lines} lines) to `{rel}` ({})",
            if overwrite {
                "overwrote existing file"
            } else {
                "created new file"
            }
        ))
        .with_meta(json!({"path": rel, "bytes": bytes, "lines": lines, "overwrite": overwrite}))
    }
}

pub struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "edit_file",
            "Replace an exact string in an existing UTF-8 file. Fails loudly when the old string is \
             missing or matches more than once (pass replace_all for multi-replace). The write is \
             atomic and requires approval; refused in read_only mode. Returns the changed lines plus \
             a diff preview.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string", "description": "Exact text to replace"},
                    "new_string": {"type": "string", "description": "Replacement text"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        )
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = match request.arg_str("path") {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let (old, new) = match (request.arg_str("old_string"), request.arg_str("new_string")) {
            (Ok(o), Ok(n)) => (o, n),
            (Err(e), _) | (_, Err(e)) => return ToolOutcome::err(e.to_string()),
        };
        let replace_all = request.opt_bool("replace_all").unwrap_or(false);

        let plan = match file_ops::plan_edit(&ctx.workspace, path, old, new, replace_all) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };

        let policy = ctx.settings.approval_policy().await;
        let decision = auto_decision(&policy, ToolClass::Mutating, &plan.rel);
        let detail = format!(
            "{} replacement(s) at line(s) {}\n{}",
            plan.occurrences,
            plan.line_numbers
                .iter()
                .take(10)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            plan.diff
        );
        if let Err(refusal) = gate(
            &ctx,
            ApprovalKind::FileEdit,
            decision,
            &format!("edit_file `{}`", plan.rel),
            format!("Edit {}", plan.rel),
            detail,
            json!({
                "path": plan.rel,
                "replacements": plan.occurrences,
                "lines": plan.line_numbers,
                "diff": file_ops::preview(&plan.diff, 2000),
                "old_preview": file_ops::preview(old, 200),
                "new_preview": file_ops::preview(new, 200),
            }),
        )
        .await
        {
            return refusal;
        }

        if let Err(e) = file_ops::write_atomic(&plan.abs, plan.new_content.as_bytes()) {
            return ToolOutcome::err(e.to_string());
        }

        let lines = plan
            .line_numbers
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        ToolOutcome::ok(format!(
            "edited `{}`: {} replacement(s) at line(s) {lines}\n{}",
            plan.rel, plan.occurrences, plan.diff
        ))
        .with_meta(json!({
            "path": plan.rel,
            "replacements": plan.occurrences,
            "lines": plan.line_numbers,
            "bytes": plan.new_content.len(),
        }))
    }
}

pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn spec(&self) -> ToolSpec {
        spec(
            "run_command",
            "Run a shell command with `/bin/sh -c` in the workspace and stream stdout/stderr. \
             This is NOT a sandbox: the command runs as the server user with that user's full \
             filesystem and network access, so treat it as trusted unless the approval policy says \
             otherwise. Requires approval per policy; refused in read_only mode. Output is kept \
             head+tail within the configured limit; timeouts kill the whole process group.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "cwd": {"type": "string", "description": "Workspace-relative working directory (default '.')"},
                    "timeout_secs": {"type": "integer", "description": "Optional timeout; clamped to the configured maximum"}
                },
                "required": ["command"]
            }),
        )
    }
    fn class(&self) -> ToolClass {
        ToolClass::Shell
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let command = match request.arg_str("command") {
            Ok(c) => c.to_string(),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if command.trim().is_empty() {
            return ToolOutcome::err("command may not be empty");
        }
        let cwd_arg = request.opt_str("cwd").unwrap_or(".");
        let limits = ctx.limits().await;
        let requested = request
            .opt_u64("timeout_secs")
            .unwrap_or(limits.command_timeout_secs)
            .max(1);
        let timeout_secs = requested.min(limits.max_command_timeout_secs.max(1)).max(1);

        let cwd = match ctx.workspace.resolve_existing(cwd_arg) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        if !cwd.is_dir() {
            return ToolOutcome::err(format!(
                "cwd `{cwd_arg}` is not a directory; run_command needs a directory inside the workspace"
            ));
        }
        let cwd_label = ctx.workspace.relative(&cwd);

        let policy = ctx.settings.approval_policy().await;
        let decision = auto_decision(&policy, ToolClass::Shell, &command);
        let title = format!("Run: {}", file_ops::preview(command.trim(), 100));
        let detail = format!("$ {command}\ncwd: {cwd_label}\ntimeout: {timeout_secs}s");
        if let Err(refusal) = gate(
            &ctx,
            ApprovalKind::ShellCommand,
            decision,
            "run_command",
            title,
            detail,
            json!({"command": command, "cwd": cwd_label, "timeout_secs": timeout_secs}),
        )
        .await
        {
            return refusal;
        }

        let outcome = match command::run(
            command::CommandRequest {
                call_id: request.call_id.clone(),
                command: command.clone(),
                cwd,
                cwd_label: cwd_label.clone(),
                timeout: Duration::from_secs(timeout_secs),
                max_output_bytes: limits.max_command_output_bytes,
            },
            &ctx,
        )
        .await
        {
            Ok(o) => o,
            Err(kc_core::Error::Cancelled) => {
                return ToolOutcome::err("cancelled before the command started")
            }
            Err(e) => return ToolOutcome::err(e.to_string()),
        };

        let mut text = format!(
            "$ {command}\ncwd: {cwd_label}\nexit code: {}\nduration: {:.2}s",
            outcome
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none (terminated by signal)".to_string()),
            outcome.duration.as_secs_f64()
        );
        if outcome.timed_out {
            text.push_str(&format!(
                "\n[timed out after {timeout_secs}s: the process group was sent SIGTERM, then SIGKILL]"
            ));
        }
        if outcome.cancelled {
            text.push_str("\n[cancelled: the process group was terminated]");
        }
        text.push_str(&format!(
            "\n\n--- stdout ({} bytes{}) ---\n{}",
            outcome.stdout.total_bytes,
            if outcome.stdout.truncated {
                ", truncated"
            } else {
                ""
            },
            if outcome.stdout.text.is_empty() {
                "(empty)"
            } else {
                outcome.stdout.text.trim_end_matches('\n')
            }
        ));
        if !outcome.stderr.text.is_empty() {
            text.push_str(&format!(
                "\n\n--- stderr ({} bytes{}) ---\n{}",
                outcome.stderr.total_bytes,
                if outcome.stderr.truncated {
                    ", truncated"
                } else {
                    ""
                },
                outcome.stderr.text.trim_end_matches('\n')
            ));
        }

        let (text, cut) = truncate_output(&text, limits.max_tool_output_bytes);
        ToolOutcome {
            ok: outcome.ok(),
            content: text,
            truncated: cut || outcome.truncated(),
            meta: Some(json!({
                "command": command,
                "cwd": cwd_label,
                "exit_code": outcome.exit_code,
                "duration_ms": outcome.duration.as_millis() as u64,
                "timed_out": outcome.timed_out,
                "cancelled": outcome.cancelled,
                "stdout_bytes": outcome.stdout.total_bytes,
                "stderr_bytes": outcome.stderr.total_bytes,
                "timeout_secs": timeout_secs,
            })),
        }
    }
}

pub struct GitStatus;

#[async_trait]
impl Tool for GitStatus {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_status",
            "Show git branch and working-tree status (`git status --porcelain=v1 -b`) for the \
             workspace or a subdirectory. Read-only, no approval. Reports is_repo: false when the \
             path is not in a git repository.",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Workspace-relative path (default '.')"}}
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = request.opt_str("path").unwrap_or(".");
        let limits = ctx.limits().await;
        match git::status(&ctx.workspace, Some(path)).await {
            Ok(report) => {
                let (text, cut) = truncate_output(&report.text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text).truncated(cut).with_meta(json!({
                    "is_repo": report.is_repo,
                    "branch": report.branch,
                    "entries": report.entries,
                }))
            }
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

pub struct GitDiff;

#[async_trait]
impl Tool for GitDiff {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_diff",
            "Show the staged or unstaged diff (`git diff [--staged] [base] -- <path>`) for the \
             workspace or a path. Read-only, no approval. Output is bounded by the tool-output limit.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative file or directory"},
                    "staged": {"type": "boolean", "description": "Diff the index instead of the working tree"},
                    "base": {"type": "string", "description": "Optional git ref to diff against"}
                }
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let staged = request.opt_bool("staged").unwrap_or(false);
        let base = request.opt_str("base");
        let path = request.opt_str("path");
        let limits = ctx.limits().await;
        match git::diff(&ctx.workspace, path, staged, base).await {
            Ok(report) => {
                let (text, cut) = truncate_output(&report.text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text).truncated(cut).with_meta(
                    json!({"empty": report.empty, "staged": staged, "base": base, "path": path}),
                )
            }
            Err(e) => ToolOutcome::err(e.to_string()),
        }
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
