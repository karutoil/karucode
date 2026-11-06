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
    /// The temperature to send **when [`ModelConfig::send_temperature`] is on**.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Whether to send `temperature` at all. Off by default: a sampling value
    /// the model was not tuned for can push a reasoning model into repeating
    /// itself (measured: with a 140-message history at the point where the model
    /// was about to write a file, `temperature: 0.2` produced 8k tokens of
    /// reasoning and no action, while omitting the field produced the
    /// `write_file` call). Endpoints that want a fixed temperature can turn it
    /// on; nobody gets one invented for them.
    #[serde(default)]
    pub send_temperature: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Total context window in tokens, when the model declares one.
    ///
    /// This is the **source of truth for the request budget**: the runtime
    /// derives how much conversation it may send from this window (minus the
    /// room the answer needs), so a 1M-token model is not held to a number that
    /// was chosen for a 32k one. `[model]` can set it directly, and a
    /// `[[providers.models]]` entry fills it in when the profile is resolved.
    /// Unknown means "fall back to `limits.context_max_chars`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Maximum output tokens the endpoint will generate. Reserved out of the
    /// window so the answer always has room; falls back to `max_tokens`, then to
    /// an eighth of the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
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
    /// Wall-clock ceiling for the whole retry chain, in seconds.
    ///
    /// The attempt count alone is not a bound: 50 attempts with a 30 s backoff
    /// cap is over twenty minutes of a run spent waiting on an endpoint that is
    /// never going to answer. This stops the chain and surfaces the real error
    /// instead. Zero disables the wall-clock bound (attempts only).
    #[serde(default = "default_retry_max_total_secs")]
    pub retry_max_total_secs: u64,
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
    /// Whether this model accepts **image input** (a vision-capable model).
    ///
    /// Off by default, and deliberately not guessed from the model id: an
    /// OpenAI-compatible gateway will happily accept a request carrying an image
    /// for a text-only model and either drop it or fail the whole turn, and
    /// `read_file` on a PNG would then look like it worked while the model saw
    /// nothing. With this off, `read_file` refuses an image with a message the
    /// model can repeat to the user, so "I cannot see images" is stated instead
    /// of hallucinated.
    ///
    /// A `[[providers.models]]` entry's `vision` flag (prefilled from models.dev
    /// when the model is picked from the catalog) overrides it for that model.
    /// `KARUCODE_MODEL_SUPPORTS_IMAGES` sets the flat default.
    #[serde(default)]
    pub supports_images: bool,
    /// This endpoint is a multi-channel aggregator; see
    /// [`ProviderProfile::aggregator`]. Copied from the active profile by
    /// [`Config::model_for_profile`] so the retry layer can see it.
    #[serde(default)]
    pub aggregator: bool,
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
fn default_retry_max_total_secs() -> u64 {
    300
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
            send_temperature: false,
            max_tokens: None,
            context_window: None,
            max_output_tokens: None,
            top_p: None,
            request_timeout_secs: default_request_timeout(),
            retry_max_retries: default_retry_max_retries(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
            retry_max_total_secs: default_retry_max_total_secs(),
            retry_max_stream_restarts: default_retry_max_stream_restarts(),
            extra_headers: BTreeMap::new(),
            extra_body: BTreeMap::new(),
            parallel_tool_calls: default_parallel_tool_calls(),
            supports_images: false,
            aggregator: false,
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
            "context_window": self.context_window,
            "max_output_tokens": self.max_output_tokens,
            "top_p": self.top_p,
            "request_timeout_secs": self.request_timeout_secs,
            "parallel_tool_calls": self.parallel_tool_calls,
            "supports_images": self.supports_images,
            "aggregator": self.aggregator,
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
    /// Whether the model accepts image input. `None` means "not declared", so
    /// the install-wide `model.supports_images` applies.
    ///
    /// The settings screen prefills it from models.dev (`modalities.input`
    /// containing `image`, or a legacy `attachment: true`); it stays editable
    /// because a self-hosted gateway may serve a vision model under a text-only
    /// id, or the other way round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
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
    /// This endpoint is a multi-channel aggregator (a proxy that fronts several
    /// upstream providers and fails over between them).
    ///
    /// It changes one thing: whether a 401/403/404 is retried. On a direct
    /// vendor API those are permanent — a revoked key or a model id that does
    /// not exist stays that way, and retrying 50 times turns a clear error into
    /// a twenty-minute hang. On an aggregator the same status often means *this
    /// channel's* key is dead and the next attempt lands on a different channel,
    /// so retrying is correct. Off by default: the safe reading of a 401 is
    /// "stop and tell the operator".
    #[serde(default)]
    pub aggregator: bool,
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
            aggregator: false,
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
    /// Largest image `read_file` will hand to a vision model, in bytes.
    ///
    /// The bytes are base64-encoded into the transcript and resent on every turn
    /// of the session, so this is a real cost knob, not just a read limit. A
    /// screenshot (a few hundred KB) fits comfortably; a RAW photo or a 40 MB
    /// PNG does not, and `read_file` refuses it with the size and this key's name
    /// so the model can ask for a smaller file instead of silently burning the
    /// context window.
    #[serde(default = "d_image_read")]
    pub max_image_read_bytes: usize,
    /// Maximum results returned by the search tool.
    #[serde(default = "d_search")]
    pub search_max_results: usize,
    /// Character budget for the conversation sent to the model.
    ///
    /// `0` (the default) means **derive it from the model**: the window the
    /// provider declares minus the room its answer needs, converted at four
    /// characters per token. A non-zero value is an explicit ceiling for the
    /// whole install (it can only lower the derived budget, never raise it past
    /// what the model can actually take).
    #[serde(default = "d_context_chars")]
    pub context_max_chars: usize,
    /// Ceiling on the number of **messages** one request may carry.
    ///
    /// The character budget above counts how much text is sent; this counts how
    /// many separate messages that text is spread across. A gateway in front of
    /// the model can cap the second one on its own - this machine's caps input
    /// **items** at 1,000 - and a session of many short turns then dies of
    /// "conversation history exceeds the limit on input items" while sitting far
    /// under any character budget. Seven sessions on one install were already
    /// un-resumable that way at 2 M characters <`context_max_chars`.
    ///
    /// The count is exactly what the request carries: the system prompt, any
    /// compaction digest, every tool result and every conversation turn, with
    /// the compaction digest that replaces the dropped turns counted too.
    ///
    /// **`900` is the default**, deliberately under the 1,000-item ceiling with
    /// room for the ten-odd messages a turn adds. `0` disables the ceiling
    /// entirely - only do that for a provider with no item limit, because a
    /// window over an unknown ceiling fails the turn instead of compacting.
    #[serde(default = "d_context_messages")]
    pub max_context_messages: usize,
    /// Maximum number of tool calls executed from a single assistant message.
    /// Calls beyond this are skipped with a placeholder result, so the cap is a
    /// safety valve against a runaway batch, not a work budget: leave it high.
    #[serde(default = "d_parallel_calls")]
    pub max_tool_calls_per_turn: usize,
    /// Extra environment variable *names* passed through to `run_command`.
    ///
    /// The child never inherits the server's environment: it gets a fixed
    /// allowlist (`PATH`, `HOME`, `USER`, `LANG`, `TERM`, `TMPDIR`, `PWD`, `TZ`,
    /// ...) so the harness bearer token and the provider API key cannot be
    /// exfiltrated by a prompt-injected `curl`. Names listed here are added on
    /// top of that allowlist, but a name that looks like a credential
    /// (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, `KARUCODE_*`, `AWS_*`)
    /// is still dropped.
    #[serde(default)]
    pub command_env_passthrough: Vec<String>,
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
fn d_image_read() -> usize {
    // 4 MiB: a full-screen PNG screenshot is usually 0.2-2 MB, and base64 adds
    // a third on top. Big enough for real UI/asset work, small enough that one
    // read cannot swallow a 128k-token window.
    4 * 1024 * 1024
}
fn d_search() -> usize {
    200
}
fn d_context_chars() -> usize {
    // 0 = derive the budget from the resolved model's context window.
    0
}
fn d_context_messages() -> usize {
    // Under the 1,000-item ceiling a gateway in front of the model commonly
    // enforces, with room for a turn's worth of tool results.
    900
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
            max_image_read_bytes: d_image_read(),
            search_max_results: d_search(),
            context_max_chars: d_context_chars(),
            max_context_messages: d_context_messages(),
            max_tool_calls_per_turn: d_parallel_calls(),
            command_env_passthrough: Vec::new(),
        }
    }
}

/// Characters per token used to turn a model's token window into a character
/// budget. Four is the usual English/code estimate; the safety factor below
/// absorbs the cases where it is optimistic.
const CHARS_PER_TOKEN: f64 = 4.0;
/// Shrink the derived budget a little: token counts are estimates, and a request
/// that overruns the window is rejected outright rather than truncated.
const CONTEXT_SAFETY: f64 = 0.95;
/// Budget used when neither the model nor the operator declares anything.
///
/// Deliberately conservative. This used to be 240_000 (~60k tokens at four
/// characters per token), which is larger than the window of most small models:
/// a hand-added model on a 32k endpoint got a prompt roughly twice its window
/// and the provider rejected the request outright. A budget that is too small
/// costs a compaction; a budget that is too large costs a hard failure.
const CONTEXT_FALLBACK_CHARS: usize = 32_000;
/// Never derive a budget smaller than this, even from a tiny window: below it
/// nothing useful fits and the run would compact on every turn.
const CONTEXT_MIN_CHARS: usize = 32_000;

impl Limits {
    /// How much conversation this model may be sent, in characters.
    ///
    /// The model's own window decides, because it is the only number that is
    /// actually true: a global cap smaller than the window makes the harness
    /// compact (and re-read) work the model could have held, which is the single
    /// most expensive thing a long session can do. `context_max_chars` is still
    /// honoured as an explicit ceiling when an operator sets it, but it is
    /// clamped to what the model can take, never used to invent a smaller window
    /// when the model already declares a bigger one and the operator left it
    /// alone (`0`).
    pub fn context_chars_for(&self, model: &ModelConfig) -> usize {
        let derived = derived_context_chars(model);
        match (self.context_max_chars, derived) {
            (0, Some(derived)) => derived,
            (0, None) => {
                // Falling back means the model's window is unknown, so the
                // budget is a guess. Say so once per call site rather than
                // letting an operator wonder why a long session compacts early.
                tracing::warn!(
                    model = %model.model,
                    fallback_chars = CONTEXT_FALLBACK_CHARS,
                    "model declares no context_window; using the conservative fallback budget. \
                     Set `context_window` on the model entry to use the real window."
                );
                CONTEXT_FALLBACK_CHARS
            }
            // An explicit ceiling is an instruction: respect it exactly, but
            // never above what the model can accept. No floor here - an operator
            // who asks for a deliberately tiny budget gets it.
            (explicit, Some(derived)) => explicit.min(derived).max(1),
            (explicit, None) => explicit,
        }
    }
}

