//! First-party coding tools.
//!
//! File tools are confined to a [`kc_core::paths::Workspace`]. `run_command` is
//! explicitly *not* sandboxed: it spawns a real process as the server user, in
//! its own process group so cancellation can kill the whole tree.

// @kc-doc: builtin-tools
// @kc-title: The built-in tools the agent can call
// @kc-summary: What each first-party tool does, how it is gated, and the limits and gotchas that shape its output.
// @kc-tags: tools, agent, files, search, shell, git
// @kc-applies-to: tool:read_file, tool:list_dir, tool:search_files, tool:write_file, tool:edit_file, tool:run_command, tool:git_status, tool:git_diff, tool:karucode_docs, tool:spawn_agent, tool:spawn_agents, tool:send_message, tool:wait_for_agents, tool:list_subagents, tool:agent_history, tool:pause_agents, tool:resume_agents, tool:kill_agents, tool:report_status, route:POST /api/sessions/:id/rewind, config:[limits], config:allow_temp_dir, env:KARUCODE_ALLOW_TEMP_DIR, config:[model], config:model.supports_images, config:limits.max_image_read_bytes, env:KARUCODE_MODEL_SUPPORTS_IMAGES
//
// | Tool | Class | Approval | What it does |
// | --- | --- | --- | --- |
// | `read_file` | read_only | none | UTF-8 text with numbered lines and a `path - lines A-B of N` header; `offset`/`limit` (default 400, max 2000 lines). `N` is the real line count of the whole file, even when `limits.max_file_read_bytes` cut the read short. A PNG/JPEG/GIF/WebP is attached as an image for a model that takes image input; a text-only model gets a refusal it can pass on |
// | `list_dir` | read_only | none | directory listing, `.gitignore`-aware, directories first, max depth 3, 500 entries; the walk stops at the cap and a truncated listing reports the total as a lower bound (`500+`) |
// | `search_files` | read_only | none | content search via ripgrep, with a pure-Rust regex fallback; returns `path:line: text`. The result cap is global across files, and long matching lines keep their first 300 characters in both engines |
// | `write_file` | mutating | policy | create/overwrite atomically, max 2 MiB per call; journals the previous bytes for a rewind |
// | `edit_file` | mutating | policy | exact-string replace, refuses an ambiguous or missing match, returns a diff; revalidates the file after approval and journals the previous bytes |
// | `run_command` | shell | policy | `/bin/sh -c` in the workspace, incrementally UTF-8-decoded streamed output, timeout, own process group. On timeout or cancellation the whole group gets SIGTERM, then SIGKILL after a grace period, so a child that ignores SIGTERM cannot survive |
// | `git_status` | read_only | none | branch + porcelain status for the workspace or a subdirectory, **confined to the workspace** |
// | `git_diff` | read_only | none | working-tree or `--staged` diff, optionally against a ref, **confined to the workspace** (paths are workspace-relative) |
// | `karucode_docs` | read_only | none | search and read karucode's own documentation (this system) |
//
// The delegate family - `spawn_agent`, `spawn_agents`, `send_message`,
// `wait_for_agents`, `list_subagents`, `agent_history`, `pause_agents`,
// `resume_agents`, `kill_agents` and `report_status` - is registered by the
// runtime (`crates/kc-runtime/src/subagent.rs`), not here: every one of them needs
// the runtime's live agent registry, which a stateless tool cannot reach. They are
// class `delegate`, and `spawn_agent`/`spawn_agents` take a `background` flag that
// detaches the child so the caller can keep working. The `subagents` topic is
// their reference; this page lists them so the table above is the whole built-in
// surface. Plugin tools are namespaced `plugin__<plugin>__<tool>` and MCP tools
// `mcp__<server>__<tool>`, so a collision with a first-party name is impossible.
//
// # Examples the model is expected to use
//
// ```json
// {"name": "read_file", "arguments": {"path": "src/main.rs", "offset": 120, "limit": 40}}
// {"name": "search_files", "arguments": {"pattern": "fn main", "glob": "*.rs"}}
// {"name": "edit_file", "arguments": {"path": "a.txt", "old_string": "old", "new_string": "new"}}
// ```
//
// # Output limits
//
// Tool output kept for the model is bounded by `limits.max_tool_output_bytes`
// (default 24000), **including the truncation marker**. A cut retains head+tail so
// a final runtime-budget reduction does not discard a trailing error; unusually
// tiny budgets that cannot fit the explanatory marker retain a UTF-8-safe prefix.
// `run_command` retains raw stdout head+tail and trailing stderr within that final
// budget rather than front-truncating an already-rendered marker. Capture is first
// bounded per stream by `max_command_output_bytes`; long command lines in the
// result header are previewed at 512 characters (metadata still has the command).
//
// # Gotchas
//
// * `edit_file` is exact-match and ambiguity-safe: it refuses when the old string
//   appears twice (pass `replace_all`) or not at all. Read the region first. After
//   approval it re-reads and re-plans with a SHA-256 fingerprint; if the file
//   changed during the wait, it refuses without writing so the fresh diff can be
//   reviewed. Dense `replace_all` diffs use an 80-line capped block preview rather
//   than allocating LCS/operation state for every changed line.
// * File tools are confined to the run's project directory **plus the system
//   temporary directory**: `..`, any other absolute path and symlink escapes are
//   refused, for reads and writes alike. See "Scratch space" below.
// * `run_command` is **not** sandboxed. It runs as the server user, in a process
//   group that is killed on timeout or cancellation; an approved command can do
//   anything that user can do.
// * `write_file` and `edit_file` must capture the file's previous bytes in the
//   rewind journal immediately before they write; if an attached journal cannot
//   persist a complete before-image, the tool refuses and leaves the file
//   unchanged. `POST /api/sessions/:id/rewind` with `revert` can then put the file
//   back. Direct tool calls with no journal keep their non-journaled behaviour.
//   A tool that changes files without recording cannot be undone - `run_command`
//   is the big one, and the preview lists it under `opaque_tools`.
// * `run_command` returns `ok: false` for a non-zero exit, with the exit code in
//   the header - a failing test run is a failed tool call, not a silent success.
// * `search_files` honours `.gitignore`. Searching ignored/build output needs a
//   pattern under an explicitly named path, not a glob trick. The pure-Rust
//   fallback walks lazily, observes cancellation during walking/reads, and skips
//   files over 4 MiB; bounded reads cannot grow past that cap during the scan.
// * `search_files` caps results globally, not per file, and both engines agree on
//   the cap. A match on a very long line keeps its first 300 **characters** rather
//   than being replaced by a placeholder; the cap is counted in characters, not
//   bytes, so a line of multi-byte text is cut in the same place by both engines
//   (ripgrep's own `--max-columns` counts bytes, so it is set wide enough that our
//   character cap is the one that decides). A path containing `:` is reported
//   correctly: ripgrep is asked for `--null` records.
// * `run_command` reports a **success** when the command exits 0 and leaves a
//   child running in the background (`build & exit 0`) instead of waiting for that
//   child to close the output pipes. The result says the output may be incomplete
//   and `background_output` is set in the metadata.
// * On Unix, backslashes in filenames are literal characters, including in tool
//   metadata and rewind checkpoint keys; they are not rewritten as `/` separators.
// * A narrowed `git_status`/`git_diff` path also accepts a deleted tracked file
//   (or one whose parent directory was deleted). The path is checked lexically and
//   through its deepest existing ancestor, so symlink escapes are still refused.
// * `read_file` reports the file's real line count in its header even when the
//   byte cap truncated the content; when the byte cap (not the line window) ended
//   the read, there is no "continue with offset" hint, because those lines cannot
//   be reached without raising `limits.max_file_read_bytes`.
// * Binary files are refused by `read_file` instead of being rendered as noise:
//   the check covers the whole window that would be returned, so a NUL deep in a
//   large file cannot reach the model as U+0000.
//
// # Scratch space: the temp directory
//
// A file tool may reach the system temporary directory as well as the project.
// Anything a build, a screenshot helper, a browser profile or a download leaves
// in `/tmp` can then be read, searched, edited and written without a path error,
// and an agent asked to save something "to a temp file" has somewhere legal to
// put it.
//
// This is on by default because it is not a security boundary. `run_command`
// already runs as the server user with that user's whole filesystem, and `/tmp`
// is writable by every process on the machine; refusing `write_file` a path that
// `/bin/sh` could `echo` into buys nothing and fails a whole class of ordinary
// work. What the allowlist protects is the *model* not wandering into
// `~/.ssh` or `/etc`, and that still holds.
//
// What the temp root is **not** is the whole filesystem. The root is checked like
// any other root:
//
// * `..` may not climb out of it - `/tmp/../etc/passwd` is refused;
// * a symlink inside it that points elsewhere is refused;
// * neither is `/etc` or a sibling directory of `/tmp`.
//
// Turn it off to go back to project-only confinement:
//
// ```toml
// allow_temp_dir = false      # or KARUCODE_ALLOW_TEMP_DIR=0
// ```
//
// The root is `TMPDIR` when it is set and absolute, otherwise the platform
// default (`/tmp` on Linux). A configured root that does not exist is skipped. A
// workspace that itself lives under `/tmp` (a scratch or CI checkout) still gets
// it: the temp dir being an ancestor of the project is exactly what such a layout
// asks for, so it is not treated as a mistake. Only `/` is refused, because that
// one value would make every absolute path legal.
// `run_command` is unaffected either way: it is not confined at all. Its
// children still get their normal environment, so they write to `/tmp` whatever
// this setting says.
//
// # Images
//
// `read_file` on an image does not return text. The file is classified by its
// magic bytes (not its extension): a PNG, JPEG, GIF or WebP is base64-encoded and
// attached to the tool result, so a vision-capable model looks at the pixels.
//
// This only happens when the session's model accepts image input. It does when
// the resolved model says so:
//
// ```toml
// [model]
// supports_images = true          # or KARUCODE_MODEL_SUPPORTS_IMAGES=1
//
// [[providers.models]]
// id = "gpt-4o"
// vision = true                   # per-model override, prefilled from models.dev
// ```
//
// With it off, `read_file` refuses with a sentence the model is expected to repeat
// to the user: it cannot see images, and switching models or describing the image
// are the ways forward. That refusal is the point - a text-only model that "reads"
// an image silently would otherwise invent what is in it.
//
// The image bytes count against `limits.max_image_read_bytes` (4 MiB by default)
// and are kept in the transcript, so they are resent on every later turn of the
// session. A file over the cap is refused with its size and the key's name.
// Formats the vision APIs reject (BMP, ICO, TIFF, AVIF, HEIC) are named and
// refused too, with the conversion command to use; SVG is text and is read as
// text.
//
// # Environment
//
// `run_command` does **not** inherit the server's environment. The child gets a
// cleared environment plus an allowlist: `PATH`, `HOME`, `USER`, `LOGNAME`,
// `SHELL`, `LANG`, `LC_ALL` (forced to `C`), `TERM`, `TMPDIR`, `PWD`, `TZ`, and
// any names in `limits.command_env_passthrough`. A variable is only passed when
// it is actually set, so no empty values are invented.
//
// Secrets are never passed to a command: `KARUCODE_*`, `AWS_*`, `OPENAI_*`,
// `ANTHROPIC_*`, `GITHUB_TOKEN`, `GH_TOKEN`, `NPM_TOKEN` and any name ending in
// `_API_KEY`, `_TOKEN`, `_SECRET` or `_PASSWORD` are dropped even if listed in
// `command_env_passthrough`. The harness bearer token and the provider API key
// therefore cannot be exfiltrated by a prompt-injected `curl`. A command that
// needs a credential must be given it explicitly (e.g. read from a file).
//
// Full reference: `README.md` (What it does), `docs/SECURITY.md`.

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
use tokio_util::sync::CancellationToken;

