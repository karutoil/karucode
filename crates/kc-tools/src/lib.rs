//! First-party coding tools.
//!
//! File tools are confined to a [`kc_core::paths::Workspace`]. `run_command` is
//! explicitly *not* sandboxed: it spawns a real process as the server user, in
//! its own process group so cancellation can kill the whole tree.

// @kc-doc: builtin-tools
// @kc-title: The built-in tools the agent can call
// @kc-summary: What each first-party tool does, how it is gated, and the limits and gotchas that shape its output.
// @kc-tags: tools, agent, files, search, shell, git
// @kc-applies-to: tool:read_file, tool:list_dir, tool:search_files, tool:glob, tool:write_file, tool:edit_file, tool:run_command, tool:git_status, tool:git_diff, tool:git_log, tool:karucode_docs, tool:spawn_agent, tool:spawn_agents, tool:send_message, tool:wait_for_agents, tool:list_subagents, tool:agent_history, tool:pause_agents, tool:resume_agents, tool:kill_agents, tool:report_status, route:POST /api/sessions/:id/rewind, config:[limits], config:limits.max_file_read_bytes, config:limits.search_max_results, config:limits.max_tool_output_bytes, config:allow_temp_dir, env:KARUCODE_ALLOW_TEMP_DIR, config:[model], config:model.supports_images, config:limits.max_image_read_bytes, env:KARUCODE_MODEL_SUPPORTS_IMAGES, env:RIPGREP_CONFIG_PATH
//
// | Tool | Class | Approval | What it does |
// | --- | --- | --- | --- |
// | `read_file` | read_only | none | UTF-8 text with numbered lines and a `path - lines A-B of N` header; `offset`/`limit` (default 400, max 2000 lines). One call reads one file (`path`), several files (`paths`) or several windows of one file (`ranges`); a bad path or range is reported inline and the other reads still return. `limits.max_file_read_bytes` bounds the bytes one read *returns*, not the offsets it can reach, so any line of a huge file is readable. A PNG/JPEG/GIF/WebP given as `path` is attached as an image for a model that takes image input; a text-only model gets a refusal it can pass on |
// | `list_dir` | read_only | none | directory listing, `.gitignore`-aware, directories first, max depth 3, 500 entries; the walk stops at the cap and a truncated listing reports the total as a lower bound (`500+`) |
// | `search_files` | read_only | none | content search via ripgrep, with a pure-Rust regex fallback; returns `path:line: text`, plus `path:line- text` context lines with `context`, or one path per line with `files_only`. The result cap is global across files, `max_matches_per_file` stops one noisy file spending it, and long matching lines keep their first 300 characters in both engines |
// | `glob` | read_only | none | name-based finder (not a content search): a glob over the same `.gitignore`-aware walker the search fallback uses. Directories carry a trailing `/`; bounded at 200 paths and a truncated walk says so |
// | `write_file` | mutating | policy | create/overwrite atomically, max 2 MiB per call; journals the previous bytes for a rewind |
// | `edit_file` | mutating | policy | exact-string replace, refuses an ambiguous or missing match, returns a diff; revalidates the file after approval and journals the previous bytes |
// | `run_command` | shell | policy | `/bin/sh -c` in the workspace, incrementally UTF-8-decoded streamed output, timeout, own process group. On timeout or cancellation the whole group gets SIGTERM, then SIGKILL after a grace period, so a child that ignores SIGTERM cannot survive |
// | `git_status` | read_only | none | branch + porcelain status for the workspace or a subdirectory, **confined to the workspace** |
// | `git_diff` | read_only | none | working-tree or `--staged` diff, optionally against a ref, **confined to the workspace** (paths are workspace-relative) |
// | `git_log` | read_only | none | bounded commit history: one line per commit (`sha date subject`), `max_count` default 20 and hard-capped at 200, optional `path` narrowing, **confined to the workspace** |
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
// {"name": "read_file", "arguments": {"paths": ["Cargo.toml", "README.md"]}}
// {"name": "read_file", "arguments": {"path": "src/lib.rs", "ranges": [{"offset": 1, "limit": 40}, {"offset": 900, "limit": 40}]}}
// {"name": "search_files", "arguments": {"pattern": "fn main", "glob": "*.rs"}}
// {"name": "search_files", "arguments": {"pattern": "spawn_agent", "context": 2, "max_matches_per_file": 5}}
// {"name": "search_files", "arguments": {"pattern": "TODO", "files_only": true}}
// {"name": "glob", "arguments": {"pattern": "*.toml"}}
// {"name": "git_log", "arguments": {"path": "crates/kc-tools", "max_count": 10}}
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
//   the cap. `max_matches_per_file` adds a per-file cap on top, so one noisy file
//   cannot spend the whole global budget; `0` (the default) leaves only the
//   global cap. `context` (N lines before *and* after each match, like `grep -C`)
//   is reported as `path:line- text` so a context line is never mistaken for the
//   matching line, and it is capped at 10 lines. Reaching the global cap still
//   drains the accepted match's requested after-context, but never admits a later
//   real match. Overlapping windows keep every real hit labelled as a hit.
//   `files_only` returns the
//   distinct paths that matched, one per line, like `grep -l`; `context` has no
//   meaning there and is ignored. Both engines produce the **same shape** for
//   all three - the crate's promise is that the model cannot tell them apart
//   except by the engine label in the header.
// * `search_files` keeps the first 300 **characters** of a very long matching
//   line rather than replacing it with a placeholder; the cap is counted in
//   characters, not bytes, so a line of multi-byte text is cut in the same place
//   by both engines (ripgrep's own `--max-columns` counts bytes, so it is set
//   wide enough that our character cap is the one that decides). A literal suffix
//   that resembles ripgrep's preview marker is preserved on a short line. Paths
//   containing `:` or newlines are reported correctly: ripgrep paths end at NUL,
//   and only a newline after that delimiter ends the record. For security,
//   ripgrep is spawned without inherited `RIPGREP_CONFIG_PATH`; a user config's
//   `--pre` command therefore cannot turn this read-only tool into shell execution.
// * `glob` matches an entry's file name, its path relative to `path` and its
//   workspace-relative path, so `*.rs`, `src/**/*.rs` and `crates/*/src/lib.rs`
//   all work. It lists directories too, with a trailing `/`, and its walk stops
//   at 200 paths (the note says the cap cut it). The run's cancellation token is
//   checked on every yielded entry or error. Read-only, no approval.
// * `run_command` reports a **success** when the command exits 0 and leaves a
//   child running in the background (`build & exit 0`) instead of waiting for that
//   child to close the output pipes. The result says the output may be incomplete
//   and `background_output` is set in the metadata.
// * On Unix, backslashes in filenames are literal characters, including in tool
//   metadata and rewind checkpoint keys; they are not rewritten as `/` separators.
// * A narrowed `git_status`/`git_diff`/`git_log` path also accepts a deleted
//   tracked file (or one whose parent directory was deleted). The path is checked
//   lexically and through its deepest existing ancestor, so symlink escapes are
//   still refused.
// * `read_file` reports the file's real line count in its header even when the
//   byte cap truncated the content. The cap bounds the bytes *returned* by one
//   read, not the offsets that can be asked for: an `offset` past what
//   `limits.max_file_read_bytes` covers is reached by streaming forward to that
//   window (bounded work, cancellable), so a 400 KB file is readable at line 9 000
//   without raising the limit or shelling out to `sed`. A window cut by the byte
//   cap - not by `limit` and not by the end of the file - carries the cap note and
//   no "continue with offset" hint, because that hint would promise lines the same
//   cap cannot return; ask for the next window explicitly instead.
// * A batched `read_file` (`paths` or `ranges`) divides `limits.max_tool_output_bytes`
//   across its sections, so a wide batch still shows every file's header and first
//   lines instead of returning one file in full. Up to 32 reads are attempted per
//   call; a larger request is cut and says how many were dropped. A failed entry
//   keeps its place in the result as `[error: ...]` and the others still return -
//   `ok: false` is reported only when *every* read failed. Images are only
//   attached for a single `path`, so read an image on its own.
// * Binary files are refused by `read_file` instead of being rendered as noise:
//   the check covers the whole window that would be returned, and the offset in
//   the message is the byte offset in the file, even for a far window; so a NUL
//   deep in a large file cannot reach the model as U+0000.
// * `git_log` runs with the same hardened config as `git_status`/`git_diff` and
//   additionally disables signature display, because showing a signature runs the
//   repository's own `gpg` program. It prints `sha date subject` per commit and
//   requests one extra commit so a "showing the latest N" note is a fact rather
//   than a guess.
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

