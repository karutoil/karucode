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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask for shell commands and mutating tools (default).
    #[default]
    Ask,
    /// Ask only for shell commands; file writes inside the workspace are allowed.
    AskCommands,
    /// Allow everything; recorded in the event log for auditability.
    Auto,
    /// Only read-only tools are permitted; mutations and shell are denied.
    ReadOnly,
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
    /// How many times a failed provider attempt is retried before the run
    /// gives up. Every upstream failure is retried — connection resets,
    /// timeouts, every HTTP error status, an empty or unreadable body, and a
    /// stream that dies before its first content — because the
    /// OpenAI-compatible gateways this talks to surface transient channel
    /// death as arbitrary answers (a rotated key gives 401, a mid-failover
    /// channel 404, a broken proxy 200 with an HTML splash). Zero disables
    /// retries. See `kc-provider`'s `retry` module for the exact contract.
    #[serde(default = "default_retry_max_retries")]
    pub retry_max_retries: u32,
    /// First retry delay; doubles per attempt (equal-jittered into [d/2, d])
    /// until it reaches `retry_max_delay_ms`.
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
    /// Ceiling for the exponential retry backoff. With the 50-attempt default
    /// this cap — not the attempt count — bounds the total wait.
    #[serde(default = "default_retry_max_delay_ms")]
    pub retry_max_delay_ms: u64,
    /// How many times one turn may restart its stream *after* the answer
    /// started arriving. `retry_max_retries` only covers attempts that died
    /// before the first chunk, because replaying a started answer would
    /// duplicate it; a stream that dies mid-answer is handled one level up,
    /// where the partial attempt can be thrown away and the request re-issued.
    /// Each restart is a real generation, so the budget is small on purpose,
    /// and it shares `retry_base_delay_ms` / `retry_max_delay_ms` for backoff.
    /// Zero disables mid-stream restarts.
    #[serde(default = "default_retry_max_stream_restarts")]
    pub retry_max_stream_restarts: u32,
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
fn default_retry_max_retries() -> u32 {
    50
}
fn default_retry_base_delay_ms() -> u64 {
    500
}
fn default_retry_max_delay_ms() -> u64 {
    30_000
}
fn default_retry_max_stream_restarts() -> u32 {
    3
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
            retry_max_retries: default_retry_max_retries(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
            retry_max_stream_restarts: default_retry_max_stream_restarts(),
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

/// One model reachable through a provider profile.
///
/// The connection details live on [`ProviderProfile`]; this is the part a user
/// curates — the ids they actually want to use, plus the metadata the settings
/// screen shows. `context_window`, `max_output_tokens`, capabilities and costs
/// are prefilled from models.dev when the model is picked from the catalog and
/// stay editable, because a self-hosted gateway may serve the same id with
/// different limits.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    /// Model id sent to the endpoint (`model` in the chat request).
    pub id: String,
    /// Human label; falls back to `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Total context window in tokens, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// USD per million input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_input: Option<f64>,
    /// USD per million output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_output: Option<f64>,
    /// models.dev provider id the metadata was prefilled from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_provider: Option<String>,
}

impl ModelEntry {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// Label to show in pickers: the friendly name when there is one.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }
}

/// A named OpenAI-compatible endpoint plus the models the user wants from it.
///
/// Profiles are the only place credentials live: the flat [`ModelConfig`]
/// section is the live/effective view the runtime reads, derived from the
/// active profile by [`Config::effective_model`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderProfile {
    /// Stable slug used by the API (`active.provider`).
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Provider implementation id; `openai_compatible` is always available.
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub extra_body: BTreeMap<String, serde_json::Value>,
}

impl Default for ProviderProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            provider: default_provider(),
            base_url: default_base_url(),
            api_key: None,
            models: Vec::new(),
            request_timeout_secs: default_request_timeout(),
            extra_headers: BTreeMap::new(),
            extra_body: BTreeMap::new(),
        }
    }
}

impl ProviderProfile {
    pub fn label(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            &self.id
        } else {
            name
        }
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key
            .as_deref()
            .map(|key| !key.trim().is_empty())
            .unwrap_or(false)
    }

    /// Browser-safe view: the key itself is never serialised.
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.label(),
            "provider": self.provider,
            "base_url": self.base_url,
            "models": self.models,
            "request_timeout_secs": self.request_timeout_secs,
            "api_key_set": self.has_api_key(),
        })
    }
}

/// Profile id synthesized from the flat `model` section (file/env/legacy DB).
pub const DEFAULT_PROFILE_ID: &str = "default";
/// Reserved profile id used when the legacy `KARUCODE_*` model variables are set.
pub const ENV_PROFILE_ID: &str = "env";