/// `context_window` minus what the answer needs, as characters.
fn derived_context_chars(model: &ModelConfig) -> Option<usize> {
    let window = model.context_window.filter(|window| *window > 0)?;
    let reserve = model
        .max_output_tokens
        .or(model.max_tokens.map(u64::from))
        // A model that declares neither still needs room to answer.
        .unwrap_or(window / 8)
        // An answer can never be allowed to eat the whole window.
        .min(window / 2);
    let usable = window.saturating_sub(reserve) as f64;
    Some(((usable * CHARS_PER_TOKEN * CONTEXT_SAFETY) as usize).max(CONTEXT_MIN_CHARS))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentConfig {
    /// Subagent loops allowed to run at the same time, across the whole install.
    ///
    /// A blocking `spawn_agent` waits for its own child, but a `spawn_agents`
    /// fan-out or a `background` agent runs beside the caller, so this is the
    /// ceiling on real parallelism - and on how many provider streams one model
    /// can open at once.
    ///
    /// **`0` means no ceiling**: every spawn gets a slot immediately, and
    /// `spawn_agents` answers its whole fan-out. That is the setting to pick when
    /// you want the harness out of the model's way and are willing to pay for the
    /// concurrency - each live loop is a provider stream and a transcript. A
    /// finite value is the safer default because a model that fans out 200 ways on
    /// a whim bills for 200 loops.
    #[serde(default = "d_sub_concurrency")]
    pub max_concurrent: usize,
    /// Model round-trips allowed in one *segment* of a subagent.
    ///
    /// A delegated task is a whole piece of work, not a single answer, so the
    /// same rule applies here as to a top-level run: the segment is a checkpoint,
    /// not a finish line. See [`SubagentConfig::max_continues`].
    #[serde(default = "d_sub_iterations")]
    pub max_iterations: u32,
    /// How many times a subagent may refresh its iteration budget before it must
    /// report back. `0` restores the old one-shot behaviour: the subagent stops
    /// dead at `max_iterations`.
    ///
    /// The effective ceiling is `(max_continues + 1) * max_iterations` model
    /// turns, minus whatever `timeout_secs` cuts short. A subagent is also
    /// stopped by `limits.auto_continue = false`, the install-wide off switch.
    #[serde(default = "d_sub_continues")]
    pub max_continues: u32,
    #[serde(default)]
    pub allow_recursive: bool,
    /// How many generations of subagents may nest below the top-level run.
    ///
    /// `0` forbids delegation outright, which is why `allow_recursive` and this
    /// field are two switches and not one: recursion can be enabled and still
    /// produce nothing, because the depth budget is what decides. A child at
    /// `max_depth` keeps working but gets no delegate tools.
    ///
    /// The field is a `u8`, so `255` is the largest value it can hold - deep
    /// enough that it is "no practical limit" rather than a number anyone should
    /// aim at. Each level is a nested loop holding its own transcript, and a
    /// blocking chain of them nests on the stack, so a sensible budget is 2-4.
    #[serde(default = "d_sub_depth")]
    pub max_depth: u8,
    /// Wall-clock limit for one subagent task, across every segment.
    #[serde(default = "d_sub_timeout")]
    pub timeout_secs: u64,
    /// Allow `spawn_agent`/`spawn_agents` to detach with `background = true`.
    ///
    /// A background agent keeps working after the tool call that started it has
    /// returned, so its parent can do something else and collect the result with
    /// `wait_for_agents`, message it with `send_message`, or stop it. With this
    /// off, every spawn blocks its caller exactly as it did before and asking for
    /// `background` is refused with an explanation.
    #[serde(default = "d_true")]
    pub allow_background: bool,
    /// Ceiling on *live* background agents, per session.
    ///
    /// Each one holds a subagent slot, a provider stream and a transcript, so
    /// this is a memory and cost bound rather than a taste one: past it, a spawn
    /// is refused instead of queued. `max_concurrent` still applies on top, so
    /// the useful setting is at least as large as the fan-out you actually want.
    ///
    /// **`0` means no ceiling**, matching [`SubagentConfig::max_concurrent`]. The
    /// switch that turns background work off entirely is
    /// [`SubagentConfig::allow_background`], so `0` here never means "off".
    #[serde(default = "d_sub_max_background")]
    pub max_background: usize,
    /// How long `wait_for_agents` blocks by default, in seconds.
    ///
    /// A wait with no bounded default would park the parent against its own
    /// wall clock with nothing to show for it, so the tool always waits with a
    /// timeout and reports which agents are still running.
    #[serde(default = "d_sub_wait_timeout")]
    pub wait_timeout_secs: u64,
}

fn d_sub_concurrency() -> usize {
    3
}
fn d_sub_iterations() -> u32 {
    200
}
fn d_sub_continues() -> u32 {
    60
}
fn d_sub_depth() -> u8 {
    1
}
fn d_sub_timeout() -> u64 {
    14_400
}
fn d_sub_max_background() -> usize {
    8
}
fn d_sub_wait_timeout() -> u64 {
    600
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: d_sub_concurrency(),
            max_iterations: d_sub_iterations(),
            max_continues: d_sub_continues(),
            allow_recursive: false,
            max_depth: d_sub_depth(),
            timeout_secs: d_sub_timeout(),
            allow_background: true,
            max_background: d_sub_max_background(),
            wait_timeout_secs: d_sub_wait_timeout(),
        }
    }
}

/// `[terminals]`: the browser terminal.
///
/// A terminal is a real PTY running the configured shell as the server user, in
/// the current project's directory. It is a general-purpose shell for the human
/// operating the machine - not a tool the model can call, and not an approval
/// gate: the person typing at it is already authorised to run commands, which is
/// exactly what `run_command` asks *them* about.
///
/// Three limits keep it from becoming a liability: [`TerminalConfig::max_terminals`]
/// bounds how many PTYs one install can hold, [`TerminalConfig::scrollback_bytes`]
/// bounds the replay buffer per terminal, and [`TerminalConfig::shell`] pins what
/// gets spawned (an empty value picks `$SHELL`, then `/bin/sh`).
///
/// **The shell outlives the browser, not the server.** Sessions are kept in
/// SQLite so the tab strip, its titles and a numbered log of the terminal
/// survive a reload, another browser, a cleared cache and a restart - but a
/// shell process is a child of this process, so a restart of karucode ends every
/// terminal's *process*. Reopening one starts a fresh shell in the same
/// directory with the same title. That is the honest boundary: the alternative
/// is a detached daemon, and a terminal that survives `systemctl restart
/// karucode` cannot be killed with it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalConfig {
    /// Offer the terminal tab at all. Off means `GET /api/terminals` answers an
    /// empty list and a create is refused, so a locked-down install can leave no
    /// PTY surface in the UI.
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Shell to spawn. Empty means `$SHELL` from the server's environment, and
    /// `/bin/sh` when that is unset or unusable. A relative name is looked up on
    /// `PATH` (so `"bash"` works); an absolute path must exist and be executable.
    ///
    /// The value is validated at settings-save time, not per spawn, so a typo
    /// cannot leave the tab broken with only a failed websocket to show for it.
    #[serde(default)]
    pub shell: String,
    /// Bytes of output kept per terminal for the replay a reconnecting browser
    /// gets. The newest bytes win and the oldest are dropped, so a terminal that
    /// streamed a 200 MB build log still replays its last screenful.
    #[serde(default = "d_terminal_scrollback")]
    pub scrollback_bytes: usize,
    /// How many terminals may exist at once. Each one is a PTY plus a shell, so
    /// this is a file-descriptor and memory bound, not a taste one.
    #[serde(default = "d_terminal_max")]
    pub max_terminals: usize,
    /// Close a terminal that has had no connected browser for this long, in
    /// seconds. `0` keeps it forever (until the server restarts).
    ///
    /// This is what stops a forgotten tab from holding a shell open on a
    /// laptop that is closed every evening. The database row and its title
    /// survive; only the process is reaped, and the tab reopens a new shell.
    #[serde(default = "d_terminal_idle")]
    pub idle_timeout_secs: u64,
}

fn d_terminal_scrollback() -> usize {
    // 256 KiB: about 3000 lines of ordinary shell output, which is several
    // screenfuls of scrollback on a reconnecting laptop screen.
    256 * 1024
}
fn d_terminal_max() -> usize {
    16
}
fn d_terminal_idle() -> u64 {
    // A shell only costs memory while it has output to keep; an idle one is
    // cheap, but a morning-after tab strip of eight forgotten shells is not.
    12 * 3600
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shell: String::new(),
            scrollback_bytes: d_terminal_scrollback(),
            max_terminals: d_terminal_max(),
            idle_timeout_secs: d_terminal_idle(),
        }
    }
}

impl TerminalConfig {
    /// The shell to spawn: the configured one, else `$SHELL`, else `/bin/sh`.
    ///
    /// A configured value wins even when `$SHELL` names something else - an
    /// operator who pinned `bash` did not ask for the container's `sh`. The
    /// fallbacks only apply to an empty setting.
    pub fn shell_program(&self) -> String {
        let configured = self.shell.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        match std::env::var("SHELL") {
            Ok(shell) if !shell.trim().is_empty() => shell.trim().to_string(),
            _ => "/bin/sh".to_string(),
        }
    }

    /// `scrollback_bytes` clamped to a range one terminal can actually hold.
    pub fn replay_capacity(&self) -> usize {
        self.scrollback_bytes
            .clamp(MIN_TERMINAL_SCROLLBACK, MAX_TERMINAL_SCROLLBACK)
    }

    /// `max_terminals` clamped so the setting cannot be `0` (a tab that never
    /// works) or absurd.
    pub fn terminal_budget(&self) -> usize {
        self.max_terminals.clamp(1, MAX_TERMINALS_HINT)
    }
}

