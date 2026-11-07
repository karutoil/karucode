//! Example karucode plugin.
//!
//! Demonstrates the whole surface a plugin usually needs:
//!
//! * two tools (`hello`, `word_count`) so `tools/list` exercises more than one;
//! * a `before_prompt` hook that contributes a line to the system prompt;
//! * a declared permission (`files.read`) reported in the manifest;
//! * declarative UI: a button in the composer's action row, a command-palette
//!   entry, and a hide request for the inspector's **Agents** tab.
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
    UiActionRequest, UiActionResult, UiActionResultExt,
};
use serde_json::json;

fn main() -> std::io::Result<()> {
    // One handler, reached from two places: a button and a keyboard shortcut. That
    // is the point of naming the *action* rather than the control.
    let greet = |req: UiActionRequest| {
        let workspace = if req.workspace.is_empty() {
            "an unknown workspace"
        } else {
            req.workspace.as_str()
        };
        UiActionResult::ok(format!("Hello from example-hello, in {workspace}!"))
    };

    PluginBuilder::new("example-hello", "0.1.0")
        .description("Example plugin: greets, counts words, contributes a note and a button")
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
        // A button in the composer's action row. Clicking it runs `ui/action`,
        // which is a plain callback in this process: no route of its own, no tool
        // call, and no approval prompt (the click is the approval). The answer is
        // a toast; `refresh` would re-read one of this plugin's panels.
        .button_with(
            "composer.toolbar",
            "greet",
            "Say hello",
            Some("spark"),
            Some("Ask example-hello to greet this workspace"),
            greet,
        )
        // The same action on a chord. The app refuses a bare key and any chord it
        // already uses, so a plugin cannot take over typing or the command palette.
        .keybinding("greet-key", "Say hello (shortcut)", "mod+shift+h", greet)
        // A palette command that hands work to the agent instead of running code:
        // the cheapest useful contribution, because it needs no handler at all.
        .message_command(
            "plan",
            "Ask about the plan",
            "Summarise the plan for this workspace.",
        )
        // Cosmetic only: `inspector.tab.agents` is on the app's hideable list, and
        // the host would refuse anything else. Hiding is app-wide, so a real
        // plugin should say why it does it — and a person can undo it in
        // Settings → Plugins & MCP.
        .hide("inspector.tab.agents")
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