mod bounded_pipe;
pub mod command;
pub mod docs_tool;
pub mod favicon;
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

/// Run blocking file work off the runtime's worker threads.
///
/// Reading a file, walking a directory or computing an edit plan is CPU- and
/// IO-bound; on a runtime worker it stalls every other task on that thread for
/// its whole duration (an 8 MiB `edit_file` is seconds). `spawn_blocking` is the
/// same tool `search::search_fallback` already uses for its walker, and the join
/// error is surfaced rather than swallowed so a panic is not reported as success.
async fn blocking<T, F>(
    what: &str,
    cancel: &CancellationToken,
    work: F,
) -> std::result::Result<kc_core::Result<T>, ToolOutcome>
where
    F: FnOnce() -> kc_core::Result<T> + Send + 'static,
    T: Send + 'static,
{
    if cancel.is_cancelled() {
        return Ok(Err(kc_core::Error::Cancelled));
    }
    let closure_cancel = cancel.clone();
    // Check again inside the closure. spawn_blocking can sit in a queue after a
    // run is cancelled; a mutating closure must not begin its side effects then.
    let work = move || {
        if closure_cancel.is_cancelled() {
            return Err(kc_core::Error::Cancelled);
        }
        work()
    };
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => Ok(result),
        Err(e) => Err(ToolOutcome::err(format!("{what} task failed: {e}"))),
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
            "Read a file inside the workspace. Text comes back as numbered lines with a \
             `path — lines A-B of N (bytes)` header (use offset/limit for large files: default \
             400 lines, max 2000). An image (PNG/JPEG/GIF/WebP) is attached for a model that \
             accepts image input; on a text-only model it is refused with an explanation, as are \
             binary files and image formats vision APIs do not accept. Output is capped by the \
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

        // An image is not text. Classify it first: a vision model gets the pixels,
        // a text-only model gets a refusal it can pass on instead of a guess.
        match file_ops::probe_image(&ctx.workspace, path) {
            Ok(file_ops::ImageProbe::Vision {
                rel,
                media_type,
                size,
            }) => {
                if !ctx.supports_images {
                    return ToolOutcome::err(format!(
                        "`{rel}` is an image ({media_type}, {}). The active model cannot see images \
                         (model.supports_images is off), so read_file cannot show it to you. Tell \
                         the user plainly that you cannot view images, and that they can describe it, \
                         or switch to a vision-capable model (model.supports_images = true, or \
                         vision = true on the model's provider entry) and ask again.",
                        file_ops::format_size(size)
                    ));
                }
                return match file_ops::read_image(
                    &ctx.workspace,
                    path,
                    media_type,
                    limits.max_image_read_bytes,
                ) {
                    Ok(image) => {
                        let text = format!(
                            "{rel} — {media_type} ({})\n[the image itself is attached to this result: look \
                             at it directly, there is no text to read]",
                            file_ops::format_size(size)
                        );
                        ToolOutcome::ok(text)
                            .with_images(vec![image])
                            .with_meta(json!({
                                "path": rel,
                                "media_type": media_type,
                                "bytes": size,
                                "image": true,
                            }))
                    }
                    Err(e) => ToolOutcome::err(e.to_string()),
                };
            }
            Ok(file_ops::ImageProbe::Unsupported {
                rel,
                media_type,
                size,
            }) => {
                return ToolOutcome::err(format!(
                    "`{rel}` is a {media_type} image ({}), a format the vision APIs do not accept, \
                     so read_file cannot attach it. Convert it first - e.g. run_command with \
                     `magick '{rel}' out.png` or `ffmpeg -i '{rel}' out.png` - then read the \
                     converted file. Accepted formats: {}.",
                    file_ops::format_size(size),
                    file_ops::VISION_MEDIA_TYPES.join(", ")
                ));
            }
            // Not an image, or a path error the text read reports precisely.
            Ok(file_ops::ImageProbe::NotAnImage) | Err(_) => {}
        }

        // The file is read and (when the byte cap cut it) scanned to count lines:
        // blocking work that must not run on a runtime worker.
        let rendered = match blocking("read_file", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let path = path.to_string();
            let max_bytes = limits.max_file_read_bytes;
            move || file_ops::read_file(&ws, &path, offset, limit, max_bytes)
        })
        .await
        {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(rendered)) => rendered,
        };
        let (text, cut) = truncate_output(&rendered.text, limits.max_tool_output_bytes);
        ToolOutcome::ok(text)
            .truncated(cut || rendered.truncated)
            .with_meta(rendered.meta)
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
             Capped at 500 entries; the walk stops at the cap, so a truncated listing says the \
             total is a lower bound (`500+`).",
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
        // The walk stats every entry it visits; on a runtime worker that stalls
        // every other task on the thread for as long as the directory takes.
        let rendered = match blocking("list_dir", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let path = path.to_string();
            move || file_ops::list_dir(&ws, &path, depth, file_ops::MAX_LIST_ENTRIES)
        })
        .await
        {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(rendered)) => rendered,
        };
        let (text, cut) = truncate_output(&rendered.text, limits.max_tool_output_bytes);
        ToolOutcome::ok(text)
            .truncated(cut || rendered.truncated)
            .with_meta(rendered.meta)
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

        // Approval can wait indefinitely. A cancellation that arrived while the
        // prompt (or checkpoint write) was pending must not turn into a file write.
        if ctx.cancelled() {
            return ToolOutcome::err("write_file cancelled before the file was changed");
        }

        // Before-image, immediately before the write it describes. Recording
        // after the write would journal the new bytes, and a rewind that
        // "restores" them is a no-op that looks like a successful revert.
        let captured = ctx.capture_checkpoint(&rel, "write_file").await;
        if !captured {
            return ToolOutcome::err(format!(
                "refusing to write `{rel}` because its rewind checkpoint could not be captured; the file is unchanged. Retry after checkpoint storage is available."
            ));
        }
        if ctx.cancelled() {
            return ToolOutcome::err("write_file cancelled before the file was changed");
        }

        // The write is a create + `sync_all` + rename on a worker thread: fsync can
        // block for a long time on a busy disk.
        let written = blocking("write_file", &ctx.cancel, {
            let abs = abs.clone();
            let content = content.to_string();
            move || file_ops::write_atomic(&abs, content.as_bytes())
        })
        .await;
        match written {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(())) => {}
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

        // Edit planning reads the whole file, finds every match and builds the
        // diff: CPU- and IO-bound work that must not run on a runtime worker -
        // `MAX_EDIT_BYTES` is 8 MiB, so a large file would otherwise stall every
        // other task on this thread for the duration. Approval still happens
        // after this plan and before the write, so its semantics are unchanged.
        let plan = match blocking("edit_file planning", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let path = path.to_string();
            let old = old.to_string();
            let new = new.to_string();
            move || file_ops::plan_edit(&ws, &path, &old, &new, replace_all)
        })
        .await
        {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(plan)) => plan,
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

        if ctx.cancelled() {
            return ToolOutcome::err("edit_file cancelled before the file was changed");
        }

        // The approved preview is a snapshot, not a lock. Re-read and re-plan after
        // the approval wait so an intervening edit is never overwritten by stale
        // bytes. A changed fingerprint needs a new approval: even when old_string
        // still matches, its line numbers and surrounding diff are no longer what
        // the user approved. The conservative refusal also covers ambiguous or
        // missing matches in the fresh plan (returned directly by plan_edit).
        let approved_fingerprint = plan.fingerprint;
        let approved_rel = plan.rel.clone();
        let plan = match blocking("edit_file revalidation", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let path = path.to_string();
            let old = old.to_string();
            let new = new.to_string();
            move || file_ops::plan_edit(&ws, &path, &old, &new, replace_all)
        })
        .await
        {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(plan)) => plan,
        };
        if plan.fingerprint != approved_fingerprint {
            return ToolOutcome::err(format!(
                "`{approved_rel}` changed while edit_file was waiting for approval. The file was not modified; re-read it and issue edit_file again so the fresh replacement and diff can be reviewed."
            ));
        }
        if ctx.cancelled() {
            return ToolOutcome::err("edit_file cancelled before the file was changed");
        }

        // Before-image, immediately before the write it describes. A managed
        // journal must acknowledge a complete capture or the edit is refused.
        let captured = ctx.capture_checkpoint(&plan.rel, "edit_file").await;
        if !captured {
            return ToolOutcome::err(format!(
                "refusing to edit `{}` because its rewind checkpoint could not be captured; the file is unchanged. Retry after checkpoint storage is available.",
                plan.rel
            ));
        }
        if ctx.cancelled() {
            return ToolOutcome::err("edit_file cancelled before the file was changed");
        }

        // Write the revalidated bytes off the runtime worker, same as write_file.
        let written = blocking("edit_file", &ctx.cancel, {
            let abs = plan.abs.clone();
            let new_content = plan.new_content.clone();
            move || file_ops::write_atomic(&abs, new_content.as_bytes())
        })
        .await;
        match written {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(())) => {}
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
             head+tail within the configured limit, and stdout and stderr are budgeted separately so \
             a long stdout cannot push stderr out. Timeouts kill the whole process group. A command \
             that exits 0 while a background child still holds the output pipes is reported as a \
             success whose output may be incomplete, not as a timeout.",
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

        let command_display = file_ops::preview(command.trim(), 512);
        let command_was_clipped = command_display != command.trim();
        let mut text = format!(
            "$ {command_display}\ncwd: {cwd_label}\nexit code: {}\nduration: {:.2}s",
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
        if outcome.background_output {
            // The leader exited 0 but a child it started in the background still
            // held the output pipes. Waiting for EOF would have turned this
            // success into a timeout; stopping early would silently lose output.
            // Say both things instead of implying the capture is complete.
            text.push_str(&format!(
                "\n[note: a process started in the background is still running and still holds \
                 the output pipes, so the stdout/stderr below may be incomplete - this is not a \
                 timeout, the command itself exited {}]",
                outcome
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "without a status".to_string())
            ));
        }
        let (text, output_cut) =
            render_command_output(text, &outcome, limits.max_tool_output_bytes);
        ToolOutcome {
            ok: outcome.ok(),
            content: text,
            truncated: output_cut || command_was_clipped || outcome.truncated(),
            meta: Some(json!({
                "command": command,
                "cwd": cwd_label,
                "exit_code": outcome.exit_code,
                "duration_ms": outcome.duration.as_millis() as u64,
                "timed_out": outcome.timed_out,
                "cancelled": outcome.cancelled,
                "background_output": outcome.background_output,
                "stdout_bytes": outcome.stdout.total_bytes,
                "stderr_bytes": outcome.stderr.total_bytes,
                "timeout_secs": timeout_secs,
            })),
            images: Vec::new(),
        }
    }
}

