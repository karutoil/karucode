//! `report_karucode_issue`: turn "report this issue to karucode" into a GitHub issue.
//!
//! The feature is two halves, and the tool is only the second one:
//!
//! * **The trigger is the user's sentence.** The tool's own description and the
//!   paragraph `kc_core::prompt` splices in both say the agent may only reach for
//!   this when the user asked for the problem to be reported *to karucode* and
//!   said *what* the problem is. Code cannot read intent, so the hard rail here is
//!   the second half of that rule: a `summary` that does not say what went wrong
//!   (shorter than [`MIN_SUMMARY_CHARS`]) is refused with what to ask for.
//! * **The bundle is collected here.** Build, platform, approval policy, the
//!   effective model, a few limits, plugin/MCP status and this session's last run
//!   are gathered from the live server, then swept for configured secrets. No
//!   message body, project file, environment value, workspace path or hostname is
//!   ever part of it - a bug report is not a data export.
//!
//! Delivery uses the operator's own `gh` login, exactly like the source-control
//! manager: karucode stores no GitHub token for this. The report is written to
//! `<data_dir>/issue-reports/` **first**, so a failed network call never loses the
//! text the user just wrote; `gh` is only asked to file that saved copy. When `gh`
//! is missing or not authenticated the tool reports that it did not send, and
//! hands back the saved file plus the two ways to finish.
//!
//! Nothing leaves the machine before the approval gate shows the complete body.

// @kc-doc: issue-report
// @kc-title: Report an issue to karucode
// @kc-summary: A user asking to report a problem to karucode gets a redacted diagnostics bundle filed as a GitHub issue — with their words, no secrets, and an approval prompt showing the exact body.
// @kc-tags: issue, bug report, github, diagnostics, logs, privacy, support
// @kc-applies-to: tool:report_karucode_issue, cmd:/report, env:KARUCODE_ISSUE_REPO, env:KARUCODE_ISSUE_SEND, file:crates/kc-server/src/issue_report.rs, file:docs/internal/issue-report.md, dir:data_dir/issue-reports, config:approvals.mode
//
// # Saying it
//
// A user gets a report filed by saying so in a session - "report this issue to
// karucode", "file a bug with karucode about this", "send this to the karucode
// repo". The agent calls `report_karucode_issue` **only when both halves are
// present**: the request names karucode as the place to report to, and the user
// has said what the problem is. If either half is missing the agent asks for it;
// a bug in the user's own project is never filed upstream.
//
// When the *agent* is what is broken there is a second way in that needs no model
// turn at all: type the command in the message box.
//
// ```text
// /report the run stopped after a tool call and never finished
// ```
//
// `/report` builds the same bundle, saves it, and answers with the file path, a
// prefilled GitHub link and the `gh issue create` command - and stops there. A
// slash command has no approval prompt, so it never sends on its own; filing is a
// decision the person makes by clicking the link or running the command.
//
// # What gets filed
//
// The issue carries the user's report (`summary`, plus `steps` and `expected`
// when the conversation has them) above a diagnostics block collected from the
// running server:
//
// * the build (`karucode <build> (<commit>, <date>)`), channel, target and
//   whether the tree was dirty;
// * OS, architecture and kernel release;
// * the approval policy, server uptime, registered tool count, and whether web
//   sign-in is required;
// * the effective provider (implementation id, endpoint host, model, whether a
//   key is configured) and the context window;
// * the limits that shape behaviour (`max_tool_output_bytes`,
//   `max_command_output_bytes`, `max_file_read_bytes`, `context_max_chars`,
//   `max_iterations`, `max_run_secs`, `parallel_tool_calls`);
// * every plugin and MCP server with its status, tool count and error;
// * this session's last run: status, model, when it started, how long it took
//   and its error, if any.
//
// # What never gets filed
//
// The bundle contains no API key, bearer token or other secret - the values are
// never read, and the rendered title and body are swept against every secret the
// settings know about before they are written, so a key the user pasted into their
// own text is replaced with `***` too. It also contains no message text, no project
// file content, no environment variable value, no absolute workspace path and no
// hostname. `attach_diagnostics: false` files the user's words alone.
//
// # How it is sent
//
// 1. The full body is rendered and the approval policy is consulted as for any
//    other external tool: `read_only` refuses, `auto` proceeds, every other mode
//    asks. The approval prompt's detail **is the report**, so the operator reads
//    exactly what will leave before answering.
// 2. The body is written to `<data_dir>/issue-reports/<epoch>-<slug>.md`.
// 3. If `gh` is installed and authenticated for the server user, the saved copy
//    is filed with `gh issue create --repo <repo> --title <title> --body-file
//    <path>` and the issue URL is returned.
// 4. Otherwise nothing is sent. The tool returns `ok: false` (honestly: the
//    report was not filed) with the saved path, the `gh issue create` command
//    that finishes the job, and the prefilled
//    `https://github.com/<repo>/issues/new` link when the body fits a URL.
//
// # Which repository
//
// Reports go to `karutoil/karucode`. An operator running a fork sets
// `KARUCODE_ISSUE_REPO=owner/name` to send theirs elsewhere; a value that is not
// exactly `owner/name` is ignored and the upstream repository is used, because a
// typo must not file a report nowhere.
//
// # Turning sending off
//
// `KARUCODE_ISSUE_SEND=0` (also `false`, `no`, `off`) keeps the whole feature and
// removes only the network: every report is still written to
// `<data_dir>/issue-reports/`, the answer still carries the saved path and the
// `gh issue create` command, and nothing is filed. Use it on an install that must
// not talk to GitHub, or when every report should be read before it is public. An
// empty value counts as unset, so `KARUCODE_ISSUE_SEND=` does not silently disable
// filing.
//
// # Gotchas
//
// * `gh` runs as the server user. On a deployment where that user has never run
//   `gh auth login`, the tool saves the report and says it did not send; that is
//   the normal outcome on a fresh install, not a failure to fix.
// * The saved copy stays on disk under `<data_dir>/issue-reports/`, so the same
//   body can be re-filed after `gh auth login` without asking the user to repeat
//   themselves.
// * `auto` approval mode files the report without a prompt. The prompt rule in
//   the system prompt still applies - the agent may only call the tool when the
//   user asked for it - but nothing else stops it, which is what `auto` means.
// * The severity, labels and title of the issue are whatever the title says; no
//   label is applied, because an unknown label makes `gh issue create` fail.

