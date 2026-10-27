//! Content search: ripgrep when available, a pure-Rust walker otherwise.
//!
//! Both engines honour `.gitignore`, produce the same `path:line: text` shape
//! and stop at the same result budget, so the caller (and the model) cannot
//! tell them apart except for the engine label in the header.

use crate::file_ops::BINARY_SNIFF_BYTES;
use globset::{Glob, GlobSet, GlobSetBuilder};
use kc_core::paths::Workspace;
use kc_core::{Error, Result};
use regex::RegexBuilder;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// Lines longer than this are cut so a minified bundle cannot flood the prompt.
pub const MAX_MATCH_COLUMNS: usize = 300;
const MAX_FALLBACK_FILE_BYTES: u64 = 4 * 1024 * 1024;
const RG_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngine {
    Ripgrep,
    Builtin,
}

impl SearchEngine {
    pub fn label(&self) -> &'static str {
        match self {
            SearchEngine::Ripgrep => "ripgrep",
            SearchEngine::Builtin => "builtin-walker",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub pattern: String,
    /// Workspace-relative path to search in (file or directory).
    pub path: String,
    pub glob: Option<String>,
    pub case_insensitive: bool,
    pub max_results: usize,
}

#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// Workspace-relative path.
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub engine: SearchEngine,
    pub matches: Vec<SearchMatch>,
    pub hit_limit: bool,
    /// Non-fatal problem worth surfacing (partial read errors, rg warnings).
    pub warning: Option<String>,
}

impl SearchResult {
    /// `path:line: text` lines, or a short "no matches" body.
    pub fn body(&self) -> String {
        if self.matches.is_empty() {
            return "no matches".to_string();
        }
        let mut out = String::new();
        for (i, m) in self.matches.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("{}:{}: {}", m.path, m.line, m.text));
        }
        out
    }
}

/// Search the workspace, preferring ripgrep and falling back to the built-in
/// walker when `rg` is not installed.
pub async fn search(
    ws: &Workspace,
    query: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<SearchResult> {
    search_with_program("rg", ws, query, cancel).await
}

/// Engine selection with a parameterised binary name, so tests can prove the
/// fallback path without touching the process environment.
pub(crate) async fn search_with_program(
    program: &str,
    ws: &Workspace,
    query: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<SearchResult> {
    match search_with_rg(program, ws, query, cancel).await? {
        Some(result) => Ok(result),
        None => {
            tracing::debug!("`{program}` not found on PATH, using the built-in search walker");
            search_fallback(ws, query).await
        }
    }
}

/// Run ripgrep. `Ok(None)` means the binary is unavailable (caller falls back).
pub async fn search_with_rg(
    program: &str,
    ws: &Workspace,
    query: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<Option<SearchResult>> {
    let base = ws.resolve_existing(&query.path)?;
    let max_results = query.max_results.max(1);

    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("--line-number")
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        .arg("--max-columns")
        .arg(MAX_MATCH_COLUMNS.to_string())
        .arg("--max-count")
        .arg(max_results.to_string())
        // Honor .gitignore even outside a git repository, matching the fallback
        // walker (ripgrep requires a repo by default).
        .arg("--no-require-git");
    if query.case_insensitive {
        cmd.arg("-i");
    }
    if let Some(glob) = &query.glob {
        cmd.arg("-g").arg(glob);
    }
    cmd.arg("--")
        .arg(&query.pattern)
        .arg(&base)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::Tool(format!(
                "failed to spawn ripgrep for pattern {:?}: {e}",
                query.pattern
            )))
        }
    };

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(Error::Cancelled);
        }
        res = async {
            let stdout = read_pipe(&mut stdout_pipe).await;
            let stderr = read_pipe(&mut stderr_pipe).await;
            let status = child.wait().await;
            (stdout, stderr, status)
        } => res,
        _ = tokio::time::sleep(RG_TIMEOUT) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(Error::Timeout(format!(
                "ripgrep did not finish within {}s searching {:?}; narrow the path or pattern",
                RG_TIMEOUT.as_secs(),
                base
            )));
        }
    };

    let (stdout, stderr, status) = outcome;
    let status = status.map_err(|e| Error::Tool(format!("failed to wait for ripgrep: {e}")))?;
    let code = status.code();
    let stderr_text = String::from_utf8_lossy(&stderr).trim().to_string();
    let stdout_text = String::from_utf8_lossy(&stdout).into_owned();

    let mut matches: Vec<SearchMatch> = Vec::new();
    for line in stdout_text.lines() {
        if let Some(m) = parse_rg_line(ws, &base, line) {
            matches.push(m);
            if matches.len() >= max_results {
                break;
            }
        }
    }
    let hit_limit = matches.len() >= max_results;

    let mut warning = None;
    match code {
        Some(0) | Some(1) => {}
        Some(other) => {
            if matches.is_empty() {
                // Very old ripgrep builds lack --no-require-git: fall back.
                if stderr_text.contains("unrecognized flag") || stderr_text.contains("unknown flag")
                {
                    tracing::debug!(
                        "installed ripgrep rejects our flags, using the built-in walker: {stderr_text}"
                    );
                    return Ok(None);
                }
                let detail = if stderr_text.is_empty() {
                    format!("ripgrep exited with status {other}")
                } else {
                    stderr_text
                };
                return Err(Error::Tool(format!(
                    "ripgrep failed for pattern {:?}: {detail}",
                    query.pattern
                )));
            }
            warning = Some(format!("ripgrep exited with status {other}: {stderr_text}"));
        }
        None => {
            warning = Some("ripgrep was terminated by a signal".to_string());
        }
    }
    if warning.is_none() && !stderr_text.is_empty() {
        warning = Some(stderr_text);
    }

    Ok(Some(SearchResult {
        engine: SearchEngine::Ripgrep,
        matches,
        hit_limit,
        warning,
    }))
}

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>) -> Vec<u8> {
    let Some(pipe) = pipe.as_mut() else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf).await;
    buf
}

