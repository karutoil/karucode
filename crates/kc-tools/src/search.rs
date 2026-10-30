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
        // `-H` forces the `path:` prefix even when ripgrep is given a single
        // file argument. Without it rg emits bare `line:text`, which the parser
        // cannot attribute to a file, so every match was silently dropped.
        .arg("--with-filename")
        .arg("--max-columns")
        .arg(MAX_MATCH_COLUMNS.to_string())
        // Without this, `--max-columns` replaces the whole line with the literal
        // text `[Omitted long matching line]`, so a match on a minified bundle
        // arrives with no content at all.
        .arg("--max-columns-preview")
        // `--max-count` is *per file*, not global: it is only a per-file safety
        // valve here. The global cap is enforced by the streaming reader below,
        // which stops at `max_results` matches and kills the child.
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
            // Stream stdout and stop at the global cap instead of buffering the
            // whole result set: a broad pattern on a large repo can emit
            // hundreds of MB, of which all but `max_results` are discarded.
            // `read_matches` takes ownership of the pipe so it is closed the
            // moment the cap is reached; otherwise ripgrep would block writing
            // to a full pipe while we wait for it to exit.
            let (matches, hit_limit) =
                read_matches(stdout_pipe.take(), ws, &base, max_results).await;
            let stderr = read_pipe(&mut stderr_pipe).await;
            let status = child.wait().await;
            (matches, hit_limit, stderr, status)
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

    let (matches, hit_limit, stderr, status) = outcome;
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
        hit_limit,
        warning,
    }))
}

/// Read ripgrep's stdout line by line, stopping as soon as `max_results`
/// matches have been collected.
///
/// Takes ownership of the pipe so it is dropped (closed) on return: ripgrep then
/// dies of SIGPIPE instead of writing the rest of a huge result set, and cannot
/// block on a full pipe while the caller waits for it to exit. The buffer never
/// holds more than one record, so memory is bounded by [`MAX_RG_RECORD_BYTES`]
/// rather than by the size of the output.
///
/// Returns the matches and whether the cap was reached.
async fn read_matches<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    ws: &Workspace,
    base: &Path,
    max_results: usize,
) -> (Vec<SearchMatch>, bool) {
    let mut matches: Vec<SearchMatch> = Vec::new();
    let Some(pipe) = pipe else {
        return (matches, false);
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
        while let Some(nl) = rest.iter().position(|b| *b == b'\n') {
            record.extend_from_slice(&rest[..nl]);
            if let Some(m) = parse_rg_line(ws, base, &String::from_utf8_lossy(&record)) {
                matches.push(m);
                if matches.len() >= max_results {
                    return (matches, true);
                }
            }
            record.clear();
            rest = &rest[nl + 1..];
        }
        record.extend_from_slice(rest);
        if record.len() > MAX_RG_RECORD_BYTES {
            // A single absurdly long record: drop it rather than grow forever.
            record.clear();
        }
    }
    if !record.is_empty() {
        if let Some(m) = parse_rg_line(ws, base, &String::from_utf8_lossy(&record)) {
            matches.push(m);
        }
    }
    let hit_limit = matches.len() >= max_results;
    (matches, hit_limit)
}

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>) -> Vec<u8> {
    let Some(pipe) = pipe.as_mut() else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf).await;
    buf
}

/// Parse one `path:line:text` record.
///
/// ripgrep normally emits the filename prefix (`-H` is passed for exactly this
/// reason), but a bare `line:text` record is still accepted and attributed to
/// `base`: dropping it would turn a real match into a silent "no matches".
fn parse_rg_line(ws: &Workspace, base: &Path, line: &str) -> Option<SearchMatch> {
    let mut parts = line.splitn(3, ':');
    let first = parts.next()?;
    let second = parts.next();
    let third = parts.next();

    // `path:line:text` when the second field is a line number, else `line:text`
    // relative to `base`.
    let (raw_path, raw_line, text) = match (second, third) {
        (Some(second), Some(third)) => match second.parse::<u64>() {
            Ok(n) => (Some(first), n, third),
            Err(_) => (None, first.parse::<u64>().ok()?, second),
        },
        // `line:text` with an empty text field.
        (Some(second), None) => (None, first.parse::<u64>().ok()?, second),
        (None, _) => return None,
    };

    let abs: PathBuf = match raw_path {
        Some(raw_path) => {
            let path = Path::new(raw_path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                base.join(path)
            }
        }
        None => base.to_path_buf(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

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
        let m = parse_rg_line(&ws, &file_base, "1:alpha").expect("bare record must parse");
        assert_eq!(m.path, "one.txt");
        assert_eq!(m.line, 1);
        assert_eq!(m.text, "alpha");

        // The normal `path:line:text` shape still works against a directory base.
        let dir_base = ws.resolve_existing(".").unwrap();
        let m = parse_rg_line(&ws, &dir_base, "one.txt:3:alpha").expect("prefixed record");
        assert_eq!(m.path, "one.txt");
        assert_eq!(m.line, 3);
        assert_eq!(m.text, "alpha");

        // Garbage is still rejected rather than invented.
        assert!(parse_rg_line(&ws, &dir_base, "not a match").is_none());
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
}