use crate::state::AppState;
use crate::version;
use async_trait::async_trait;
use kc_core::approval::ApprovalKind;
use kc_core::config::Config;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec};
use kc_core::Settings;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The name the model calls. Kept here so a test can assert on the exact string.
pub const TOOL_NAME: &str = "report_karucode_issue";

/// Where a report is filed when nothing overrides it.
pub const UPSTREAM_REPO: &str = "karutoil/karucode";

/// Environment variable an operator of a fork sets to change [`UPSTREAM_REPO`].
pub const REPO_ENV: &str = "KARUCODE_ISSUE_REPO";

/// Environment variable that turns sending off: with `0`, `false`, `no` or `off`
/// every report is still written to `<data_dir>/issue-reports/`, and none is filed.
///
/// This is the switch for an install that must not talk to GitHub (an air-gapped
/// box, or an operator who wants every report read before it is public), and it is
/// what a test sets so a laptop with an authenticated `gh` cannot file an issue
/// while the suite runs.
pub const SEND_ENV: &str = "KARUCODE_ISSUE_SEND";

/// Shortest `summary` that counts as "what the issue is".
///
/// "it broke", "doesn't work" and "help" are under this; a sentence that says
/// what the user did and what happened is over it. The refusal names what to ask
/// for, so the model gets a next step instead of a wall.
const MIN_SUMMARY_CHARS: usize = 24;

/// Longest `summary` accepted, in characters. Past this the caller is pasting a
/// transcript, which is not what this field is for (and is message text).
const MAX_SUMMARY_CHARS: usize = 8_000;

/// Longest optional section (`steps`, `expected`). Over this the section is cut
/// and the body says so, the same contract the rest of the tool surface keeps.
const MAX_OPTION_CHARS: usize = 4_000;

/// Longest issue title, in characters, before the derived one is cut with `…`.
const MAX_TITLE_CHARS: usize = 120;

/// Longest error text quoted from a plugin, an MCP server or a run.
const MAX_ERROR_CHARS: usize = 1_200;

/// How much of the rendered report the approval prompt shows inline. The whole
/// body is also in the approval payload, and the file is already on disk.
const APPROVAL_DETAIL_CHARS: usize = 3_500;

/// Longest encoded body embedded in a `github.com/.../issues/new?body=` link.
///
/// GitHub's URL limit is around 8 KiB *after* percent-encoding, and markdown
/// encodes three bytes for every space and newline, so this is measured on the
/// encoded form rather than the character count. Past it the link carries the
/// title only and the body comes from the saved file.
const MAX_PREFILL_BODY_BYTES: usize = 6_000;

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

/// One parsed `report_karucode_issue` call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    /// The user's report in their own words. Required.
    summary: String,
    /// One-line issue title. Derived from `summary` when absent.
    title: String,
    /// Repro steps, when the conversation has them.
    steps: Option<String>,
    /// What the user expected instead.
    expected: Option<String>,
    /// Whether to attach the collected diagnostics block. Default true.
    diagnostics: bool,
}

impl Request {
    /// Parse and validate the call. `Err` is a ready-to-return refusal that says
    /// what to ask the user for.
    fn parse(call: &ToolCallRequest) -> Result<Self, String> {
        let summary = call.opt_str("summary").unwrap_or("").trim().to_string();
        let chars = summary.chars().count();
        if chars < MIN_SUMMARY_CHARS {
            return Err(format!(
                "`summary` must say what the issue is: what the user did, what they expected and \
                 what happened instead - at least {MIN_SUMMARY_CHARS} characters, got {chars}. \
                 Ask the user what the problem is, then call this again with their answer. Do not \
                 file a report you had to invent."
            ));
        }
        if chars > MAX_SUMMARY_CHARS {
            return Err(format!(
                "`summary` is {chars} characters; the cap is {MAX_SUMMARY_CHARS}. Keep the user's \
                 own description and drop any pasted transcript - the diagnostics block carries \
                 the machine state."
            ));
        }

        let title = match call
            .opt_str("title")
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(title) => clean_title(title),
            None => clean_title(&summary),
        };
        if title.is_empty() {
            return Err(
                "`title` resolved to nothing; give a one-line title or a `summary` with words in it."
                    .to_string(),
            );
        }

        let steps = option(call.opt_str("steps"));
        let expected = option(call.opt_str("expected"));

        Ok(Self {
            summary,
            title,
            steps,
            expected,
            diagnostics: call.opt_bool("attach_diagnostics").unwrap_or(true),
        })
    }
}

/// Trim an optional section, dropping one that is empty and cutting one that is
/// over [`MAX_OPTION_CHARS`] (the body says it was cut).
fn option(raw: Option<&str>) -> Option<String> {
    let text = raw.unwrap_or("").trim();
    if text.is_empty() {
        return None;
    }
    Some(cut_chars(text, MAX_OPTION_CHARS))
}

/// A one-line title: first non-empty line, whitespace collapsed, bounded.
fn clean_title(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = cut_chars(&collapsed, MAX_TITLE_CHARS);
    while title.ends_with('.') {
        title.pop();
        title = title.trim_end().to_string();
    }
    title
}

