//! karucode's internal documentation, compiled from the code itself.
//!
//! Every capability that a user or an agent can ask about — a config key, an
//! environment variable, a CLI flag, an API route, a tool, a plugin/MCP setup
//! step, an approval rule — carries an `@kc-doc` comment block in the file that
//! implements it. [`build.rs`](../build.rs) extracts those blocks at compile time
//! and this crate exposes them as a searchable registry, so the agent's answer
//! about karucode comes from the same revision as the code, not from a wiki that
//! drifted.
//!
//! The convention, the commands and the review rules live in
//! [`docs/INTERNAL-DOCS.md`](../../docs/INTERNAL-DOCS.md) and [`AGENTS.md`](../../AGENTS.md).
//!
//! ```text
//! /// @kc-doc: mcp
//! /// @kc-title: Configure MCP servers
//! /// @kc-summary: Connect stdio or streamable-HTTP servers and use their tools.
//! /// @kc-tags: mcp, tools, setup
//! /// @kc-applies-to: config:[[mcp_servers]], route:GET /api/mcp
//! ///
//! /// Body markdown...
//! ```
//!
//! The agent reaches this registry through the read-only `karucode_docs` tool
//! (`crates/kc-tools/src/docs_tool.rs`); humans reach the generated markdown in
//! `docs/internal/`.
//!
//! ```
//! let hits = kc_docs::search("configure a provider", 5);
//! assert!(!hits.is_empty());
//! ```
//!
//! @kc-doc: documentation
//! @kc-title: How karucode's own documentation works
//! @kc-summary: Doc blocks in the code compile into the built-in docs the agent reads; add, update and delete them with the code.
//! @kc-tags: docs, contributing, agents, meta
//! @kc-applies-to: file:AGENTS.md, file:docs/INTERNAL-DOCS.md, tool:karucode_docs, cmd:cargo run -p kc-docs -- --write
//!
//! # Why the docs live in the code
//!
//! karucode answers questions about itself (how do I add a provider profile, how
//! do I write a plugin, how do I wire an MCP server). A separate wiki rots the
//! first week nobody updates it, and a stale answer is worse than no answer
//! because the agent states it with confidence. Putting the documentation next to
//! the implementation and compiling it in means a change to the code and a change
//! to its documentation land in the same diff, or the build fails.
//!
//! # The workflow
//!
//! 1. Add the block in the file that implements the behaviour. Metadata first,
//!    then a blank comment line, then markdown.
//! 2. Regenerate the readable copy: `cargo run -p kc-docs -- --write`.
//! 3. Check it the way CI does: `cargo run -p kc-docs -- --check` (also
//!    `cargo test -p kc-docs`).
//! 4. Commit the source change and `docs/internal/` together. `--check` fails if
//!    they disagree.
//!
//! Deleting a capability means deleting its block **and** its slug from
//! [`REQUIRED_SLUGS`] in `crates/kc-docs/src/lib.rs` in the same change. The list
//! is the guard against a feature quietly losing its documentation.
//!
//! # Writing a block that an agent can actually use
//!
//! * Answer "how do I do X" for one task per block. If a doc needs two unrelated
//!   procedures, write two blocks.
//! * Put the exact identifiers in `@kc-applies-to`: env var names, TOML keys,
//!   routes, tool names. Search weights that field heavily, so a user who pastes
//!   `KARUCODE_AUTH_TOKEN` or `GET /api/config` lands on the right doc.
//! * Show a complete, copy-pasteable example. Partial config snippets are the
//!   most common source of wrong answers.
//! * State the gotchas the code actually has (defaults, limits, what is refused).
//!   If a behaviour is surprising, it belongs here.
//! * Link the deep reference (`docs/PLUGINS.md`, `docs/API.md`) instead of
//!   duplicating it line by line.
//! * Never document an intention that the code does not implement. The doc
//!   compiles from the same revision as the code; a confident lie is the one
//!   failure mode this system cannot detect for you.
//!
//! # How the agent sees it
//!
//! The system prompt lists every doc slug, and the `karucode_docs` tool offers
//! three actions: `list`, `search` (ranked over title, tags, applies-to, summary
//! and body) and `read` (the full markdown plus its source location). The tool is
//! read-only and never touches the workspace, so it works in any project and
//! needs no approval.

