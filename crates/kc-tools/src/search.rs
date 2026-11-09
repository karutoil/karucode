//! Content search: ripgrep when available, a pure-Rust walker otherwise.
//!
//! Both engines honour `.gitignore`, produce the same `path:line: text` shape,
//! keep the same first [`MAX_MATCH_COLUMNS`] **characters** of a long matching
//! line and stop at the same result budget, so the caller (and the model) cannot
//! tell them apart except for the engine label in the header. ripgrep is asked for
//! `--null` records (`path\0line:text`) so a filename containing `:` is parsed
//! unambiguously, and its byte-based `--max-columns` is set wide enough that the
//! character cap is what actually cuts the text.

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
/// Largest `context` the search will honour, in lines before and after a match.
///
/// `-C 5000` would turn one match into ten thousand lines of prompt, and the
/// runtime cap would then chop the result arbitrarily instead of the search
/// returning something shaped.
pub const MAX_CONTEXT_LINES: usize = 10;
const MAX_FALLBACK_FILE_BYTES: u64 = 4 * 1024 * 1024;
const RG_TIMEOUT: Duration = Duration::from_secs(120);
/// Safety valve for the streaming reader: a single record longer than this is
/// discarded instead of growing the buffer without bound. `--max-columns`
/// keeps real records far below it.
const MAX_RG_RECORD_BYTES: usize = 1024 * 1024;

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
    /// Lines of context before *and* after each match (`grep -C N`). `0` = off.
    pub context: usize,
    /// Return the distinct paths that matched instead of the matching lines.
    pub files_only: bool,
    /// Cap on reported matches *per file*, so one noisy file cannot spend the
    /// global result cap on its own. `0` = no per-file cap.
    pub max_matches_per_file: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Workspace-relative path.
    pub path: String,
    /// 1-based line number; `0` in `files_only` mode, where an entry is a path.
    pub line: u64,
    pub text: String,
    /// A context line (`-C N`) rather than the matching line itself.
    pub is_context: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub engine: SearchEngine,
    /// Reported entries in file/line order: matching lines and their context
    /// lines, or one entry per path in `files_only` mode.
    pub matches: Vec<SearchMatch>,
    /// How many entries in `matches` are matches (or paths), not context lines.
    pub matched: usize,
    /// `matches` holds paths, not lines.
    pub files_only: bool,
    pub hit_limit: bool,
    /// Non-fatal problem worth surfacing (partial read errors, rg warnings).
    pub warning: Option<String>,
}

impl SearchResult {
    /// Distinct matching paths, in the order they first matched.
    pub fn files(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for m in &self.matches {
            if !seen.contains(&m.path.as_str()) {
                seen.push(m.path.as_str());
            }
        }
        seen
    }

    /// `path:line: text` for a match and `path:line- text` for a context line -
    /// the same shape grep uses, and the same shape from both engines. In
    /// `files_only` mode it is one path per line.
    pub fn body(&self) -> String {
        if self.matches.is_empty() {
            return "no matches".to_string();
        }
        if self.files_only {
            return self.files().join("\n");
        }
        let mut out = String::new();
        for (i, m) in self.matches.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!(
                "{}:{}{} {}",
                m.path,
                m.line,
                if m.is_context { '-' } else { ':' },
                m.text
            ));
        }
        out
    }

    /// [`SearchResult::body`] rendered within `max_bytes`, plus how many entries
    /// (matches, context lines or paths) it really contains.
    ///
    /// A search whose result list does not fit the call's output budget used to be
    /// head+tail spliced: the model saw the first lines, the last lines, and no
    /// statement of how many matching lines were in between. Bounding the render
    /// here lets the caller say "showing the first K of M" and name the narrowing
    /// that finds the rest.
    pub fn body_bounded(&self, max_bytes: usize) -> (String, usize) {
        if self.matches.is_empty() {
            return ("no matches".to_string(), 0);
        }
        let rows: Vec<String> = if self.files_only {
            self.files().into_iter().map(str::to_string).collect()
        } else {
            self.matches
                .iter()
                .map(|m| {
                    format!(
                        "{}:{}{} {}",
                        m.path,
                        m.line,
                        if m.is_context { '-' } else { ':' },
                        m.text
                    )
                })
                .collect()
        };
        let mut out = String::new();
        let mut shown = 0usize;
        for row in &rows {
            let cost = row.len() + usize::from(shown > 0);
            if shown > 0 && out.len() + cost > max_bytes {
                break;
            }
            if shown > 0 {
                out.push('\n');
            }
            out.push_str(row);
            shown += 1;
        }
        (out, shown)
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
            search_fallback_with_cancel(ws, query, cancel).await
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
    // `--max-count` is ripgrep's *per-file* cap and ours was the global one, so
    // the per-file cap is what it is now asked for; the global cap stays in the
    // streaming reader below. Without a per-file cap, `max_results` remains the
    // safety valve it always was.
    let per_file = if query.max_matches_per_file > 0 {
        query.max_matches_per_file.min(max_results)
    } else {
        max_results
    };

    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("--line-number")
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        // `-H` forces the `path:` prefix even when ripgrep is given a single
        // file argument. Without it rg emits bare `line:text`, which the parser
        // cannot attribute to a file, so every match was silently dropped.
        .arg("--with-filename")
        // Ask for `path\0line:text` instead of `path:line:text`. A filename may
        // contain `:` - with the colon-separated form a match in `a:b.txt` was
        // parsed as path `a`, line `b.txt` and dropped, while the built-in walker
        // found it, so the two engines disagreed.
        .arg("--null")
        .arg("--max-columns")
        // Bytes, because that is the unit `--max-columns` counts. It is set wide
        // enough (4 bytes per character) that the *character* cap in
        // `normalise_match_text` decides where the text ends, so both engines cut
        // at the same character and the documented limit is in characters.
        // `--max-columns` still bounds the bytes ripgrep writes for one line.
        .arg((MAX_MATCH_COLUMNS * 4).to_string())
        // Without this, `--max-columns` replaces the whole line with the literal
        // text `[Omitted long matching line]`, so a match on a minified bundle
        // arrives with no content at all.
        .arg("--max-columns-preview")
        .arg("--max-count")
        .arg(per_file.to_string())
        // Honor .gitignore even outside a git repository, matching the fallback
        // walker (ripgrep requires a repo by default).
        .arg("--no-require-git");
    if query.case_insensitive {
        cmd.arg("-i");
    }
    if let Some(glob) = &query.glob {
        cmd.arg("-g").arg(glob);
    }
    if query.context > 0 && !query.files_only {
        // Both sides, the way `grep -C` reads. Group separators (`--`) are
        // dropped by the parser: the fallback emits none, and the line numbers
        // already say where a group resumes.
        cmd.arg("--before-context")
            .arg(query.context.min(MAX_CONTEXT_LINES).to_string())
            .arg("--after-context")
            .arg(query.context.min(MAX_CONTEXT_LINES).to_string());
    }
    if query.files_only {
        // `-l`: one path per file, and no line records at all.
        cmd.arg("--files-with-matches");
    }
    cmd.arg("--")
        .arg(&query.pattern)
        .arg(&base)
        .env("LC_ALL", "C")
        // A configured `--pre` command would turn this read-only tool into an
        // arbitrary command runner without going through the approval policy.
        // Search never needs a user-level ripgrep config: every option it uses is
        // explicit above.
        .env_remove("RIPGREP_CONFIG_PATH")
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
            // Stream stdout and stop at the global cap instead of buffering the
            // whole result set: a broad pattern on a large repo can emit
            // hundreds of MB, of which all but `max_results` are discarded.
            // `read_matches` takes ownership of the pipe so it is closed the
            // moment the cap is reached; otherwise ripgrep would block writing
            // to a full pipe while we wait for it to exit.
            let (found, stderr) = tokio::join!(
                async {
                    if query.files_only {
                        read_match_paths(stdout_pipe.take(), ws, &base, max_results).await
                    } else {
                        read_matches(
                            stdout_pipe.take(),
                            ws,
                            &base,
                            max_results,
                            query.context.min(MAX_CONTEXT_LINES),
                        )
                        .await
                    }
                },
                read_pipe(&mut stderr_pipe)
            );
            let status = child.wait().await;
            (found, stderr, status)
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

    let (found, stderr, status) = outcome;
    let (matches, matched, hit_limit) = found;
    let status = status.map_err(|e| Error::Tool(format!("failed to wait for ripgrep: {e}")))?;
    let code = status.code();
    let stderr_text = String::from_utf8_lossy(&stderr).trim().to_string();

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
            // Expected when the reader stopped at the cap and killed the child.
            if !hit_limit {
                warning = Some("ripgrep was terminated by a signal".to_string());
            }
        }
    }
    if warning.is_none() && !stderr_text.is_empty() {
        warning = Some(stderr_text);
    }

    Ok(Some(SearchResult {
        engine: SearchEngine::Ripgrep,
        matches,
        matched,
        files_only: query.files_only,
        hit_limit,
        warning,
    }))
}

