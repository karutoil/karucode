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
    truncate_output, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec,
};
use serde_json::json;

const DESCRIPTION: &str = "Search and read karucode's own built-in documentation. Call this before answering any \
question about karucode itself: configuration, environment variables, model providers or gateways, plugins, MCP \
servers, approvals, tools, the HTTP API, projects, subagents, the web UI, or how these docs are maintained. The text \
is compiled from the code at build time, so it matches the running version. Actions: \"list\" (all topics), \
\"search\" (ranked matches for a query), \"read\" (full markdown for one slug). Read-only, no approval, works in any \
workspace. Prefer it over the workspace tools when the question is about karucode rather than about the user's \
project.";

const MAX_LIMIT: usize = 20;

pub struct KarucodeDocs;

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
                        "description": "max search results, 1-20 (default 5)"
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
                match kc_docs::render_doc(requested) {
                    Some(text) => text,
                    None => {
                        return ToolOutcome::err(format!(
                            "no documentation topic `{requested}`. Known topics: {}",
                            kc_docs::slugs().join(", ")
                        ))
                    }
                }
            }
            other => {
                return ToolOutcome::err(format!(
                    "unknown action `{other}`: use \"list\", \"search\" or \"read\""
                ))
            }
        };

        let limits = ctx.limits().await;
        let (text, cut) = truncate_output(&body, limits.max_tool_output_bytes);
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

    struct TestSettings;

    #[async_trait]
    impl Settings for TestSettings {
        async fn model(&self) -> ModelConfig {
            ModelConfig::default()
        }
        async fn approval_policy(&self) -> ApprovalPolicy {
            ApprovalPolicy::default()
        }
        async fn limits(&self) -> Limits {
            Limits::default()
        }
        async fn subagent_config(&self) -> SubagentConfig {
            SubagentConfig::default()
        }
    }

    fn ctx() -> ToolCtx {
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
            settings: Arc::new(TestSettings),
            depth: 0,
            supports_images: false,
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

    #[test]
    fn it_is_read_only_and_needs_no_approval() {
        assert_eq!(KarucodeDocs.class(), ToolClass::ReadOnly);
        assert!(!KarucodeDocs.requires_approval());
        assert_eq!(KarucodeDocs.spec().name, "karucode_docs");
    }
}