use std::collections::BTreeSet;

pub mod parse;
pub mod scan;

pub use parse::{DocError, ParsedDoc};

/// One documentation entry, borrowed from the build-time generated table.
#[derive(Debug, Clone, Copy)]
pub struct RawDoc {
    pub slug: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub tags: &'static [&'static str],
    pub applies_to: &'static [&'static str],
    pub body: &'static str,
    /// `path:line` of the `@kc-doc:` marker that produced this entry.
    pub source: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/generated_docs.rs"));

/// Topics that must always exist. Removing one is a deliberate decision about the
/// product, so it must also remove the slug here — the diff then shows the intent
/// instead of silently dropping a capability's documentation.
pub const REQUIRED_SLUGS: &[&str] = &[
    "approvals",
    "builtin-tools",
    "configuration",
    "deployment",
    "documentation",
    "http-api",
    "mcp",
    "plugins",
    "projects",
    "provider-profiles",
    "prompt-cache",
    "run-server",
    "security",
    "subagents",
    "themes",
    "web-ui",
];

/// Minimum body length for a required topic. A one-line stub is not a doc.
pub const MIN_REQUIRED_BODY_CHARS: usize = 400;

/// Shift every Markdown heading in a body down one level, so a body that starts
/// at `# Section` nests under the generated `# Title`. Lines inside fenced code
/// blocks are left alone (shell/TOML comments start with `#`).
pub fn demote_headings(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 16);
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
        } else if !in_fence && line.starts_with('#') {
            out.push('#');
            out.push_str(line);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

impl RawDoc {
    /// Full markdown rendering, including provenance.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n> {}\n\n", self.title, self.summary));
        out.push_str(&format!("- **Slug:** `{}`\n", self.slug));
        if !self.tags.is_empty() {
            out.push_str(&format!("- **Tags:** {}\n", self.tags.join(", ")));
        }
        if !self.applies_to.is_empty() {
            out.push_str(&format!(
                "- **Applies to:** {}\n",
                self.applies_to
                    .iter()
                    .map(|a| format!("`{a}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str(&format!("- **Source:** `{}`\n\n---\n\n", self.source));
        out.push_str(demote_headings(self.body.trim_end()).trim_end());
        out.push('\n');
        out
    }

    /// One `path:line: text` line, for search results.
    pub fn one_line(&self) -> String {
        format!("`{}` — {} ({})", self.slug, self.summary, self.source)
    }
}

/// Every documentation entry, sorted by slug.
pub fn all() -> &'static [RawDoc] {
    GENERATED
}

/// Look up one entry by its stable slug.
pub fn get(slug: &str) -> Option<&'static RawDoc> {
    GENERATED.iter().find(|doc| doc.slug == slug)
}

/// Every slug, sorted. This is what the system prompt shows the model.
pub fn slugs() -> Vec<&'static str> {
    GENERATED.iter().map(|doc| doc.slug).collect()
}

pub fn count() -> usize {
    GENERATED.len()
}

/// `- `slug` — title` lines, used as the system-prompt topic index.
pub fn topic_index() -> String {
    GENERATED
        .iter()
        .map(|doc| format!("- `{}` — {}", doc.slug, doc.title))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Slugs from [`REQUIRED_SLUGS`] that are missing from the compiled registry.
pub fn missing_required() -> Vec<&'static str> {
    REQUIRED_SLUGS
        .iter()
        .copied()
        .filter(|slug| get(slug).is_none())
        .collect()
}

/// Quality problems that should fail CI: missing required topics and required
/// topics whose body is too thin to answer a question.
pub fn quality_problems() -> Vec<String> {
    let mut problems = Vec::new();
    for slug in missing_required() {
        problems.push(format!(
            "required topic `{slug}` is missing; add an @kc-doc block for it or remove the slug \
             from REQUIRED_SLUGS with a reason"
        ));
    }
    for slug in REQUIRED_SLUGS {
        if let Some(doc) = get(slug) {
            let len = doc.body.chars().count();
            if len < MIN_REQUIRED_BODY_CHARS {
                problems.push(format!(
                    "required topic `{slug}` has a {len}-character body (minimum \
                     {MIN_REQUIRED_BODY_CHARS}); it must explain how to do the task, not just \
                     name it"
                ));
            }
        }
    }
    problems
}

/// One ranked search result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub doc: &'static RawDoc,
    pub score: i32,
    pub matched_terms: usize,
    pub snippet: String,
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "in", "is", "it", "me", "my", "of", "on", "or", "set", "the", "to", "use", "using", "was",
    "what", "when", "where", "which", "with", "you", "your",
];

fn tokenize(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in query
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/'))
    {
        let term = raw.trim_matches(|c: char| c == '.' || c == '-' || c == '/' || c == '_');
        if term.len() < 2 || STOPWORDS.contains(&term) {
            continue;
        }
        if !terms.iter().any(|t| t == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

/// 2 when the term appears as a whole word, 1 when it is only a substring.
fn hit_strength(haystack: &str, term: &str) -> i32 {
    let Some(pos) = haystack.find(term) else {
        return 0;
    };
    let before_ok = pos == 0
        || !haystack[..pos]
            .chars()
            .next_back()
            .map(|c| c.is_alphanumeric())
            .unwrap_or(false);
    let after = pos + term.len();
    let after_ok = after >= haystack.len()
        || !haystack[after..]
            .chars()
            .next()
            .map(|c| c.is_alphanumeric())
            .unwrap_or(false);
    if before_ok && after_ok {
        2
    } else {
        1
    }
}

fn field_score(doc: &RawDoc, term: &str) -> (i32, bool) {
    let mut score = 0;
    let mut matched = false;
    let mut apply = |text: &str, weight: i32| {
        let strength = hit_strength(&text.to_lowercase(), term);
        if strength > 0 {
            score += strength * weight;
            matched = true;
        }
    };
    apply(doc.slug, 12);
    apply(doc.title, 8);
    apply(&doc.tags.join(" "), 6);
    apply(&doc.applies_to.join(" "), 7);
    apply(doc.summary, 5);
    apply(doc.body, 2);
    (score, matched)
}

fn snippet_for(doc: &RawDoc, terms: &[String]) -> String {
    for line in doc.body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") || line.starts_with('#') {
            continue;
        }
        let lower = line.to_lowercase();
        if terms.iter().any(|t| lower.contains(t.as_str())) {
            return truncate_chars(line, 200);
        }
    }
    truncate_chars(doc.summary, 200)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Rank the documentation against a free-text query.
///
/// A doc qualifies when it matches at least two thirds of the meaningful terms
/// (all of them for one- and two-term queries). Results are ordered by number of
/// matched terms, then score, then slug — deterministic, so tests and the agent
/// see the same order every time.
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let phrase = query.trim().to_lowercase();
    let mut hits: Vec<Hit> = Vec::new();

    for doc in GENERATED {
        let mut score = 0;
        let mut matched = 0usize;
        for term in &terms {
            let (field, found) = field_score(doc, term);
            score += field;
            if found {
                matched += 1;
            }
        }
        if matched == 0 || matched * 3 < terms.len() * 2 {
            continue;
        }
        if phrase.len() >= 4 {
            let body = doc.body.to_lowercase();
            if doc.title.to_lowercase().contains(&phrase) {
                score += 30;
            }
            if doc.applies_to.join(" ").to_lowercase().contains(&phrase) {
                score += 40;
            }
            if body.contains(&phrase) {
                score += 25;
            }
        }
        hits.push(Hit {
            doc,
            score,
            matched_terms: matched,
            snippet: snippet_for(doc, &terms),
        });
    }

    hits.sort_by(|a, b| {
        b.matched_terms
            .cmp(&a.matched_terms)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.doc.slug.cmp(b.doc.slug))
    });
    hits.truncate(limit.max(1));
    hits
}

/// Render search hits for the tool (`slug`, score, snippet, provenance).
pub fn render_hits(query: &str, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!(
            "no documentation matched {query:?}. Try a single keyword (`provider`, `mcp`, \
             `plugins`, `approvals`, `api`) or call with action \"list\" to see every topic."
        );
    }
    let mut out = format!("{} match(es) for {query:?}:\n", hits.len());
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. `{}` — {}\n   {}\n   source: {}\n",
            i + 1,
            hit.doc.slug,
            hit.doc.title,
            hit.snippet,
            hit.doc.source
        ));
    }
    out.push_str(
        "\nRead one with action \"read\" and the slug, e.g. {\"action\": \"read\", \"doc\": \"mcp\"}.",
    );
    out
}