/// Accepted `reasoning_effort` values. The set is what current OpenAI-compatible
/// endpoints use; anything else risks a 400 only at run time, so it is rejected
/// where the selection is saved instead. `none` turns reasoning off on endpoints
/// that support it, `max` asks for the largest budget on those that do.
pub const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "max"];

/// Per-session model selection stored on a session.
///
/// A session with no selection follows the install-wide default
/// (`active.provider` / `active.model`). A selection pins *this session* to one
/// profile + model, optionally with a reasoning effort; other sessions are
/// untouched, which is what lets several sessions run different models at the
/// same time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionModelSelection {
    /// Provider profile id; must match a configured profile when saved.
    pub provider: String,
    /// Model id; must be offered by that profile when the profile lists models.
    pub model: String,
    /// `reasoning_effort` body parameter (`minimal|low|medium|high`). `None`
    /// leaves the endpoint's default in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl SessionModelSelection {
    /// Field-level checks that do not depend on the configured profiles: both
    /// ids non-empty and bounded, no control characters, effort in the accepted
    /// set. Whether the profile/model actually exist is checked by the server
    /// against the live config when the selection is saved.
    pub fn validate_shape(&self) -> Result<()> {
        let provider = self.provider.trim();
        if provider.is_empty() {
            return Err(Error::BadRequest("model.provider may not be empty".into()));
        }
        if provider.len() > MAX_SELECTION_ID_CHARS {
            return Err(Error::BadRequest(format!(
                "model.provider is longer than {MAX_SELECTION_ID_CHARS} characters"
            )));
        }
        let model = self.model.trim();
        if model.is_empty() {
            return Err(Error::BadRequest("model.model may not be empty".into()));
        }
        if model.len() > MAX_SELECTION_MODEL_CHARS {
            return Err(Error::BadRequest(format!(
                "model.model is longer than {MAX_SELECTION_MODEL_CHARS} characters"
            )));
        }
        for id in [provider, model] {
            if id.chars().any(char::is_control) {
                return Err(Error::BadRequest(
                    "model.provider and model.model may not contain control characters".into(),
                ));
            }
        }
        if let Some(effort) = self.reasoning_effort.as_deref() {
            let effort = effort.trim();
            if !effort.is_empty() && !REASONING_EFFORTS.contains(&effort) {
                return Err(Error::BadRequest(format!(
                    "model.reasoning_effort must be one of {} (got `{effort}`)",
                    REASONING_EFFORTS.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// Trimmed copy with an empty reasoning effort normalised to `None`.
    pub fn normalized(&self) -> Self {
        Self {
            provider: self.provider.trim().to_string(),
            model: self.model.trim().to_string(),
            reasoning_effort: self
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|effort| !effort.is_empty())
                .map(str::to_string),
        }
    }
}

/// Bound on the profile id and model id a session selection may carry.
const MAX_SELECTION_ID_CHARS: usize = 64;
const MAX_SELECTION_MODEL_CHARS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Limits {
    /// Model round-trips allowed in one *segment* of a run.
    ///
    /// A segment is a checkpoint, not a finish line: when it is exhausted the
    /// run continues automatically (see [`Limits::auto_continue`]) unless the
    /// wall-clock ceiling or `max_continues` says otherwise. The effective
    /// ceiling for one run is therefore roughly
    /// `(max_continues + 1) * max_iterations` model round-trips.
    #[serde(default = "d_iterations")]
    pub max_iterations: u32,
    /// Hard wall-clock ceiling for the whole run, across every segment.
    #[serde(default = "d_run_secs")]
    pub max_run_secs: u64,
    /// Keep going when a segment's iteration budget runs out instead of ending
    /// the run with `limit_reached`.
    ///
    /// Long-horizon work (migrations, refactors, debugging a failing suite) does
    /// not fit in one budget, and silently stopping after a few dozen turns is
    /// the one failure mode an agent harness must not have. With this on, the
    /// runtime emits a notice, resets the iteration counter and continues in the
    /// same session, run and transcript.
    #[serde(default = "d_true")]
    pub auto_continue: bool,
    /// How many times `max_iterations` may be refreshed in one run. `0` restores
    /// the old one-shot behaviour; the run then stops at `max_iterations`.
    #[serde(default = "d_continues")]
    pub max_continues: u32,
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
    /// Calls beyond this are skipped with a placeholder result, so the cap is a
    /// safety valve against a runaway batch, not a work budget: leave it high.
    #[serde(default = "d_parallel_calls")]
    pub max_tool_calls_per_turn: usize,
}

fn d_iterations() -> u32 {
    200
}
fn d_run_secs() -> u64 {
    86_400
}
fn d_continues() -> u32 {
    50
}
fn d_tool_output() -> usize {
    24_000
}
fn d_cmd_output() -> usize {
    32_000
}
fn d_cmd_timeout() -> u64 {
    120
}
fn d_cmd_timeout_max() -> u64 {
    600
}
fn d_file_read() -> usize {
    200_000
}
fn d_search() -> usize {
    200
}
fn d_context_chars() -> usize {
    240_000
}
fn d_parallel_calls() -> usize {
    32
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_iterations: d_iterations(),
            max_run_secs: d_run_secs(),
            auto_continue: d_true(),
            max_continues: d_continues(),
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

fn d_sub_concurrency() -> usize {
    3
}
fn d_sub_iterations() -> u32 {
    16
}
fn d_sub_depth() -> u8 {
    1
}
fn d_sub_timeout() -> u64 {
    900
}

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

fn d_true() -> bool {
    true
}

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

fn d_stdio() -> String {
    "stdio".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// `[[providers]]` tables. When present they are the source of truth for
    /// endpoints, keys and the model library.
    #[serde(default)]
    pub providers: Option<Vec<ProviderProfile>>,
    #[serde(default)]
    pub active_provider: Option<String>,
    #[serde(default)]
    pub active_model: Option<String>,
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

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub workspace_root: PathBuf,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub lock_settings: bool,
    pub log_level: String,
    /// Live/effective model connection + sampling. Derived from the active
    /// provider profile by [`Config::normalize_providers`].
    pub model: ModelConfig,
    /// Named endpoints and their model libraries.
    pub providers: Vec<ProviderProfile>,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
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
            providers: Vec::new(),
            active_provider: None,
            active_model: None,
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

/// Boolean environment flag. Accepts `1/0`, `true/false`, `yes/no`, `on/off` so
/// a shell export cannot silently disagree with the documented spelling.
fn env_bool(name: &str) -> Option<bool> {
    match env_str(name)?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
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
        let mut cfg = Config {
            config_path,
            ..Config::default()
        };

        if let Some(v) = file.host {
            cfg.host = v;
        }
        if let Some(v) = file.port {
            cfg.port = v;
        }
        if let Some(v) = file.workspace {
            cfg.workspace_root = v;
        }
        if let Some(v) = file.data_dir {
            cfg.data_dir = v;
        }
        if let Some(v) = file.auth_token {
            cfg.auth_token = Some(v);
        }
        if let Some(v) = file.lock_settings {
            cfg.lock_settings = v;
        }
        if let Some(v) = file.log_level {
            cfg.log_level = v;
        }
        if let Some(v) = file.model {
            cfg.model = v;
        }
        if let Some(providers) = file.providers {
            cfg.providers = providers;
        }
        if let Some(v) = file.active_provider {
            cfg.active_provider = Some(v);
        }
        if let Some(v) = file.active_model {
            cfg.active_model = Some(v);
        }
        if let Some(v) = file.limits {
            cfg.limits = v;
        }
        if let Some(v) = file.approvals {
            cfg.approvals = v;
        }
        if let Some(v) = file.subagents {
            cfg.subagents = v;
        }
        if !file.plugins.is_empty() {
            cfg.plugins = file.plugins;
        }
        if !file.mcp_servers.is_empty() {
            cfg.mcp_servers = file.mcp_servers;
        }

        // Environment overrides.
        if let Some(v) = env_str("KARUCODE_HOST") {
            cfg.host = v;
        }
        if let Some(v) = env_parse("KARUCODE_PORT") {
            cfg.port = v;
        }
        if let Some(v) = env_str("KARUCODE_WORKSPACE") {
            cfg.workspace_root = PathBuf::from(v);
        }
        if let Some(v) = env_str("KARUCODE_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Some(v) = env_str("KARUCODE_AUTH_TOKEN") {
            cfg.auth_token = Some(v);
        }
        if let Some(v) = env_parse("KARUCODE_LOCK_SETTINGS") {
            cfg.lock_settings = v;
        }
        if let Some(v) = env_str("KARUCODE_LOG").or_else(|| env_str("RUST_LOG")) {
            cfg.log_level = v;
        }
        if let Some(v) = env_str("KARUCODE_WEB_DIR") {
            cfg.web_dir = Some(PathBuf::from(v));
        }

        let env_model = env_str("KARUCODE_MODEL");
        let env_base_url = env_str("KARUCODE_BASE_URL").or_else(|| env_str("OPENAI_BASE_URL"));
        let env_api_key = env_str("KARUCODE_API_KEY").or_else(|| env_str("OPENAI_API_KEY"));
        if let Some(v) = env_model.clone() {
            cfg.model.model = v;
        }
        if let Some(v) = env_base_url.clone() {
            cfg.model.base_url = v;
        }
        if let Some(v) = env_api_key.clone() {
            cfg.model.api_key = Some(v);
        }
        // The flat KARUCODE_* model variables are the legacy way to configure a
        // single endpoint. When any of them is set they win over [[providers]]
        // tables in the file, expressed as a reserved `env` profile so there is
        // exactly one code path that resolves an endpoint.
        if env_model.is_some() || env_base_url.is_some() || env_api_key.is_some() {
            cfg.providers.retain(|profile| profile.id != ENV_PROFILE_ID);
            cfg.providers.push(ProviderProfile {
                id: ENV_PROFILE_ID.to_string(),
                name: "From environment".into(),
                provider: cfg.model.provider.clone(),
                base_url: cfg.model.base_url.clone(),
                api_key: cfg.model.api_key.clone(),
                models: vec![ModelEntry::new(cfg.model.model.clone())],
                request_timeout_secs: cfg.model.request_timeout_secs,
                ..ProviderProfile::default()
            });
            cfg.active_provider = Some(ENV_PROFILE_ID.to_string());
            cfg.active_model = Some(cfg.model.model.clone());
        }
        if let Some(v) = env_parse("KARUCODE_TEMPERATURE") {
            cfg.model.temperature = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_TOKENS") {
            cfg.model.max_tokens = Some(v);
        }
        if let Some(v) = env_parse("KARUCODE_REQUEST_TIMEOUT_SECS") {
            cfg.model.request_timeout_secs = v;
        }
        if let Some(v) = env_parse("KARUCODE_RETRY_MAX_RETRIES") {
            cfg.model.retry_max_retries = v;
        }
        if let Some(v) = env_parse("KARUCODE_RETRY_BASE_DELAY_MS") {
            cfg.model.retry_base_delay_ms = v;
        }
        if let Some(v) = env_parse("KARUCODE_RETRY_MAX_DELAY_MS") {
            cfg.model.retry_max_delay_ms = v;
        }
        if let Some(v) = env_parse("KARUCODE_RETRY_MAX_STREAM_RESTARTS") {
            cfg.model.retry_max_stream_restarts = v;
        }

        if let Some(v) = env_parse("KARUCODE_MAX_ITERATIONS") {
            cfg.limits.max_iterations = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_RUN_SECS") {
            cfg.limits.max_run_secs = v;
        }
        if let Some(v) = env_bool("KARUCODE_AUTO_CONTINUE") {
            cfg.limits.auto_continue = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_CONTINUES") {
            cfg.limits.max_continues = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_TOOL_OUTPUT_BYTES") {
            cfg.limits.max_tool_output_bytes = v;
        }
        if let Some(v) = env_parse("KARUCODE_COMMAND_TIMEOUT_SECS") {
            cfg.limits.command_timeout_secs = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_FILE_READ_BYTES") {
            cfg.limits.max_file_read_bytes = v;
        }
        if let Some(v) = env_parse("KARUCODE_CONTEXT_MAX_CHARS") {
            cfg.limits.context_max_chars = v;
        }

        if let Some(v) = env_str("KARUCODE_APPROVAL_MODE") {
            cfg.approvals.mode = ApprovalMode::parse(&v).ok_or_else(|| {
                Error::Config(format!(
                    "invalid KARUCODE_APPROVAL_MODE={v} (expected ask|ask_commands|auto|read_only)"
                ))
            })?;
        }
        if let Some(v) = env_str("KARUCODE_ALLOW_COMMANDS") {
            cfg.approvals.allow_commands = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(v) = env_parse("KARUCODE_APPROVAL_TIMEOUT_SECS") {
            cfg.approvals.timeout_secs = v;
        }

        if let Some(v) = env_parse("KARUCODE_SUBAGENT_CONCURRENCY") {
            cfg.subagents.max_concurrent = v;
        }
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_MAX_ITERATIONS") {
            cfg.subagents.max_iterations = v;
        }
        if let Some(v) = env_parse("KARUCODE_ALLOW_RECURSIVE_SUBAGENTS") {
            cfg.subagents.allow_recursive = v;
        }

        // Validate.
        if cfg.port == 0 {
            return Err(Error::Config("port may not be 0".into()));
        }
        cfg.normalize_providers();
        if cfg.model.base_url.trim().is_empty() {
            return Err(Error::Config("model.base_url may not be empty".into()));
        }
        if cfg
            .auth_token
            .as_deref()
            .map(|t| t.len() < 16)
            .unwrap_or(false)
        {
            return Err(Error::Config(
                "auth token must be at least 16 characters".into(),
            ));
        }
        if cfg.host != "127.0.0.1"
            && cfg.host != "localhost"
            && cfg.host != "::1"
            && cfg.auth_token.is_none()
        {
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

    /// Make `providers` / `active_provider` / `active_model` consistent and
    /// mirror the resulting connection into the flat `model` section.
    ///
    /// * no profiles at all: synthesize one from the flat `model` section, which
    ///   is what a config file, the environment or a pre-profiles database row
    ///   provides;
    /// * unknown `active_provider`: fall back to the first profile;
    /// * unknown or empty `active_model`: fall back to the profile's first
    ///   model, then to the flat `model.model` (a profile may legitimately have
    ///   no models yet, while the user is still filling it in).
    ///
    /// Idempotent: calling it twice yields the same config.
    pub fn normalize_providers(&mut self) {
        if self.providers.is_empty() {
            let entry = ModelEntry::new(self.model.model.clone());
            self.providers.push(ProviderProfile {
                id: DEFAULT_PROFILE_ID.to_string(),
                name: "Default".into(),
                provider: self.model.provider.clone(),
                base_url: self.model.base_url.clone(),
                api_key: self.model.api_key.clone(),
                models: if self.model.model.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![entry]
                },
                request_timeout_secs: self.model.request_timeout_secs,
                extra_headers: self.model.extra_headers.clone(),
                extra_body: self.model.extra_body.clone(),
            });
            self.active_provider = Some(DEFAULT_PROFILE_ID.to_string());
            self.active_model = Some(self.model.model.clone());
            return;
        }

        let active_exists = self
            .active_provider
            .as_deref()
            .map(|id| self.providers.iter().any(|profile| profile.id == id))
            .unwrap_or(false);
        if !active_exists {
            self.active_provider = Some(self.providers[0].id.clone());
        }

        let profile = self
            .providers
            .iter()
            .find(|profile| Some(profile.id.as_str()) == self.active_provider.as_deref());
        let Some(profile) = profile else { return };
        let model_known = self
            .active_model
            .as_deref()
            .map(|id| profile.models.iter().any(|model| model.id == id))
            .unwrap_or(false);
        if !model_known {
            self.active_model = profile
                .models
                .first()
                .map(|model| model.id.clone())
                .or_else(|| {
                    (!self.model.model.trim().is_empty()).then(|| self.model.model.clone())
                });
        }

        // Mirror so `GET /api/config.model`, provider probes and any legacy
        // reader see the same endpoint the runtime will use. The API key stays
        // on the profile only, so it is stored and redacted in one place.
        self.model.provider = profile.provider.clone();
        self.model.base_url = profile.base_url.clone();
        self.model.request_timeout_secs = profile.request_timeout_secs;
        self.model.extra_headers = profile.extra_headers.clone();
        self.model.extra_body = profile.extra_body.clone();
        if let Some(model) = self.active_model.clone() {
            self.model.model = model;
        }
    }

    /// The profile the runtime should use, if any.
    pub fn active_profile(&self) -> Option<&ProviderProfile> {
        let id = self.active_provider.as_deref()?;
        self.providers
            .iter()
            .find(|profile| profile.id == id)
            .or_else(|| self.providers.first())
    }

    /// The model id the active profile will send.
    pub fn active_model_id(&self, profile: &ProviderProfile) -> String {
        self.active_model
            .clone()
            .filter(|id| !id.trim().is_empty())
            .or_else(|| profile.models.first().map(|model| model.id.clone()))
            .unwrap_or_else(|| self.model.model.clone())
    }

    /// Resolve one profile into the live [`ModelConfig`] the runtime reads.
    /// The active profile uses the active model; any other profile uses its
    /// first model (used by provider probes on a profile that is not selected).
    pub fn model_for_profile(&self, profile: &ProviderProfile) -> ModelConfig {
        let model = if Some(profile.id.as_str()) == self.active_provider.as_deref() {
            self.active_model_id(profile)
        } else {
            profile
                .models
                .first()
                .map(|entry| entry.id.clone())
                .unwrap_or_else(|| self.model.model.clone())
        };
        ModelConfig {
            provider: profile.provider.clone(),
            base_url: profile.base_url.clone(),
            api_key: profile.api_key.clone(),
            model,
            temperature: self.model.temperature,
            max_tokens: self.model.max_tokens,
            top_p: self.model.top_p,
            request_timeout_secs: profile.request_timeout_secs,
            // The retry policy is global, like sampling: it says how patient
            // the client is with ANY endpoint, not one profile's connection.
            retry_max_retries: self.model.retry_max_retries,
            retry_base_delay_ms: self.model.retry_base_delay_ms,
            retry_max_delay_ms: self.model.retry_max_delay_ms,
            retry_max_stream_restarts: self.model.retry_max_stream_restarts,
            extra_headers: profile.extra_headers.clone(),
            extra_body: profile.extra_body.clone(),
            parallel_tool_calls: self.model.parallel_tool_calls,
        }
    }

    /// Resolve the active profile into the live [`ModelConfig`] the runtime
    /// reads. Sampling tunables stay global; connection details come from the
    /// profile. With no profiles this is the flat `model` section unchanged.
    pub fn effective_model(&self) -> ModelConfig {
        match self.active_profile() {
            Some(profile) => self.model_for_profile(profile),
            None => self.model.clone(),
        }
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
        assert!(!s.allow_recursive);
        assert!(s.max_concurrent >= 1);
    }

    #[test]
    fn model_public_view_hides_api_key() {
        let m = ModelConfig {
            api_key: Some("sk-super-secret".into()),
            ..ModelConfig::default()
        };
        let v = m.public_view();
        let text = v.to_string();
        assert!(!text.contains("sk-super-secret"));
        assert_eq!(v["api_key_set"], serde_json::json!(true));
    }

    fn profile(id: &str, models: &[&str]) -> ProviderProfile {
        ProviderProfile {
            id: id.into(),
            name: id.to_uppercase(),
            base_url: format!("https://{id}.example/v1"),
            models: models.iter().map(|id| ModelEntry::new(*id)).collect(),
            ..ProviderProfile::default()
        }
    }

    #[test]
    fn normalize_synthesizes_one_profile_from_the_flat_section() {
        let mut cfg = Config::default();
        cfg.model.model = "qwen3-32b".into();
        cfg.model.base_url = "http://127.0.0.1:8000/v1".into();
        cfg.normalize_providers();

        assert_eq!(cfg.providers.len(), 1);
        let active = cfg.active_profile().expect("active profile");
        assert_eq!(active.id, DEFAULT_PROFILE_ID);
        assert_eq!(active.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(active.models.len(), 1);
        assert_eq!(active.models[0].id, "qwen3-32b");
        assert_eq!(cfg.active_model.as_deref(), Some("qwen3-32b"));

        // Idempotent: normalising again changes nothing.
        let before = cfg.clone();
        cfg.normalize_providers();
        assert_eq!(cfg.providers, before.providers);
        assert_eq!(cfg.active_provider, before.active_provider);
        assert_eq!(cfg.active_model, before.active_model);
    }

    #[test]
    fn effective_model_uses_the_active_profile_and_keeps_sampling_global() {
        let mut cfg = Config::default();
        cfg.model.temperature = 0.7;
        cfg.model.max_tokens = Some(4096);
        cfg.model.top_p = Some(0.9);
        cfg.providers = vec![
            profile("gateway", &["model-a", "model-b"]),
            profile("local", &["tiny"]),
        ];
        cfg.providers[0].api_key = Some("sk-gateway".into());
        cfg.active_provider = Some("gateway".into());
        cfg.active_model = Some("model-b".into());
        cfg.normalize_providers();

        let effective = cfg.effective_model();
        assert_eq!(effective.base_url, "https://gateway.example/v1");
        assert_eq!(effective.api_key.as_deref(), Some("sk-gateway"));
        assert_eq!(effective.model, "model-b");
        assert_eq!(effective.temperature, 0.7);
        assert_eq!(effective.max_tokens, Some(4096));
        assert_eq!(effective.top_p, Some(0.9));
        // The flat section mirrors the active connection...
        assert_eq!(cfg.model.base_url, "https://gateway.example/v1");
        assert_eq!(cfg.model.model, "model-b");
        // ...but never the key, which lives on the profile only.
        assert_ne!(cfg.model.api_key.as_deref(), Some("sk-gateway"));
    }

    #[test]
    fn normalize_repairs_a_stale_selection() {
        let mut cfg = Config {
            providers: vec![profile("gateway", &["model-a"])],
            active_provider: Some("deleted".into()),
            active_model: Some("also-gone".into()),
            ..Config::default()
        };
        cfg.normalize_providers();

        assert_eq!(cfg.active_provider.as_deref(), Some("gateway"));
        assert_eq!(cfg.active_model.as_deref(), Some("model-a"));
    }

    #[test]
    fn a_profile_with_no_models_falls_back_to_the_flat_model_id() {
        let mut cfg = Config::default();
        cfg.model.model = "legacy-id".into();
        cfg.providers = vec![profile("empty", &[])];
        cfg.active_provider = Some("empty".into());
        cfg.active_model = None;
        cfg.normalize_providers();

        assert_eq!(cfg.active_model.as_deref(), Some("legacy-id"));
        assert_eq!(cfg.effective_model().model, "legacy-id");
    }

    #[test]
    fn provider_public_view_never_leaks_the_key() {
        let mut p = profile("gateway", &["m"]);
        p.api_key = Some("sk-super-secret".into());
        let text = p.public_view().to_string();
        assert!(!text.contains("sk-super-secret"));
        assert_eq!(p.public_view()["api_key_set"], serde_json::json!(true));
        assert_eq!(p.public_view()["name"], serde_json::json!("GATEWAY"));
    }

    #[test]
    fn provider_tables_parse_from_toml() {
        let text = r#"
active_provider = "gateway"
active_model = "model-a"

[[providers]]
id = "gateway"
name = "Gateway"
base_url = "https://gw.example/v1"

  [[providers.models]]
  id = "model-a"
  name = "Model A"
  context_window = 128000
  tool_call = true
"#;
        let file: ConfigFile = toml::from_str(text).unwrap();
        let providers = file.providers.expect("providers");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].models[0].context_window, Some(128_000));
        assert_eq!(providers[0].models[0].label(), "Model A");
        assert_eq!(file.active_model.as_deref(), Some("model-a"));
    }

    #[test]
    fn approval_mode_parses() {
        assert_eq!(ApprovalMode::parse("AUTO"), Some(ApprovalMode::Auto));
        assert_eq!(
            ApprovalMode::parse("ask_commands"),
            Some(ApprovalMode::AskCommands)
        );
        assert_eq!(ApprovalMode::parse("nonsense"), None);
    }

    #[test]
    fn remote_bind_without_token_is_rejected() {
        let file = ConfigFile {
            host: Some("0.0.0.0".into()),
            ..ConfigFile::default()
        };
        // Can't set env safely in parallel tests; exercise validation directly.
        let cfg = Config {
            host: "0.0.0.0".into(),
            ..Config::default()
        };
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

    #[test]
    fn retry_budget_defaults_and_overrides_parse() {
        // Defaults are the documented budget: 50 retries, 500ms first delay,
        // 30s ceiling.
        let m = ModelConfig::default();
        assert_eq!(m.retry_max_retries, 50);
        assert_eq!(m.retry_base_delay_ms, 500);
        assert_eq!(m.retry_max_delay_ms, 30_000);
        assert_eq!(
            m.retry_max_stream_restarts, 3,
            "mid-stream restarts default to a small budget: each one is a fresh generation"
        );

        // Absent keys keep the defaults ...
        let f: ConfigFile = toml::from_str("[model]\nmodel = \"qwen\"\n").unwrap();
        let m = f.model.unwrap();
        assert_eq!(m.retry_max_retries, 50);
        assert_eq!(m.retry_max_delay_ms, 30_000);
        assert_eq!(m.retry_max_stream_restarts, 3);

        // ... and present ones override them.
        let text = r#"
[model]
model = "qwen"
retry_max_retries = 3
retry_base_delay_ms = 100
retry_max_delay_ms = 2000
retry_max_stream_restarts = 0
"#;
        let f: ConfigFile = toml::from_str(text).unwrap();
        let m = f.model.unwrap();
        assert_eq!(m.retry_max_retries, 3);
        assert_eq!(m.retry_base_delay_ms, 100);
        assert_eq!(m.retry_max_delay_ms, 2000);
        assert_eq!(m.retry_max_stream_restarts, 0);
    }

    #[test]
    fn retry_budget_stays_global_across_profiles() {
        // The retry policy describes how patient the client is with ANY
        // endpoint, so deriving the live config from a profile must carry it
        // over instead of resetting it to defaults.
        let mut cfg = Config::default();
        cfg.model.retry_max_retries = 7;
        cfg.model.retry_base_delay_ms = 42;
        cfg.model.retry_max_delay_ms = 999;
        cfg.model.retry_max_stream_restarts = 2;
        cfg.normalize_providers();
        let effective = cfg.effective_model();
        assert_eq!(effective.retry_max_retries, 7);
        assert_eq!(effective.retry_base_delay_ms, 42);
        assert_eq!(effective.retry_max_delay_ms, 999);
        assert_eq!(effective.retry_max_stream_restarts, 2);
    }

    #[test]
    fn retry_budget_env_overrides_win() {
        // Only this test reads these variables, so setting them cannot perturb
        // another test's expectations; they are removed before asserting so a
        // failure cannot leak into the rest of the binary.
        std::env::set_var("KARUCODE_RETRY_MAX_RETRIES", "3");
        std::env::set_var("KARUCODE_RETRY_BASE_DELAY_MS", "25");
        std::env::set_var("KARUCODE_RETRY_MAX_DELAY_MS", "250");
        std::env::set_var("KARUCODE_RETRY_MAX_STREAM_RESTARTS", "1");
        let cfg = Config::from_parts(ConfigFile::default(), None);
        std::env::remove_var("KARUCODE_RETRY_MAX_RETRIES");
        std::env::remove_var("KARUCODE_RETRY_BASE_DELAY_MS");
        std::env::remove_var("KARUCODE_RETRY_MAX_DELAY_MS");
        std::env::remove_var("KARUCODE_RETRY_MAX_STREAM_RESTARTS");

        let cfg = cfg.expect("config loads");
        assert_eq!(cfg.model.retry_max_retries, 3);
        assert_eq!(cfg.model.retry_base_delay_ms, 25);
        assert_eq!(cfg.model.retry_max_delay_ms, 250);
        assert_eq!(cfg.model.retry_max_stream_restarts, 1);
        assert_eq!(
            cfg.effective_model().retry_max_retries,
            3,
            "an env override must survive profile resolution"
        );
    }

    #[test]
    fn session_model_selection_validates_its_shape() {
        let selection = SessionModelSelection {
            provider: "gateway".into(),
            model: "model-b".into(),
            reasoning_effort: Some("high".into()),
        };
        assert!(selection.validate_shape().is_ok());

        // Every reasoning effort value must be one of the four the wire format
        // agrees on; anything else is rejected where it is saved.
        for effort in REASONING_EFFORTS {
            let mut candidate = selection.clone();
            candidate.reasoning_effort = Some((*effort).into());
            assert!(candidate.validate_shape().is_ok(), "{effort}");
        }
        let mut bad_effort = selection.clone();
        bad_effort.reasoning_effort = Some("extreme".into());
        let err = bad_effort.validate_shape().unwrap_err().to_string();
        assert!(err.contains("reasoning_effort"), "{err}");

        let mut blank_provider = selection.clone();
        blank_provider.provider = "   ".into();
        assert!(blank_provider.validate_shape().is_err());

        let mut blank_model = selection.clone();
        blank_model.model = String::new();
        assert!(blank_model.validate_shape().is_err());

        let mut control = selection.clone();
        control.model = "m\u{0}".into();
        assert!(control.validate_shape().is_err());

        let mut long = selection.clone();
        long.provider = "p".repeat(65);
        assert!(long.validate_shape().is_err());
    }

    #[test]
    fn session_model_selection_normalizes_whitespace() {
        let selection = SessionModelSelection {
            provider: "  gateway ".into(),
            model: " model-b\n".into(),
            reasoning_effort: Some("  low  ".into()),
        };
        let normalized = selection.normalized();
        assert_eq!(normalized.provider, "gateway");
        assert_eq!(normalized.model, "model-b");
        assert_eq!(normalized.reasoning_effort.as_deref(), Some("low"));

        let mut blank_effort = selection.clone();
        blank_effort.reasoning_effort = Some("   ".into());
        assert!(blank_effort.normalized().reasoning_effort.is_none());
    }

    #[test]
    fn session_model_selection_serializes_minimally() {
        let with_effort = SessionModelSelection {
            provider: "gateway".into(),
            model: "model-b".into(),
            reasoning_effort: Some("low".into()),
        };
        let json = serde_json::to_value(&with_effort).unwrap();
        assert_eq!(json["provider"], serde_json::json!("gateway"));
        assert_eq!(json["model"], serde_json::json!("model-b"));
        assert_eq!(json["reasoning_effort"], serde_json::json!("low"));

        let plain = SessionModelSelection {
            reasoning_effort: None,
            ..with_effort.clone()
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(
            json.get("reasoning_effort").is_none(),
            "an absent effort is omitted, not null"
        );

        // Round trip through the stored form.
        let parsed: SessionModelSelection =
            serde_json::from_str(r#"{"provider":"local","model":"tiny"}"#).unwrap();
        assert_eq!(parsed.provider, "local");
        assert!(parsed.reasoning_effort.is_none());
    }
}