/// Cut `text` to at most `max` characters, marking the cut with `…`.
///
/// Character-counted, not byte-counted, so a title never splits a multi-byte
/// character in half.
fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max.saturating_sub(1)).collect::<String>();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// The facts
// ---------------------------------------------------------------------------

/// One row of the diagnostics block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    /// `karucode <build> (<commit>, <date>)` plus a `+dirty` marker.
    build: String,
    channel: String,
    target: String,
    platform: String,
    kernel: Option<String>,
    approval_mode: String,
    uptime_secs: u64,
    tools: usize,
    sign_in_required: bool,
    /// The effective provider implementation (`openai_compatible`, …).
    provider: String,
    /// The active profile id, when one is configured.
    profile: Option<String>,
    endpoint: String,
    model: String,
    key_present: bool,
    context_window: Option<u64>,
    /// `(key, value)` of the limits that shape behaviour.
    limits: Vec<(String, String)>,
    /// `(name, one-line description)` of every plugin the host knows.
    plugins: Vec<(String, String)>,
    /// `(name, one-line description)` of every configured MCP server.
    mcp: Vec<(String, String)>,
    /// This session's last run.
    last_run: Option<LastRun>,
}

/// The last run of the reporting session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LastRun {
    status: String,
    model: String,
    started_at: i64,
    finished_at: Option<i64>,
    error: Option<String>,
}

/// Gather the diagnostics from the live server.
async fn collect(state: &AppState, session_id: &str) -> Facts {
    let config = state.settings().snapshot();
    let plugins = {
        let host = state.plugins().lock().await;
        host.diagnostics()
            .into_iter()
            .map(|diagnostics| {
                let mut line = format!(
                    "{} ({})",
                    crate::state::plugin_status_str(&diagnostics.status),
                    diagnostics.tools.len()
                );
                if let Some(version) = diagnostics
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.version.trim())
                    .filter(|version| !version.is_empty())
                {
                    line.push_str(&format!(", v{version}"));
                }
                if let Some(error) = one_line(diagnostics.error.as_deref()) {
                    line.push_str(&format!(": {error}"));
                }
                (diagnostics.name, line)
            })
            .collect::<Vec<_>>()
    };
    let mcp = {
        let manager = state.mcp().lock().await;
        manager
            .statuses()
            .into_iter()
            .map(|status| {
                let mut line = format!("{}, {} tools", status.status, status.tools.len());
                if let Some(error) = one_line(status.error.as_deref()) {
                    line.push_str(&format!(": {error}"));
                }
                (format!("{} ({})", status.name, status.transport), line)
            })
            .collect::<Vec<_>>()
    };
    let last_run = match state.store().list_runs(session_id, 1).await {
        Ok(mut runs) => runs.pop().map(|run| LastRun {
            status: run.status,
            model: run.model,
            started_at: run.started_at,
            finished_at: run.finished_at,
            error: one_line_long(run.error.as_deref()),
        }),
        // A store that cannot answer is not a reason to refuse the report: the
        // body simply says the run history was unavailable.
        Err(_) => None,
    };

    Facts {
        build: format!(
            "{}{}",
            version::one_line(),
            if version::DIRTY { " +dirty" } else { "" }
        ),
        channel: version::CHANNEL.to_string(),
        target: version::TARGET.to_string(),
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        kernel: kernel_release(),
        approval_mode: config.approvals.mode.as_str().to_string(),
        uptime_secs: state.uptime_secs(),
        tools: state.tool_count(),
        sign_in_required: state.sign_in_required(),
        provider: config.model.provider.clone(),
        profile: config
            .active_provider
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        endpoint: public_url(&config.model.base_url),
        model: config.model.model.clone(),
        key_present: config
            .model
            .api_key
            .as_deref()
            .map(|key| !key.trim().is_empty())
            .unwrap_or(false),
        context_window: config.model.context_window,
        limits: limit_rows(&config),
        plugins,
        mcp,
        last_run,
    }
}

/// The kernel release on Linux, or `None` where there is no such file (macOS).
fn kernel_release() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The limits a bug report actually needs, as `(key, value)` rows.
fn limit_rows(config: &Config) -> Vec<(String, String)> {
    let limits = &config.limits;
    vec![
        (
            "limits.max_tool_output_bytes".into(),
            limits.max_tool_output_bytes.to_string(),
        ),
        (
            "limits.max_command_output_bytes".into(),
            limits.max_command_output_bytes.to_string(),
        ),
        (
            "limits.max_file_read_bytes".into(),
            limits.max_file_read_bytes.to_string(),
        ),
        (
            "limits.context_max_chars".into(),
            limits.context_max_chars.to_string(),
        ),
        (
            "limits.max_iterations".into(),
            if limits.max_iterations == 0 {
                "0 (no ceiling)".to_string()
            } else {
                limits.max_iterations.to_string()
            },
        ),
        (
            "limits.max_run_secs".into(),
            limits.max_run_secs.to_string(),
        ),
        (
            "limits.parallel_tool_calls".into(),
            limits.parallel_tool_calls.to_string(),
        ),
    ]
}

/// A URL with any credential and query string removed.
///
/// A base URL may carry `user:pass@` or a `?token=` the provider needs; neither
/// belongs in a public issue, and neither changes what a maintainer has to know,
/// which is *which endpoint* answered.
fn public_url(raw: &str) -> String {
    let without_query = raw.split(['?', '#']).next().unwrap_or("").trim();
    let without_user = match without_query.rfind('@') {
        Some(at) => match without_query[..at].find("//") {
            Some(scheme) => format!(
                "{}{}",
                &without_query[..scheme + 2],
                &without_query[at + 1..]
            ),
            None => without_query[at + 1..].to_string(),
        },
        None => without_query.to_string(),
    };
    without_user.trim_end_matches('/').to_string()
}

