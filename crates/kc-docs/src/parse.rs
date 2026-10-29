//! Parser for the `@kc-doc` comment blocks that are karucode's internal
//! documentation.
//!
//! This module is compiled **twice**: as part of the library and, through
//! `#[path]`, as a module of `build.rs`. It must therefore depend on nothing but
//! `std` and must not reference crate-internal items.
//!
//! The format is deliberately line-oriented so a doc block is readable in the
//! editor and survives `cargo fmt` untouched:
//!
//! ```text
//! /// @kc-doc: mcp
//! /// @kc-title: Configure MCP servers
//! /// @kc-summary: One sentence, used by search results.
//! /// @kc-tags: mcp, tools, setup
//! /// @kc-applies-to: config:[[mcp_servers]], route:GET /api/mcp
//! ///
//! /// Markdown body...
//! ```
//!
//! Metadata lines are only recognised *before* the body starts, so a body that
//! documents the format itself can quote `@kc-...` lines safely.

/// Marker that opens a documentation block.
pub const MARKER: &str = "@kc-doc";

/// One parsed documentation entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDoc {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub applies_to: Vec<String>,
    /// Markdown body with the comment prefix removed.
    pub body: String,
    /// Source file, relative to the workspace root, with forward slashes.
    pub path: String,
    /// 1-based line of the `@kc-doc:` marker.
    pub line: usize,
}

impl ParsedDoc {
    /// `path:line`, the form used in messages and in the generated docs.
    pub fn location(&self) -> String {
        format!("{}:{}", self.path, self.line)
    }
}

/// A parse or validation failure. `Display` renders `path:line: message` so the
/// build script and the CLI can print it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocError {
    pub path: String,
    pub line: usize,
    pub message: String,
}