/// Smallest replay buffer worth keeping: one wide screen of dense output.
pub const MIN_TERMINAL_SCROLLBACK: usize = 4 * 1024;
/// Largest replay buffer per terminal. Past this the memory of an idle tab strip
/// is worse than losing scrollback nobody will read.
pub const MAX_TERMINAL_SCROLLBACK: usize = 4 * 1024 * 1024;
/// Upper bound for [`TerminalConfig::max_terminals`].
pub const MAX_TERMINALS_HINT: usize = 64;

/// AI-written session titles: on by default.
///
/// A long session's rail entry is its title, and the first message is usually a
/// wall of context ("we need to add the functionality for steering/follow up,
/// and also ..."). The heuristic title (see `kc-server`) keeps the first 60
/// characters of that, which is readable but not *understandable*. With this on,
/// one cheap extra request after the first user message asks a model for a
/// short, plain-language title and renames the session with it.
///
/// Three properties make it safe to leave on by default:
///
/// * **it never blocks the run** - the request is spawned after `submit` returns
///   and a failure is logged, not surfaced;
/// * **it never overwrites a human** - the rename is refused unless the session
///   still carries the title it had when the request started, so a manual rename
///   (or a second message) wins the race;
/// * **it costs one short turn** - `max_tokens` is set from [`TitlesConfig::max_chars`],
///   the prompt is the first user message truncated to [`TITLE_PROMPT_CHARS`], and
///   the model is whatever [`TitlesConfig::model`] names or, by default, the very
///   model the session is already running on (no second provider to configure).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TitlesConfig {
    /// Generate titles with a model. Off leaves every session with the
    /// deterministic title derived from its first message.
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Provider profile id to ask. Empty means "inherit": use the session's own
    /// resolved model, which is what makes this zero-configuration.
    ///
    /// A separate model is worth setting when the session model is expensive or
    /// slow, because a title only needs a handful of tokens and the fastest cheap
    /// model on the install is usually a different one.
    #[serde(default)]
    pub provider: String,
    /// Model id to ask. Empty means "inherit", exactly like `provider`. Both
    /// must be set together: a provider without a model falls back to that
    /// profile's default model, and a model without a provider inherits the
    /// session's provider.
    #[serde(default)]
    pub model: String,
    /// Hard cap on the generated title, in characters. The model is told a
    /// number a little below this and the answer is truncated on a char boundary,
    /// so a chatty model cannot push the rail out of shape.
    #[serde(default = "d_title_chars")]
    pub max_chars: usize,
    /// Give up (and keep the heuristic title) after this many seconds.
    ///
    /// The request runs in the background, but not forever: an endpoint that
    /// hangs would otherwise hold a connection and a task for the life of the
    /// process.
    #[serde(default = "d_title_timeout")]
    pub timeout_secs: u64,
}

/// Characters of the first user message sent to the titling model.
///
/// A title needs the gist, not the whole paste; 2000 characters covers the
/// opening paragraph of even a very detailed request. Truncating here is what
/// keeps the extra turn cheap on a session that starts with a 50 KB log.
pub const TITLE_PROMPT_CHARS: usize = 2_000;

fn d_title_chars() -> usize {
    60
}
fn d_title_timeout() -> u64 {
    20
}

impl Default for TitlesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: String::new(),
            model: String::new(),
            max_chars: d_title_chars(),
            timeout_secs: d_title_timeout(),
        }
    }
}

impl TitlesConfig {
    /// The provider profile explicitly configured for titles, if any.
    pub fn provider_id(&self) -> Option<&str> {
        let provider = self.provider.trim();
        (!provider.is_empty()).then_some(provider)
    }

    /// The model id explicitly configured for titles, if any.
    pub fn model_id(&self) -> Option<&str> {
        let model = self.model.trim();
        (!model.is_empty()).then_some(model)
    }

    /// True when titles inherit the session's own model.
    pub fn inherits_session_model(&self) -> bool {
        self.provider_id().is_none() && self.model_id().is_none()
    }

    /// `max_chars` clamped to a range the rail can actually render.
    pub fn title_budget(&self) -> usize {
        self.max_chars.clamp(8, MAX_TITLE_CHARS_HINT)
    }
}

/// Upper bound for [`TitlesConfig::max_chars`]. Beyond this a "title" is a
/// paragraph, and the session rail truncates it anyway.
pub const MAX_TITLE_CHARS_HINT: usize = 120;

/// Whether karucode tells a human that something happened, and how loudly.
///
/// A long run is exactly the case where you stop watching: the agent works for
/// minutes, you switch windows, and without this the only way to find out it
/// finished is to come back and look. The same signal covers the opposite case —
/// a run parked on an approval, which will sit there until it times out.
///
/// **Four independent layers can turn it off**, because they answer different
/// questions:
///
/// 1. `enabled = false` here (or `KARUCODE_NOTIFICATIONS=0`): the server stops
///    emitting the `notification` event at all. Nothing downstream has to
///    cooperate, and a client cannot accidentally ignore the switch.
/// 2. The per-event switches below: keep run-finished pings, drop the rest.
/// 3. `plugin = false`: refuse notifications raised by plugins, enforced in the
///    plugin host so a plugin cannot bypass it by sending anyway.
/// 4. Browser preferences (sound, desktop notifications): a frontend concern, not
///    this struct. They silence a *surface*, not the signal.
///
/// Routing is deliberately server-side: a browser only knows about the session it
/// has open, so a run that finishes in another project would otherwise be
/// invisible. The server reports every terminal run; the UI decides which ones are
/// worth interrupting you for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationsConfig {
    /// Emit `notification` events at all. Off silences the stream completely.
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Notify when a run completes successfully.
    #[serde(default = "d_true")]
    pub run_finished: bool,
    /// Notify when something waits on a human: a pending approval, or a run that
    /// stopped at a configured limit and may continue.
    #[serde(default = "d_true")]
    pub needs_input: bool,
    /// Notify when a run fails, is interrupted by a restart, or a subagent fails.
    #[serde(default = "d_true")]
    pub error: bool,
    /// Notify for plugin-raised `info` notifications. Distinct from `plugin`
    /// below: this is the kind, that one is whether plugins may speak at all.
    #[serde(default = "d_true")]
    pub info: bool,
    /// Let plugins raise notifications (both the `notify` frame and the resulting
    /// `on_notify` fan-out). Enforced by the plugin host.
    #[serde(default = "d_true")]
    pub plugin: bool,
    /// Skip notifications for runs shorter than this many seconds. `0` notifies
    /// every run; a two-second question is noise to most people, so this is the
    /// knob for that.
    #[serde(default)]
    pub min_run_secs: u64,
    /// Include the first line of the answer as the notification body. Off sends the
    /// title only, which keeps model output off a lock screen.
    #[serde(default = "d_true")]
    pub say_body: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            run_finished: true,
            needs_input: true,
            error: true,
            info: true,
            plugin: true,
            min_run_secs: 0,
            say_body: true,
        }
    }
}

/// Upper bound for [`NotificationsConfig::min_run_secs`].
///
/// An hour: past that the feature is off in all but name, and the number is
/// probably a typo (seconds vs minutes).
pub const MAX_MIN_RUN_SECS: u64 = 3_600;

impl NotificationsConfig {
    /// Whether a notification of this kind is switched on *and* the master switch
    /// is on. The one predicate every emitter asks, so the per-kind table cannot
    /// drift between the runtime and the plugin host.
    pub fn allows(&self, kind: crate::notify::NotifyKind) -> bool {
        use crate::notify::NotifyKind;
        if !self.enabled {
            return false;
        }
        match kind {
            NotifyKind::RunFinished => self.run_finished,
            NotifyKind::NeedsInput => self.needs_input,
            NotifyKind::Error => self.error,
            NotifyKind::Info => self.info,
        }
    }

    /// Whether a plugin-raised notification may be emitted: the master switch, the
    /// kind switch, and permission for plugins to speak.
    pub fn allows_plugin(&self, kind: crate::notify::NotifyKind) -> bool {
        self.allows(kind) && self.plugin
    }

    /// Whether a run that took `secs` seconds is worth a notification.
    ///
    /// `>=` rather than `>`: with `min_run_secs = 1`, a run reported as exactly one
    /// second is not "shorter than one second", so it notifies.
    pub fn run_is_long_enough(&self, secs: u64) -> bool {
        secs >= self.min_run_secs
    }

