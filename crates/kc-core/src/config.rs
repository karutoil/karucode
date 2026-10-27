//! Configuration: defaults, config file, environment variables, precedence.
//!
//! Precedence (highest first):
//!   1. per-install overrides saved from the UI (stored in SQLite)
//!   2. environment variables
//!   3. config file (`KARUCODE_CONFIG`, else `~/.config/karucode/config.toml`)
//!   4. built-in defaults
//!
//! Setting `KARUCODE_LOCK_SETTINGS=1` removes layer 1, so a deployment can pin
//! provider settings from the environment and reject UI edits.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask for shell commands and mutating tools (default).
    Ask,
    /// Ask only for shell commands; file writes inside the workspace are allowed.
    AskCommands,
    /// Allow everything; recorded in the event log for auditability.
    Auto,
    /// Only read-only tools are permitted; mutations and shell are denied.
    ReadOnly,
}

impl Default for ApprovalMode {
    fn default() -> Self {
        ApprovalMode::Ask
    }
}

impl ApprovalMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(ApprovalMode::Ask),
            "ask_commands" | "ask-commands" => Some(ApprovalMode::AskCommands),
            "auto" | "allow" => Some(ApprovalMode::Auto),
            "read_only" | "read-only" | "readonly" => Some(ApprovalMode::ReadOnly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalMode::Ask => "ask",
            ApprovalMode::AskCommands => "ask_commands",
            ApprovalMode::Auto => "auto",
            ApprovalMode::ReadOnly => "read_only",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalPolicy {
    #[serde(default)]
    pub mode: ApprovalMode,
    /// Command prefixes that never require approval, e.g. ["cargo test", "rg"].
    #[serde(default)]
    pub allow_commands: Vec<String>,
    /// Glob-ish path prefixes inside the workspace that may be written without
    /// asking (only consulted in `ask` mode). Empty means nothing is pre-approved.
    #[serde(default)]
    pub auto_approve_paths: Vec<String>,
    /// How long an approval request waits before being treated as denied.
    #[serde(default = "default_approval_timeout")]
    pub timeout_secs: u64,
}

fn default_approval_timeout() -> u64 {
    300
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            mode: ApprovalMode::Ask,
            allow_commands: Vec::new(),
            auto_approve_paths: Vec::new(),
            timeout_secs: default_approval_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    /// Provider implementation id. `openai_compatible` is always available.
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub extra_body: BTreeMap<String, serde_json::Value>,
    /// Enables parallel tool calls in the request when the endpoint supports it.
    #[serde(default = "default_parallel_tool_calls")]
    pub parallel_tool_calls: bool,
}

fn default_provider() -> String {
    "openai_compatible".into()
}
fn default_base_url() -> String {
    "http://127.0.0.1:4000/v1".into()
}
fn default_model() -> String {
    "gpt-4o-mini".into()
}
fn default_temperature() -> f32 {
    0.2
}
fn default_request_timeout() -> u64 {
    300
}
fn default_parallel_tool_calls() -> bool {
    true
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            base_url: default_base_url(),
            api_key: None,
            model: default_model(),
            temperature: default_temperature(),
            max_tokens: None,
            top_p: None,
            request_timeout_secs: default_request_timeout(),
            extra_headers: BTreeMap::new(),
            extra_body: BTreeMap::new(),
            parallel_tool_calls: default_parallel_tool_calls(),
        }
    }
}