impl DocError {
    pub fn new(path: &str, line: usize, message: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            line,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.path, self.line, self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Lexer state carried across the lines of one file.
///
/// Doc blocks must be real line comments, not text that merely *looks* like a
/// comment inside a string literal. Without this, the examples in this module's
/// own tests would be compiled into the documentation. Only what a compiler
/// would see as a comment is accepted.
#[derive(Debug, Default, Clone)]
pub struct LexState {
    in_block_comment: bool,
    /// Number of `#` in an open Rust raw string (`r#"..."#`).
    raw_hashes: Option<usize>,
    in_double_quote: bool,
    in_single_quote_char: bool,
    in_backtick: bool,
}

impl LexState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The content of a line comment on this line, with the marker and one following
/// space removed. Returns `None` when the line carries no comment visible to the
/// compiler (including lines inside a multi-line string).
fn comment_body(line: &str, state: &mut LexState) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if state.in_block_comment {
            if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                state.in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if let Some(hashes) = state.raw_hashes {
            if bytes[i] == b'"' && closes_raw_string(bytes, i, hashes) {
                state.raw_hashes = None;
                i += 1 + hashes;
            } else {
                i += 1;
            }
            continue;
        }

        if state.in_double_quote || state.in_backtick {
            let quote = if state.in_double_quote { b'"' } else { b'`' };
            if bytes[i] == b'\\' {
                i += 2;
            } else if bytes[i] == quote {
                state.in_double_quote = false;
                state.in_backtick = false;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }

        if state.in_single_quote_char {
            if bytes[i] == b'\\' {
                i += 2;
            } else if bytes[i] == b'\'' {
                state.in_single_quote_char = false;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }

        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let mut rest = &line[i + 2..];
                if let Some(stripped) = rest.strip_prefix('/').or_else(|| rest.strip_prefix('!')) {
                    rest = stripped;
                }
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                return Some(rest.to_string());
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                state.in_block_comment = true;
                i += 2;
            }
            b'"' => {
                state.in_double_quote = true;
                i += 1;
            }
            b'`' => {
                state.in_backtick = true;
                i += 1;
            }
            b'\'' => {
                // A char literal closes on this line; a lifetime (`'a`) does not.
                if char_literal_closes(bytes, i) {
                    state.in_single_quote_char = true;
                }
                i += 1;
            }
            b'r' | b'b' if raw_string_opens(bytes, i).is_some() => {
                let (skip, hashes) = raw_string_opens(bytes, i).expect("checked above");
                state.raw_hashes = Some(hashes);
                i += skip;
            }
            _ => i += 1,
        }
    }
    None
}

/// At byte `i` of a potential `r#*"` opener, return (bytes to skip, hash count).
fn raw_string_opens(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let mut j = i;
    if bytes.get(j) == Some(&b'b') {
        j += 1;
    }
    if bytes.get(j) != Some(&b'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0usize;
    while bytes.get(j) == Some(&b'#') {
        hashes += 1;
        j += 1;
    }
    if bytes.get(j) == Some(&b'"') {
        Some((j + 1 - i, hashes))
    } else {
        None
    }
}

/// Does `bytes[start..]` begin with `"` followed by `hashes` `#` characters?
fn closes_raw_string(bytes: &[u8], start: usize, hashes: usize) -> bool {
    if bytes.get(start) != Some(&b'"') {
        return false;
    }
    (0..hashes).all(|offset| bytes.get(start + 1 + offset) == Some(&b'#'))
}

/// True when the `'` at `i` starts a char literal that closes on this line.
fn char_literal_closes(bytes: &[u8], i: usize) -> bool {
    match bytes.get(i + 1) {
        Some(b'\\') => {
            // `'\n'`, `'\''`, `'\u{1F600}'` — look for the closing quote.
            bytes[i + 2..].contains(&b'\'')
        }
        Some(_) => bytes.get(i + 2) == Some(&b'\''),
        None => false,
    }
}

/// The slug of a `@kc-doc:` marker line, if this line is one.
fn marker_slug(content: &str) -> Option<&str> {
    let rest = content.trim().strip_prefix(MARKER)?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let slug = rest.trim();
    if slug.is_empty() {
        // `@kc-doc:` with nothing after it is still a marker: the caller reports
        // the empty slug instead of silently ignoring the line.
        Some("")
    } else {
        Some(slug)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meta {
    Title,
    Summary,
    Tags,
    AppliesTo,
}

fn meta_key(key: &str) -> Option<Meta> {
    match key {
        "title" => Some(Meta::Title),
        "summary" => Some(Meta::Summary),
        "tags" | "tag" => Some(Meta::Tags),
        "applies-to" | "applies_to" => Some(Meta::AppliesTo),
        _ => None,
    }
}

fn split_list(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in value.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if !out.iter().any(|existing| existing == item) {
            out.push(item.to_string());
        }
    }
    out
}

fn valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 80 {
        return false;
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.starts_with('.') || slug.ends_with('.')
    {
        return false;
    }
    slug.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.' || c == '_')
}

/// Extract every complete `@kc-doc` block from one source file.
///
/// Incomplete blocks are reported in the returned errors and omitted from the
/// docs, so one typo does not hide the rest of the file's documentation.
pub fn extract(path: &str, text: &str) -> (Vec<ParsedDoc>, Vec<DocError>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    let mut state = LexState::new();
    let mut i = 0usize;

    while i < lines.len() {
        let Some(content) = comment_body(lines[i], &mut state) else {
            i += 1;
            continue;
        };
        let Some(slug) = marker_slug(&content).map(str::to_string) else {
            i += 1;
            continue;
        };
        let start = i + 1; // 1-based line of the marker

        // Collect the contiguous run of comment lines that forms the block.
        let mut block: Vec<(usize, String)> = vec![(start, content)];
        let mut j = i + 1;
        while j < lines.len() {
            match comment_body(lines[j], &mut state) {
                Some(body) => {
                    block.push((j + 1, body));
                    j += 1;
                }
                None => break,
            }
        }
        i = j;

        match build_doc(path, start, &slug, &block[1..]) {
            Ok(doc) => docs.push(doc),
            Err(mut errs) => errors.append(&mut errs),
        }
    }

    (docs, errors)
}

fn build_doc(
    path: &str,
    start: usize,
    slug: &str,
    rest: &[(usize, String)],
) -> Result<ParsedDoc, Vec<DocError>> {
    let mut errors = Vec::new();
    let slug = slug.trim().to_string();
    if !valid_slug(&slug) {
        errors.push(DocError::new(
            path,
            start,
            format!(
                "invalid @kc-doc slug `{slug}`: use lowercase letters, digits, '-', '.' or '_' \
                 (1-80 characters, no leading '-' or '.')"
            ),
        ));
    }

    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut applies_to: Vec<String> = Vec::new();
    let mut body_lines: Vec<String> = Vec::new();
    let mut body_started = false;

    for (line_no, raw) in rest {
        let trimmed = raw.trim();
        if !body_started {
            if trimmed.is_empty() {
                // A blank comment line ends the metadata block. It must not
                // swallow the rest of the body: everything after it is prose,
                // including lines that happen to start with `@kc-`.
                body_started = true;
                continue;
            }
            if let Some(meta_rest) = trimmed.strip_prefix("@kc-") {
                let Some((key, value)) = meta_rest.split_once(':') else {
                    errors.push(DocError::new(
                        path,
                        *line_no,
                        format!(
                            "malformed metadata line `{trimmed}`: expected `@kc-<key>: <value>` \
                             (keys: title, summary, tags, applies-to)"
                        ),
                    ));
                    continue;
                };
                let key = key.trim();
                let value = value.trim();
                match meta_key(key) {
                    Some(Meta::Title) => {
                        set_once(&mut title, value, "title", path, *line_no, &mut errors)
                    }
                    Some(Meta::Summary) => {
                        set_once(&mut summary, value, "summary", path, *line_no, &mut errors)
                    }
                    Some(Meta::Tags) => tags = split_list(value),
                    Some(Meta::AppliesTo) => applies_to = split_list(value),
                    None => errors.push(DocError::new(
                        path,
                        *line_no,
                        format!(
                            "unknown metadata key `@kc-{key}`; allowed keys are title, summary, \
                             tags and applies-to"
                        ),
                    )),
                }
                continue;
            }
            body_started = true;
        }
        body_lines.push(raw.clone());
    }

    let title = title.unwrap_or_default();
    let summary = summary.unwrap_or_default();
    if title.is_empty() {
        errors.push(DocError::new(
            path,
            start,
            "missing `@kc-title:` (one line describing what a reader can do with this doc)",
        ));
    } else if title.chars().count() > 140 {
        errors.push(DocError::new(
            path,
            start,
            "`@kc-title:` is longer than 140 characters; keep it a title, not a paragraph",
        ));
    }
    if summary.is_empty() {
        errors.push(DocError::new(
            path,
            start,
            "missing `@kc-summary:` (one sentence, shown in search results)",
        ));
    } else if summary.chars().count() > 240 {
        errors.push(DocError::new(
            path,
            start,
            "`@kc-summary:` is longer than 240 characters; move detail into the body",
        ));
    }

    let body = body_lines.join("\n").trim_end().to_string();
    if body.trim().is_empty() {
        errors.push(DocError::new(
            path,
            start,
            "empty documentation body: a title and summary alone do not tell anyone how to \
             do the task",
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ParsedDoc {
        slug,
        title,
        summary,
        tags,
        applies_to,
        body,
        path: path.to_string(),
        line: start,
    })
}

fn set_once(
    slot: &mut Option<String>,
    value: &str,
    key: &str,
    path: &str,
    line: usize,
    errors: &mut Vec<DocError>,
) {
    if slot.is_some() {
        errors.push(DocError::new(
            path,
            line,
            format!("duplicate `@kc-{key}:` in one documentation block"),
        ));
        return;
    }
    if value.is_empty() {
        errors.push(DocError::new(path, line, format!("`@kc-{key}:` is empty")));
        return;
    }
    *slot = Some(value.to_string());
}

/// Cross-file checks: slugs are identifiers, so they must be unique.
pub fn validate_unique(docs: &[ParsedDoc]) -> Vec<DocError> {
    let mut errors = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for doc in docs {
        if let Some((_, previous)) = seen.iter().find(|(slug, _)| slug == &doc.slug) {
            errors.push(DocError::new(
                &doc.path,
                doc.line,
                format!(
                    "duplicate @kc-doc slug `{}`: already declared at {previous}. Slugs are stable \
                     identifiers; pick a distinct one or merge the two blocks",
                    doc.slug
                ),
            ));
        } else {
            seen.push((doc.slug.clone(), doc.location()));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
//! A module.
// Nothing to see here.

/// @kc-doc: mcp
/// @kc-title: Configure MCP servers
/// @kc-summary: Connect stdio or HTTP servers and expose their tools.
/// @kc-tags: mcp, tools, setup
/// @kc-applies-to: config:[[mcp_servers]], route:GET /api/mcp
///
/// ## Steps
///
/// ```toml
/// [[mcp_servers]]
/// name = "filesystem"
/// ```
pub struct Marker;
"#;

    #[test]
    fn parses_a_block_and_ignores_other_comments() {
        let (docs, errors) = extract("crates/kc-mcp/src/lib.rs", SAMPLE);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(docs.len(), 1);
        let doc = &docs[0];
        assert_eq!(doc.slug, "mcp");
        assert_eq!(doc.title, "Configure MCP servers");
        assert_eq!(doc.tags, vec!["mcp", "tools", "setup"]);
        assert_eq!(doc.applies_to.len(), 2);
        assert_eq!(doc.line, 5);
        assert!(doc.body.starts_with("## Steps"));
        // Indentation inside a fenced block must survive.
        assert!(doc.body.contains("\nname = \"filesystem\""));
    }

    #[test]
    fn markers_inside_string_literals_are_ignored() {
        let text = r##"
const SAMPLE: &str = r#"
/// @kc-doc: not-a-doc
/// @kc-title: Nope
/// @kc-summary: Nope
/// body
"#;

let s = "// @kc-doc: also-not";
let t = "```";

/// @kc-doc: real
/// @kc-title: Real
/// @kc-summary: Real summary.
///
/// Body.
"##;
        let (docs, errors) = extract("x.rs", text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(docs.len(), 1, "{docs:?}");
        assert_eq!(docs[0].slug, "real");
    }

    #[test]
    fn double_slash_comments_work_for_typescript() {
        let text = "// @kc-doc: web-ui\n// @kc-title: The web UI\n// @kc-summary: Panels.\n//\n// Body text.\n";
        let (docs, errors) = extract("web/src/app.tsx", text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(docs[0].slug, "web-ui");
        assert_eq!(docs[0].body, "Body text.");
    }

    #[test]
    fn missing_required_metadata_is_an_error() {
        let text = "/// @kc-doc: broken\n///\n/// Body.\n";
        let (docs, errors) = extract("x.rs", text);
        assert!(docs.is_empty());
        let joined = errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("missing `@kc-title:`"));
        assert!(joined.contains("missing `@kc-summary:`"));
    }

    #[test]
    fn unknown_metadata_key_is_an_error() {
        let text = "/// @kc-doc: x\n/// @kc-titel: typo\n/// @kc-summary: s\n///\n/// Body\n";
        let (docs, errors) = extract("x.rs", text);
        assert!(docs.is_empty());
        assert!(errors[0]
            .message
            .contains("unknown metadata key `@kc-titel`"));
    }

    #[test]
    fn metadata_after_the_body_is_body_text() {
        let text = "/// @kc-doc: x\n/// @kc-title: T\n/// @kc-summary: S\n///\n/// @kc-doc: quoted in the body\n";
        let (docs, errors) = extract("x.rs", text);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(docs[0].body.contains("@kc-doc: quoted"));
    }

    #[test]
    fn a_blank_source_line_ends_the_block() {
        let text = "/// @kc-doc: a\n/// @kc-title: A\n/// @kc-summary: S\n///\n/// Body one.\n\n/// @kc-doc: b\n/// @kc-title: B\n/// @kc-summary: S\n///\n/// Body two.\n";
        let (docs, errors) = extract("x.rs", text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].body, "Body one.");
        assert_eq!(docs[1].body, "Body two.");
    }

    #[test]
    fn invalid_slug_is_rejected() {
        for slug in ["Provider", "provider config", "-leading", "dots.", "sl/ash"] {
            let text =
                format!("/// @kc-doc: {slug}\n/// @kc-title: T\n/// @kc-summary: S\n///\n/// B\n");
            let (docs, errors) = extract("x.rs", &text);
            assert!(docs.is_empty(), "slug {slug:?} should be rejected");
            assert!(errors
                .iter()
                .any(|e| e.message.contains("invalid @kc-doc slug")));
        }
        assert!(valid_slug("provider-profiles"));
        assert!(valid_slug("api.v1"));
        assert!(valid_slug("mcp_servers"));
    }

    #[test]
    fn duplicate_slugs_are_reported_across_files() {
        let a = "/// @kc-doc: same\n/// @kc-title: A\n/// @kc-summary: S\n///\n/// Body A\n";
        let b = "/// @kc-doc: same\n/// @kc-title: B\n/// @kc-summary: S\n///\n/// Body B\n";
        let mut docs = extract("a.rs", a).0;
        docs.extend(extract("b.rs", b).0);
        let errors = validate_unique(&docs);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("duplicate @kc-doc slug `same`"));
        assert!(errors[0].message.contains("a.rs:1"));
    }
}
