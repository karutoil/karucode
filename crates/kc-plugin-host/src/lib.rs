//! Out-of-process plugin host: discovery, supervision, capability negotiation.

use kc_core::config::PluginConfig;
use kc_core::plugin::PluginDiagnostics;
use kc_core::tool::Tool;
use std::path::Path;
use std::sync::Arc;

pub struct PluginHost {
    plugins: Vec<PluginSlot>,
}

pub(crate) struct PluginSlot {
    pub config: PluginConfig,
    pub diagnostics: PluginDiagnostics,
}

impl PluginHost {
    pub fn discover(_configs: &[PluginConfig], _data_dir: &Path) -> Self {
        Self { plugins: Vec::new() }
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub async fn start_all(&mut self) {}

    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }

    pub fn diagnostics(&self) -> Vec<PluginDiagnostics> {
        self.plugins.iter().map(|p| p.diagnostics.clone()).collect()
    }

    pub async fn set_enabled(&mut self, _name: &str, _enabled: bool) -> kc_core::Result<PluginDiagnostics> {
        todo!("kc-plugin-host: enable/disable")
    }

    pub async fn call_hook(&self, _hook: &str, _payload: serde_json::Value) -> Vec<serde_json::Value> {
        Vec::new()
    }

    pub async fn shutdown(&mut self) {}
}