fn command_stream_heading(label: &str, stream: &command::FinishedStream) -> String {
    format!(
        "\n\n--- {label} ({} bytes{}) ---\n",
        stream.total_bytes,
        if stream.truncated { ", truncated" } else { "" }
    )
}

/// Assemble a command result whose marker and section headers are included in
/// the real final byte budget. Stream bodies are re-rendered from raw retained
/// head/tail bytes, so the code never truncates an already rendered marker and
/// never gives a long stdout permission to evict a short trailing stderr.
fn render_command_output(
    header: String,
    outcome: &command::CommandOutcome,
    max_bytes: usize,
) -> (String, bool) {
    let stdout_heading = command_stream_heading("stdout", &outcome.stdout);
    let has_stderr = !outcome.stderr.text.is_empty();
    let stderr_heading = if has_stderr {
        command_stream_heading("stderr", &outcome.stderr)
    } else {
        String::new()
    };
    let stdout_empty = outcome.stdout.text.is_empty();
    let empty_marker_len = if stdout_empty { "(empty)".len() } else { 0 };
    let fixed = header.len() + stdout_heading.len() + stderr_heading.len() + empty_marker_len;

    // Tiny configured/remaining budgets cannot fit both headings. Use the shared
    // final head+tail limiter, which still retains a trailing error where space
    // permits, and always respects the actual cap.
    if fixed >= max_bytes {
        let mut full = header;
        full.push_str(&stdout_heading);
        full.push_str(if stdout_empty {
            "(empty)"
        } else {
            &outcome.stdout.text
        });
        if has_stderr {
            full.push_str(&stderr_heading);
            full.push_str(&outcome.stderr.text);
        }
        return truncate_output(&full, max_bytes);
    }

    let available = max_bytes - fixed;
    let (stdout_budget, stderr_budget) = if !has_stderr || stdout_empty {
        (0, available)
    } else {
        let mut stdout = available / 4 * 3;
        let mut stderr = available - stdout;
        // Do not reserve thousands of bytes for an error stream that only needs
        // one line; stdout gets the spare. The symmetric branch helps when stdout
        // is short but stderr is long. Previously cut streams can always use their
        // allocation to retain more raw data, so only uncut streams donate.
        if !outcome.stderr.truncated && outcome.stderr.text.len() < stderr {
            stdout += stderr - outcome.stderr.text.len();
            stderr = outcome.stderr.text.len();
        }
        if !outcome.stdout.truncated && outcome.stdout.text.len() < stdout {
            stderr += stdout - outcome.stdout.text.len();
            stdout = outcome.stdout.text.len();
        }
        (stdout, stderr)
    };

    let stdout_text = if stdout_empty {
        "(empty)".to_string()
    } else if has_stderr {
        outcome.stdout.render_with_budget(stdout_budget)
    } else {
        outcome.stdout.render_with_budget(available)
    };
    let stderr_text = if has_stderr {
        outcome.stderr.render_with_budget(stderr_budget)
    } else {
        String::new()
    };
    let stdout_cut = !stdout_empty
        && outcome.stdout.text.len() > if has_stderr { stdout_budget } else { available };
    let stderr_cut = has_stderr && outcome.stderr.text.len() > stderr_budget;

    let mut text = header;
    text.push_str(&stdout_heading);
    text.push_str(&stdout_text);
    if has_stderr {
        text.push_str(&stderr_heading);
        text.push_str(&stderr_text);
    }
    debug_assert!(text.len() <= max_bytes, "{} > {max_bytes}", text.len());
    (text, stdout_cut || stderr_cut)
}

pub struct GitStatus;

#[async_trait]
impl Tool for GitStatus {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_status",
            "Show git branch and working-tree status (`git status --porcelain=v1 -b`) for the \
             workspace or a subdirectory. Read-only, no approval. Reports is_repo: false when the \
             path is not in a git repository. Confined to the workspace: when the workspace is a \
             subdirectory of a bigger repository, only changes inside it are listed (paths are \
             workspace-relative) and the enclosing repository's paths are not revealed.",
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
                    "confined": report.confined,
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
            "Show the staged or unstaged diff (`git diff [--staged] [base] --relative -- <path>`) \
             for the workspace or a path. Read-only, no approval. Confined to the workspace: when \
             the workspace is a subdirectory of a bigger repository, the patch is relative to the \
             workspace root and changes outside it are not shown. Output is bounded by the \
             tool-output limit.",
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
        Arc::new(docs_tool::KarucodeDocs),
    ]
}