/// Read ripgrep's stdout record by record, stopping at `max_results` real matches
/// but retaining the requested after-context for the last accepted match.
///
/// With `--null`, a path is terminated by NUL and the record by the first newline
/// *after* that NUL. Newlines before it are legal filename bytes and must not be
/// treated as record boundaries. The pending buffer is bounded by
/// [`MAX_RG_RECORD_BYTES`], and taking ownership of the pipe closes it as soon as
/// enough output has been consumed so ripgrep cannot block writing the rest.
///
/// Returns `(entries, matched, hit the cap)`. Group separators are omitted, just
/// as they are by the built-in walker.
async fn read_matches<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    ws: &Workspace,
    base: &Path,
    max_results: usize,
    context: usize,
) -> (Vec<SearchMatch>, usize, bool) {
    let mut out = RgMatches {
        matches: Vec::new(),
        matched: 0,
        max_results: max_results.max(1),
        trailing_context: None,
    };
    let Some(pipe) = pipe else {
        return (out.matches, out.matched, false);
    };
    let mut reader = tokio::io::BufReader::new(pipe);
    let mut record: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        record.extend_from_slice(&chunk[..n]);
        while let Some(end) = ordinary_record_end(&record) {
            let complete = record[..end].to_vec();
            record.drain(..=end);
            if complete == b"--" {
                // At the cap a separator proves the accepted match's context is
                // complete. Before it, separators carry no result information.
                if out.trailing_context.is_some() {
                    return (out.matches, out.matched, true);
                }
                continue;
            }
            if let Some(found) = parse_rg_record(ws, base, &complete) {
                if out.accept(found, context) {
                    return (out.matches, out.matched, true);
                }
            }
        }
        if record.len() > MAX_RG_RECORD_BYTES {
            // A single absurdly long record: drop it rather than grow forever.
            record.clear();
        }
    }
    if !record.is_empty() && record != b"--" {
        if let Some(found) = parse_rg_record(ws, base, &record) {
            out.accept(found, context);
        }
    }
    let hit_limit = out.matched >= out.max_results;
    (out.matches, out.matched, hit_limit)
}

/// Return the newline delimiter for one ordinary ripgrep record. A bare `--\n`
/// group separator has no NUL; all real records do, because `--with-filename` and
/// `--null` are always passed together.
fn ordinary_record_end(record: &[u8]) -> Option<usize> {
    // Check a leading separator before scanning ahead for a later record's NUL.
    if record.starts_with(b"--\n") {
        return Some(2);
    }
    let nul = record.iter().position(|b| *b == 0)?;
    record[nul + 1..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|nl| nul + 1 + nl)
}

struct RgMatches {
    matches: Vec<SearchMatch>,
    matched: usize,
    max_results: usize,
    trailing_context: Option<usize>,
}

impl RgMatches {
    /// Add one record. `true` means no more output may be accepted. In particular,
    /// once the cap is hit a later real match is neither counted nor relabelled as
    /// context; only the accepted hit's own bounded after-context may follow.
    fn accept(&mut self, found: SearchMatch, context: usize) -> bool {
        if let Some(remaining) = self.trailing_context {
            if !found.is_context || remaining == 0 {
                return true;
            }
            self.matches.push(found);
            let remaining = remaining - 1;
            self.trailing_context = Some(remaining);
            return remaining == 0;
        }

        self.matched += usize::from(!found.is_context);
        let capped = !found.is_context && self.matched >= self.max_results;
        self.matches.push(found);
        if capped {
            self.trailing_context = Some(context);
            return context == 0;
        }
        false
    }
}