/// Render the full topic list.
pub fn render_list() -> String {
    let mut out = format!("{} documentation topics:\n", GENERATED.len());
    for doc in GENERATED {
        out.push_str(&format!(
            "\n- `{}` — {}\n  {}",
            doc.slug, doc.title, doc.summary
        ));
    }
    out
}

/// Render one doc for the tool, with provenance and a hint about neighbours.
pub fn render_doc(slug: &str) -> Option<String> {
    let doc = get(slug)?;
    let mut out = doc.markdown();
    out.push_str(&format!(
        "\n\n---\n_Source of truth: `{}`. If the code and this text disagree, the code wins; \
         the doc is compiled from that file at build time._\n",
        doc.source
    ));
    Some(out)
}

/// Debug aid: the set of source files that contributed a block.
pub fn source_files() -> Vec<&'static str> {
    let mut set: BTreeSet<&'static str> = BTreeSet::new();
    for doc in GENERATED {
        if let Some((path, _)) = doc.source.rsplit_once(':') {
            set.insert(path);
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_topic_is_documented() {
        let problems = quality_problems();
        assert!(
            problems.is_empty(),
            "documentation quality gate failed:\n{}",
            problems.join("\n")
        );
    }

    #[test]
    fn slugs_are_unique_and_sorted() {
        let slugs = slugs();
        let mut sorted = slugs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            slugs, sorted,
            "the registry must be sorted and duplicate-free"
        );
    }

    #[test]
    fn search_finds_the_obvious_questions() {
        let cases = [
            ("configure a provider profile", "provider-profiles"),
            ("KARUCODE_AUTH_TOKEN", "security"),
            ("how do I write a plugin", "plugins"),
            ("mcp server stdio", "mcp"),
            ("approval mode read_only", "approvals"),
            ("GET /api/config", "http-api"),
        ];
        for (query, expected) in cases {
            let hits = search(query, 5);
            assert!(!hits.is_empty(), "query {query:?} returned no hits at all");
            assert!(
                hits.iter().take(3).any(|h| h.doc.slug == expected),
                "query {query:?} did not rank `{expected}` in the top 3: {:?}",
                hits.iter()
                    .map(|h| (h.doc.slug, h.score, h.matched_terms))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn read_and_list_render() {
        let doc = get("mcp").expect("the mcp topic exists");
        let rendered = render_doc("mcp").expect("renders");
        assert!(rendered.contains(doc.title));
        assert!(rendered.contains(doc.source));
        assert!(render_list().contains("`mcp`"));
        assert!(render_doc("does-not-exist").is_none());
    }

    #[test]
    fn empty_query_returns_nothing() {
        assert!(search("", 5).is_empty());
        assert!(search("the and of", 5).is_empty());
    }
}