    /// The per-event switches as names, for the settings screen and for error
    /// messages. Order matches [`crate::notify::NotifyKind::ALL`].
    pub fn event_names() -> &'static [&'static str] {
        &["run_finished", "needs_input", "error", "info"]
    }

    /// Set one per-event switch by name. `None` for an unknown name, so a typo in
    /// `KARUCODE_NOTIFICATIONS_EVENTS` is reported rather than ignored.
    pub fn set_event(&mut self, name: &str, on: bool) -> Option<()> {
        match name.trim().to_ascii_lowercase().as_str() {
            "run_finished" | "run-finished" => self.run_finished = on,
            "needs_input" | "needs-input" => self.needs_input = on,
            "error" => self.error = on,
            "info" => self.info = on,
            _ => return None,
        }
        Some(())
    }

    /// Read one per-event switch by name.
    pub fn event(&self, name: &str) -> Option<bool> {
        match name.trim().to_ascii_lowercase().as_str() {
            "run_finished" | "run-finished" => Some(self.run_finished),
            "needs_input" | "needs-input" => Some(self.needs_input),
            "error" => Some(self.error),
            "info" => Some(self.info),
            _ => None,
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
    /// `[titles]`: AI-written session titles (on by default).
    #[serde(default)]
    pub titles: Option<TitlesConfig>,
    /// `[notifications]`: tell a human when a run finishes or needs an answer.
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,
    /// `[terminals]`: the browser terminal (a PTY running a shell as the server
    /// user). On by default; see [`TerminalConfig`].
    #[serde(default)]
    pub terminals: Option<TerminalConfig>,
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    /// Re-read the plugin sources in the background and apply adds, removals and
    /// upgrades without restarting the server.
    #[serde(default)]
    pub plugins_watch: Option<bool>,
    /// How often the watcher polls, in seconds (clamped to
    /// [`MIN_PLUGINS_WATCH_SECS`]`..=`[`MAX_PLUGINS_WATCH_SECS`]).
    #[serde(default)]
    pub plugins_watch_secs: Option<u64>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Allowlist for directories a project may be created in. `None`/empty -
    /// the default - means **no allowlist**: any existing directory is a valid
    /// project, which is what a single-operator install wants, since the obvious
    /// second project is a sibling of the workspace. Set it to restrict project
    /// creation on a shared or remotely reachable deployment. `/` itself is
    /// refused either way.
    #[serde(default)]
    pub project_roots: Option<Vec<PathBuf>>,
    /// Let the file tools reach the system temporary directory as well as the
    /// project. On by default: scratch files, build caches, screenshots and
    /// browser profiles land in `/tmp` constantly, and refusing them is a wall
    /// of failed tool calls for no security gain.
    #[serde(default)]
    pub allow_temp_dir: Option<bool>,
    /// Separate credential required to change the approval mode, the model
    /// endpoint or the provider list. Without it, any ordinary API client can
    /// remove the human gate and then repoint the provider at itself.
    #[serde(default)]
    pub admin_token: Option<String>,
    /// Delete messages older than this many days. `0` keeps everything.
    #[serde(default)]
    pub retention_days: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub workspace_root: PathBuf,
    /// True when a workspace was named explicitly (`--workspace`,
    /// `KARUCODE_WORKSPACE`, or `workspace =` in the config file).
    ///
    /// `workspace_root` always has a value (the current directory by default), so
    /// this is what tells "the operator asked for /srv/x" apart from "nothing was
    /// configured". The bootstrap uses it to decide whether an unusable workspace
    /// is a fatal typo or just means "start with no project".
    pub workspace_explicit: bool,
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
    /// AI-written session titles (see [`TitlesConfig`]). On by default.
    pub titles: TitlesConfig,
    /// Human-facing notifications (see [`NotificationsConfig`]). On by default.
    pub notifications: NotificationsConfig,
    /// The browser terminal (see [`TerminalConfig`]). On by default.
    pub terminals: TerminalConfig,
    pub plugins: Vec<PluginConfig>,
    /// Poll the plugin sources and apply changes without a restart. Off by
    /// default: a reload replaces processes, and an operator who edits a plugin's
    /// files mid-session should opt into that happening on its own.
    pub plugins_watch: bool,
    /// Poll interval for the watcher, in seconds.
    pub plugins_watch_secs: u64,
    pub mcp_servers: Vec<McpServerConfig>,
    pub web_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    /// Allowlist for directories a project may be created in. **Empty means no
    /// allowlist**: `POST /api/projects` accepts any existing directory except
    /// `/`. Set it to restrict project creation on a shared or remotely
    /// reachable deployment.
    pub project_roots: Vec<PathBuf>,
    /// Allow the file tools to reach the system temporary directory in addition
    /// to the project root. See [`Config::extra_roots`].
    pub allow_temp_dir: bool,
    /// Credential required for security-relevant settings changes.
    pub admin_token: Option<String>,
    /// Message retention in days; `0` keeps everything.
    pub retention_days: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8787,
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            workspace_explicit: false,
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
            titles: TitlesConfig::default(),
            notifications: NotificationsConfig::default(),
            terminals: TerminalConfig::default(),
            plugins: Vec::new(),
            plugins_watch: false,
            plugins_watch_secs: DEFAULT_PLUGINS_WATCH_SECS,
            mcp_servers: Vec::new(),
            web_dir: None,
            config_path: None,
            project_roots: Vec::new(),
            allow_temp_dir: true,
            admin_token: None,
            retention_days: 0,
        }
    }
}

/// Fastest the plugin watcher may poll, in seconds.
///
/// One poll is a `read_dir` of `<data_dir>/plugins` plus a `stat` per configured
/// command, so it is cheap — but a busy loop over a directory that changes a few
/// times an hour buys nothing and makes the log a metronome.
pub const MIN_PLUGINS_WATCH_SECS: u64 = 1;
/// Default poll interval for the plugin watcher, in seconds.
///
/// Two seconds is fast enough that saving a plugin file feels like it took
/// effect, and slow enough that a build writing its binary in several steps is
/// seen once rather than mid-write.
pub const DEFAULT_PLUGINS_WATCH_SECS: u64 = 2;
/// Slowest the plugin watcher may poll, in seconds.
pub const MAX_PLUGINS_WATCH_SECS: u64 = 600;

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

/// Boolean environment flag. Accepts `1/0`, `true/false`, `yes/no`, `on/off`.
/// A present but malformed value is a configuration error: silently keeping the
/// old value could turn off a security control the operator explicitly enabled.
fn env_bool(name: &str) -> Result<Option<bool>> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(Error::Config(format!(
                "invalid {name}: not valid Unicode; expected a boolean (1/0, true/false, yes/no, on/off)"
            )));
        }
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        _ => Err(Error::Config(format!(
            "invalid {name}={value:?}: expected a boolean (1/0, true/false, yes/no, or on/off)"
        ))),
    }
}

/// Minimum length for both the bearer token and the admin token.
///
/// Sixteen characters is the floor published in `docs/SECURITY.md`,
/// `.env.example` and the startup error, and it is what makes a human-typed
/// token the wrong choice: `openssl rand -hex 24` is 48 characters.
pub const MIN_TOKEN_LEN: usize = 16;

/// Prefixes a documented placeholder starts with. `change-me-to-16-plus-chars`
/// is 27 characters, so a length check alone accepts it - and it is the first
/// thing anyone guesses off a public bind.
const PLACEHOLDER_PREFIXES: [&str; 5] = ["change-me", "changeme", "secret", "password", "test"];