/// How many separate reads one `read_file` call may ask for.
///
/// A batch is not refused for being large; it is cut and says so. The cap exists
/// so one call cannot ask the filesystem for a thousand files.
const MAX_MULTI_READS: usize = 32;

/// One requested read of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileRead {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

/// What a batch entry produced: the rendered read, or the reason it could not be
/// read. One bad path never discards the good ones.
enum ReadEntry {
    Read {
        /// The path as the caller wrote it, for the section separator.
        path: String,
        rendered: file_ops::Rendered,
    },
    Failed {
        label: String,
        message: String,
    },
}

/// One entry of the parsed request: a read to perform, or an entry that is
/// malformed (a `paths` element that is not a string, a range with an unknown
/// key). A malformed entry keeps its place in the result instead of failing the
/// whole call.
enum PlannedRead {
    Read(FileRead),
    Bad { label: String, message: String },
}

/// One `read_file` call, parsed.
struct ReadPlan {
    reads: Vec<PlannedRead>,
    /// True when the caller used `paths`/`ranges`, so the batch result shape is
    /// kept even for a single entry.
    batch: bool,
    /// Reads the per-call cap dropped.
    dropped: usize,
}

/// Parse `path` / `paths` / `ranges` into the reads the call asked for.
///
/// `Err` is a usage failure at the top level (two shapes at once, an empty list,
/// no shape at all): it is not clear what was asked, so there is nothing to
/// report "inline and continue" with. A single bad *entry* is a
/// [`PlannedRead::Bad`] and does not disturb its neighbours.
fn parse_reads(arguments: &serde_json::Value) -> std::result::Result<ReadPlan, String> {
    let offset = arguments.get("offset").and_then(|v| v.as_u64());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());
    let single = arguments.get("path").and_then(|v| v.as_str());
    let paths = arguments.get("paths").filter(|v| !v.is_null());
    let ranges = arguments.get("ranges").filter(|v| !v.is_null());

    let mut reads = Vec::new();
    let batch = paths.is_some() || ranges.is_some();

    if let Some(paths) = paths {
        if single.is_some() {
            return Err(
                "read_file takes either `path` (one file) or `paths` (several files), not both"
                    .into(),
            );
        }
        if ranges.is_some() {
            return Err(
                "read_file takes either `paths` (several files) or `path` + `ranges` (several \
                 windows of one file), not both"
                    .into(),
            );
        }
        let Some(items) = paths.as_array() else {
            return Err("`paths` must be an array of path strings".into());
        };
        if items.is_empty() {
            return Err("`paths` is empty; name at least one file".into());
        }
        for (i, item) in items.iter().enumerate() {
            let label = format!("paths[{i}]");
            match item.as_str().filter(|p| !p.trim().is_empty()) {
                Some(path) => reads.push(PlannedRead::Read(FileRead {
                    path: path.to_string(),
                    offset,
                    limit,
                })),
                None => reads.push(PlannedRead::Bad {
                    label,
                    message: "this entry is not a non-empty path string".into(),
                }),
            }
        }
    } else if let Some(ranges) = ranges {
        let Some(path) = single else {
            return Err(
                "`ranges` applies to one file: give `path` as well, e.g. {\"path\":\"a.rs\",\
                 \"ranges\":[{\"offset\":1,\"limit\":40},{\"offset\":800,\"limit\":40}]}"
                    .into(),
            );
        };
        let Some(items) = ranges.as_array() else {
            return Err("`ranges` must be an array of {\"offset\", \"limit\"} objects".into());
        };
        if items.is_empty() {
            return Err("`ranges` is empty; give at least one {\"offset\", \"limit\"}".into());
        }
        for (i, item) in items.iter().enumerate() {
            let label = format!("{path} (range {i})");
            let Some(object) = item.as_object() else {
                reads.push(PlannedRead::Bad {
                    label,
                    message: "this range is not an object like {\"offset\": 1, \"limit\": 40}"
                        .into(),
                });
                continue;
            };
            if let Some(key) = object
                .keys()
                .find(|key| *key != "offset" && *key != "limit")
            {
                reads.push(PlannedRead::Bad {
                    label,
                    message: format!(
                        "this range has an unknown key `{key}`; a range is {{\"offset\", \"limit\"}}"
                    ),
                });
                continue;
            }
            reads.push(PlannedRead::Read(FileRead {
                path: path.to_string(),
                offset: object.get("offset").and_then(|v| v.as_u64()).or(offset),
                limit: object.get("limit").and_then(|v| v.as_u64()).or(limit),
            }));
        }
    } else if let Some(path) = single {
        reads.push(PlannedRead::Read(FileRead {
            path: path.to_string(),
            offset,
            limit,
        }));
    }

    if reads.is_empty() {
        return Err(
            "read_file needs `path` (one file), `paths` (several files) or `path` + `ranges` \
             (several windows of one file)"
                .into(),
        );
    }
    // A request wider than the cap is cut and says so, rather than refused.
    let dropped = reads.len().saturating_sub(MAX_MULTI_READS);
    reads.truncate(MAX_MULTI_READS);
    Ok(ReadPlan {
        reads,
        batch,
        dropped,
    })
}

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        spec(
            "read_file",
            "Read one file, several files, or several windows of one file, inside the workspace. \
             Text comes back as numbered lines with a `path — lines A-B of N (bytes)` header (use \
             offset/limit for large files: default 400 lines, max 2000). Any line of a large file \
             is reachable: `limits.max_file_read_bytes` bounds the bytes *returned* by one read, \
             not the offsets you can ask for. `paths` reads several files in one call and \
             `ranges` reads several windows of `path`; a bad path or range is reported inline and \
             the other reads still return. An image (PNG/JPEG/GIF/WebP) passed as `path` is \
             attached for a model that accepts image input; on a text-only model it is refused \
             with an explanation, as are binary files and image formats vision APIs do not \
             accept. Output is capped by the configured tool-output limit.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative path (one file)"},
                    "offset": {"type": "integer", "description": "1-based first line to return (default 1)"},
                    "limit": {"type": "integer", "description": "Max lines to return per read (default 400, max 2000)"},
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Several files in one call, each read with `offset`/`limit`"
                    },
                    "ranges": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "offset": {"type": "integer"},
                                "limit": {"type": "integer"}
                            }
                        },
                        "description": "Several windows of `path`: [{\"offset\": 1, \"limit\": 40}, …]"
                    }
                }
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let plan = match parse_reads(&request.arguments) {
            Ok(plan) => plan,
            Err(message) => return ToolOutcome::err(message),
        };
        let limits = ctx.limits().await;
        let (reads, batch, dropped) = (plan.reads, plan.batch, plan.dropped);

        // An image is not text. Classify the single-file form first: a vision model
        // gets the pixels, a text-only model gets a refusal it can pass on. A batch
        // is text-only - attaching several images would ignore the per-call cap.
        if !batch {
            let path = match reads.first() {
                Some(PlannedRead::Read(request)) => request.path.as_str(),
                // A malformed single entry is reported by the read loop below.
                Some(PlannedRead::Bad { .. }) | None => "",
            };
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
        }

        // Each file is read and (when the byte cap cut it) scanned to count lines:
        // blocking work that must not run on a runtime worker. One read failing
        // must not discard the ones that worked.
        let entries = match blocking("read_file", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let cancel = ctx.cancel.clone();
            let max_bytes = limits.max_file_read_bytes;
            move || {
                let mut out = Vec::with_capacity(reads.len());
                for planned in reads {
                    let request = match planned {
                        PlannedRead::Read(request) => request,
                        // A malformed entry keeps its place and is not attempted.
                        PlannedRead::Bad { label, message } => {
                            out.push(ReadEntry::Failed { label, message });
                            continue;
                        }
                    };
                    if cancel.is_cancelled() {
                        out.push(ReadEntry::Failed {
                            label: request.path,
                            message: "cancelled before this read started".into(),
                        });
                        continue;
                    }
                    match file_ops::read_file_cancellable(
                        &ws,
                        &request.path,
                        request.offset,
                        request.limit,
                        max_bytes,
                        &cancel,
                    ) {
                        Ok(rendered) => out.push(ReadEntry::Read {
                            path: request.path,
                            rendered,
                        }),
                        Err(kc_core::Error::Cancelled) => out.push(ReadEntry::Failed {
                            label: request.path,
                            message: "cancelled during this read".into(),
                        }),
                        Err(e) => out.push(ReadEntry::Failed {
                            label: request.path,
                            message: e.to_string(),
                        }),
                    }
                }
                Ok(out)
            }
        })
        .await
        {
            Err(refusal) => return refusal,
            Ok(Err(e)) => return ToolOutcome::err(e.to_string()),
            Ok(Ok(entries)) => entries,
        };

        if !batch {
            return match entries.into_iter().next() {
                Some(ReadEntry::Read { rendered, .. }) => {
                    let (text, cut) = truncate_output(&rendered.text, limits.max_tool_output_bytes);
                    ToolOutcome::ok(text)
                        .truncated(cut || rendered.truncated)
                        .with_meta(rendered.meta)
                }
                Some(ReadEntry::Failed { message, .. }) => ToolOutcome::err(message),
                None => ToolOutcome::err("read_file read nothing"),
            };
        }
        render_batch(entries, dropped, limits.max_tool_output_bytes)
    }
}