/// One line from an optional, possibly multi-line error, bounded.
fn one_line(raw: Option<&str>) -> Option<String> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    let first = text.lines().next().unwrap_or("").trim();
    Some(cut_chars(first, 200))
}

/// One line from an optional error, keeping more of it (a run error is the point
/// of the report, so it keeps [`MAX_ERROR_CHARS`] rather than a headline).
fn one_line_long(raw: Option<&str>) -> Option<String> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    Some(cut_chars(text, MAX_ERROR_CHARS))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the issue body: the user's report, then the diagnostics.
fn render(request: &Request, facts: &Facts) -> String {
    let mut out = String::with_capacity(4_096);
    section(&mut out, "What happened", &request.summary);
    if let Some(expected) = &request.expected {
        section(&mut out, "What you expected", expected);
    }
    if let Some(steps) = &request.steps {
        section(&mut out, "How to reproduce", steps);
    }
    if request.diagnostics {
        out.push_str(&diagnostics_block(facts));
    }
    out.push_str(
        "\n---\n\n_Reported from karucode with `report_karucode_issue`. Every configured secret \
         was replaced with `***` before this text was written; no message text, project \
         file or environment value is included._\n",
    );
    out
}

/// One `### heading` section, trimmed.
fn section(out: &mut String, heading: &str, body: &str) {
    out.push_str(&format!("### {heading}\n\n{}\n\n", body.trim()));
}

/// The collected diagnostics, as Markdown.
fn diagnostics_block(facts: &Facts) -> String {
    let mut out = String::with_capacity(2_048);
    out.push_str("### karucode diagnostics\n\n");

    let platform = match &facts.kernel {
        Some(kernel) => format!("{} (kernel {kernel})", facts.platform),
        None => facts.platform.clone(),
    };
    let provider = match &facts.profile {
        Some(id) => format!(
            "`{}` (profile `{id}`) · key {}",
            facts.provider,
            if facts.key_present {
                "configured"
            } else {
                "not configured"
            }
        ),
        None => format!(
            "`{}` · key {}",
            facts.provider,
            if facts.key_present {
                "configured"
            } else {
                "not configured"
            }
        ),
    };
    let model = match facts.context_window {
        Some(window) => format!("`{}` · context {window}", facts.model),
        None => format!("`{}`", facts.model),
    };
    let mut rows: Vec<(&str, String)> = vec![
        ("karucode", facts.build.clone()),
        ("channel", facts.channel.clone()),
        ("target", facts.target.clone()),
        ("platform", platform),
        ("approval mode", facts.approval_mode.clone()),
        ("uptime", human_duration(facts.uptime_secs)),
        ("tools registered", facts.tools.to_string()),
        (
            "web sign-in",
            if facts.sign_in_required {
                "required".to_string()
            } else {
                "not required".to_string()
            },
        ),
        ("provider", provider),
        ("endpoint", format!("`{}`", facts.endpoint)),
        ("model", model),
    ];

    out.push_str("| Field | Value |\n| --- | --- |\n");
    for (field, value) in rows.drain(..) {
        out.push_str(&format!("| {field} | {} |\n", cell(&value)));
    }

    out.push_str("\n#### Limits\n\n| Key | Value |\n| --- | --- |\n");
    for (key, value) in &facts.limits {
        out.push_str(&format!("| `{}` | {} |\n", cell(key), cell(value)));
    }

    out.push_str("\n#### Plugins\n\n");
    if facts.plugins.is_empty() {
        out.push_str("None configured.\n");
    } else {
        for (name, status) in &facts.plugins {
            out.push_str(&format!("- `{}` — {}\n", cell(name), cell(status)));
        }
    }

    out.push_str("\n#### MCP servers\n\n");
    if facts.mcp.is_empty() {
        out.push_str("None configured.\n");
    } else {
        for (name, status) in &facts.mcp {
            out.push_str(&format!("- {} — {}\n", cell(name), cell(status)));
        }
    }

    out.push_str("\n#### Last run in this session\n\n");
    match &facts.last_run {
        None => out.push_str("No run recorded for this session.\n"),
        Some(run) => {
            let mut line = format!("`{}`", run.status);
            if !run.model.trim().is_empty() {
                line.push_str(&format!(" on `{}`", run.model));
            }
            let started = ago(run.started_at);
            let took = run
                .finished_at
                .map(|finished| {
                    format!(
                        ", ran {}",
                        human_duration(finished.saturating_sub(run.started_at) as u64)
                    )
                })
                .unwrap_or_default();
            line.push_str(&format!(", started {started}{took}"));
            out.push_str(&line);
            out.push('\n');
            if let Some(error) = &run.error {
                out.push_str(&format!("\n```text\n{}\n```\n", error.trim()));
            }
        }
    }
    out
}

/// A Markdown table cell: no newline, no unescaped pipe.
fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// A duration a person reads: `42s`, `3m 7s`, `2h 4m`, `4d 2h`.
fn human_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{}m {}s", minutes, secs % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{}h {}m", hours, minutes % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

/// "12m ago" for an epoch-seconds timestamp, without pulling in a time crate.
fn ago(epoch_secs: i64) -> String {
    let now = now_secs() as i64;
    match now.saturating_sub(epoch_secs) {
        delta if delta < 5 => "just now".to_string(),
        delta => format!("{} ago", human_duration(delta as u64)),
    }
}