/// Read `--files-with-matches --null` output: a NUL-terminated path per file.
///
/// Paths are NUL-separated, not newline-separated, so this cannot reuse
/// [`read_matches`]. Stops at the cap and drops the pipe, which kills ripgrep.
async fn read_match_paths<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    ws: &Workspace,
    base: &Path,
    max_results: usize,
) -> (Vec<SearchMatch>, usize, bool) {
    let mut matches: Vec<SearchMatch> = Vec::new();
    let Some(pipe) = pipe else {
        return (matches, 0, false);
    };
    let mut reader = tokio::io::BufReader::new(pipe);
    let mut record: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut rest = &chunk[..n];
        while let Some(nul) = rest.iter().position(|b| *b == 0) {
            record.extend_from_slice(&rest[..nul]);
            if let Some(found) = path_entry(ws, base, &record) {
                matches.push(found);
                if matches.len() >= max_results {
                    let matched = matches.len();
                    return (matches, matched, true);
                }
            }
            record.clear();
            rest = &rest[nul + 1..];
        }
        record.extend_from_slice(rest);
        if record.len() > MAX_RG_RECORD_BYTES {
            record.clear();
        }
    }
    if !record.is_empty() {
        if let Some(found) = path_entry(ws, base, &record) {
            matches.push(found);
        }
    }
    let matched = matches.len();
    (matches, matched, matched >= max_results)
}

