//! The read-only `karucode_docs` tool.
//!
//! karucode answers questions about *itself* (how to add a provider, write a
//! plugin, wire an MCP server, change the approval policy). Those answers are
//! compiled into the binary by [`kc_docs`], so the agent can read them in any
//! workspace without network access and without guessing at flags that no longer
//! exist.
//!
//! The tool is deliberately one entry point with three actions instead of three
//! tools: it keeps the tool list short and the model only has to learn one name.

use async_trait::async_trait;
use kc_core::tool::{
    truncate_output_with_advice, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec,
};
use serde_json::json;

const DESCRIPTION: &str = "Search and read karucode's own built-in documentation. Call this before answering any \
question about karucode itself: configuration, environment variables, model providers or gateways, plugins, MCP \
servers, approvals, tools, the HTTP API, projects, subagents, the web UI, or how these docs are maintained. The text \
is compiled from the code at build time, so it matches the running version. Actions: \"list\" (all topics), \
\"search\" (ranked matches for a query), \"read\" (one topic, paged with `offset`/`limit` so a long topic says \
exactly which offset continues it). Read-only, no approval, works in any workspace. Prefer it over the workspace \
tools when the question is about karucode rather than about the user's project.";

const MAX_LIMIT: usize = 20;
/// Lines of a topic returned by one `read` when the call does not ask for more.
const DEFAULT_DOC_LINES: usize = 400;
/// Lines of a topic one `read` will return at most.
const MAX_DOC_LINES: usize = 2000;
/// Room reserved for the page header and the continuation hint.
const DOC_RESERVE: usize = 300;

pub struct KarucodeDocs;

/// Render one page of a topic: `offset` is 1-based, `limit` bounds the lines, and
/// `budget` (`limits.max_tool_output_bytes`) bounds the bytes.
///
/// A long topic used to arrive as one body that the shared cap spliced head+tail:
/// the model saw the beginning and the end of a page and had no way to ask for the
/// middle. Paging it here keeps the continuation exact - the page header states the
/// range and the last line is the call that reads the next one.
fn page_doc(slug: &str, text: &str, offset: usize, limit: usize, budget: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = (offset - 1).min(total);
    if total == 0 {
        return "this topic is empty".to_string();
    }
    if start >= total {
        return format!(
            "`{slug}` has {total} lines; offset {offset} is past the end. Read it with \
             {{\"action\":\"read\",\"doc\":\"{slug}\",\"offset\":1}}"
        );
    }
    let end = (start + limit).min(total);
    let mut body = String::new();
    let stop = budget.saturating_sub(DOC_RESERVE);
    let mut written = start;
    for line in &lines[start..end] {
        let cost = line.len() + 1;
        if written > start && body.len() + cost > stop {
            break;
        }
        body.push('\n');
        body.push_str(line);
        written += 1;
    }
    let mut out = format!(
        "`{slug}` — lines {}-{} of {total}",
        start + 1,
        written.max(start + 1)
    );
    out.push_str(&body);
    if written < total {
        out.push_str(&format!(
            "\n\n[next {} lines omitted; continue with \
             {{\"action\":\"read\",\"doc\":\"{slug}\",\"offset\":{}}}]",
            total - written,
            written + 1
        ));
    }
    out
}