fn parse_rg_line(ws: &Workspace, base: &Path, line: &str) -> Option<SearchMatch> {
    let mut parts = line.splitn(3, ':');
    let raw_path = parts.next()?;
    let raw_line: u64 = parts.next()?.parse().ok()?;
    let text = parts.next().unwrap_or("");
    let path = Path::new(raw_path);
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    Some(SearchMatch {
        path: ws.relative(&abs),
        line: raw_line,
        text: text.to_string(),
    })
}

/// Built-in search: used when `rg` is unavailable, and by tests directly.
pub async fn search_fallback(ws: &Workspace, query: &SearchQuery) -> Result<SearchResult> {
    let ws = ws.clone();
    let query = query.clone();
    tokio::task::spawn_blocking(move || search_fallback_sync(&ws, &query))
        .await
        .map_err(|e| Error::Internal(format!("search task panicked: {e}")))?
}

/// Synchronous implementation of the fallback walker.
pub fn search_fallback_sync(ws: &Workspace, query: &SearchQuery) -> Result<SearchResult> {
    let base = ws.resolve_existing(&query.path)?;
    let max_results = query.max_results.max(1);
    let regex = RegexBuilder::new(&query.pattern)
        .case_insensitive(query.case_insensitive)
        .build()
        .map_err(|e| {
            Error::BadRequest(format!(
                "invalid regex {:?}: {e} (the built-in engine uses the Rust regex syntax)",
                query.pattern
            ))
        })?;
    let globset = match &query.glob {
        Some(pattern) => Some(build_globset(pattern)?),
        None => None,
    };

    let mut files: Vec<PathBuf> = Vec::new();
    let mut walk_errors = 0usize;
    if base.is_file() {
        files.push(base.clone());
    } else {
        let walker = ignore::WalkBuilder::new(&base)
            .require_git(false)
            .git_global(false)
            .follow_links(false)
            .filter_entry(|entry| {
                let name = entry.file_name();
                name != ".git" && name != "node_modules"
            })
            .build();
        for entry in walker {
            match entry {
                Ok(e) if e.file_type().map(|t| t.is_file()).unwrap_or(false) => {
                    files.push(e.into_path());
                }
                Ok(_) => {}
                Err(_) => walk_errors += 1,
            }
        }
    }

    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut hit_limit = false;
    'files: for file in files {
        if let Some(set) = &globset {
            let rel = ws.relative(&file);
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !set.is_match(&rel) && !set.is_match(&name) {
                continue;
            }
        }
        let Ok(meta) = std::fs::metadata(&file) else {
            walk_errors += 1;
            continue;
        };
        if meta.len() > MAX_FALLBACK_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&file) else {
            walk_errors += 1;
            continue;
        };
        if bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let rel = ws.relative(&file);
        for (idx, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            matches.push(SearchMatch {
                path: rel.clone(),
                line: idx as u64 + 1,
                text: truncate_columns(line, MAX_MATCH_COLUMNS),
            });
            if matches.len() >= max_results {
                hit_limit = true;
                break 'files;
            }
        }
    }

    let warning = if walk_errors > 0 {
        Some(format!(
            "{walk_errors} paths could not be read and were skipped"
        ))
    } else {
        None
    };

    Ok(SearchResult {
        engine: SearchEngine::Builtin,
        matches,
        hit_limit,
        warning,
    })
}

fn build_globset(pattern: &str) -> Result<GlobSet> {
    let glob = Glob::new(pattern)
        .map_err(|e| Error::BadRequest(format!("invalid glob {pattern:?}: {e}")))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder
        .build()
        .map_err(|e| Error::BadRequest(format!("invalid glob {pattern:?}: {e}")))
}

fn truncate_columns(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max).collect();
    out.push('…');
    out
}