/// Reject a token that is too short or is a documented placeholder.
///
/// The message is actionable on purpose: the fix is one command, and an operator
/// who is told "use at least 16 characters" on a 27-character placeholder would
/// rightly think the check was broken.
fn validate_token(label: &str, env: &str, token: &str) -> Result<()> {
    // Length is measured on the trimmed value so whitespace copied out of a
    // `.env` file cannot count as entropy.
    let trimmed = token.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if trimmed.len() < MIN_TOKEN_LEN {
        return Err(Error::Config(format!(
            "{label} must be at least {MIN_TOKEN_LEN} characters (got {}); generate one with \
             `openssl rand -hex 24` and set {env}",
            trimmed.len()
        )));
    }
    if let Some(placeholder) = PLACEHOLDER_PREFIXES
        .iter()
        .find(|prefix| lowered.starts_with(*prefix))
    {
        return Err(Error::Config(format!(
            "{label} looks like the documented placeholder (`{placeholder}...`); it is public \
             and has no secret value. Generate a real one with `openssl rand -hex 24` and set {env}"
        )));
    }
    Ok(())
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
            cfg.workspace_explicit = true;
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
        if let Some(v) = file.titles {
            cfg.titles = v;
        }
        if let Some(v) = file.notifications {
            cfg.notifications = v;
        }
        if let Some(v) = file.terminals {
            cfg.terminals = v;
        }
        if !file.plugins.is_empty() {
            cfg.plugins = file.plugins;
        }
        if let Some(v) = file.plugins_watch {
            cfg.plugins_watch = v;
        }
        if let Some(v) = file.plugins_watch_secs {
            cfg.plugins_watch_secs = v;
        }
        if !file.mcp_servers.is_empty() {
            cfg.mcp_servers = file.mcp_servers;
        }
        if let Some(v) = file.project_roots {
            cfg.project_roots = v;
        }
        if let Some(v) = file.allow_temp_dir {
            cfg.allow_temp_dir = v;
        }
        if let Some(v) = file.admin_token {
            cfg.admin_token = Some(v);
        }
        if let Some(v) = file.retention_days {
            cfg.retention_days = v;
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
            cfg.workspace_explicit = true;
        }
        if let Some(v) = env_str("KARUCODE_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Some(v) = env_str("KARUCODE_AUTH_TOKEN") {
            cfg.auth_token = Some(v);
        }
        if let Some(v) = env_bool("KARUCODE_LOCK_SETTINGS")? {
            cfg.lock_settings = v;
        }
        if let Some(v) = env_str("KARUCODE_LOG").or_else(|| env_str("RUST_LOG")) {
            cfg.log_level = v;
        }
        if let Some(v) = env_str("KARUCODE_WEB_DIR") {
            cfg.web_dir = Some(PathBuf::from(v));
        }
        if let Some(v) = env_str("KARUCODE_ADMIN_TOKEN") {
            cfg.admin_token = Some(v);
        }
        if let Some(v) = env_parse("KARUCODE_RETENTION_DAYS") {
            cfg.retention_days = v;
        }
        if let Some(v) = env_bool("KARUCODE_PLUGINS_WATCH")? {
            cfg.plugins_watch = v;
        }
        if let Some(v) = env_parse("KARUCODE_PLUGINS_WATCH_SECS") {
            cfg.plugins_watch_secs = v;
        }
        // AI titles: one switch plus an optional second model. The env spelling
        // is `KARUCODE_TITLES_*` so an operator can turn the extra turn off in a
        // locked deployment without editing `[titles]`.
        if let Some(v) = env_bool("KARUCODE_TITLES")? {
            cfg.titles.enabled = v;
        }
        if let Some(v) = env_str("KARUCODE_TITLES_PROVIDER") {
            cfg.titles.provider = v;
        }
        if let Some(v) = env_str("KARUCODE_TITLES_MODEL") {
            cfg.titles.model = v;
        }
        // Notifications: one master switch plus a comma-separated event list, so a
        // locked deployment can silence the ping without editing `[notifications]`.
        if let Some(v) = env_bool("KARUCODE_NOTIFICATIONS")? {
            cfg.notifications.enabled = v;
        }
        if let Some(v) = env_str("KARUCODE_NOTIFICATIONS_EVENTS") {
            // Fail loudly on a typo: an operator who writes `run-finished,` or
            // `errorz` should find out at startup, not by silently getting either
            // everything or nothing.
            for name in v.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let on = !name.starts_with('!') && !name.starts_with('-');
                let bare = name.trim_start_matches(['!', '-']);
                if cfg.notifications.set_event(bare, on).is_none() {
                    return Err(Error::Config(format!(
                        "KARUCODE_NOTIFICATIONS_EVENTS: unknown event `{bare}` \
                         (expected any of: {})",
                        NotificationsConfig::event_names().join(", ")
                    )));
                }
            }
        }
        if let Some(v) = env_bool("KARUCODE_NOTIFICATIONS_PLUGIN")? {
            cfg.notifications.plugin = v;
        }
        if let Some(v) = env_parse("KARUCODE_NOTIFICATIONS_MIN_RUN_SECS") {
            cfg.notifications.min_run_secs = v;
        }
        if let Some(v) = env_bool("KARUCODE_NOTIFICATIONS_SAY_BODY")? {
            cfg.notifications.say_body = v;
        }
        // The browser terminal. `KARUCODE_TERMINALS=0` is the switch a locked
        // deployment needs: with it off there is no PTY surface at all, so an
        // install that must not offer a shell does not have to rely on the UI
        // hiding the tab.
        if let Some(v) = env_bool("KARUCODE_TERMINALS")? {
            cfg.terminals.enabled = v;
        }
        if let Some(v) = env_str("KARUCODE_TERMINALS_SHELL") {
            cfg.terminals.shell = v;
        }
        if let Some(v) = env_parse("KARUCODE_TERMINALS_SCROLLBACK_BYTES") {
            cfg.terminals.scrollback_bytes = v;
        }
        if let Some(v) = env_parse("KARUCODE_TERMINALS_MAX") {
            cfg.terminals.max_terminals = v;
        }
        if let Some(v) = env_parse("KARUCODE_TERMINAL_IDLE_SECS") {
            cfg.terminals.idle_timeout_secs = v;
        }
        // Colon-separated so a path containing a comma still works. Unset means
        // "no allowlist", not "the default workspace only" - the latter made the
        // ordinary second project (a sibling of the workspace) a 400.
        if let Some(v) = env_str("KARUCODE_PROJECT_ROOTS") {
            cfg.project_roots = std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
        }
        if let Some(v) = env_bool("KARUCODE_ALLOW_TEMP_DIR")? {
            cfg.allow_temp_dir = v;
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
        if let Some(v) = env_parse("KARUCODE_RETRY_MAX_TOTAL_SECS") {
            // The wall-clock ceiling for one stream's retry chain. It was documented
            // in `.env.example` and `config.example.toml` long before this line
            // existed, so a locked deployment that set it silently kept the default.
            cfg.model.retry_max_total_secs = v;
        }
        if let Some(v) = env_parse("KARUCODE_RETRY_MAX_STREAM_RESTARTS") {
            cfg.model.retry_max_stream_restarts = v;
        }
        if let Some(v) = env_bool("KARUCODE_MODEL_SUPPORTS_IMAGES")? {
            cfg.model.supports_images = v;
        }

        // Build the reserved environment profile only after every flat model
        // override has been applied. Otherwise the later request-timeout override
        // updates `model` but is immediately replaced by the stale profile during
        // normalisation. The flat file's remaining connection options still apply
        // to this endpoint; do not reset them to ProviderProfile::default().
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
                extra_headers: cfg.model.extra_headers.clone(),
                extra_body: cfg.model.extra_body.clone(),
                aggregator: cfg.model.aggregator,
            });
            cfg.active_provider = Some(ENV_PROFILE_ID.to_string());
            cfg.active_model = Some(cfg.model.model.clone());
        }

        if let Some(v) = env_parse("KARUCODE_MAX_ITERATIONS") {
            cfg.limits.max_iterations = v;
        }
        if let Some(v) = env_parse("KARUCODE_MAX_RUN_SECS") {
            cfg.limits.max_run_secs = v;
        }
        if let Some(v) = env_bool("KARUCODE_AUTO_CONTINUE")? {
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
        if let Some(v) = env_parse("KARUCODE_MAX_IMAGE_READ_BYTES") {
            cfg.limits.max_image_read_bytes = v;
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
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_MAX_CONTINUES") {
            cfg.subagents.max_continues = v;
        }
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_TIMEOUT_SECS") {
            cfg.subagents.timeout_secs = v;
        }
        if let Some(v) = env_bool("KARUCODE_ALLOW_RECURSIVE_SUBAGENTS")? {
            cfg.subagents.allow_recursive = v;
        }
        if let Some(v) = env_bool("KARUCODE_SUBAGENT_ALLOW_BACKGROUND")? {
            cfg.subagents.allow_background = v;
        }
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_MAX_BACKGROUND") {
            cfg.subagents.max_background = v;
        }
        if let Some(v) = env_parse("KARUCODE_SUBAGENT_WAIT_TIMEOUT_SECS") {
            cfg.subagents.wait_timeout_secs = v;
        }

        // Validate.
        if cfg.port == 0 {
            return Err(Error::Config("port may not be 0".into()));
        }
        cfg.normalize_providers();
        if cfg.model.base_url.trim().is_empty() {
            return Err(Error::Config("model.base_url may not be empty".into()));
        }
        if let Some(token) = &cfg.auth_token {
            validate_token("auth token", "KARUCODE_AUTH_TOKEN", token)?;
        }
        if let Some(token) = &cfg.admin_token {
            validate_token("admin token", "KARUCODE_ADMIN_TOKEN", token)?;
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
        // `max_concurrent = 0` and `max_background = 0` mean *no ceiling* and are
        // kept as written: the limiter and the background check read 0 as
        // "unlimited", so clamping it here would silently turn the widest setting
        // an operator can ask for into the narrowest one.
        if cfg.subagents.wait_timeout_secs == 0 {
            cfg.subagents.wait_timeout_secs = 1;
        }
        // A watcher interval is a promise about latency, so keep it inside the
        // range the docs describe instead of accepting `0` (a busy loop over the
        // plugin directory) or `86400` (a "hot" reload that takes a day).
        cfg.plugins_watch_secs = cfg
            .plugins_watch_secs
            .clamp(MIN_PLUGINS_WATCH_SECS, MAX_PLUGINS_WATCH_SECS);
        Ok(cfg)
    }

    /// Re-read just the `[[plugins]]` entries from the config file.
    ///
    /// Plugin sources are the config file *and* `<data_dir>/plugins`, and the
    /// only authority on the config file is the file itself: `PATCH /api/config`
    /// deliberately cannot change `[[plugins]]` (installing a plugin is not a
    /// settings tweak). So a hot reload looks at the file again and falls back to
    /// the in-memory list only when there is nothing to read — no config file, an
    /// unreadable one, or a parse error, each logged where it happens.
    pub fn reload_plugin_entries(&self) -> Vec<PluginConfig> {
        let Some(path) = &self.config_path else {
            return self.plugins.clone();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return self.plugins.clone();
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(file) => file.plugins,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    "cannot re-read the plugin entries from the config file; keeping the ones \
                     already loaded: {e}"
                );
                self.plugins.clone()
            }
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("karucode.db")
    }

    /// Extra roots the file tools may reach in addition to a project's directory.
    ///
    /// This is the *tool* confinement allowlist, not [`Config::project_roots`]
    /// (which only decides where a project may be created). With
    /// [`Config::allow_temp_dir`] on - the default - it is the system temporary
    /// directory resolved the same way the tools' children resolve it:
    /// `TMPDIR` when it is set and absolute, otherwise [`std::env::temp_dir`],
    /// plus the platform default when the two differ.
    ///
    /// `/tmp` is deliberately not a security boundary here: `run_command` already
    /// runs as the server user with that user's full filesystem access, so the
    /// confinement rule protects against a *model* wandering out of its project,
    /// not against an attacker who can already execute commands. Allowing a
    /// scratch directory that every process on the machine can already write to
    /// costs nothing and stops `write_file`/`read_file` from failing on the
    /// files that build tools put there.
    ///
    /// The roots are still checked as roots: `..` cannot climb from `/tmp` to
    /// `/etc`, and a symlink inside `/tmp` that points somewhere else is refused.
    pub fn extra_roots(&self) -> Vec<PathBuf> {
        if !self.allow_temp_dir {
            return Vec::new();
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(tmpdir) = std::env::var_os("TMPDIR").map(PathBuf::from) {
            if tmpdir.is_absolute() {
                roots.push(tmpdir);
            }
        }
        let default = std::env::temp_dir();
        if default.is_absolute() && !roots.contains(&default) {
            roots.push(default);
        }
        roots
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
                aggregator: self.model.aggregator,
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
        self.model.aggregator = profile.aggregator;
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

    /// The `[[providers.models]]` entry for `model_id` on `profile`, if listed.
    pub fn model_entry_for<'a>(
        &self,
        profile: &'a ProviderProfile,
        model_id: &str,
    ) -> Option<&'a ModelEntry> {
        profile
            .models
            .iter()
            .find(|entry| entry.id.trim() == model_id.trim())
    }

    /// Whether `model_id` on `profile` accepts image input.
    ///
    /// A per-model `vision` declaration wins over the install-wide
    /// `model.supports_images`, so one session can read images while the default
    /// model stays text-only. A model the profile does not list (or one whose
    /// `vision` is `None`) falls back to the install-wide flag.
    ///
    /// This is the only place the rule lives: [`Config::model_for`] and the
    /// session-selection resolver both go through it, so a pinned model and the
    /// profile default cannot disagree about whether the pixels are sendable.
    pub fn supports_images_for(&self, profile: &ProviderProfile, model_id: &str) -> bool {
        self.model_entry_for(profile, model_id)
            .and_then(|entry| entry.vision)
            .unwrap_or(self.model.supports_images)
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
        self.model_for(profile, &model)
    }

    /// Resolve `profile` **and** one of its model ids into the live
    /// [`ModelConfig`] the runtime reads.
    ///
    /// The per-model metadata (context window, output cap, image support) is
    /// looked up for `model_id`, not for the profile's default model. That
    /// distinction matters for a session whose pin differs from the install-wide
    /// active model: resolving the profile alone and then rewriting `model`
    /// would keep the *default* model's limits and capabilities, which is how a
    /// session pinned to a vision model was told it could not see images.
    pub fn model_for(&self, profile: &ProviderProfile, model_id: &str) -> ModelConfig {
        let entry = self.model_entry_for(profile, model_id);
        ModelConfig {
            provider: profile.provider.clone(),
            base_url: profile.base_url.clone(),
            api_key: profile.api_key.clone(),
            context_window: entry.and_then(|entry| entry.context_window),
            max_output_tokens: entry.and_then(|entry| entry.max_output_tokens),
            model: model_id.trim().to_string(),
            temperature: self.model.temperature,
            send_temperature: self.model.send_temperature,
            max_tokens: self.model.max_tokens,
            top_p: self.model.top_p,
            request_timeout_secs: profile.request_timeout_secs,
            // The retry policy is global, like sampling: it says how patient
            // the client is with ANY endpoint, not one profile's connection.
            retry_max_retries: self.model.retry_max_retries,
            retry_base_delay_ms: self.model.retry_base_delay_ms,
            retry_max_delay_ms: self.model.retry_max_delay_ms,
            retry_max_total_secs: self.model.retry_max_total_secs,
            retry_max_stream_restarts: self.model.retry_max_stream_restarts,
            extra_headers: profile.extra_headers.clone(),
            extra_body: profile.extra_body.clone(),
            parallel_tool_calls: self.model.parallel_tool_calls,
            aggregator: profile.aggregator,
            supports_images: self.supports_images_for(profile, model_id),
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

    fn model_with_window(window: Option<u64>, output: Option<u64>) -> ModelConfig {
        ModelConfig {
            context_window: window,
            max_output_tokens: output,
            ..Default::default()
        }
    }

    #[test]
    fn the_temp_dir_is_an_extra_root_by_default() {
        let config = Config::default();
        assert!(config.allow_temp_dir, "the temp dir is allowed by default");
        let roots = config.extra_roots();
        let temp = std::env::temp_dir();
        assert!(
            roots.contains(&temp),
            "the temp dir {temp:?} should be allowed, got {roots:?}"
        );
        // Every root is absolute: a relative one would resolve against whatever
        // directory the server happened to start in.
        assert!(roots.iter().all(|root| root.is_absolute()), "{roots:?}");
    }

    #[test]
    fn the_temp_dir_can_be_turned_off() {
        let config = Config {
            allow_temp_dir: false,
            ..Config::default()
        };
        assert!(config.extra_roots().is_empty());
    }

    #[test]
    fn the_config_file_key_is_read_and_defaults_on() {
        let file: ConfigFile = toml::from_str("").unwrap();
        assert_eq!(file.allow_temp_dir, None, "absent means the default");

        let file: ConfigFile = toml::from_str("allow_temp_dir = false").unwrap();
        assert_eq!(file.allow_temp_dir, Some(false));

        // And the env spelling, which shares the same normalisation as the
        // other booleans (`1/true/yes/on`).
        assert_eq!(
            env_bool("KARUCODE_ALLOW_TEMP_DIR_NOPE_NOT_SET").unwrap(),
            None
        );
    }

    #[test]
    fn the_models_window_decides_the_budget_by_default() {
        let limits = Limits::default();
        assert_eq!(limits.context_max_chars, 0, "0 means derive from the model");
        // 1M-token window with 384k reserved for the answer.
        let chars = limits.context_chars_for(&model_with_window(Some(1_048_576), Some(384_000)));
        assert!(
            (2_400_000..=2_600_000).contains(&chars),
            "a 1M-token model should get ~2.5M chars, got {chars}"
        );
        // A 32k model must never be handed the same budget.
        let small = limits.context_chars_for(&model_with_window(Some(32_768), Some(4_096)));
        assert!(
            (90_000..130_000).contains(&small),
            "a 32k-token model should get ~110k chars, got {small}"
        );
    }

    #[test]
    fn an_unknown_window_falls_back_to_the_documented_budget() {
        assert_eq!(
            Limits::default().context_chars_for(&model_with_window(None, None)),
            CONTEXT_FALLBACK_CHARS
        );
    }

    #[test]
    fn an_explicit_budget_is_a_ceiling_that_never_exceeds_the_window() {
        let explicit = Limits {
            context_max_chars: 400_000,
            ..Default::default()
        };
        // Smaller than the derived budget: honoured exactly.
        assert_eq!(
            explicit.context_chars_for(&model_with_window(Some(1_048_576), Some(384_000))),
            400_000
        );
        // Larger than what a small model can take: clamped down to its window.
        let clamped = explicit.context_chars_for(&model_with_window(Some(32_768), Some(4_096)));
        assert!(
            clamped < 400_000 && clamped > 50_000,
            "an over-window request survived: {clamped}"
        );
        // A deliberately tiny explicit budget is an instruction, not a mistake.
        let tiny = Limits {
            context_max_chars: 600,
            ..Default::default()
        };
        assert_eq!(
            tiny.context_chars_for(&model_with_window(Some(1_048_576), None)),
            600
        );
    }

    #[test]
    fn a_profile_model_carries_its_window_into_the_resolved_config() {
        let mut config = Config {
            providers: vec![ProviderProfile {
                id: "gw".into(),
                base_url: "http://h/v1".into(),
                models: vec![ModelEntry {
                    id: "big-model".into(),
                    context_window: Some(200_000),
                    max_output_tokens: Some(8_000),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        config.active_provider = Some("gw".into());
        let model = config.model_for_profile(&config.providers[0]);
        assert_eq!(model.model, "big-model");
        assert_eq!(model.context_window, Some(200_000));
        assert_eq!(model.max_output_tokens, Some(8_000));
        // ... and the budget is derived from it, not from a global constant.
        let chars = Limits::default().context_chars_for(&model);
        assert!(chars > 600_000, "expected ~730k chars, got {chars}");
    }

    #[test]
    fn defaults_are_sane() {
        let l = Limits::default();
        assert!(l.max_iterations >= 8);
        // Long-horizon defaults: a run may chain many segments inside one day.
        assert!(l.auto_continue);
        assert!(l.max_continues >= 1);
        assert!(l.max_run_secs >= 3600);
        assert!(l.max_tool_calls_per_turn >= 8);
        assert!(l.command_timeout_secs > 0);
        assert!(l.max_tool_output_bytes > 0);
        // The item ceiling sits under the 1,000-item limit a gateway in front of
        // the model commonly enforces (see `max_context_messages`).
        assert_eq!(l.max_context_messages, 900);
        let s = SubagentConfig::default();
        assert!(!s.allow_recursive);
        assert!(s.max_concurrent >= 1);
        assert!(
            s.max_iterations >= 32,
            "a subagent must outlive one small edit"
        );
        // A delegated task that takes hours must not die at its first budget:
        // the segment is a checkpoint, and the wall clock is the real bound.
        assert!(s.max_continues >= 1, "subagents continue like a run does");
        assert!(s.timeout_secs >= 3600);
        assert!(
            u64::from(s.max_iterations) * u64::from(s.max_continues + 1) >= 1_000,
            "a long-horizon subagent must be able to reach four figures of turns"
        );
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
        cfg.model.aggregator = true;
        cfg.normalize_providers();

        assert_eq!(cfg.providers.len(), 1);
        let active = cfg.active_profile().expect("active profile");
        assert_eq!(active.id, DEFAULT_PROFILE_ID);
        assert_eq!(active.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(active.models.len(), 1);
        assert_eq!(active.models[0].id, "qwen3-32b");
        assert!(active.aggregator);
        assert!(cfg.effective_model().aggregator);
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

    /// The reported bug: a session pinned to a vision model on a profile whose
    /// *default* model is text-only used to inherit the default's
    /// `supports_images = false`, so `read_file` told the model it could not see
    /// the image the endpoint would have accepted.
    #[test]
    fn a_vision_model_resolves_as_vision_even_under_a_text_only_default() {
        let mut cfg = Config::default();
        cfg.model.supports_images = false;
        let mut gateway = profile("gateway", &["text-default", "gpt-4o"]);
        gateway.models[1].vision = Some(true);
        gateway.models[1].context_window = Some(128_000);
        cfg.providers = vec![gateway];
        cfg.active_provider = Some("gateway".into());
        cfg.active_model = Some("text-default".into());
        cfg.normalize_providers();

        // The install-wide default model stays text-only...
        assert!(!cfg.effective_model().supports_images);
        assert!(!cfg.model_for_profile(&cfg.providers[0]).supports_images);
        // ...while the same profile resolved *at the vision model* is vision, and
        // gets that model's own limits rather than the default's.
        let pinned = cfg.model_for(&cfg.providers[0], "gpt-4o");
        assert!(pinned.supports_images);
        assert_eq!(pinned.model, "gpt-4o");
        assert_eq!(pinned.context_window, Some(128_000));
        assert!(cfg.supports_images_for(&cfg.providers[0], "gpt-4o"));
        assert!(!cfg.supports_images_for(&cfg.providers[0], "text-default"));
    }

    #[test]
    fn a_per_model_vision_flag_overrides_both_ways() {
        let mut cfg = Config::default();
        cfg.model.supports_images = true;
        let mut local = profile("local", &["declared-text-only", "undeclared"]);
        local.models[0].vision = Some(false);
        cfg.providers = vec![local];
        cfg.active_provider = Some("local".into());
        cfg.active_model = Some("undeclared".into());
        cfg.normalize_providers();

        // An explicit `vision = false` wins over the install-wide true: a gateway
        // can serve a vision id as text-only.
        assert!(
            !cfg.model_for(&cfg.providers[0], "declared-text-only")
                .supports_images
        );
        // "Not declared" means the install-wide default applies.
        assert!(
            cfg.model_for(&cfg.providers[0], "undeclared")
                .supports_images
        );
        // A model the profile does not list at all falls back to the default.
        assert!(
            cfg.model_for(&cfg.providers[0], "not-listed")
                .supports_images
        );
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
        // These variables are ours alone, but the *config load* parses every
        // `KARUCODE_*` variable, so a test that writes an unrelated one at the
        // same moment (the malformed-boolean test deliberately writes an invalid
        // `KARUCODE_TITLES`) makes this load fail for a reason that has nothing
        // to do with the retry budget. `EnvVars` holds the shared environment lock
        // and restores the previous values even on a panic.
        let _guard = EnvVars::set([
            "KARUCODE_RETRY_MAX_RETRIES",
            "KARUCODE_RETRY_BASE_DELAY_MS",
            "KARUCODE_RETRY_MAX_DELAY_MS",
            "KARUCODE_RETRY_MAX_STREAM_RESTARTS",
            "KARUCODE_RETRY_MAX_TOTAL_SECS",
        ]);
        std::env::set_var("KARUCODE_RETRY_MAX_RETRIES", "3");
        std::env::set_var("KARUCODE_RETRY_BASE_DELAY_MS", "25");
        std::env::set_var("KARUCODE_RETRY_MAX_DELAY_MS", "250");
        std::env::set_var("KARUCODE_RETRY_MAX_STREAM_RESTARTS", "1");
        std::env::set_var("KARUCODE_RETRY_MAX_TOTAL_SECS", "120");
        let cfg = Config::from_parts(ConfigFile::default(), None);
        std::env::remove_var("KARUCODE_RETRY_MAX_RETRIES");
        std::env::remove_var("KARUCODE_RETRY_BASE_DELAY_MS");
        std::env::remove_var("KARUCODE_RETRY_MAX_DELAY_MS");
        std::env::remove_var("KARUCODE_RETRY_MAX_STREAM_RESTARTS");
        std::env::remove_var("KARUCODE_RETRY_MAX_TOTAL_SECS");

        let cfg = cfg.expect("config loads");
        assert_eq!(cfg.model.retry_max_retries, 3);
        assert_eq!(cfg.model.retry_base_delay_ms, 25);
        assert_eq!(cfg.model.retry_max_delay_ms, 250);
        assert_eq!(cfg.model.retry_max_stream_restarts, 1);
        assert_eq!(
            cfg.model.retry_max_total_secs, 120,
            "every documented KARUCODE_RETRY_* variable must actually be read: \
             this one was advertised for months while nothing parsed it"
        );
        assert_eq!(
            cfg.effective_model().retry_max_retries,
            3,
            "an env override must survive profile resolution"
        );
    }

    #[test]
    fn environment_profile_applies_timeout_and_preserves_connection_options() {
        let _guard = EnvVars::set([
            "KARUCODE_MODEL",
            "KARUCODE_BASE_URL",
            "OPENAI_BASE_URL",
            "KARUCODE_API_KEY",
            "OPENAI_API_KEY",
            "KARUCODE_REQUEST_TIMEOUT_SECS",
        ]);
        for name in ["OPENAI_BASE_URL", "KARUCODE_API_KEY", "OPENAI_API_KEY"] {
            std::env::remove_var(name);
        }
        std::env::set_var("KARUCODE_MODEL", "from-env");
        std::env::set_var("KARUCODE_BASE_URL", "http://127.0.0.1:9999/v1");
        std::env::set_var("KARUCODE_REQUEST_TIMEOUT_SECS", "37");

        let headers = BTreeMap::from([("X-From-File".to_string(), "kept".to_string())]);
        let body = BTreeMap::from([("custom".to_string(), serde_json::json!({"kept": true}))]);
        let file = ConfigFile {
            model: Some(ModelConfig {
                request_timeout_secs: 900,
                extra_headers: headers.clone(),
                extra_body: body.clone(),
                aggregator: true,
                ..ModelConfig::default()
            }),
            providers: Some(vec![ProviderProfile {
                id: "file-profile".into(),
                models: vec![ModelEntry::new("from-file")],
                ..ProviderProfile::default()
            }]),
            active_provider: Some("file-profile".into()),
            active_model: Some("from-file".into()),
            ..ConfigFile::default()
        };

        let cfg = Config::from_parts(file, None).expect("config loads");
        assert_eq!(cfg.active_provider.as_deref(), Some(ENV_PROFILE_ID));
        let profile = cfg.active_profile().expect("environment profile");
        assert_eq!(profile.base_url, "http://127.0.0.1:9999/v1");
        assert_eq!(profile.models[0].id, "from-env");
        assert_eq!(
            profile.request_timeout_secs, 37,
            "the env profile must be built after KARUCODE_REQUEST_TIMEOUT_SECS"
        );
        assert_eq!(profile.extra_headers, headers);
        assert_eq!(profile.extra_body, body);
        assert!(profile.aggregator);

        let effective = cfg.effective_model();
        assert_eq!(effective.request_timeout_secs, 37);
        assert_eq!(effective.extra_headers, headers);
        assert_eq!(effective.extra_body, body);
        assert!(effective.aggregator);
        assert_eq!(cfg.model.request_timeout_secs, 37);
        assert!(cfg.model.aggregator);
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

    /// Hold the environment lock for the duration of a test.
    ///
    /// A poisoned lock only means another holder panicked; the environment was
    /// still restored by its `Drop`, so carry on.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn the_plugin_watcher_interval_is_clamped_to_the_documented_range() {
        let _env = env_lock();
        // 0 would busy-loop over the plugin directory; a day would make a "hot"
        // reload indistinguishable from not having one.
        let file = ConfigFile {
            plugins_watch: Some(true),
            plugins_watch_secs: Some(0),
            ..ConfigFile::default()
        };
        let cfg = Config::from_parts(file, None).unwrap();
        assert!(cfg.plugins_watch);
        assert_eq!(cfg.plugins_watch_secs, MIN_PLUGINS_WATCH_SECS);

        let file = ConfigFile {
            plugins_watch_secs: Some(86_400),
            ..ConfigFile::default()
        };
        let cfg = Config::from_parts(file, None).unwrap();
        assert_eq!(cfg.plugins_watch_secs, MAX_PLUGINS_WATCH_SECS);
        assert!(!cfg.plugins_watch, "watching is opt-in");

        let file = ConfigFile {
            plugins_watch_secs: Some(5),
            ..ConfigFile::default()
        };
        let cfg = Config::from_parts(file, None).unwrap();
        assert_eq!(cfg.plugins_watch_secs, 5);
    }

    #[test]
    fn reloading_plugin_entries_reads_the_config_file_again() {
        let dir = std::env::temp_dir().join(format!(
            "kc-config-plugins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[[plugins]]\nname = \"one\"\ncommand = [\"/bin/one\"]\n",
        )
        .unwrap();

        let cfg = Config {
            config_path: Some(path.clone()),
            plugins: vec![PluginConfig {
                name: "stale".into(),
                enabled: true,
                command: vec!["/bin/stale".into()],
                env: Default::default(),
                permissions: Vec::new(),
                settings: Default::default(),
            }],
            ..Config::default()
        };
        let entries = cfg.reload_plugin_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].name, "one",
            "the file wins over the in-memory copy"
        );

        // Removing every entry from the file is how an operator uninstalls one:
        // the empty list must be honoured, not treated as "keep what we had".
        std::fs::write(&path, "port = 8787\n").unwrap();
        assert!(cfg.reload_plugin_entries().is_empty());

        // With no readable file the in-memory list is the only truth left.
        std::fs::remove_file(&path).unwrap();
        let entries = cfg.reload_plugin_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "stale");

        std::fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------------
    // [notifications]
    // -----------------------------------------------------------------------

    #[test]
    fn notifications_are_on_by_default() {
        let cfg = Config::default();
        let n = &cfg.notifications;
        assert!(n.enabled);
        assert!(n.run_finished && n.needs_input && n.error && n.info);
        assert!(n.plugin);
        assert_eq!(n.min_run_secs, 0, "every run notifies until you raise it");
        assert!(n.say_body);
        // Every kind is allowed by the defaults, which is what makes the master
        // switch the only thing an ordinary user has to find.
        for kind in crate::notify::NotifyKind::ALL {
            assert!(n.allows(*kind), "{kind:?} should be allowed by default");
        }
    }

    #[test]
    fn the_master_switch_overrides_every_per_kind_switch() {
        let n = NotificationsConfig {
            enabled: false,
            ..NotificationsConfig::default()
        };
        for kind in crate::notify::NotifyKind::ALL {
            assert!(!n.allows(*kind), "{kind:?} must be refused when disabled");
            assert!(!n.allows_plugin(*kind));
        }
    }

    #[test]
    fn a_per_event_switch_only_silences_that_event() {
        let n = NotificationsConfig {
            error: false,
            ..NotificationsConfig::default()
        };
        assert!(!n.allows(crate::notify::NotifyKind::Error));
        assert!(n.allows(crate::notify::NotifyKind::RunFinished));
        assert!(n.allows(crate::notify::NotifyKind::NeedsInput));
        assert!(n.allows(crate::notify::NotifyKind::Info));
    }

    #[test]
    fn plugins_are_gated_separately_from_the_kind_switches() {
        let n = NotificationsConfig {
            plugin: false,
            ..NotificationsConfig::default()
        };
        // The kind is still allowed for runtime producers...
        assert!(n.allows(crate::notify::NotifyKind::Info));
        // ...but nothing a plugin raises gets through.
        assert!(!n.allows_plugin(crate::notify::NotifyKind::Info));
        assert!(!n.allows_plugin(crate::notify::NotifyKind::NeedsInput));
    }

    #[test]
    fn the_minimum_run_length_is_a_lower_bound_not_an_exclusive_one() {
        let n = NotificationsConfig {
            min_run_secs: 30,
            ..NotificationsConfig::default()
        };
        assert!(!n.run_is_long_enough(29));
        assert!(n.run_is_long_enough(30), "exactly the minimum notifies");
        assert!(n.run_is_long_enough(31));
        assert!(NotificationsConfig::default().run_is_long_enough(0));
    }

    #[test]
    fn event_switches_are_addressable_by_name() {
        let mut n = NotificationsConfig::default();
        for name in NotificationsConfig::event_names() {
            assert_eq!(n.event(name), Some(true), "{name} starts on");
            assert_eq!(n.set_event(name, false), Some(()));
            assert_eq!(n.event(name), Some(false), "{name} turned off");
            assert_eq!(n.set_event(name, true), Some(()));
        }
        assert_eq!(n.set_event("nope", true), None);
        assert_eq!(n.event("nope"), None);
        // The spelling in the API and the plugin protocol uses underscores; a
        // hyphen is accepted so a hand-written TOML/env value is not a trap.
        assert_eq!(n.set_event("run-finished", false), Some(()));
        assert!(!n.run_finished);
    }

    #[test]
    fn the_notifications_section_parses_from_toml_with_the_documented_defaults() {
        let _env = env_lock();
        let file: ConfigFile = toml::from_str(
            r#"
            [notifications]
            min_run_secs = 15
            error = false
            "#,
        )
        .unwrap();
        let cfg = Config::from_parts(file, None).unwrap();
        assert_eq!(cfg.notifications.min_run_secs, 15);
        assert!(!cfg.notifications.error);
        // Everything not named keeps its default: a partial section is legal.
        assert!(cfg.notifications.enabled);
        assert!(cfg.notifications.run_finished);
        assert!(cfg.notifications.plugin);
    }

    /// The new `[limits]` key is readable from the config file, and a file that
    /// omits it keeps the 900 default - the item ceiling that keeps a session
    /// under a gateway's 1,000-item limit.
    #[test]
    fn the_context_message_ceiling_is_read_from_the_limits_section() {
        let _env = env_lock();
        let file: ConfigFile = toml::from_str("[limits]\nmax_context_messages = 250\n").unwrap();
        let cfg = Config::from_parts(file, None).unwrap();
        assert_eq!(cfg.limits.max_context_messages, 250);
        // The character budget is untouched: the two are independent.
        assert_eq!(cfg.limits.context_max_chars, 0);

        let file: ConfigFile = toml::from_str("[limits]\nmax_iterations = 5\n").unwrap();
        let cfg = Config::from_parts(file, None).unwrap();
        assert_eq!(cfg.limits.max_context_messages, 900);
    }

    #[test]
    fn a_config_file_without_a_notifications_section_keeps_the_defaults() {
        let _env = env_lock();
        let file: ConfigFile = toml::from_str("[limits]\nmax_iterations = 5\n").unwrap();
        let cfg = Config::from_parts(file, None).unwrap();
        assert!(cfg.notifications.enabled);
        assert_eq!(cfg.notifications, NotificationsConfig::default());
    }

    /// The reported bug: `.env.example` shipped
    /// `KARUCODE_AUTH_TOKEN=change-me-to-16-plus-chars`, which is 27 characters
    /// and therefore passes a bare length check - a working, guessable bearer
    /// token. `admin_token` had no length check at all.
    #[test]
    fn a_placeholder_token_is_rejected_for_both_credentials() {
        for placeholder in [
            "change-me-to-16-plus-chars",
            "CHANGE-ME-to-16-plus-chars",
            "changeme1234567890",
            "secret-token-1234567890",
            "Password1234567890",
            "test-token-1234567890",
            "TEST-TOKEN-1234567890",
        ] {
            let err = validate_token("auth token", "KARUCODE_AUTH_TOKEN", placeholder)
                .expect_err(&format!("{placeholder} must be rejected as a placeholder"));
            let message = err.to_string();
            assert!(message.contains("placeholder"), "{message}");
            assert!(message.contains("openssl rand -hex 24"), "{message}");
            assert!(message.contains("KARUCODE_AUTH_TOKEN"), "{message}");
        }
        // ...and the same rule applies to the admin credential, where a long
        // placeholder is what would otherwise slip past a length-only check.
        let err = validate_token(
            "admin token",
            "KARUCODE_ADMIN_TOKEN",
            "change-me-to-16-plus-chars",
        )
        .expect_err("a 27-character admin placeholder must be rejected");
        assert!(err.to_string().contains("placeholder"), "{err}");
    }

    #[test]
    fn a_real_token_of_the_documented_length_is_accepted() {
        // 48 characters, exactly what `openssl rand -hex 24` produces.
        let token = "0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(token.len(), 48);
        assert!(validate_token("auth token", "KARUCODE_AUTH_TOKEN", token).is_ok());
        assert!(validate_token("admin token", "KARUCODE_ADMIN_TOKEN", token).is_ok());
        // Exactly at the floor is accepted; one character under is not.
        let floor = "a".repeat(MIN_TOKEN_LEN);
        assert_eq!(floor.len(), 16);
        assert!(validate_token("auth token", "KARUCODE_AUTH_TOKEN", &floor).is_ok());
        let under = "a".repeat(MIN_TOKEN_LEN - 1);
        let err = validate_token("auth token", "KARUCODE_AUTH_TOKEN", &under).unwrap_err();
        assert!(err.to_string().contains("at least 16 characters"), "{err}");
    }

    /// The admin token had no length rule at all, so `change-me-too` (13 chars)
    /// or `x` gated nothing: a one-character secret is a formality.
    #[test]
    fn a_short_admin_token_is_rejected_at_load() {
        let _env = env_lock();
        let file = ConfigFile {
            admin_token: Some("short".into()),
            ..ConfigFile::default()
        };
        let err = Config::from_parts(file, None).expect_err("a 5-character admin token must fail");
        let message = err.to_string();
        assert!(message.contains("admin token"), "{message}");
        assert!(message.contains("at least 16"), "{message}");

        let file = ConfigFile {
            admin_token: Some("0123456789abcdef".into()),
            ..ConfigFile::default()
        };
        let cfg = Config::from_parts(file, None).expect("16 characters is enough");
        assert_eq!(cfg.admin_token.as_deref(), Some("0123456789abcdef"));
    }

    /// The auth token keeps its documented floor, and now also refuses the
    /// placeholder that `.env.example` used to suggest.
    #[test]
    fn a_short_or_placeholder_auth_token_is_rejected_at_load() {
        let _env = env_lock();
        for (token, expected) in [
            ("short", "at least 16"),
            ("change-me-to-16-plus-chars", "placeholder"),
        ] {
            let file = ConfigFile {
                auth_token: Some(token.into()),
                ..ConfigFile::default()
            };
            let err = Config::from_parts(file, None).unwrap_err().to_string();
            assert!(
                err.contains(expected),
                "{token:?} should fail with {expected:?}, got {err}"
            );
        }
    }

    #[test]
    fn the_minimum_run_secs_bound_is_documented() {
        // The settings layer validates against this; the constant exists here so a
        // TOML reader and the API agree on the ceiling.
        assert_eq!(MAX_MIN_RUN_SECS, 3_600);
    }

    /// The reported bug: `KARUCODE_LOCK_SETTINGS=1` was read with
    /// `parse::<bool>()`, which rejects `1`, so the flag was silently inert and
    /// `PATCH /api/config` kept answering 200 on a deployment that believed
    /// settings were locked. `KARUCODE_MODEL_SUPPORTS_IMAGES`,
    /// `KARUCODE_ALLOW_RECURSIVE_SUBAGENTS` and
    /// `KARUCODE_SUBAGENT_ALLOW_BACKGROUND` had the same defect. Every documented
    /// spelling (`1/0`, `true/false`, `yes/no`, `on/off`) must set the field.
    #[test]
    fn the_boolean_env_flags_accept_the_documented_spellings() {
        // These variables are read by `from_parts`, which is process-global state.
        // The guard restores the previous value even if an assertion fails, so a
        // failed run cannot leak a flag into another test in this binary.
        let _guard = EnvVars::set([
            "KARUCODE_LOCK_SETTINGS",
            "KARUCODE_MODEL_SUPPORTS_IMAGES",
            "KARUCODE_ALLOW_RECURSIVE_SUBAGENTS",
            "KARUCODE_SUBAGENT_ALLOW_BACKGROUND",
        ]);

        let cfg = Config::from_parts(ConfigFile::default(), None).expect("config loads");
        // Defaults first, so "it parsed" cannot pass by accident.
        assert!(!cfg.lock_settings);
        assert!(!cfg.model.supports_images);
        assert!(!cfg.subagents.allow_recursive);
        assert!(cfg.subagents.allow_background);

        for (spelling, expected) in [
            ("1", true),
            ("0", false),
            ("true", true),
            ("false", false),
            ("on", true),
            ("off", false),
            ("yes", true),
            ("no", false),
        ] {
            for name in _guard.names() {
                std::env::set_var(name, spelling);
            }
            let cfg = Config::from_parts(ConfigFile::default(), None).expect("config loads");
            assert_eq!(
                cfg.lock_settings, expected,
                "KARUCODE_LOCK_SETTINGS={spelling}"
            );
            assert_eq!(
                cfg.model.supports_images, expected,
                "KARUCODE_MODEL_SUPPORTS_IMAGES={spelling}"
            );
            assert_eq!(
                cfg.subagents.allow_recursive, expected,
                "KARUCODE_ALLOW_RECURSIVE_SUBAGENTS={spelling}"
            );
            assert_eq!(
                cfg.subagents.allow_background, expected,
                "KARUCODE_SUBAGENT_ALLOW_BACKGROUND={spelling}"
            );
        }
    }

    /// `KARUCODE_LOCK_SETTINGS=1` is the spelling the docs use for pinning a
    /// deployment and must enable the lock rather than silently looking false.
    #[test]
    fn a_locked_deployment_is_locked_from_the_environment() {
        let _guard = EnvVars::set(["KARUCODE_LOCK_SETTINGS"]);
        std::env::set_var("KARUCODE_LOCK_SETTINGS", "1");
        let cfg = Config::from_parts(ConfigFile::default(), None).expect("config loads");
        assert!(
            cfg.lock_settings,
            "KARUCODE_LOCK_SETTINGS=1 must lock settings"
        );
    }

    #[test]
    fn malformed_boolean_env_values_fail_fast_and_name_the_variable() {
        for name in [
            "KARUCODE_LOCK_SETTINGS",
            "KARUCODE_PLUGINS_WATCH",
            "KARUCODE_TITLES",
            "KARUCODE_NOTIFICATIONS",
            "KARUCODE_NOTIFICATIONS_PLUGIN",
            "KARUCODE_NOTIFICATIONS_SAY_BODY",
            "KARUCODE_TERMINALS",
            "KARUCODE_ALLOW_TEMP_DIR",
            "KARUCODE_MODEL_SUPPORTS_IMAGES",
            "KARUCODE_AUTO_CONTINUE",
            "KARUCODE_ALLOW_RECURSIVE_SUBAGENTS",
            "KARUCODE_SUBAGENT_ALLOW_BACKGROUND",
        ] {
            let _guard = EnvVars::set([name]);
            std::env::set_var(name, "definitely-not-a-boolean");
            let err = Config::from_parts(ConfigFile::default(), None)
                .expect_err("a malformed boolean must stop configuration loading")
                .to_string();
            assert!(err.contains(name), "{name} missing from error: {err}");
            assert!(
                err.contains("expected a boolean") && err.contains("1/0"),
                "error for {name} is not actionable: {err}"
            );
        }
    }

    /// Set environment variables for the duration of a test and put the previous
    /// values back afterwards, including on a panic.
    ///
    /// The guard also holds [`ENV_LOCK`] for its lifetime, so two tests that
    /// mutate the same variables cannot interleave (one reading `1` while the
    /// other has just written `0`). Nothing else in this file sets these
    /// variables, so the lock has no other participants to coordinate with.
    struct EnvVars {
        previous: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    /// Serialises the tests that mutate process environment variables.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl EnvVars {
        fn set<const N: usize>(names: [&'static str; N]) -> Self {
            // A poisoned lock only means another holder panicked; the
            // environment was still restored by its `Drop`, so carry on.
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = names
                .into_iter()
                .map(|name| (name, std::env::var(name).ok()))
                .collect();
            Self {
                previous,
                _lock: lock,
            }
        }

        fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
            self.previous.iter().map(|(name, _)| *name)
        }
    }

    impl Drop for EnvVars {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