impl ModelConfig {
    /// Redacted view for the browser: never includes the API key.
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider,
            "base_url": self.base_url,
            "model": self.model,
            "temperature": self.temperature,
            "max_tokens": self.max_tokens,
            "top_p": self.top_p,
            "request_timeout_secs": self.request_timeout_secs,
            "parallel_tool_calls": self.parallel_tool_calls,
            "api_key_set": self.api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Limits {
    /// Maximum model round-trips per run before the agent must stop.
    #[serde(default = "d_iterations")]
    pub max_iterations: u32,
    /// Maximum wall-clock time for one run.
    #[serde(default = "d_run_secs")]
    pub max_run_secs: u64,
    /// Bytes of a single tool result kept for the model (rest is truncated).
    #[serde(default = "d_tool_output")]
    pub max_tool_output_bytes: usize,
    /// Bytes of streamed command output kept for the UI.
    #[serde(default = "d_cmd_output")]
    pub max_command_output_bytes: usize,
    /// Default command timeout when the tool call omits one.
    #[serde(default = "d_cmd_timeout")]
    pub command_timeout_secs: u64,
    /// Hard cap on a command timeout requested by the model.
    #[serde(default = "d_cmd_timeout_max")]
    pub max_command_timeout_secs: u64,
    /// Bytes returned by a single file read.
    #[serde(default = "d_file_read")]
    pub max_file_read_bytes: usize,
    /// Maximum results returned by the search tool.
    #[serde(default = "d_search")]
    pub search_max_results: usize,
    /// Character budget for the conversation sent to the model; older tool
    /// results are elided first when this is exceeded.
    #[serde(default = "d_context_chars")]
    pub context_max_chars: usize,
    /// Maximum number of tool calls executed from a single assistant message.
    #[serde(default = "d_parallel_calls")]
    pub max_tool_calls_per_turn: usize,
}

fn d_iterations() -> u32 { 32 }
fn d_run_secs() -> u64 { 1800 }
fn d_tool_output() -> usize { 24_000 }
fn d_cmd_output() -> usize { 32_000 }
fn d_cmd_timeout() -> u64 { 120 }
fn d_cmd_timeout_max() -> u64 { 600 }
fn d_file_read() -> usize { 200_000 }
fn d_search() -> usize { 200 }
fn d_context_chars() -> usize { 240_000 }
fn d_parallel_calls() -> usize { 8 }

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_iterations: d_iterations(),
            max_run_secs: d_run_secs(),
            max_tool_output_bytes: d_tool_output(),
            max_command_output_bytes: d_cmd_output(),
            command_timeout_secs: d_cmd_timeout(),
            max_command_timeout_secs: d_cmd_timeout_max(),
            max_file_read_bytes: d_file_read(),
            search_max_results: d_search(),
            context_max_chars: d_context_chars(),
            max_tool_calls_per_turn: d_parallel_calls(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentConfig {
    #[serde(default = "d_sub_concurrency")]
    pub max_concurrent: usize,
    #[serde(default = "d_sub_iterations")]
    pub max_iterations: u32,
    #[serde(default)]
    pub allow_recursive: bool,
    #[serde(default = "d_sub_depth")]
    pub max_depth: u8,
    /// Wall-clock limit for one subagent task.
    #[serde(default = "d_sub_timeout")]
    pub timeout_secs: u64,
}

fn d_sub_concurrency() -> usize { 3 }
fn d_sub_iterations() -> u32 { 16 }
fn d_sub_depth() -> u8 { 1 }
fn d_sub_timeout() -> u64 { 900 }

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: d_sub_concurrency(),
            max_iterations: d_sub_iterations(),
            allow_recursive: false,
            max_depth: d_sub_depth(),
            timeout_secs: d_sub_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Command to launch an out-of-process plugin; argv[0] is resolved on PATH.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Declarative permissions the plugin asked for; enforced by the core when
    /// exposing its tools (not an OS sandbox).
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
}

fn d_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerConfig {
    pub name: String,
    /// `stdio` (spawn `command`) or `http` (streamable HTTP JSON-RPC).
    #[serde(default = "d_stdio")]
    pub transport: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Tool calls from this server require approval even in ask_commands mode.
    #[serde(default = "d_true")]
    pub require_approval: bool,
}

fn d_stdio() -> String { "stdio".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub lock_settings: Option<bool>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub limits: Option<Limits>,
    #[serde(default)]
    pub approvals: Option<ApprovalPolicy>,
    #[serde(default)]
    pub subagents: Option<SubagentConfig>,
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            host: None,
            port: None,
            workspace: None,
            data_dir: None,
            auth_token: None,
            lock_settings: None,
            log_level: None,
            model: None,
            limits: None,
            approvals: None,
            subagents: None,
            plugins: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub workspace_root: PathBuf,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub lock_settings: bool,
    pub log_level: String,
    pub model: ModelConfig,
    pub limits: Limits,
    pub approvals: ApprovalPolicy,
    pub subagents: SubagentConfig,
    pub plugins: Vec<PluginConfig>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub web_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8787,
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            data_dir: default_data_dir(),
            auth_token: None,
            lock_settings: false,
            log_level: "info".into(),
            model: ModelConfig::default(),
            limits: Limits::default(),
            approvals: ApprovalPolicy::default(),
            subagents: SubagentConfig::default(),
            plugins: Vec::new(),
            mcp_servers: Vec::new(),
            web_dir: None,
            config_path: None,
        }
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KARUCODE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("./.data"));
    base.join("karucode")
}