/// One `files_only` entry: an absolute or base-relative path from ripgrep.
fn path_entry(ws: &Workspace, base: &Path, raw: &[u8]) -> Option<SearchMatch> {
    if raw.is_empty() {
        return None;
    }
    let raw = String::from_utf8_lossy(raw).into_owned();
    let path = Path::new(&raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    Some(SearchMatch {
        path: ws.relative(&abs),
        line: 0,
        text: String::new(),
        is_context: false,
    })
}

/// Parse one `--null` record: `path\0line:text` for a match and `path\0line-text`
/// for a context line (`-C N`).
///
/// The filename is delimited by a **NUL**, not by `:` - that is what `--null`
/// buys. Splitting the record on `:` instead (the previous parser) lost every
/// match in a path containing a colon (`a:b.txt` produced `path = "a"`,
/// `line = "b.txt"`), and disagreed with the built-in walker, which had no such
/// problem. A record without a NUL is still accepted as a bare `line:text` and
/// attributed to `base`: dropping it would turn a real match into a silent "no
/// matches", and old ripgrep builds are the only way to reach it.
fn parse_rg_record(ws: &Workspace, base: &Path, record: &[u8]) -> Option<SearchMatch> {
    let (path_bytes, rest) = match record.iter().position(|b| *b == 0) {
        Some(nul) => (&record[..nul], &record[nul + 1..]),
        None => (&record[..0], record),
    };
    let rest = String::from_utf8_lossy(rest);
    // `line:text` or `line-text`; the line number cannot contain either, and the
    // text may contain both.
    let split = rest
        .find(|c: char| !c.is_ascii_digit())
        .filter(|i| *i > 0)?;
    let (raw_line, tail) = rest.split_at(split);
    let line: u64 = raw_line.parse().ok()?;
    let is_context = tail.starts_with('-');
    let text = tail.get(1..)?;

    let abs: PathBuf = if path_bytes.is_empty() {
        base.to_path_buf()
    } else {
        let raw_path = String::from_utf8_lossy(path_bytes).into_owned();
        let path = Path::new(&raw_path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    };
    Some(SearchMatch {
        path: ws.relative(&abs),
        line,
        text: normalise_match_text(text),
        is_context,
    })
}

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>) -> Vec<u8> {
    let Some(pipe) = pipe.as_mut() else {
        return Vec::new();
    };
    const STDERR_LIMIT: usize = 64 * 1024;
    match crate::bounded_pipe::read(pipe, STDERR_LIMIT).await {
        Ok((mut buf, truncated)) => {
            if truncated {
                buf.extend_from_slice(b"\n[ripgrep stderr truncated]");
            }
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Cap a match's text at [`MAX_MATCH_COLUMNS`] **characters**, adding an ellipsis.
///
/// ripgrep's `--max-columns` counts *bytes*, so a line of multi-byte characters is
/// cut earlier than the built-in walker cuts the same line (200 CJK characters:
/// rg stops after 300 bytes / ~100 characters, the walker after 300 characters).
/// Applying the character cap here makes the two engines return the same text for
/// the same line instead of merely both being bounded, and lets the documented
/// contract be about characters - which is what a model reading the result counts
/// (verified: 200 CJK chars -> rg 637 bytes / 237 chars, builtin 606 bytes / 206
/// chars before this change).
fn normalise_match_text(text: &str) -> String {
    // Never strip ripgrep's preview marker as if it were unambiguous syntax: a
    // short real line is allowed to end in those exact bytes. The marker is at
    // least 1200 bytes into any rg-generated preview (`--max-columns` is 1200),
    // while a Rust char is at most four bytes. It therefore cannot begin within
    // the first 300 characters of a generated preview, and ordinary character
    // truncation removes it whenever the line really was long. If it is literal
    // text on a short line it survives intact.
    truncate_columns(text, MAX_MATCH_COLUMNS)
}

/// Built-in search: used when `rg` is unavailable, and by tests directly.
///
/// This convenience entry point retains the old detached behaviour for direct
/// callers. The tool itself uses [`search_fallback_with_cancel`].
pub async fn search_fallback(ws: &Workspace, query: &SearchQuery) -> Result<SearchResult> {
    search_fallback_with_cancel(ws, query, &CancellationToken::new()).await
}

pub(crate) async fn search_fallback_with_cancel(
    ws: &Workspace,
    query: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<SearchResult> {
    let ws = ws.clone();
    let query = query.clone();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || search_fallback_sync_with_cancel(&ws, &query, &cancel))
        .await
        .map_err(|e| Error::Internal(format!("search task panicked: {e}")))?
}

/// Synchronous implementation of the fallback walker, retained for callers that
/// deliberately do not have a cancellation token.
pub fn search_fallback_sync(ws: &Workspace, query: &SearchQuery) -> Result<SearchResult> {
    search_fallback_sync_with_cancel(ws, query, &CancellationToken::new())
}

fn search_fallback_sync_with_cancel(
    ws: &Workspace,
    query: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<SearchResult> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
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

    let mut scan = Scan {
        ws,
        regex: &regex,
        globset: globset.as_ref(),
        max_results,
        context: if query.files_only {
            0
        } else {
            query.context.min(MAX_CONTEXT_LINES)
        },
        per_file_cap: query.max_matches_per_file,
        files_only: query.files_only,
        cancel,
        matches: Vec::new(),
        matched: 0,
        walk_errors: 0,
    };

    let hit_limit = if base.is_file() {
        scan.entries(std::iter::once(Ok(Some(base.clone()))))?
    } else {
        // Do not materialise the tree. A matching early file can reach the global
        // cap and stop this iterator before later directories are opened at all.
        let walker = ignore::WalkBuilder::new(&base)
            .require_git(false)
            .git_global(false)
            .follow_links(false)
            .filter_entry(|entry| {
                let name = entry.file_name();
                name != ".git" && name != "node_modules"
            })
            .build();
        let entries = walker.map(|entry| match entry {
            Ok(e) if e.file_type().map(|t| t.is_file()).unwrap_or(false) => Ok(Some(e.into_path())),
            Ok(_) => Ok(None),
            Err(_) => Err(()),
        });
        scan.entries(entries)?
    };
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(fallback_result(scan, hit_limit))
}

/// One fallback search in progress: the walk feeds it files, it accumulates the
/// same entries the ripgrep path parses.
struct Scan<'a> {
    ws: &'a Workspace,
    regex: &'a regex::Regex,
    globset: Option<&'a GlobSet>,
    max_results: usize,
    context: usize,
    per_file_cap: usize,
    files_only: bool,
    cancel: &'a CancellationToken,
    matches: Vec<SearchMatch>,
    matched: usize,
    walk_errors: usize,
}

impl Scan<'_> {
    /// Walk `entries`, returning true as soon as the shared result cap is hit.
    fn entries(
        &mut self,
        entries: impl IntoIterator<Item = std::result::Result<Option<PathBuf>, ()>>,
    ) -> Result<bool> {
        for entry in entries {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let file = match entry {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(()) => {
                    self.walk_errors += 1;
                    continue;
                }
            };
            if self.file(&file)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Scan one file immediately. Only this file's bounded bytes and the returned
    /// entries are live.
    fn file(&mut self, file: &Path) -> Result<bool> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(set) = self.globset {
            let rel = self.ws.relative(file);
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !set.is_match(&rel) && !set.is_match(&name) {
                return Ok(false);
            }
        }

        let bytes = match read_fallback_file(file, self.cancel) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(false),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => {
                self.walk_errors += 1;
                return Ok(false);
            }
        };
        if bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0) {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(&bytes);
        let rel = self.ws.relative(file);

        // Collect (at most `per_file_cap + 1`) matching line numbers first: the
        // context window needs neighbours, and one extra hit is what proves the
        // per-file cap actually cut something.
        let per_file_limit = if self.per_file_cap == 0 {
            usize::MAX
        } else {
            self.per_file_cap
        };
        // Nothing beyond the remaining global budget can be reported, so a noisy
        // file is not read to its end for hits nobody will see.
        let still_useful = self.max_results.saturating_sub(self.matched).max(1);
        let collect = per_file_limit
            .saturating_add(1)
            .min(still_useful.saturating_add(1));
        let mut hits: Vec<usize> = Vec::new();
        for (idx, line) in text.lines().enumerate() {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !self.regex.is_match(line) {
                continue;
            }
            hits.push(idx);
            if hits.len() >= collect {
                break;
            }
        }
        if self.files_only {
            if hits.is_empty() {
                return Ok(false);
            }
            self.matches.push(SearchMatch {
                path: rel,
                line: 0,
                text: String::new(),
                is_context: false,
            });
            self.matched += 1;
            return Ok(self.matched >= self.max_results);
        }

        let lines: Vec<&str> = text.lines().collect();
        // Hits are ascending, so context windows overlap or adjoin: `next` is the
        // first line not yet printed and every line is emitted once in file order.
        // The collected hit list includes at most one extra hit. That hit is the
        // boundary at which global/per-file caps stop rendering, not an extra match.
        let mut next = 0usize;
        for (hit_index, &hit) in hits.iter().enumerate().take(per_file_limit) {
            if self.matched >= self.max_results {
                return Ok(true);
            }
            let from = hit.saturating_sub(self.context).max(next);
            let mut to = (hit + self.context).min(lines.len().saturating_sub(1));
            let remaining_here = per_file_limit - hit_index;
            let remaining_global = self.max_results - self.matched;
            let acceptable = remaining_here.min(remaining_global);
            if let Some(unaccepted) = hit_index
                .checked_add(acceptable)
                .and_then(|idx| hits.get(idx))
            {
                // Do not spend the cap-draining tail on a later real match. Its
                // own result is outside the budget, even when rg includes it in
                // the accepted hit's context window.
                to = to.min(unaccepted.saturating_sub(1));
            }
            for (idx, line) in lines.iter().enumerate().take(to + 1).skip(from) {
                self.matches.push(SearchMatch {
                    path: rel.clone(),
                    line: idx as u64 + 1,
                    text: truncate_columns(line, MAX_MATCH_COLUMNS),
                    // A line inside an overlapping window can itself be a match.
                    // Check every real hit position, not merely the current one.
                    is_context: hits.binary_search(&idx).is_err(),
                });
            }
            next = to.saturating_add(1);
            self.matched += 1;
        }
        Ok(self.matched >= self.max_results)
    }
}

fn fallback_result(scan: Scan<'_>, hit_limit: bool) -> SearchResult {
    let warning = if scan.walk_errors > 0 {
        Some(format!(
            "{} paths could not be read and were skipped",
            scan.walk_errors
        ))
    } else {
        None
    };
    SearchResult {
        engine: SearchEngine::Builtin,
        matches: scan.matches,
        matched: scan.matched,
        files_only: scan.files_only,
        hit_limit,
        warning,
    }
}

/// Read a regular file through the cap even if it grows after metadata.
///
/// `Ok(None)` means "not searchable text-shaped input" (too large or no longer a
/// regular file). IO failures retain the old skip-and-warn behaviour in the
/// caller; cancellation is the only fatal condition here.
fn read_fallback_file(file: &Path, cancel: &CancellationToken) -> Result<Option<Vec<u8>>> {
    read_fallback_file_bounded(file, MAX_FALLBACK_FILE_BYTES, cancel)
}

fn read_fallback_file_bounded(
    file: &Path,
    max_bytes: u64,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>> {
    use std::io::Read;

    let mut handle = std::fs::File::open(file).map_err(|e| Error::io(file, e))?;
    let meta = handle.metadata().map_err(|e| Error::io(file, e))?;
    if !meta.is_file() || meta.len() > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity((meta.len() as usize).min(max_bytes as usize));
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = match handle.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::io(file, e)),
        };
        if bytes.len().saturating_add(n) > max_bytes as usize {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    Ok(Some(bytes))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn stderr_is_drained_concurrently_and_bounded() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(1024);
        let producer = tokio::spawn(async move {
            writer.write_all(&vec![b'x'; 200_000]).await.unwrap();
        });
        let mut pipe = Some(reader);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), read_pipe(&mut pipe))
            .await
            .unwrap();
        producer.await.unwrap();
        assert!(result.ends_with(b"[ripgrep stderr truncated]"));
        assert!(result.len() < 66_000);
    }

    /// Minimal temp workspace; the crate-level test harness lives in
    /// `crate::tests`, which this module cannot reach.
    struct TempWs {
        path: PathBuf,
    }

    impl TempWs {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kc-tools-search-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.path.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        fn ws(&self) -> Workspace {
            Workspace::new(&self.path).unwrap()
        }
    }

    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn rg_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn query(pattern: &str, path: &str, max_results: usize) -> SearchQuery {
        SearchQuery {
            pattern: pattern.into(),
            path: path.into(),
            glob: None,
            case_insensitive: false,
            max_results,
            context: 0,
            files_only: false,
            max_matches_per_file: 0,
        }
    }

    /// Bug 1: ripgrep omits the filename prefix for a single file argument, so
    /// every match used to be dropped and the tool reported "no matches".
    #[tokio::test]
    async fn an_explicit_file_path_still_returns_matches() {
        let tmp = TempWs::new("single-file");
        tmp.write("one.txt", "alpha\nbeta\nalpha\n");
        let ws = tmp.ws();
        let q = query("alpha", "one.txt", 10);
        let cancel = CancellationToken::new();

        // The built-in walker always handled this; it is the reference.
        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matches.len(), 2, "{:?}", builtin.matches);
        assert_eq!(builtin.matches[0].path, "one.txt");
        assert_eq!(builtin.matches[0].line, 1);
        assert_eq!(builtin.matches[1].line, 3);

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.engine, SearchEngine::Ripgrep);
        assert_eq!(
            rg.matches.len(),
            2,
            "a single-file search must not silently return zero matches: {:?}",
            rg.matches
        );
        assert_eq!(rg.matches[0].path, "one.txt");
        assert_eq!(rg.matches[0].line, 1);
        assert_eq!(rg.matches[0].text, "alpha");
        assert_eq!(rg.matches[1].line, 3);
    }

    /// Bug 1, defensive half: a bare `line:text` record is attributed to the
    /// search base instead of being dropped.
    #[test]
    fn a_bare_line_text_record_is_attributed_to_the_base() {
        let tmp = TempWs::new("parse-bare");
        tmp.write("one.txt", "alpha\n");
        let ws = tmp.ws();

        // A bare record is relative to the search base, which for a single-file
        // search is the file itself.
        let file_base = ws.resolve_existing("one.txt").unwrap();
        let m = parse_rg_record(&ws, &file_base, b"1:alpha").expect("bare record must parse");
        assert_eq!(m.path, "one.txt");
        assert_eq!(m.line, 1);
        assert_eq!(m.text, "alpha");

        // The normal `path\0line:text` shape works against a directory base.
        let dir_base = ws.resolve_existing(".").unwrap();
        let m = parse_rg_record(&ws, &dir_base, b"one.txt\x003:alpha").expect("prefixed record");
        assert_eq!(m.path, "one.txt");
        assert_eq!(m.line, 3);
        assert_eq!(m.text, "alpha");

        // Garbage is still rejected rather than invented.
        assert!(parse_rg_record(&ws, &dir_base, b"not a match").is_none());
    }

    /// ripgrep is run with `--null`: prove the whole path round-trips, not just the
    /// parser, by searching a directory that contains such a file.
    #[tokio::test]
    async fn ripgrep_finds_a_match_in_a_path_containing_a_colon() {
        let tmp = TempWs::new("rg-colon-path");
        tmp.write("a:b.txt", "needle one\n");
        tmp.write("plain.txt", "needle two\n");
        let ws = tmp.ws();
        let q = query("needle", ".", 10);
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        let mut builtin_paths: Vec<&str> =
            builtin.matches.iter().map(|m| m.path.as_str()).collect();
        builtin_paths.sort_unstable();
        assert_eq!(builtin_paths, vec!["a:b.txt", "plain.txt"]);

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.engine, SearchEngine::Ripgrep);
        let mut rg_paths: Vec<&str> = rg.matches.iter().map(|m| m.path.as_str()).collect();
        rg_paths.sort_unstable();
        assert_eq!(
            rg_paths, builtin_paths,
            "the engines must agree, including on a path with a `:` in it"
        );
        let colon = rg
            .matches
            .iter()
            .find(|m| m.path == "a:b.txt")
            .expect("rg must report the colon path");
        assert_eq!(colon.line, 1);
        assert_eq!(colon.text, "needle one");
    }

    /// The multi-byte half of the same parity claim: `--max-columns` counts bytes,
    /// the walker counts characters, so a CJK line was cut in a different place by
    /// each engine. Both now cut at [`MAX_MATCH_COLUMNS`] characters.
    #[tokio::test]
    async fn both_engines_cut_a_wide_character_line_at_the_same_place() {
        let tmp = TempWs::new("cjk-columns");
        // 12 ASCII + 400 three-byte characters: 1 212 bytes, 412 characters, so
        // both engines have to cut (the line is longer than the cap in either
        // unit, which is what makes the comparison meaningful).
        let line = format!("NEEDLE{}{}", "x".repeat(6), "中".repeat(400));
        assert_eq!(line.chars().count(), 412);
        tmp.write("wide.txt", &line);
        let ws = tmp.ws();
        let q = query("NEEDLE", ".", 10);
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matches.len(), 1);
        let builtin_text = &builtin.matches[0].text;
        assert_eq!(
            builtin_text.chars().count(),
            MAX_MATCH_COLUMNS + 1,
            "the walker keeps {MAX_MATCH_COLUMNS} characters plus the ellipsis"
        );

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches.len(), 1);
        let rg_text = &rg.matches[0].text;
        assert_eq!(
            rg_text,
            builtin_text,
            "both engines must return the same text for the same line \
             (rg kept {} chars, walker kept {} chars)",
            rg_text.chars().count(),
            builtin_text.chars().count()
        );
        assert!(
            !rg_text.contains("omitted end of long line"),
            "ripgrep's own marker must be replaced by the shared ellipsis: {rg_text:?}"
        );
    }

    /// Bug 2: `--max-count` is per file, so the cap used to be multiplied by the
    /// number of files. The cap must be global.
    #[tokio::test]
    async fn the_result_cap_is_global_across_files() {
        let tmp = TempWs::new("global-cap");
        let body: String = (1..=20).map(|i| format!("needle {i}\n")).collect();
        tmp.write("a.txt", &body);
        tmp.write("b.txt", &body);
        tmp.write("c.txt", &body);
        let ws = tmp.ws();
        let q = query("needle", ".", 3);
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matches.len(), 3, "{:?}", builtin.matches);
        assert!(builtin.hit_limit);

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(
            rg.matches.len(),
            3,
            "the cap must be global, not per file: {:?}",
            rg.matches
        );
        assert!(rg.hit_limit, "reaching the cap must be reported");
    }

    /// Bug 2: the two engines must agree on the cap, as the module doc claims.
    #[tokio::test]
    async fn both_engines_agree_on_the_cap() {
        let tmp = TempWs::new("cap-parity");
        let body: String = (1..=30).map(|i| format!("needle {i}\n")).collect();
        tmp.write("a.txt", &body);
        tmp.write("b.txt", &body);
        let ws = tmp.ws();
        let cancel = CancellationToken::new();

        for cap in [1usize, 5, 30] {
            let q = query("needle", ".", cap);
            let builtin = search_fallback(&ws, &q).await.unwrap();
            assert_eq!(builtin.matches.len(), cap, "builtin cap {cap}");
            assert_eq!(builtin.hit_limit, cap <= 30);

            if !rg_available() {
                continue;
            }
            let rg = search_with_rg("rg", &ws, &q, &cancel)
                .await
                .unwrap()
                .expect("rg is installed");
            assert_eq!(
                rg.matches.len(),
                builtin.matches.len(),
                "engines disagree on cap {cap}: rg {:?} vs builtin {:?}",
                rg.matches.len(),
                builtin.matches.len()
            );
            assert_eq!(rg.hit_limit, builtin.hit_limit, "hit_limit at cap {cap}");
        }
    }

    /// Bug 3: `--max-columns` without `--max-columns-preview` replaced the whole
    /// line with the literal text `[Omitted long matching line]`, so a match on a
    /// minified bundle arrived with no content.
    #[tokio::test]
    async fn long_lines_keep_their_content_in_both_engines() {
        let tmp = TempWs::new("long-line");
        let line = format!("NEEDLE{}", "y".repeat(394));
        assert_eq!(line.chars().count(), 400);
        tmp.write("long.txt", &format!("{line}\n"));
        let ws = tmp.ws();
        let q = query("NEEDLE", ".", 10);
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matches.len(), 1);
        let builtin_text = &builtin.matches[0].text;
        assert!(
            !builtin_text.contains("[Omitted long matching line]"),
            "builtin: {builtin_text:?}"
        );
        assert_eq!(builtin_text.chars().count(), MAX_MATCH_COLUMNS + 1);
        assert!(builtin_text.ends_with('…'), "{builtin_text:?}");

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches.len(), 1);
        let rg_text = &rg.matches[0].text;
        assert!(
            !rg_text.contains("[Omitted long matching line]"),
            "a long match must keep its content: {rg_text:?}"
        );
        // Parity: both engines keep the same first MAX_MATCH_COLUMNS characters.
        let expected: String = line.chars().take(MAX_MATCH_COLUMNS).collect();
        assert!(
            rg_text.starts_with(&expected),
            "rg must keep the first {MAX_MATCH_COLUMNS} chars: {rg_text:?}"
        );
        assert!(
            builtin_text.starts_with(&expected),
            "builtin must keep the first {MAX_MATCH_COLUMNS} chars: {builtin_text:?}"
        );
    }

    #[tokio::test]
    async fn fallback_search_cancellation_reaches_the_blocking_worker() {
        let tmp = TempWs::new("fallback-cancel");
        tmp.write("a.txt", "needle\n");
        let ws = tmp.ws();
        let q = query("needle", ".", 10);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = search_fallback_with_cancel(&ws, &q, &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err}");
    }

    #[test]
    fn fallback_file_reads_are_capped_at_a_small_deterministic_limit() {
        let tmp = TempWs::new("fallback-read-cap");
        tmp.write("small.txt", "need");
        tmp.write("large.txt", "needle");
        let cancel = CancellationToken::new();
        let small = read_fallback_file_bounded(&tmp.path.join("small.txt"), 4, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(small, b"need");
        assert!(
            read_fallback_file_bounded(&tmp.path.join("large.txt"), 4, &cancel)
                .unwrap()
                .is_none()
        );
        cancel.cancel();
        assert!(matches!(
            read_fallback_file_bounded(&tmp.path.join("small.txt"), 4, &cancel),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn fallback_iteration_is_lazy_and_stops_pulling_at_the_match_cap() {
        use std::cell::Cell;

        let tmp = TempWs::new("fallback-lazy");
        tmp.write("first.txt", "needle\n");
        tmp.write("second.txt", "needle\n");
        let ws = tmp.ws();
        let first = ws.resolve_existing("first.txt").unwrap();
        let second = ws.resolve_existing("second.txt").unwrap();
        let visited = Cell::new(0usize);
        let entries = vec![first, second].into_iter().map(|path| {
            visited.set(visited.get() + 1);
            Ok::<Option<PathBuf>, ()>(Some(path))
        });
        let regex = RegexBuilder::new("needle").build().unwrap();
        let cancel = CancellationToken::new();
        let mut scan = Scan {
            ws: &ws,
            regex: &regex,
            globset: None,
            max_results: 1,
            context: 0,
            per_file_cap: 0,
            files_only: false,
            cancel: &cancel,
            matches: Vec::new(),
            matched: 0,
            walk_errors: 0,
        };
        let hit = scan.entries(entries).unwrap();
        assert!(hit);
        assert_eq!(scan.matched, 1, "one match fills the cap");
        assert_eq!(scan.matches.len(), 1);
        assert_eq!(visited.get(), 1, "the second path must never be yielded");
        assert_eq!(scan.walk_errors, 0);
    }

    /// A `-C N` record: `path\0line-text`. The separator between the line number
    /// and the text is `-`, not `:`.
    #[test]
    fn a_context_record_parses_as_context() {
        let tmp = TempWs::new("parse-context");
        tmp.write("one.txt", "alpha\n");
        let ws = tmp.ws();
        let base = ws.resolve_existing(".").unwrap();

        let context =
            parse_rg_record(&ws, &base, b"one.txt\x003-alpha above").expect("context record");
        assert_eq!(context.path, "one.txt");
        assert_eq!(context.line, 3);
        assert_eq!(context.text, "alpha above");
        assert!(context.is_context);

        let hit = parse_rg_record(&ws, &base, b"one.txt\x004:alpha").expect("match record");
        assert!(!hit.is_context);

        // The group separator ripgrep inserts between non-adjacent windows is not
        // a match and must not become one.
        assert!(parse_rg_record(&ws, &base, b"--").is_none());
    }

    /// `-C N` from both engines: same lines, same markers, no `--` separators.
    /// The model must not be able to tell which engine served the result.
    #[tokio::test]
    async fn both_engines_return_the_same_context_lines() {
        let tmp = TempWs::new("context-parity");
        tmp.write(
            "a.txt",
            "line1\nMATCH one\nline3\nline4\nline5\nMATCH two\nline7\n",
        );
        let ws = tmp.ws();
        let mut q = query("MATCH", "a.txt", 10);
        q.context = 1;
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        let expected = "a.txt:1- line1\n\
                        a.txt:2: MATCH one\n\
                        a.txt:3- line3\n\
                        a.txt:5- line5\n\
                        a.txt:6: MATCH two\n\
                        a.txt:7- line7";
        assert_eq!(builtin.body(), expected, "builtin context body");
        assert_eq!(builtin.matched, 2, "context lines are not matches");
        assert_eq!(builtin.matches.len(), 6);

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.body(), expected, "rg context body");
        assert_eq!(rg.matched, builtin.matched);
        assert!(
            !rg.body().contains("--"),
            "no group separator: {}",
            rg.body()
        );
    }

    /// `files_only` from both engines: the same distinct paths, one per line.
    #[tokio::test]
    async fn both_engines_agree_on_files_only() {
        let tmp = TempWs::new("files-only-parity");
        tmp.write("a.txt", "needle one\nneedle two\n");
        tmp.write("b.txt", "needle three\n");
        tmp.write("c.txt", "nothing here\n");
        let ws = tmp.ws();
        let mut q = query("needle", ".", 10);
        q.files_only = true;
        // Context is meaningless for a path list; neither engine may emit it.
        q.context = 2;
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert!(builtin.files_only);
        assert_eq!(builtin.matched, 2, "one entry per file, not per match");
        let mut builtin_files = builtin.files();
        builtin_files.sort_unstable();
        assert_eq!(builtin_files, vec!["a.txt", "b.txt"]);
        assert_eq!(builtin.body().lines().count(), 2, "{}", builtin.body());
        assert!(!builtin.body().contains("nothing here"));

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        let mut rg_files = rg.files();
        rg_files.sort_unstable();
        assert_eq!(rg_files, builtin_files, "both engines list the same files");
        assert_eq!(rg.matched, builtin.matched);
        // The shape is one path per line; walk order and rg order need not agree
        // (they never did), so compare the sorted sets, not the raw body.
        let rg_body = rg.body();
        let mut body_lines: Vec<&str> = rg_body.lines().collect();
        body_lines.sort_unstable();
        assert_eq!(body_lines, builtin_files, "rg body: {rg_body}");
        assert!(
            !rg_body.contains(':'),
            "a files-only body is paths, not `path:line`: {rg_body}"
        );
    }

    /// A per-file cap stops one noisy file from spending the global budget: the
    /// quiet file after it still appears, in both engines.
    #[tokio::test]
    async fn both_engines_apply_the_per_file_cap() {
        let tmp = TempWs::new("per-file-cap");
        let noisy: String = (1..=50).map(|i| format!("needle {i}\n")).collect();
        tmp.write("a.txt", &noisy);
        tmp.write("b.txt", "needle b1\nneedle b2\nneedle b3\n");
        let ws = tmp.ws();
        let mut q = query("needle", ".", 10);
        q.max_matches_per_file = 2;
        let cancel = CancellationToken::new();

        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(
            builtin.matched, 4,
            "two from each file: {:?}",
            builtin.matches
        );
        let mut files = builtin.files();
        files.sort_unstable();
        assert_eq!(
            files,
            vec!["a.txt", "b.txt"],
            "the noisy file must not consume the whole cap"
        );
        assert!(!builtin.hit_limit, "the global cap was not reached");

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(
            rg.matched, builtin.matched,
            "engines disagree on the per-file cap: rg {:?}",
            rg.matches
        );
        let mut rg_files = rg.files();
        rg_files.sort_unstable();
        assert_eq!(rg_files, files);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn newline_paths_round_trip_in_both_engines() {
        let tmp = TempWs::new("newline-path");
        let rel = "new\nline.txt";
        tmp.write(rel, "before\nneedle\nafter\n");

        // Control: raw rg really does put the literal newline before the NUL path
        // delimiter. This ensures the integration test below exercises the wire
        // format that used to corrupt records rather than an assumed format.
        if !rg_available() {
            return;
        }
        let raw = std::process::Command::new("rg")
            .args(["--line-number", "--no-heading", "--with-filename", "--null"])
            .arg("--")
            .arg("needle")
            .arg(tmp.path.join(rel))
            .env_remove("RIPGREP_CONFIG_PATH")
            .output()
            .unwrap();
        assert!(raw.status.success(), "{:?}", raw.stderr);
        let nul = raw.stdout.iter().position(|b| *b == 0).unwrap();
        assert!(raw.stdout[..nul].contains(&b'\n'), "{:?}", raw.stdout);

        let ws = tmp.ws();
        let mut q = query("needle", rel, 10);
        q.context = 1;
        let cancel = CancellationToken::new();
        let builtin = search_fallback(&ws, &q).await.unwrap();
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches, builtin.matches);
        assert_eq!(rg.matches.len(), 3);
        assert!(rg.matches.iter().all(|m| m.path == rel));
        assert_eq!(rg.matches[1].text, "needle");
        assert!(!rg.matches[1].is_context);
    }

    #[tokio::test]
    async fn hitting_the_cap_drains_only_the_accepted_hits_after_context() {
        let tmp = TempWs::new("cap-context-stream");
        tmp.write("a.txt", "unused\n");
        let ws = tmp.ws();
        let base = ws.resolve_existing(".").unwrap();
        let input = b"a.txt\x001:MATCH one\n\
                     a.txt\x002-after one\n\
                     a.txt\x003-after two\n\
                     a.txt\x004:MATCH two\n\
                     a.txt\x005-after other\n";
        let (matches, matched, limited) = read_matches(Some(&input[..]), &ws, &base, 1, 2).await;
        assert_eq!(matched, 1);
        assert!(limited);
        assert_eq!(
            matches.iter().map(|m| m.line).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(matches[0].text, "MATCH one");
        assert!(matches[1].is_context && matches[2].is_context);
        assert!(!matches.iter().any(|m| m.text == "MATCH two"));
    }

    #[tokio::test]
    async fn exact_cap_at_eof_still_returns_all_requested_context() {
        let tmp = TempWs::new("cap-context-eof");
        tmp.write("a.txt", "unused\n");
        let ws = tmp.ws();
        let base = ws.resolve_existing(".").unwrap();
        // Deliberately no final newline: the EOF record path must use the same
        // cap/context accounting as records that ended with one.
        let input = b"a.txt\x001:MATCH\na.txt\x002-after one\na.txt\x003-after two";
        let (matches, matched, limited) = read_matches(Some(&input[..]), &ws, &base, 1, 2).await;
        assert!(limited, "an exact cap at EOF still reached the cap");
        assert_eq!(matched, 1);
        assert_eq!(matches.len(), 3, "{matches:?}");
        assert_eq!(matches[2].text, "after two");
    }

    #[tokio::test]
    async fn overlapping_hit_windows_never_turn_real_matches_into_context() {
        let tmp = TempWs::new("overlap-hits");
        tmp.write(
            "a.txt",
            "before\nMATCH one\nmiddle\nMATCH two\nafter\nMATCH three\ntail\n",
        );
        let ws = tmp.ws();
        let mut q = query("MATCH", "a.txt", 10);
        q.context = 2;
        let cancel = CancellationToken::new();
        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matched, 3, "{:?}", builtin.matches);
        for line in [2, 4, 6] {
            let found = builtin.matches.iter().find(|m| m.line == line).unwrap();
            assert!(!found.is_context, "line {line} was relabelled: {found:?}");
        }

        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches, builtin.matches);
        assert_eq!(rg.matched, builtin.matched);
    }

    #[tokio::test]
    async fn cap_context_is_identical_in_both_engines() {
        let tmp = TempWs::new("cap-context-parity");
        tmp.write(
            "a.txt",
            "MATCH one\nafter one\nafter two\nMATCH two\nafter other\n",
        );
        let ws = tmp.ws();
        let mut q = query("MATCH", "a.txt", 1);
        q.context = 2;
        let cancel = CancellationToken::new();
        let builtin = search_fallback(&ws, &q).await.unwrap();
        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches, builtin.matches);
        assert_eq!(rg.matched, builtin.matched);
        assert_eq!(rg.hit_limit, builtin.hit_limit);
        assert_eq!(
            rg.body(),
            "a.txt:1: MATCH one\na.txt:2- after one\na.txt:3- after two"
        );
    }

    #[tokio::test]
    async fn a_short_literal_long_line_marker_is_not_stripped() {
        const LITERAL: &str = "NEEDLE [... omitted end of long line]";
        let tmp = TempWs::new("literal-marker");
        tmp.write("a.txt", &format!("{LITERAL}\n"));
        let ws = tmp.ws();
        let q = query("NEEDLE", "a.txt", 10);
        let cancel = CancellationToken::new();
        let builtin = search_fallback(&ws, &q).await.unwrap();
        assert_eq!(builtin.matches[0].text, LITERAL);
        if !rg_available() {
            return;
        }
        let rg = search_with_rg("rg", &ws, &q, &cancel)
            .await
            .unwrap()
            .expect("rg is installed");
        assert_eq!(rg.matches, builtin.matches);
        assert_eq!(rg.matches[0].text, LITERAL);
    }

    /// Re-executed so RIPGREP_CONFIG_PATH is hostile only in this child process;
    /// mutating the test runner's global environment would race unrelated tests.
    #[cfg(unix)]
    #[tokio::test]
    async fn inherited_ripgrep_config_cannot_execute_a_preprocessor() {
        const CHILD: &str = "KC_TOOLS_TEST_RG_PRE_CHILD";
        const MARKER: &str = "KC_TOOLS_TEST_RG_PRE_MARKER";
        const ROOT: &str = "KC_TOOLS_TEST_RG_PRE_ROOT";

        if std::env::var_os(CHILD).is_some() {
            let ws = Workspace::new(std::env::var_os(ROOT).unwrap()).unwrap();
            let q = query("needle", ".", 10);
            let cancel = CancellationToken::new();
            let rg = search_with_rg("rg", &ws, &q, &cancel)
                .await
                .unwrap()
                .expect("rg is installed");
            assert_eq!(rg.matches.len(), 1);
            assert_eq!(rg.matches[0].text, "needle");
            assert!(
                !std::path::Path::new(&std::env::var_os(MARKER).unwrap()).exists(),
                "RIPGREP_CONFIG_PATH executed a preprocessor in a read-only search"
            );

            let missing = ws.root().join("definitely-not-rg");
            let fallback = search_with_program(missing.to_str().unwrap(), &ws, &q, &cancel)
                .await
                .unwrap();
            assert_eq!(fallback.engine, SearchEngine::Builtin);
            assert_eq!(fallback.matches, rg.matches);
            assert!(
                !std::path::Path::new(&std::env::var_os(MARKER).unwrap()).exists(),
                "the normal fallback path executed the preprocessor"
            );
            return;
        }

        if !rg_available() {
            return;
        }
        let tmp = TempWs::new("rg-pre-environment");
        tmp.write("one.txt", "needle\n");
        let pre = tmp.path.join("pre.sh");
        std::fs::write(
            &pre,
            "#!/bin/sh\nprintf 'executed\\n' > \"$KC_TOOLS_TEST_RG_PRE_MARKER\"\ncat \"$1\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pre, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = tmp.path.join("rg.conf");
        std::fs::write(&config, format!("--pre={}\n", pre.display())).unwrap();
        let marker = tmp.path.join("PRE_RAN");

        // Prove this is a real execution primitive, not a marker that could never
        // be created. Only this direct, explicitly spawned control rg sees it.
        let proof = std::process::Command::new("rg")
            .arg("needle")
            .arg(tmp.path.join("one.txt"))
            .env("RIPGREP_CONFIG_PATH", &config)
            .env(MARKER, &marker)
            .output()
            .unwrap();
        assert!(proof.status.success(), "{:?}", proof.stderr);
        assert!(marker.exists(), "the raw-rg control did not execute --pre");
        std::fs::remove_file(&marker).unwrap();

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "search::tests::inherited_ripgrep_config_cannot_execute_a_preprocessor",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env(ROOT, &tmp.path)
            .env(MARKER, &marker)
            .env("RIPGREP_CONFIG_PATH", &config)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "child stdout:\n{}\nchild stderr:\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            !marker.exists(),
            "the child executed the inherited preprocessor"
        );
    }
}