/// Seconds since the Unix epoch, or `0` on a clock before it.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// What happened to a rendered report.
enum Delivery {
    /// `gh` filed it; carries the issue URL.
    Sent { url: String, path: PathBuf },
    /// Nothing left the machine; carries why, so the answer can say so.
    Saved { path: PathBuf, reason: String },
    /// The report could not even be written down.
    Failed(String),
}

/// The repository reports go to: [`UPSTREAM_REPO`] unless a valid override is set.
pub fn repo() -> String {
    std::env::var(REPO_ENV)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| checked_repo(raw).is_some())
        .unwrap_or_else(|| UPSTREAM_REPO.to_string())
}

/// Accept exactly `owner/name` from the characters GitHub allows in each part.
fn checked_repo(raw: &str) -> Option<String> {
    let mut parts = raw.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    let ok = |part: &str| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            // A part that is only dots (`..`, `.`) is a path, not a repository.
            && part.chars().any(|c| c.is_ascii_alphanumeric())
    };
    if ok(owner) && ok(name) && !owner.starts_with('-') && !name.starts_with('-') {
        Some(format!("{owner}/{name}"))
    } else {
        None
    }
}

/// Shell-quote one argument with single quotes.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Where saved reports live: `<data_dir>/issue-reports/`.
fn reports_dir(state: &AppState) -> PathBuf {
    state
        .db_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join("issue-reports")
}

/// Whether this install may send a report to GitHub at all.
///
/// On unless [`SEND_ENV`] says otherwise; an empty value counts as unset, so an
/// empty variable never silently disables filing.
fn sending_enabled() -> bool {
    let Ok(raw) = std::env::var(SEND_ENV) else {
        return true;
    };
    !matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// Write the body beside the database and, when `send` is set, try to file the
/// saved copy with `gh`.
///
/// `send` is `false` for the `/report` slash command, which never sends on the
/// user's behalf: it hands back the saved file and the two ways to finish.
async fn deliver(state: &AppState, repo: &str, report: &Report, send: bool) -> Delivery {
    let dir = reports_dir(state);
    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        return Delivery::Failed(format!(
            "could not create {} for the report: {error}",
            dir.display()
        ));
    }
    let stamp = now_secs();
    let path = dir.join(format!("{stamp}-{}.md", file_slug(&report.title)));
    if let Err(error) = tokio::fs::write(&path, format!("{}\n", report.body.trim_end())).await {
        return Delivery::Failed(format!("could not write {}: {error}", path.display()));
    }

    if !send {
        return Delivery::Saved {
            path,
            reason: "`/report` saves the report and never sends it".to_string(),
        };
    }

    if !sending_enabled() {
        return Delivery::Saved {
            path,
            reason: format!("{SEND_ENV} turns sending off, so reports are saved and never filed"),
        };
    }

    // `gh` is asked about auth before it is asked to file: on a fresh install the
    // second call would only fail, and its stderr is much less useful than the
    // saved-file instructions the caller gets instead.
    let auth = match crate::routes::github::run_gh(&dir, &["auth", "status"]).await {
        Ok(auth) => auth,
        Err(error) => {
            return Delivery::Saved {
                path,
                reason: format!("could not run `gh`: {error:?}"),
            }
        }
    };
    if !auth.installed {
        return Delivery::Saved {
            path,
            reason: "`gh` (GitHub CLI) is not installed on this server".to_string(),
        };
    }
    if !auth.success {
        return Delivery::Saved {
            path,
            reason: format!(
                "`gh` is not authenticated for the server user ({})",
                state.redact(
                    &one_line(Some(&format!("{}{}", auth.stdout, auth.stderr))).unwrap_or_default()
                )
            ),
        };
    }

    let body_file = path.to_string_lossy().to_string();
    let out = crate::routes::github::run_gh(
        &dir,
        &[
            "issue",
            "create",
            "--repo",
            repo,
            "--title",
            &report.title,
            "--body-file",
            &body_file,
        ],
    )
    .await;
    match out {
        Ok(out) if out.success => {
            let url = out
                .stdout
                .lines()
                .map(str::trim)
                .find(|line| line.starts_with("http"))
                .map(str::to_string);
            match url {
                Some(url) => Delivery::Sent { url, path },
                // `gh` prints the URL on stdout; a version that does not is not a
                // failure to file, and the repo URL is still the useful answer.
                None => Delivery::Sent {
                    url: format!("https://github.com/{repo}/issues"),
                    path,
                },
            }
        }
        Ok(out) => Delivery::Saved {
            path,
            reason: format!(
                "`gh issue create` failed: {}",
                state.redact(
                    &one_line(Some(&out.stderr)).unwrap_or_else(|| "no output".to_string())
                )
            ),
        },
        Err(error) => Delivery::Saved {
            path,
            reason: format!("could not run `gh issue create`: {error:?}"),
        },
    }
}

/// A filesystem-safe slug from a title: lowercase ASCII words joined by `-`.
fn file_slug(title: &str) -> String {
    let mut slug = String::with_capacity(48);
    for word in title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(8)
    {
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word.to_ascii_lowercase());
    }
    let slug = cut_chars(&slug, 48);
    if slug.is_empty() {
        "report".to_string()
    } else {
        slug
    }
}

/// One rendered report: the issue title and the body about to be filed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    title: String,
    body: String,
}

/// Sweep every configured secret out of a report before it is written or shown.
///
/// The rendered body is built from data karucode chose, so nothing secret should
/// be in it - but the *user's own words* land in both the body and the derived
/// title, and people paste keys into bug reports. The title matters as much as the
/// body: it is the `--title` of the issue and the filename of the saved copy.
fn sweep(state: &AppState, report: &mut Report) {
    report.title = state.redact(&report.title);
    report.body = state.redact(&report.body);
}

