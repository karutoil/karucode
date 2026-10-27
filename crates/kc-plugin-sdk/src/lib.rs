//! SDK for out-of-process karucode plugins (newline-delimited JSON-RPC on stdio).

pub use kc_core::plugin::*;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};

pub struct ToolRequest {
    pub name: String,
    pub arguments: Value,
}

type ToolHandler = Box<dyn Fn(ToolRequest) -> PluginToolResult + Send + Sync>;
type HookHandler = Box<dyn Fn(Value) -> Value + Send + Sync>;

pub struct PluginBuilder {
    manifest: PluginManifest,
    tools: HashMap<String, (PluginToolDef, ToolHandler)>,
    hooks: HashMap<String, HookHandler>,
}

impl PluginBuilder {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            manifest: PluginManifest {
                name: name.clone(),
                version: version.into(),
                description: String::new(),
                protocol: PLUGIN_PROTOCOL_VERSION.into(),
                capabilities: vec!["tools".into()],
                permissions: Vec::new(),
                hooks: Vec::new(),
                tool_prefix: None,
            },
            tools: HashMap::new(),
            hooks: HashMap::new(),
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.manifest.description = d.into();
        self
    }

    pub fn permission(mut self, p: impl Into<String>) -> Self {
        self.manifest.permissions.push(p.into());
        self
    }

    pub fn tool(
        mut self,
        name: &str,
        description: &str,
        parameters: Value,
        handler: impl Fn(ToolRequest) -> PluginToolResult + Send + Sync + 'static,
    ) -> Self {
        self.tools.insert(
            name.to_string(),
            (
                PluginToolDef {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters,
                    class: None,
                },
                Box::new(handler),
            ),
        );
        self
    }

    pub fn hook(mut self, name: &str, handler: impl Fn(Value) -> Value + Send + Sync + 'static) -> Self {
        self.hooks.insert(name.to_string(), Box::new(handler));
        self.manifest.hooks.push(name.to_string());
        self
    }

    /// Serve requests on stdin/stdout until EOF or shutdown.
    pub fn run(self) -> std::io::Result<()> {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut reader = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let request: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    let err = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}});
                    writeln!(stdout, "{err}")?;
                    stdout.flush()?;
                    continue;
                }
            };
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let params = request.get("params").cloned().unwrap_or(Value::Null);

            let response = match method {
                methods::INITIALIZE => Ok(json!(self.manifest)),
                methods::TOOLS_LIST => {
                    let list: Vec<PluginToolDef> =
                        self.tools.values().map(|(d, _)| d.clone()).collect();
                    Ok(json!({"tools": list}))
                }
                methods::TOOLS_CALL => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                    match self.tools.get(name) {
                        Some((_, handler)) => {
                            let result = handler(ToolRequest {
                                name: name.to_string(),
                                arguments,
                            });
                            Ok(serde_json::to_value(result).unwrap_or_else(|_| json!({"ok":false,"content":"serialisation error"})))
                        }
                        None => Err(PluginError::new(-32601, format!("unknown tool `{name}`"))),
                    }
                }
                methods::HOOKS_RUN => {
                    let hook = params.get("hook").and_then(|v| v.as_str()).unwrap_or("");
                    let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                    match self.hooks.get(hook) {
                        Some(handler) => Ok(json!({"output": handler(payload)})),
                        None => Ok(json!({"output": null})),
                    }
                }
                methods::SHUTDOWN => {
                    let _ = writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"result":null}));
                    let _ = stdout.flush();
                    return Ok(());
                }
                other => Err(PluginError::new(-32601, format!("unknown method `{other}`"))),
            };

            let msg = match response {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(e) => json!({"jsonrpc":"2.0","id":id,"error":e}),
            };
            writeln!(stdout, "{msg}")?;
            stdout.flush()?;
        }
    }
}