#[async_trait]
impl Tool for KarucodeDocs {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "karucode_docs".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "search", "read"],
                        "description": "list every topic, search for a query, or read one topic"
                    },
                    "query": {
                        "type": "string",
                        "description": "search terms, e.g. \"configure a provider\" or \"KARUCODE_AUTH_TOKEN\" (search only)"
                    },
                    "doc": {
                        "type": "string",
                        "description": "doc slug from list/search, e.g. \"mcp\" (read only)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "max search results for search (1-20, default 5), or max lines for read (1-2000, default 400)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "read only: 1-based first line of the topic (default 1); a long topic's result names the offset that continues it"
                    }
                },
                "required": ["action"]
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let action = request
            .opt_str("action")
            .unwrap_or("search")
            .trim()
            .to_ascii_lowercase();

        let body = match action.as_str() {
            "list" => kc_docs::render_list(),
            "search" => {
                let query = request.opt_str("query").unwrap_or("").trim();
                if query.is_empty() {
                    return ToolOutcome::err(
                        "action \"search\" needs a `query` (e.g. {\"action\":\"search\",\
                         \"query\":\"mcp server\"}); use action \"list\" to see every topic",
                    );
                }
                let limit = request
                    .opt_u64("limit")
                    .unwrap_or(5)
                    .clamp(1, MAX_LIMIT as u64) as usize;
                let hits = kc_docs::search(query, limit);
                kc_docs::render_hits(query, &hits)
            }
            "read" => {
                let requested = request
                    .opt_str("doc")
                    .or_else(|| request.opt_str("slug"))
                    .unwrap_or("")
                    .trim();
                if requested.is_empty() {
                    return ToolOutcome::err(format!(
                        "action \"read\" needs a `doc` slug. Known topics: {}",
                        kc_docs::slugs().join(", ")
                    ));
                }
                let full = match kc_docs::render_doc(requested) {
                    Some(text) => text,
                    None => {
                        return ToolOutcome::err(format!(
                            "no documentation topic `{requested}`. Known topics: {}",
                            kc_docs::slugs().join(", ")
                        ))
                    }
                };
                let offset = request.opt_u64("offset").unwrap_or(1).max(1) as usize;
                let limit = request
                    .opt_u64("limit")
                    .unwrap_or(DEFAULT_DOC_LINES as u64)
                    .clamp(1, MAX_DOC_LINES as u64) as usize;
                let limits = ctx.limits().await;
                page_doc(
                    requested,
                    &full,
                    offset,
                    limit,
                    limits.max_tool_output_bytes,
                )
            }
            other => {
                return ToolOutcome::err(format!(
                    "unknown action `{other}`: use \"list\", \"search\" or \"read\""
                ))
            }
        };

        let limits = ctx.limits().await;
        let (text, cut) = truncate_output_with_advice(
            &body,
            limits.max_tool_output_bytes,
            "read one topic at a time, page it with `offset`, or raise \
             limits.max_tool_output_bytes",
        );
        ToolOutcome::ok(text).truncated(cut).with_meta(json!({
            "action": action,
            "topics": kc_docs::count(),
            "docs_crate": env!("CARGO_PKG_VERSION"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::approval::{ApprovalDecision, ApprovalRequest};
    use kc_core::config::{ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
    use kc_core::events::Event;
    use kc_core::ids::{RunId, SessionId};
    use kc_core::paths::Workspace;
    use kc_core::settings::Settings;
    use kc_core::tool::ApprovalHandle;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct DenyAll;

    #[async_trait]
    impl kc_core::tool::ApprovalGate for DenyAll {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            panic!("karucode_docs must never ask for approval")
        }
    }

    struct TestSettings {
        limits: Limits,
    }

    #[async_trait]
    impl Settings for TestSettings {
        async fn model(&self) -> ModelConfig {
            ModelConfig::default()
        }
        async fn approval_policy(&self) -> ApprovalPolicy {
            ApprovalPolicy::default()
        }
        async fn limits(&self) -> Limits {
            self.limits.clone()
        }
        async fn subagent_config(&self) -> SubagentConfig {
            SubagentConfig::default()
        }
    }

    fn ctx() -> ToolCtx {
        ctx_with(Limits::default())
    }

    fn ctx_with(limits: Limits) -> ToolCtx {
        let workspace = Workspace::new(std::env::temp_dir()).expect("temp dir is a workspace");
        ToolCtx {
            workspace: Arc::new(workspace),
            cancel: CancellationToken::new(),
            approvals: ApprovalHandle::new(
                Arc::new(DenyAll),
                SessionId("s".into()),
                RunId("r".into()),
                None,
            ),
            events: Arc::new(|_e: Event| {}),
            settings: Arc::new(TestSettings { limits }),
            depth: 0,
            supports_images: false,
            checkpoints: None,
        }
    }

    async fn call(args: serde_json::Value) -> ToolOutcome {
        KarucodeDocs
            .invoke(
                ToolCallRequest {
                    call_id: "c1".into(),
                    name: "karucode_docs".into(),
                    arguments: args,
                },
                ctx(),
            )
            .await
    }

    #[tokio::test]
    async fn lists_every_topic() {
        let out = call(json!({"action": "list"})).await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("`mcp`"));
        assert!(out.content.contains("`provider-profiles`"));
    }

    #[tokio::test]
    async fn searches_and_reads() {
        let out = call(json!({"action": "search", "query": "configure a provider"})).await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("provider-profiles"));

        let out = call(json!({"action": "read", "doc": "mcp"})).await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("mcp_servers"));
        assert!(out.content.contains("Source of truth"));
    }

    #[tokio::test]
    async fn bad_input_is_actionable() {
        let out = call(json!({"action": "search"})).await;
        assert!(!out.ok);
        assert!(out.content.contains("needs a `query`"));

        let out = call(json!({"action": "read", "doc": "nope"})).await;
        assert!(!out.ok);
        assert!(out.content.contains("no documentation topic `nope`"));

        let out = call(json!({"action": "dance"})).await;
        assert!(!out.ok);
        assert!(out.content.contains("unknown action"));
    }

    #[tokio::test]
    async fn a_long_topic_is_paged_with_an_exact_continuation() {
        // The biggest required topic is well past one page at a small line limit.
        let limits = Limits {
            max_tool_output_bytes: 24_000,
            ..Default::default()
        };
        let out = KarucodeDocs
            .invoke(
                ToolCallRequest {
                    call_id: "c1".into(),
                    name: "karucode_docs".into(),
                    arguments: json!({"action": "read", "doc": "builtin-tools", "limit": 20}),
                },
                ctx_with(limits),
            )
            .await;
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.starts_with("`builtin-tools` — lines 1-20 of "),
            "the page must state its range: {}",
            out.content
        );
        let hint = out
            .content
            .rsplit("continue with ")
            .next()
            .expect("a continuation hint");
        assert!(
            hint.contains("\"action\":\"read\"") && hint.contains("\"offset\":21"),
            "the hint must be the exact next call: {hint}"
        );

        // Following the hint returns the very next line of the topic, not a
        // re-rendered page from the top.
        let out =
            call(json!({"action": "read", "doc": "builtin-tools", "offset": 21, "limit": 5})).await;
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.starts_with("`builtin-tools` — lines 21-25 of "),
            "{}",
            out.content
        );
        let full = kc_docs::render_doc("builtin-tools").unwrap();
        let expected: Vec<&str> = full.lines().skip(20).take(5).collect();
        for line in expected {
            assert!(
                out.content.contains(line),
                "line {line:?} missing from the page:\n{}",
                out.content
            );
        }

        // A page that runs past the end says so instead of returning nothing.
        let out = call(json!({"action": "read", "doc": "builtin-tools", "offset": 100_000})).await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("is past the end"), "{}", out.content);
    }

    #[test]
    fn it_is_read_only_and_needs_no_approval() {
        assert_eq!(KarucodeDocs.class(), ToolClass::ReadOnly);
        assert!(!KarucodeDocs.requires_approval());
        assert_eq!(KarucodeDocs.spec().name, "karucode_docs");
    }
}