/// A report saved for the `/report` slash command.
pub(crate) struct SavedReport {
    pub path: PathBuf,
    pub prefill: String,
    pub title: String,
    pub repo: String,
}

/// Save a report on behalf of the `/report` slash command.
///
/// The command exists for the case where the *agent* is what is broken, so it runs
/// the same validation and builds the same bundle, and stops before the network: it
/// writes the file and answers with the path, the prefilled link and the `gh`
/// command. Sending from here would be a network action nobody approved - the
/// command path has no approval prompt at all.
pub(crate) async fn save_from_command(
    state: &AppState,
    session_id: &str,
    text: &str,
) -> Result<SavedReport, String> {
    let call = ToolCallRequest {
        call_id: "slash-report".to_string(),
        name: TOOL_NAME.to_string(),
        arguments: json!({ "summary": text }),
    };
    let request = Request::parse(&call)?;
    let repo = repo();
    let mut report = Report {
        title: request.title.clone(),
        body: render(&request, &collect(state, session_id).await),
    };
    sweep(state, &mut report);
    match deliver(state, &repo, &report, false).await {
        Delivery::Saved { path, .. } => Ok(SavedReport {
            prefill: prefill_url(&repo, &report),
            path,
            title: report.title,
            repo,
        }),
        Delivery::Failed(error) => Err(error),
        // `send = false` cannot send; if that ever changes, say so rather than
        // claiming a URL that came out of nowhere.
        Delivery::Sent { url, .. } => Err(format!(
            "the report was filed at {url}, which `/report` must never do"
        )),
    }
}

// ---------------------------------------------------------------------------
// The tool
// ---------------------------------------------------------------------------

/// `report_karucode_issue`, registered by the server because the diagnostics live
/// there (`crates/kc-server/src/state.rs::reload_tools`).
pub struct ReportIssue(AppState);

/// Build the tool over the live server state.
pub fn tool(state: AppState) -> Arc<dyn Tool> {
    Arc::new(ReportIssue(state))
}

#[async_trait]
impl Tool for ReportIssue {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL_NAME.into(),
            description: "File a bug report on the karucode project at github.com/karutoil/karucode. \
Use it only when the user has asked for a problem to be reported to karucode AND has said what \
the problem is; if either half is missing, ask for it instead of filling it in yourself, and never \
file a bug in the user's own project upstream. `summary` is the user's report in their words: what \
they did, what they expected, what happened instead. The tool collects the build, platform, \
approval mode, provider, limits, plugin and MCP state and this session's last run and error, sweeps \
every configured secret out of the text, shows the exact body to the operator for approval, and \
files it with the server user's own `gh` login. If `gh` is missing or not authenticated nothing is \
sent: the report is saved under the data directory and the result says where, with the command \
that finishes the job. It never attaches message text, project files or environment values."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "What went wrong, in the user's words: what they did, what they expected, what happened. Required - a report that does not say what the issue is, or is shorter than 24 characters, is refused."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional one-line issue title (max 120 characters). Defaults to the first line of `summary`."
                    },
                    "steps": {
                        "type": "string",
                        "description": "Optional: the exact steps or commands that reproduce it, when the conversation has them."
                    },
                    "expected": {
                        "type": "string",
                        "description": "Optional: what the user expected to happen instead."
                    },
                    "attach_diagnostics": {
                        "type": "boolean",
                        "default": true,
                        "description": "Attach the automatically collected, redacted diagnostics (build, platform, provider, limits, plugins, MCP servers, last run). Set false only when the user asked for their own words alone."
                    }
                },
                "required": ["summary"],
                "additionalProperties": false
            }),
            origin: Default::default(),
        }
    }

    /// External, not read-only: the report leaves the machine. It asks for itself
    /// through the approval gate below, so `read_only` refuses and `auto` does not
    /// prompt - exactly like every other external tool.
    fn class(&self) -> ToolClass {
        ToolClass::External
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let request = match Request::parse(&request) {
            Ok(request) => request,
            Err(refusal) => return ToolOutcome::err(refusal),
        };
        let repo = repo();
        let mut report = Report {
            title: request.title.clone(),
            body: render(
                &request,
                &collect(&self.0, ctx.approvals.session_id.as_str()).await,
            ),
        };
        // The report is built from data this server chose; this sweep is what
        // catches a secret the *user* pasted into their own words, and any error
        // text that echoed a header.
        sweep(&self.0, &mut report);

        let policy = self.0.settings().approval_policy().await;
        match kc_tools::auto_decision(&policy, ToolClass::External, "") {
            kc_core::approval::AutoDecision::Allow => {}
            kc_core::approval::AutoDecision::Deny => {
                return ToolOutcome::err(
                    "filing a karucode issue is refused: the approval policy is `read_only`, so \
                     nothing may leave this machine. Switch approvals.mode to `ask`, `ask_commands` \
                     or `auto` to file it, or give the user the report text to file themselves.",
                )
            }
            kc_core::approval::AutoDecision::Ask => {
                let detail = format!(
                    "This files a public GitHub issue at github.com/{repo}. The body below is the \
                     whole report - read it before answering; nothing else is sent.\n\n{}",
                    cut_chars(&report.body, APPROVAL_DETAIL_CHARS)
                );
                let decision = ctx
                    .approvals
                    .ask(
                        ApprovalKind::NetworkTool,
                        format!("File this report on GitHub at {repo}?"),
                        detail,
                        json!({ "repo": repo, "title": report.title, "body": report.body }),
                    )
                    .await;
                if !decision.is_allowed() {
                    return ToolOutcome::err(format!(
                        "denied by user ({}) for filing a karucode issue; nothing was sent and no \
                         file was written",
                        decision.as_str()
                    ));
                }
            }
        }

        match deliver(&self.0, &repo, &report, true).await {
            Delivery::Sent { url, path } => ToolOutcome::ok(format!(
                "Filed as {url}\n\nThe same body is saved at {} for reference. Tell the user the \
                 issue URL.",
                path.display()
            ))
            .with_meta(json!({ "sent": true, "url": url, "path": path })),
            Delivery::Saved { path, reason } => {
                let reason = self.0.redact(&reason);
                let prefill = prefill_url(&repo, &report);
                ToolOutcome::err(format!(
                    "NOT SENT: {reason}.\n\nThe full report is saved at {} - hand that path to the \
                     user. Two ways to finish, neither of which this tool can do by itself:
- run `gh issue create --repo {repo} --title {} --body-file {}` (asks for approval like any \
other command), or
- open {prefill} and paste the body from that file.
Do not tell the user it was reported: it was written, not filed.",
                    path.display(),
                    shell_quote(&report.title),
                    shell_quote(&path.to_string_lossy()),
                ))
                .with_meta(
                    json!({ "sent": false, "reason": reason, "path": path, "prefill": prefill }),
                )
            }
            Delivery::Failed(reason) => ToolOutcome::err(format!(
                "could not save the report, so nothing was filed: {reason}"
            )),
        }
    }
}

