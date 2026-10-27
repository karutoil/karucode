//! Example karucode plugin.
//!
//! Demonstrates the whole surface a plugin usually needs:
//!
//! * two tools (`hello`, `word_count`) so `tools/list` exercises more than one;
//! * a `before_prompt` hook that contributes a line to the system prompt;
//! * a declared permission (`files.read`) reported in the manifest.
//!
//! Build and run it through the host:
//!
//! ```bash
//! cargo build -p example-hello
//! # then in config.toml:
//! # [[plugins]]
//! # name = "example-hello"
//! # command = ["target/debug/example-hello"]
//! ```
//!
//! It is a plain stdin/stdout process: run it by hand to poke the protocol.

use kc_plugin_sdk::{
    hooks, permissions, PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest,
};
use serde_json::json;

fn main() -> std::io::Result<()> {
    PluginBuilder::new("example-hello", "0.1.0")
        .description("Example plugin: greets, counts words and contributes a prompt note")
        .permission(permissions::FILES_READ)
        .tool(
            "hello",
            "Return a greeting for the given name.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Who to greet." }
                },
                "required": ["name"]
            }),
            |req: ToolRequest| {
                let name = match req.arguments.get("name").and_then(|v| v.as_str()) {
                    Some(name) if !name.trim().is_empty() => name,
                    _ => "world",
                };
                PluginToolResult::ok(format!("Hello, {name}! (from the example-hello plugin)"))
            },
        )
        .tool(
            "word_count",
            "Count the words, lines and characters in a text.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to measure." }
                },
                "required": ["text"]
            }),
            |req: ToolRequest| {
                let text = req.str_arg("text");
                if text.is_empty() {
                    return PluginToolResult::err("word_count needs a non-empty `text` argument");
                }
                let words = text.split_whitespace().count();
                let lines = text.lines().count();
                let chars = text.chars().count();
                PluginToolResult::ok(format!("{words} words, {lines} lines, {chars} characters"))
                    .with_meta(json!({"words": words, "lines": lines, "chars": chars}))
            },
        )
        .hook(hooks::BEFORE_PROMPT, |payload| {
            let workspace = payload
                .get("workspace")
                .and_then(|v| v.as_str())
                .unwrap_or("the workspace");
            json!(format!(
                "Plugin note: the example-hello plugin is active in {workspace}. \
                 It can greet people and count words."
            ))
        })
        .hook(hooks::ON_RUN_END, |payload| {
            // Reserved for future use; returning null means "no contribution".
            let _ = payload;
            serde_json::Value::Null
        })
        .run()
}