/// Join several reads into one result: a separator naming the path, the read's
/// own header and numbered lines, then the next. A failed entry keeps its place
/// with the reason, so the model can fix just that one.
///
/// The per-call byte cap is divided across the sections *before* anything is
/// rendered, because a first-come-first-served cap would return the first file
/// in full and drop the rest - the batch would then be no better than a single
/// read. The division is recomputed from what is left as each section is written,
/// so the finished text fits the cap without the final head+tail cut removing a
/// section in the middle.
fn render_batch(entries: Vec<ReadEntry>, dropped: usize, max_bytes: usize) -> ToolOutcome {
    let mut text = String::new();
    let mut reads = Vec::with_capacity(entries.len());
    let mut failures = 0usize;
    let mut any_truncated = false;
    let total = entries.len();
    // Reserve the truncation note up front: the last file must not push it out.
    let mut remaining = max_bytes.saturating_sub(if dropped > 0 { 160 } else { 0 });
    for (i, entry) in entries.iter().enumerate() {
        let newline = usize::from(i > 0);
        let before = text.len();
        let header = match entry {
            ReadEntry::Read { path, .. } => format!("--- {path} ---\n"),
            ReadEntry::Failed { label, .. } => format!("--- {label} ---\n"),
        };
        if newline == 1 {
            text.push('\n');
        }
        // The separator and the numbered header are part of the cost of this
        // section; they are subtracted before the budget is divided so four
        // sections plus four separators still fit the call's cap exactly.
        let overhead = newline + header.len();
        let share = remaining.saturating_sub(overhead) / (total - i).max(1);
        text.push_str(&header);
        match entry {
            ReadEntry::Read { rendered, .. } => {
                let (body, cut) = truncate_output(&rendered.text, share.max(1));
                any_truncated |= cut;
                text.push_str(&body);
                reads.push(rendered.meta.clone());
            }
            ReadEntry::Failed { label, message } => {
                failures += 1;
                text.push_str(&format!("[error: {message}]"));
                reads.push(json!({"path": label, "error": message}));
            }
        }
        remaining = remaining.saturating_sub(text.len() - before);
    }
    if failures == entries.len() {
        // Nothing worked: a failure, not a success with notes in it.
        return ToolOutcome::err(text);
    }
    if dropped > 0 {
        text.push_str(&format!(
            "\n\n[note: {dropped} more read(s) were not attempted; read_file takes at most \
             {MAX_MULTI_READS} per call]"
        ));
    }
    let (text, cut) = truncate_output(&text, max_bytes);
    ToolOutcome::ok(text)
        .truncated(cut || any_truncated)
        .with_meta(json!({
            "reads": reads,
            "read_count": total,
            "failed_count": failures,
            "dropped_count": dropped,
        }))
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
             walker (both honour .gitignore) and both return exactly the same shape. Returns \
             `path:line: text` matches; `context` adds the `N` lines either side of each match as \
             `path:line- text`, `files_only` returns just the distinct paths that matched, and \
             `max_matches_per_file` stops one noisy file from spending the whole result cap. \
             Results are bounded by the configured search limit.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regex (ripgrep syntax)"},
                    "path": {"type": "string", "description": "Workspace-relative file or directory (default '.')"},
                    "glob": {"type": "string", "description": "Optional glob filter, e.g. *.rs"},
                    "max_results": {"type": "integer", "description": "Cap on returned matches"},
                    "case_sensitive": {"type": "boolean", "description": "Default true"},
                    "context": {"type": "integer", "description": "Lines of context before and after each match, like grep -C (default 0, max 10)"},
                    "files_only": {"type": "boolean", "description": "Return distinct matching paths instead of lines, like grep -l"},
                    "max_matches_per_file": {"type": "integer", "description": "Cap on matches reported per file (default 0 = only the global cap)"}
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
        let context = request
            .opt_u64("context")
            .unwrap_or(0)
            .min(search::MAX_CONTEXT_LINES as u64) as usize;
        let files_only = request.opt_bool("files_only").unwrap_or(false);
        let per_file = request
            .opt_u64("max_matches_per_file")
            .unwrap_or(0)
            .min(max_results as u64) as usize;
        let query = search::SearchQuery {
            pattern: pattern.to_string(),
            path: request.opt_str("path").unwrap_or(".").to_string(),
            glob: request.opt_str("glob").map(|g| g.to_string()),
            case_insensitive: !request.opt_bool("case_sensitive").unwrap_or(true),
            max_results,
            context,
            files_only,
            max_matches_per_file: per_file,
        };

        match search::search(&ctx.workspace, &query, &ctx.cancel).await {
            Ok(result) => {
                let mut header = format!(
                    "{} match(es) for pattern {:?} in `{}` (engine: {}, cap {})",
                    result.matched,
                    query.pattern,
                    query.path,
                    result.engine.label(),
                    max_results
                );
                if files_only {
                    header = format!(
                        "{} file(s) matching pattern {:?} in `{}` (engine: {}, cap {})",
                        result.matched,
                        query.pattern,
                        query.path,
                        result.engine.label(),
                        max_results
                    );
                } else if context > 0 {
                    header.push_str(&format!(", {context} line(s) of context"));
                }
                if per_file > 0 {
                    header.push_str(&format!(", max {per_file} match(es) per file"));
                }
                let mut text = format!("{header}\n\n{}", result.body());
                if result.hit_limit {
                    let what = if files_only { "file" } else { "match" };
                    text.push_str(&format!(
                        "\n\n[hit the {max_results}-{what} cap; narrow the pattern, add a glob or \
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
                        "matches": result.matched,
                        "lines": result.matches.len(),
                        "files": result.files().len(),
                        "files_only": files_only,
                        "context": context,
                        "max_matches_per_file": per_file,
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

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn spec(&self) -> ToolSpec {
        spec(
            "glob",
            "Find files and directories by name with a glob. This is the name-based finder (not a \
             content search): it is `.gitignore`-aware and skips `.git`/`node_modules`. The pattern \
             is matched against each entry's file name, its path relative to `path` and its \
             workspace-relative path, so `*.rs`, `src/**/*.rs` and `crates/*/src/lib.rs` all work. \
             Directories are listed with a trailing `/`. Bounded at 200 paths, and a truncated \
             result says the walk stopped at the cap. Read-only, no approval.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob for a name or path, e.g. *.rs or src/**/*.rs"},
                    "path": {"type": "string", "description": "Workspace-relative directory to search under (default '.')"},
                    "max_results": {"type": "integer", "description": "Cap on returned paths (default 200, max 200)"}
                },
                "required": ["pattern"]
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let pattern = match request.arg_str("pattern") {
            Ok(pattern) => pattern.to_string(),
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let path = request.opt_str("path").unwrap_or(".").to_string();
        let max = request
            .opt_u64("max_results")
            .unwrap_or(file_ops::MAX_GLOB_ENTRIES as u64)
            .clamp(1, file_ops::MAX_GLOB_ENTRIES as u64) as usize;
        let limits = ctx.limits().await;
        let rendered = match blocking("glob", &ctx.cancel, {
            let ws = ctx.workspace.clone();
            let path = path.clone();
            let cancel = ctx.cancel.clone();
            move || file_ops::glob_cancellable(&ws, &pattern, &path, max, &cancel)
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

pub struct GitLog;

#[async_trait]
impl Tool for GitLog {
    fn spec(&self) -> ToolSpec {
        spec(
            "git_log",
            "Show recent commit history: one line per commit (`sha date subject`), read-only and \
             no approval. `max_count` defaults to 20 and is capped at 200; when the log continues \
             past it, the result says so. `path` narrows to commits that touch that workspace path. \
             Confined to the workspace: when the workspace is a subdirectory of a bigger repository, \
             commits that touch nothing inside it are not listed.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative file or directory (default '.')"},
                    "max_count": {"type": "integer", "description": "Commits to show (default 20, max 200)"}
                }
            }),
        )
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let path = request.opt_str("path");
        let max_count = request.opt_u64("max_count").map(|n| n as usize);
        let limits = ctx.limits().await;
        match git::log(&ctx.workspace, path, max_count).await {
            Ok(report) => {
                let (text, cut) = truncate_output(&report.text, limits.max_tool_output_bytes);
                ToolOutcome::ok(text)
                    .truncated(cut || report.capped)
                    .with_meta(json!({
                        "is_repo": report.is_repo,
                        "commits": report.commits.len(),
                        "capped": report.capped,
                        "confined": report.confined,
                        "path": path.unwrap_or("."),
                    }))
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
        Arc::new(Glob),
        Arc::new(WriteFile),
        Arc::new(EditFile),
        Arc::new(RunCommand),
        Arc::new(GitStatus),
        Arc::new(GitDiff),
        Arc::new(GitLog),
        Arc::new(docs_tool::KarucodeDocs),
    ]
}
