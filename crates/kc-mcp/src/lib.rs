//! MCP client: stdio and streamable-HTTP transports, tools namespaced as
//! `mcp__<server>__<tool>`.

use kc_core::config::McpServerConfig;
use kc_core::tool::Tool;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    pub status: String,
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct McpManager {
    configs: Vec<McpServerConfig>,
}

impl McpManager {
    pub fn new(configs: &[McpServerConfig]) -> Self {
        Self {
            configs: configs.to_vec(),
        }
    }

    pub fn server_count(&self) -> usize {
        self.configs.len()
    }

    pub async fn connect_all(&mut self) {}

    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }

    pub fn statuses(&self) -> Vec<McpServerStatus> {
        self.configs
            .iter()
            .map(|c| McpServerStatus {
                name: c.name.clone(),
                transport: c.transport.clone(),
                status: "disconnected".into(),
                tools: Vec::new(),
                error: None,
            })
            .collect()
    }

    pub async fn shutdown(&mut self) {}
}