/// The `issues/new` link, with the body only when it fits a URL.
fn prefill_url(repo: &str, report: &Report) -> String {
    let base = format!(
        "https://github.com/{repo}/issues/new?title={}",
        url_encode(&report.title)
    );
    let body = url_encode(&report.body);
    if body.len() > MAX_PREFILL_BODY_BYTES {
        return base;
    }
    format!("{base}&body={body}")
}

/// Percent-encode everything outside the unreserved set, so a title with `&` or a
/// body with `#` cannot break the query.
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: serde_json::Value) -> ToolCallRequest {
        ToolCallRequest {
            call_id: "c1".into(),
            name: TOOL_NAME.into(),
            arguments,
        }
    }

    fn facts() -> Facts {
        Facts {
            build: "karucode 123 (abc1234, 2026-01-01T00:00:00Z)".into(),
            channel: "stable".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            platform: "linux x86_64".into(),
            kernel: Some("6.8.0-45-generic".into()),
            approval_mode: "ask".into(),
            uptime_secs: 3_725,
            tools: 31,
            sign_in_required: true,
            provider: "openai_compatible".into(),
            profile: Some("omniroute".into()),
            endpoint: "https://gateway.example.com/v1".into(),
            model: "claude-sonnet".into(),
            key_present: true,
            context_window: Some(200_000),
            limits: vec![("limits.max_tool_output_bytes".into(), "24000".into())],
            plugins: vec![("session-status".into(), "ready (2, v1.0.0)".into())],
            mcp: vec![("catalyst (http)".into(), "ready, 180 tools".into())],
            last_run: Some(LastRun {
                status: "failed".into(),
                model: "claude-sonnet".into(),
                started_at: (now_secs() as i64) - 90,
                finished_at: Some((now_secs() as i64) - 60),
                error: Some("provider error: 500 upstream".into()),
            }),
        }
    }

    #[test]
    fn a_vague_summary_is_refused_with_what_to_ask_for() {
        let err = Request::parse(&call(json!({ "summary": "it broke" }))).unwrap_err();
        assert!(err.contains("what the issue is"), "{err}");
        assert!(err.contains("Ask the user"), "{err}");

        let missing = Request::parse(&call(json!({}))).unwrap_err();
        assert!(missing.contains("at least"), "{missing}");
    }

    #[test]
    fn a_real_summary_parses_and_derives_a_title() {
        let request = Request::parse(&call(json!({
            "summary": "The run stopped after the tool call and never finished.\nIt said limit reached.",
        })))
        .unwrap();
        assert_eq!(
            request.title,
            "The run stopped after the tool call and never finished"
        );
        assert!(request.diagnostics);
        assert_eq!(request.steps, None);
    }

    #[test]
    fn an_explicit_title_wins_and_is_bounded() {
        let long = "word ".repeat(60);
        let request = Request::parse(&call(json!({
            "summary": "Something real happened in the run and the model could not finish it.",
            "title": format!("\n  {long}  \n"),
        })))
        .unwrap();
        assert!(request.title.chars().count() <= MAX_TITLE_CHARS);
        assert!(request.title.ends_with('…'), "{}", request.title);
        assert!(!request.title.contains('\n'));
    }

    #[test]
    fn a_title_loses_its_trailing_full_stop() {
        assert_eq!(clean_title("Provider returns 500."), "Provider returns 500");
        assert_eq!(clean_title("a\n\nb"), "a");
        assert_eq!(clean_title("   "), "");
    }

    #[test]
    fn an_oversized_optional_section_is_cut_and_marked() {
        let request = Request::parse(&call(json!({
            "summary": "Something real happened in the run and the model could not finish it.",
            "steps": "x".repeat(MAX_OPTION_CHARS + 100),
        })))
        .unwrap();
        let steps = request.steps.unwrap();
        assert_eq!(steps.chars().count(), MAX_OPTION_CHARS);
        assert!(steps.ends_with('…'));
    }

    #[test]
    fn the_body_carries_the_report_then_the_diagnostics() {
        let request = Request::parse(&call(json!({
            "summary": "The run stopped after the tool call and never finished.",
            "expected": "The run finishes.",
            "steps": "1. ask for a file\n2. wait",
        })))
        .unwrap();
        let body = render(&request, &facts());

        assert!(body.starts_with("### What happened"), "{body}");
        assert!(body.contains("### What you expected"));
        assert!(body.contains("### How to reproduce"));
        assert!(body.contains("### karucode diagnostics"));
        assert!(body.contains("karucode 123 (abc1234"));
        assert!(body.contains("| platform | linux x86_64 (kernel 6.8.0-45-generic) |"));
        assert!(body.contains("| approval mode | ask |"));
        assert!(body.contains("| uptime | 1h 2m |"));
        assert!(body.contains("| web sign-in | required |"));
        assert!(body
            .contains("| provider | `openai_compatible` (profile `omniroute`) · key configured |"));
        assert!(body.contains("https://gateway.example.com/v1"));
        assert!(body.contains("`claude-sonnet` · context 200000"));
        assert!(body.contains("`limits.max_tool_output_bytes` | 24000 |"));
        assert!(body.contains("- `session-status` — ready (2, v1.0.0)"));
        assert!(body.contains("- catalyst (http) — ready, 180 tools"));
        assert!(body.contains("`failed` on `claude-sonnet`, started 1m 30s ago, ran 30s"));
        assert!(body.contains("provider error: 500 upstream"));
        assert!(body.contains("Every configured secret was replaced with `***`"));
    }

    #[test]
    fn empty_inventories_say_none_rather_than_nothing() {
        let request = Request::parse(&call(json!({
            "summary": "Something real happened in the run and the model could not finish it.",
        })))
        .unwrap();
        let mut empty = facts();
        empty.plugins.clear();
        empty.mcp.clear();
        empty.last_run = None;
        empty.kernel = None;
        let body = render(&request, &empty);
        assert!(body.contains("#### Plugins\n\nNone configured."), "{body}");
        assert!(
            body.contains("#### MCP servers\n\nNone configured."),
            "{body}"
        );
        assert!(body.contains("No run recorded for this session."), "{body}");
        assert!(body.contains("| platform | linux x86_64 |"), "{body}");
    }

    #[test]
    fn diagnostics_can_be_left_out() {
        let request = Request::parse(&call(json!({
            "summary": "Something real happened in the run and the model could not finish it.",
            "attach_diagnostics": false,
        })))
        .unwrap();
        let body = render(&request, &facts());
        assert!(!body.contains("### karucode diagnostics"), "{body}");
        assert!(!body.contains("karucode 123"), "{body}");
        assert!(body.contains("### What happened"));
    }

    #[test]
    fn a_pipe_or_newline_never_breaks_a_table_row() {
        let request = Request::parse(&call(json!({
            "summary": "Something real happened in the run and the model could not finish it.",
        })))
        .unwrap();
        let mut tricky = facts();
        tricky.plugins = vec![("a|b".into(), "ready\nline two".into())];
        let body = render(&request, &tricky);
        assert!(body.contains("- `a\\|b` — ready line two"), "{body}");
        // Every table row has exactly the three separators a two-column row needs.
        for row in body.lines().filter(|line| line.starts_with("| `")) {
            assert_eq!(row.matches('|').count(), 3, "row: {row}");
        }
    }

    #[test]
    fn an_endpoint_loses_its_userinfo_and_query() {
        assert_eq!(
            public_url("https://user:pass@gateway.example.com/v1?token=abc"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(
            public_url("https://gateway.example.com/v1/"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(
            public_url("http://127.0.0.1:8787/v1#frag"),
            "http://127.0.0.1:8787/v1"
        );
        assert_eq!(public_url("gateway.example.com"), "gateway.example.com");
    }

    #[test]
    fn only_an_owner_slash_name_is_a_repository() {
        assert_eq!(
            checked_repo("karutoil/karucode").as_deref(),
            Some("karutoil/karucode")
        );
        assert_eq!(checked_repo("a/b/c"), None);
        assert_eq!(checked_repo("../etc"), None);
        assert_eq!(checked_repo("karutoil/"), None);
        assert_eq!(checked_repo("/karucode"), None);
        assert_eq!(checked_repo("karutoil/karu code"), None);
        assert_eq!(checked_repo("-x/y"), None);
    }

    #[test]
    fn a_slug_is_safe_and_bounded() {
        assert_eq!(file_slug("Provider returns 500!"), "provider-returns-500");
        assert_eq!(file_slug("!!!"), "report");
        assert!(file_slug(&"a ".repeat(80)).chars().count() <= 48);
    }

    #[test]
    fn durations_read_like_a_person_wrote_them() {
        assert_eq!(human_duration(7), "7s");
        assert_eq!(human_duration(67), "1m 7s");
        assert_eq!(human_duration(3_725), "1h 2m");
        assert_eq!(human_duration(90_000), "1d 1h");
    }

    #[test]
    fn shell_quoting_survives_a_quote_in_a_title() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn a_body_too_long_for_a_url_is_left_out_of_it() {
        let short = Report {
            title: "Provider returns 500 & the run stops".into(),
            body: "Short body with a # and &".into(),
        };
        let url = prefill_url("karutoil/karucode", &short);
        assert!(
            url.starts_with("https://github.com/karutoil/karucode/issues/new?title="),
            "{url}"
        );
        assert!(url.contains("%26"), "the & is encoded: {url}");
        assert!(url.contains("&body=Short%20body%20with%20a%20%23%20and%20%26"));
        assert!(url.contains("title=Provider%20returns%20500%20%26%20the%20run%20stops"));

        // Encoding is what is bounded, not the character count: 2,100 spaces are
        // 2,100 characters and 6,300 bytes of `%20`, over the ceiling.
        let long = Report {
            title: "t".into(),
            body: " ".repeat((MAX_PREFILL_BODY_BYTES / 3) + 100),
        };
        assert!(
            long.body.chars().count() < MAX_PREFILL_BODY_BYTES,
            "the body is short in characters and long once encoded"
        );
        assert!(!prefill_url("karutoil/karucode", &long).contains("&body="));
    }
}
