//! Example karucode plugin: one tool and one prompt hook.

use kc_plugin_sdk::{hooks, PluginBuilder, PluginToolResult, ToolRequest};
use serde_json::json;

fn main() -> std::io::Result<()> {
    PluginBuilder::new("example-hello", "0.1.0")
        .description("Example plugin: greets and contributes a prompt note")
        .permission(kc_plugin_sdk::permissions::FILES_READ)
        .tool(
            "hello",
            "Return a greeting for the given name.",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            |req: ToolRequest| {
                let name = req
                    .arguments
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("world");
                PluginToolResult {
                    ok: true,
                    content: format!("Hello, {name}! (from the example-hello plugin)"),
                    meta: None,
                }
            },
        )
        .hook(hooks::BEFORE_PROMPT, |_payload| {
            json!("Plugin note: the example-hello plugin is active.")
        })
        .run()
}