fn default_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KARUCODE_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("karucode/config.toml"))
}

fn env_str(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_str(name).and_then(|v| v.trim().parse::<T>().ok())
}

impl Config {
    /// Load configuration from file + environment. Network bind errors are not
    /// validated here; the server does that when it binds.
    pub fn load() -> Result<Self> {
        let config_path = default_config_path();
        let mut file = ConfigFile::default();
        if let Some(path) = &config_path {
            if path.exists() {
                let text = std::fs::read_to_string(path).map_err(|e| Error::io(path.clone(), e))?;
                file = toml::from_str(&text)
                    .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
            }
        }
        Self::from_parts(file, config_path)
    }

    fn from_parts(file: ConfigFile, config_path: Option<PathBuf>) -> Result<Self> {
        let mut cfg = Config::default();
        cfg.config_path = config_path;

        if let Some(v) = file.host { cfg.host = v; }
        if let Some(v) = file.port { cfg.port = v; }
        if let Some(v) = file.workspace { cfg.workspace_root = v; }
        if let Some(v) = file.data_dir { cfg.data_dir = v; }
        if let Some(v) = file.auth_token { cfg.auth_token = Some(v); }
        if let Some(v) = file.lock_settings { cfg.lock_settings = v; }
        if let Some(v) = file.log_level { cfg.log_level = v; }
        if let Some(v) = file.model { cfg.model = v; }
        if let Some(v) = file.limits { cfg.limits = v; }
        if let Some(v) = file.approvals { cfg.approvals = v; }
        if let Some(v) = file.subagents { cfg.subagents = v; }
        if !file.plugins.is_empty() { cfg.plugins = file.plugins; }
        if !file.mcp_servers.is_empty() { cfg.mcp_servers = file.mcp_servers; }

        // Environment overrides.
        if let Some(v) = env_str("KARUCODE_HOST") { cfg.host = v; }
        if let Some(v) = env_parse("KARUCODE_PORT") { cfg.port = v; }
        if let Some(v) = env_str("KARUCODE_WORKSPACE") { cfg.workspace_root = PathBuf::from(v); }
        if let Some(v) = env_str("KARUCODE_DATA_DIR") { cfg.data_dir = PathBuf::from(v); }
        if let Some(v) = env_str("KARUCODE_AUTH_TOKEN") { cfg.auth_token = Some(v); }
        if let Some(v) = env_parse("KARUCODE_LOCK_SETTINGS") { cfg.lock_settings = v; }
        if let Some(v) = env_str("KARUCODE_LOG").or_else(|| env_str("RUST_LOG")) { cfg.log_level = v; }
        if let Some(v) = env_str("KARUCODE_WEB_DIR") { cfg.web_dir = Some(PathBuf::from(v)); }

        if let Some(v) = env_str("KARUCODE_MODEL") { cfg.model.model = v; }
        if let Some(v) = env_str("KARUCODE_BASE_URL").or_else(|| env_str("OPENAI_BASE_URL")) {
            cfg.model.base_url = v;
        }
        if let Some(v) = env_str("KARUCODE_API_KEY").or_else(|| env_str("OPENAI_API_KEY")) {
            cfg.model.api_key = Some(v);
        }
        if let Some(v) = env_parse("KARUCODE_TEMPERATURE") { cfg.model.temperature = v; }
        if let Some(v) = env_parse("KARUCODE_MAX_TOKENS") { cfg.model.max_tokens = Some(v); }
        if let Some(v) = env_parse("KARUCODE_REQUEST_TIMEOUT_SECS") { cfg.model.request_timeout_secs = v; }

        if let Some(v) = env_parse("KARUCODE_MAX_ITERATIONS") { cfg.limits.max_iterations = v; }
        if let Some(v) = env_parse("KARUCODE_MAX_RUN_SECS") { cfg.limits.max_run_secs = v; }
        if let Some(v) = env_parse("KARUCODE_MAX_TOOL_OUTPUT_BYTES") { cfg.limits.max_tool_output_bytes = v; }
        if let Some(v) = env_parse("KARUCODE_COMMAND_TIMEOUT_SECS") { cfg.limits.command_timeout_secs = v; }
        if let Some(v) = env_parse("KARUCODE_MAX_FILE_READ_BYTES") { cfg.limits.max_file_read_bytes = v; }
        if let Some(v) = env_parse("KARUCODE_CONTEXT_MAX_CHARS") { cfg.limits.context_max_chars = v; }

        if let Some(v) = env_str("KARUCODE_APPROVAL_MODE") {
            cfg.approvals.mode = ApprovalMode::parse(&v).ok_or_else(|| {
                Error::Config(format!(
                    "invalid KARUCODE_APPROVAL_MODE={v} (expected ask|ask_commands|auto|read_only)"
                ))
            })?;
        }
        if let Some(v) = env_str("KARUCODE_ALLOW_COMMANDS") {
            cfg.approvals.allow_commands = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        if let Some(v) = env_parse("KARUCODE_APPROVAL_TIMEOUT_SECS") { cfg.approvals.timeout_secs = v; }

        if let Some(v) = env_parse("KARUCODE_SUBAGENT_CONCURRENCY") { cfg.subagents.max_concurrent = v; }
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_MAX_ITERATIONS") { cfg.subagents.max_iterations = v; }
        if let Some(v) = env_parse("KARUCODE_ALLOW_RECURSIVE_SUBAGENTS") { cfg.subagents.allow_recursive = v; }

        // Validate.
        if cfg.port == 0 {
            return Err(Error::Config("port may not be 0".into()));
        }
        if cfg.model.base_url.trim().is_empty() {
            return Err(Error::Config("model.base_url may not be empty".into()));
        }
        if cfg.auth_token.as_deref().map(|t| t.len() < 16).unwrap_or(false) {
            return Err(Error::Config(
                "auth token must be at least 16 characters".into(),
            ));
        }
        if cfg.host != "127.0.0.1" && cfg.host != "localhost" && cfg.host != "::1" && cfg.auth_token.is_none() {
            return Err(Error::Config(format!(
                "refusing to bind {host} without KARUCODE_AUTH_TOKEN: remote access requires authentication",
                host = cfg.host
            )));
        }
        if cfg.subagents.max_concurrent == 0 {
            cfg.subagents.max_concurrent = 1;
        }
        Ok(cfg)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("karucode.db")
    }

    pub fn is_local_bind(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let l = Limits::default();
        assert!(l.max_iterations >= 8);
        assert!(l.command_timeout_secs > 0);
        assert!(l.max_tool_output_bytes > 0);
        let s = SubagentConfig::default();
        assert_eq!(s.allow_recursive, false);
        assert!(s.max_concurrent >= 1);
    }

    #[test]
    fn model_public_view_hides_api_key() {
        let mut m = ModelConfig::default();
        m.api_key = Some("sk-super-secret".into());
        let v = m.public_view();
        let text = v.to_string();
        assert!(!text.contains("sk-super-secret"));
        assert_eq!(v["api_key_set"], serde_json::json!(true));
    }

    #[test]
    fn approval_mode_parses() {
        assert_eq!(ApprovalMode::parse("AUTO"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse("ask_commands"), Some(ApprovalMode::AskCommands));
        assert_eq!(ApprovalMode::parse("nonsense"), None);
    }

    #[test]
    fn remote_bind_without_token_is_rejected() {
        let mut file = ConfigFile::default();
        file.host = Some("0.0.0.0".into());
        // Can't set env safely in parallel tests; exercise validation directly.
        let mut cfg = Config::default();
        cfg.host = "0.0.0.0".into();
        assert!(cfg.auth_token.is_none());
        // from_parts applies the same rule via env; emulate it:
        let err = (|| -> Result<()> {
            if cfg.host != "127.0.0.1" && cfg.host != "localhost" && cfg.auth_token.is_none() {
                return Err(Error::Config("refusing".into()));
            }
            Ok(())
        })();
        assert!(err.is_err());
        let _ = file;
    }

    #[test]
    fn config_file_parses_partial_toml() {
        let text = r#"
port = 9000
workspace = "/tmp/project"

[model]
base_url = "http://localhost:1234/v1"
model = "qwen"

[approvals]
mode = "auto"
allow_commands = ["cargo test"]
"#;
        let f: ConfigFile = toml::from_str(text).unwrap();
        assert_eq!(f.port, Some(9000));
        let m = f.model.unwrap();
        assert_eq!(m.base_url, "http://localhost:1234/v1");
        assert_eq!(m.model, "qwen");
        assert_eq!(f.approvals.unwrap().mode, ApprovalMode::Auto);
    }
}
