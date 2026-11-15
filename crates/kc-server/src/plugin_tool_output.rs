//! Per-plugin tool-output budgets: the operator's decisions, and the ask.
//!
//! `limits.max_tool_output_bytes` (24,000 bytes) is the ceiling on how much of
//! one tool result the model gets to see, for every tool. A single plugin may be
//! given its own budget instead - higher or lower - and this module is the whole
//! of that feature's write side.
//!
//! # Who can create a grant
//!
//! Nobody but the operator. Concretely:
//!
//! * the **agent** files a [`Request`](kc_core::plugin::tool_output::Request)
//!   (`POST /api/plugins/:name/tool-output/request`, or the CLI that wraps it).
//!   A request grants nothing: until an answer is recorded the plugin keeps the
//!   global default.
//! * the **operator** answers it (`POST /api/plugins/:name/tool-output/decision`,
//!   which takes the admin gate when the install has one, or the CLI verb that
//!   refuses to run without a terminal). The answer is written to the store as a
//!   [`Record`](kc_core::plugin::tool_output::Record) - who, when, which surface,
//!   what was asked, why - and into the shared grant map the plugin host reads on
//!   every tool call.
//! * a **plugin** cannot, anywhere: `plugin.toml` in its own directory is
//!   stripped of the key with a warning by the host, an installed manifest is
//!   stripped by `plugin install`, and `[[plugins]] tool_output_bytes` in the
//!   operator's config file only *narrows* what the operator already granted.
//!
//! # One ask is one ask
//!
//! A request is refused while an answer is on record. Silence, a dismissal and a
//! timeout are all a refusal, and a refusal is remembered: re-asking needs the
//! operator to clear it first. That is what keeps "ask the operator for a bigger
//! budget" from turning into a loop, and it is why nothing re-prompts at startup.

use std::sync::Arc;

use kc_core::error::Error;
use kc_core::plugin::tool_output::{
    clamp_bytes, Decision, Event, Grants, Record, Request, DEFAULT_WAIT_SECS, MAX_BYTES,
    MAX_WAIT_SECS, MIN_BYTES,
};
use kc_store::SqliteStore;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{ApiError, ApiResult};
use crate::settings::SettingsHandle;

/// The surface that answered, recorded with the decision.
pub const CHANNEL_WEB: &str = "web";
/// A decision taken from a terminal (the CLI refuses without one).
pub const CHANNEL_CLI: &str = "cli-tty";
/// A request or decision that came over the API without a declared surface.
pub const CHANNEL_API: &str = "api";
/// The operator never answered: a timeout or a dismissal is a refusal.
pub const CHANNEL_TIMEOUT: &str = "timeout";
/// Who is recorded when nobody answered.
pub const DECIDED_BY_NO_ANSWER: &str = "no answer";

/// What `GET /api/plugins/:name/tool-output` answers, and what the plugin
/// listing carries per plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputView {
    pub plugin: String,
    /// The bytes the host really applies to one result from this plugin.
    pub effective_bytes: usize,
    /// `true` when that number is a grant, `false` when it is the global default.
    pub granted: bool,
    /// `limits.max_tool_output_bytes`, for comparison in the UI.
    pub global_bytes: usize,
    /// `limits.max_plugin_tool_output_bytes`: the largest grant this install
    /// accepts.
    pub ceiling_bytes: usize,
    /// The operator's `[[plugins]] tool_output_bytes`, if the config sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_bytes: Option<usize>,
    /// The answer on record, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Record>,
    /// The ask waiting for an answer, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Request>,
    /// The append-only trail, oldest first.
    #[serde(default)]
    pub history: Vec<Event>,
}

impl ToolOutputView {
    /// The compact form the plugin listing carries: the effective number, who
    /// granted it and when, and whether an ask is waiting.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "effective_bytes": self.effective_bytes,
            "granted": self.granted,
            "global_bytes": self.global_bytes,
            "ceiling_bytes": self.ceiling_bytes,
            "declared_bytes": self.declared_bytes,
            "granted_bytes": self.decision.as_ref().and_then(|d| d.granted_bytes()),
            "decided_by": self.decision.as_ref().map(|d| d.decided_by.clone()),
            "decided_at": self.decision.as_ref().map(|d| d.decided_at),
            "reason": self.decision.as_ref().map(|d| d.reason.clone()),
            "pending": self.pending.as_ref().map(|p| serde_json::json!({
                "bytes": p.bytes,
                "reason": p.reason,
                "requested_at": p.requested_at,
            })),
        })
    }
}

/// The service behind the three routes, and the startup load of stored answers.
pub struct PluginToolOutput {
    store: Arc<SqliteStore>,
    grants: Arc<Grants>,
    settings: Arc<SettingsHandle>,
    /// The live registry, consulted for "does this plugin exist". It is the same
    /// source `GET /api/plugins` reads, so the listing and a budget request can
    /// never disagree about which names are real.
    plugins: Arc<tokio::sync::Mutex<kc_plugin_host::PluginHost>>,
}

impl PluginToolOutput {
    /// Build the service and load every stored decision into `grants`.
    ///
    /// A store that cannot be read is *not* fatal: the install keeps the global
    /// default for every plugin and logs why, which is the safe direction.
    pub async fn load(
        store: Arc<SqliteStore>,
        grants: Arc<Grants>,
        settings: Arc<SettingsHandle>,
        plugins: Arc<tokio::sync::Mutex<kc_plugin_host::PluginHost>>,
    ) -> Self {
        let service = Self {
            store,
            grants,
            settings,
            plugins,
        };
        service.reload().await;
        service
    }

    /// Re-read the stored decisions. Called at startup and after any write, so a
    /// decision survives a restart and applies to the very next tool call.
    pub async fn reload(&self) -> usize {
        match self.store.plugin_tool_output_all().await {
            Ok(records) => {
                let count = records.len();
                for record in records {
                    self.grants.set(record);
                }
                count
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "could not read the per-plugin tool-output decisions; every plugin keeps the \
                     global limits.max_tool_output_bytes default"
                );
                0
            }
        }
    }

    pub fn grants(&self) -> Arc<Grants> {
        Arc::clone(&self.grants)
    }

    /// `limits.max_tool_output_bytes`: the default for any plugin without a grant.
    pub fn global_bytes(&self) -> usize {
        self.settings.snapshot().limits.max_tool_output_bytes
    }

    /// The largest grant this install accepts.
    pub fn ceiling_bytes(&self) -> usize {
        self.settings.snapshot().limits.plugin_tool_output_ceiling()
    }

    /// The operator's `[[plugins]] tool_output_bytes`, which may only narrow.
    pub fn declared_bytes(&self, plugin: &str) -> Option<usize> {
        self.settings
            .snapshot()
            .plugins
            .iter()
            .find(|p| p.name == plugin)
            .and_then(|p| p.tool_output_bytes)
    }

    /// The budget the host applies, and whether it came from a grant.
    pub fn effective_bytes(&self, plugin: &str) -> (usize, bool) {
        match self
            .grants
            .effective_bytes(plugin, self.declared_bytes(plugin))
        {
            Some(granted) => (granted.min(self.ceiling_bytes()), true),
            None => (self.global_bytes(), false),
        }
    }

    /// Everything the UI or a client needs about one plugin's budget.
    pub async fn view(&self, plugin: &str) -> ApiResult<ToolOutputView> {
        self.refuse_unknown(plugin).await?;
        let (effective_bytes, granted) = self.effective_bytes(plugin);
        Ok(ToolOutputView {
            plugin: plugin.to_string(),
            effective_bytes,
            granted,
            global_bytes: self.global_bytes(),
            ceiling_bytes: self.ceiling_bytes(),
            declared_bytes: self.declared_bytes(plugin),
            decision: self
                .store
                .plugin_tool_output(plugin)
                .await
                .map_err(store_error)?,
            pending: self
                .store
                .plugin_tool_output_request(plugin)
                .await
                .map_err(store_error)?,
            history: self
                .store
                .plugin_tool_output_events(plugin)
                .await
                .map_err(store_error)?,
        })
    }

    /// The compact per-plugin summary for `GET /api/plugins`, or `None` if the
    /// plugin is unknown. A failed read degrades to the global default rather
    /// than failing the listing.
    pub async fn summary(&self, plugin: &str) -> serde_json::Value {
        match self.view(plugin).await {
            Ok(view) => view.summary(),
            Err(_) => serde_json::json!({
                "effective_bytes": self.global_bytes(),
                "granted": false,
                "global_bytes": self.global_bytes(),
                "ceiling_bytes": self.ceiling_bytes(),
                "pending": null,
            }),
        }
    }

    /// How long a request waits for an answer before the silence is a refusal.
    pub fn wait_secs(requested: Option<u64>) -> u64 {
        requested
            .unwrap_or(DEFAULT_WAIT_SECS)
            .clamp(1, MAX_WAIT_SECS)
    }

    /// File an ask. Grants nothing; it only makes the question answerable.
    pub async fn request(
        &self,
        plugin: &str,
        bytes: usize,
        reason: &str,
    ) -> ApiResult<ToolOutputView> {
        self.refuse_unknown(plugin).await?;
        let bytes = self.validate(bytes)?;
        if let Some(record) = self
            .store
            .plugin_tool_output(plugin)
            .await
            .map_err(store_error)?
        {
            return Err(ApiError::conflict(format!(
                "the operator already answered this on {} ({}): {}. Only they can change it, in \
                 Settings → Plugins & MCP or with `karucode plugin tool-output decide`. Do not ask \
                 again.",
                record.decided_at,
                record.decision.as_str(),
                if record.reason.trim().is_empty() {
                    "no reason given"
                } else {
                    record.reason.as_str()
                }
            )));
        }
        let filed = self
            .store
            .request_plugin_tool_output(&Request {
                plugin: plugin.to_string(),
                bytes,
                reason: reason.trim().to_string(),
                requested_at: now_ms(),
            })
            .await
            .map_err(store_error)?;
        if !filed {
            return Err(ApiError::conflict(
                "the operator has already decided this plugin's tool-output budget, so the request \
                 was not filed. Only they can reopen it.",
            ));
        }
        self.view(plugin).await
    }

    /// Record the operator's answer. The only path that creates a grant.
    pub async fn decide(
        &self,
        plugin: &str,
        decision: Decision,
        bytes: Option<usize>,
        reason: &str,
        channel: &str,
    ) -> ApiResult<ToolOutputView> {
        self.refuse_unknown(plugin).await?;
        // A timeout can only ever refuse. The channel is named by the caller, so
        // without this an unattended `karucode plugin tool-output request` that
        // ran out of time could report a grant nobody gave.
        let decision = if channel == CHANNEL_TIMEOUT {
            Decision::Refused
        } else {
            decision
        };
        let pending = self
            .store
            .plugin_tool_output_request(plugin)
            .await
            .map_err(store_error)?;
        let requested_bytes = match (bytes, &pending) {
            (Some(bytes), _) => bytes,
            (None, Some(pending)) => pending.bytes,
            (None, None) => self.global_bytes(),
        };
        let granted = match decision {
            Decision::Granted => Some(self.validate(requested_bytes)?),
            Decision::Refused => None,
        };
        let record = Record {
            plugin: plugin.to_string(),
            decision,
            bytes: granted,
            requested_bytes,
            reason: if reason.trim().is_empty() {
                pending.map(|p| p.reason).unwrap_or_default()
            } else {
                reason.trim().to_string()
            },
            decided_by: if channel == CHANNEL_TIMEOUT {
                DECIDED_BY_NO_ANSWER.to_string()
            } else {
                "operator".to_string()
            },
            channel: channel.to_string(),
            decided_at: now_ms(),
        };
        self.store
            .decide_plugin_tool_output(&record)
            .await
            .map_err(store_error)?;
        self.grants.set(record);
        self.view(plugin).await
    }

    /// Forget a decision so the operator can be asked again.
    pub async fn clear(&self, plugin: &str, channel: &str) -> ApiResult<ToolOutputView> {
        self.refuse_unknown(plugin).await?;
        self.store
            .clear_plugin_tool_output(plugin, &format!("operator ({channel})"), now_ms())
            .await
            .map_err(store_error)?;
        self.grants.clear(plugin);
        self.view(plugin).await
    }

    /// Reject a value outside the range a grant may carry, naming both numbers so
    /// the operator can pick a legal one instead of guessing.
    fn validate(&self, bytes: usize) -> ApiResult<usize> {
        let ceiling = self.ceiling_bytes();
        if bytes < MIN_BYTES {
            return Err(ApiError::bad_request(format!(
                "{bytes} bytes is below the smallest tool-output budget ({MIN_BYTES}). A result \
                 that small cannot carry the header and the truncation marker, let alone the \
                 answer."
            )));
        }
        if bytes > ceiling {
            return Err(ApiError::bad_request(format!(
                "{bytes} bytes is above this install's ceiling of {ceiling} bytes \
                 (limits.max_plugin_tool_output_bytes; the accepted range is {MIN_BYTES}..={MAX_BYTES}). \
                 Nothing above it would survive the turn's share of the context window anyway."
            )));
        }
        Ok(clamp_bytes(bytes))
    }

    /// A budget belongs to a plugin that exists; asking for one on a name the
    /// host does not know is a typo, not a request.
    ///
    /// "Exists" is the registry's own answer, not the config file's: a plugin
    /// installed as a disk manifest at `<data_dir>/plugins/<name>/plugin.toml`
    /// has no `[[plugins]]` entry, and one that failed to start still owns its
    /// name. Both are askable, exactly as both are listed by `GET /api/plugins`.
    async fn refuse_unknown(&self, plugin: &str) -> ApiResult<()> {
        let known = {
            let host = self.plugins.lock().await;
            host.knows(plugin)
        };
        if known {
            return Ok(());
        }
        Err(ApiError::not_found(format!(
            "no plugin named `{plugin}` is loaded. Add its [[plugins]] entry (or install it, and \
             reload if the server is already running) first; then ask for a tool-output budget."
        )))
    }
}

fn store_error(error: Error) -> ApiError {
    ApiError::internal(format!(
        "could not read or write the tool-output decision: {error}"
    ))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_is_clamped_to_a_sane_window() {
        assert_eq!(PluginToolOutput::wait_secs(None), DEFAULT_WAIT_SECS);
        assert_eq!(PluginToolOutput::wait_secs(Some(0)), 1, "0 is not instant");
        assert_eq!(PluginToolOutput::wait_secs(Some(5)), 5);
        assert_eq!(
            PluginToolOutput::wait_secs(Some(86_400)),
            MAX_WAIT_SECS,
            "a request cannot park a run for a day"
        );
    }
}
// ---------------------------------------------------------------------------
// `karucode plugin tool-output …` — the CLI half of the ask
// ---------------------------------------------------------------------------

/// What the CLI was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputCommand {
    /// File the ask and wait for the operator's answer. The agent's verb.
    Request,
    /// Print the record.
    Show,
    /// Record the operator's answer. Needs a terminal.
    Decide,
    /// Forget the answer. Needs a terminal.
    Clear,
}

/// `karucode plugin tool-output <command> …`, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutputArgs {
    pub command: ToolOutputCommand,
    pub plugin: Option<String>,
    pub bytes: Option<usize>,
    pub reason: String,
    pub wait_secs: Option<u64>,
    /// `Some(true)` = grant, `Some(false)` = refuse.
    pub decision: Option<bool>,
    pub url: Option<String>,
    pub config: Option<std::path::PathBuf>,
}

/// The server the CLI talks to, resolved from the config file like the server's
/// own flags do.
fn server_url(explicit: Option<&str>, config: &kc_core::config::Config) -> String {
    if let Some(url) = explicit {
        return url.trim_end_matches('/').to_string();
    }
    let host = if config.host.is_empty() || config.host == "0.0.0.0" || config.host == "::" {
        "127.0.0.1"
    } else {
        config.host.as_str()
    };
    format!("http://{host}:{}", config.port)
}

fn open_config(config: Option<&std::path::Path>) -> anyhow::Result<kc_core::config::Config> {
    if let Some(path) = config {
        if !path.exists() {
            anyhow::bail!(
                "config file {} does not exist; create it or omit --config",
                path.display()
            );
        }
        std::env::set_var("KARUCODE_CONFIG", path);
    }
    kc_core::config::Config::load()
        .map_err(|e| anyhow::anyhow!("could not load configuration: {e}"))
}

/// Parse the words after `plugin tool-output`.
pub fn parse_tool_output(words: &[String]) -> anyhow::Result<ToolOutputArgs> {
    let command = match words.first().map(String::as_str) {
        Some("request") => ToolOutputCommand::Request,
        Some("show") => ToolOutputCommand::Show,
        Some("decide") => ToolOutputCommand::Decide,
        Some("clear") => ToolOutputCommand::Clear,
        Some(other) => anyhow::bail!(
            "unknown `plugin tool-output` command `{other}`; expected `request`, `show`, `decide` \
             or `clear`"
        ),
        None => anyhow::bail!(
            "`karucode plugin tool-output` needs a command: `request <plugin> --bytes <n> \
             --reason \"...\"`, `show [<plugin>]`, `decide <plugin> --allow|--refuse`, or \
             `clear <plugin>`"
        ),
    };
    let mut args = ToolOutputArgs {
        command,
        plugin: None,
        bytes: None,
        reason: String::new(),
        wait_secs: None,
        decision: None,
        url: None,
        config: None,
    };
    let mut i = 1usize;
    while i < words.len() {
        let word = &words[i];
        let (flag, inline) = match word.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (word.clone(), None),
        };
        match flag.as_str() {
            "--bytes" => {
                args.bytes = Some(
                    take_word("--bytes", inline, words, &mut i)?
                        .parse()
                        .map_err(|_| {
                            anyhow::anyhow!("`--bytes` must be a whole number of bytes")
                        })?,
                )
            }
            "--reason" => args.reason = take_word("--reason", inline, words, &mut i)?,
            "--wait" => {
                args.wait_secs = Some(
                    take_word("--wait", inline, words, &mut i)?
                        .parse()
                        .map_err(|_| anyhow::anyhow!("`--wait` must be a number of seconds"))?,
                )
            }
            "--url" => args.url = Some(take_word("--url", inline, words, &mut i)?),
            "--config" => {
                args.config = Some(std::path::PathBuf::from(take_word(
                    "--config", inline, words, &mut i,
                )?))
            }
            "--allow" | "--grant" => args.decision = Some(true),
            "--refuse" | "--deny" => args.decision = Some(false),
            other if other.starts_with('-') => {
                anyhow::bail!("unknown option `{other}` for `plugin tool-output`")
            }
            _ => {
                if args.plugin.is_some() {
                    anyhow::bail!(
                        "`plugin tool-output` takes one plugin name; `{}` is extra",
                        words[i]
                    );
                }
                args.plugin = Some(words[i].clone());
            }
        }
        i += 1;
    }
    Ok(args)
}

fn take_word(
    flag: &str,
    inline: Option<String>,
    words: &[String],
    i: &mut usize,
) -> anyhow::Result<String> {
    match inline {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => anyhow::bail!("`{flag}` requires a non-empty value"),
        None => {
            *i += 1;
            words
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("`{flag}` requires a value"))
        }
    }
}

/// Refuse an operator-only verb unless this really is a person at a terminal.
///
/// There is no flag, environment variable or non-interactive path around this:
/// `run_command` has no terminal, so an agent cannot answer its own request.
fn require_terminal(what: &str) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return Ok(());
    }
    anyhow::bail!(
        "`{what}` is the operator's decision, so it runs only in an interactive terminal. Start a \
         browser or a shell and answer the request there; an agent cannot answer it on your behalf"
    )
}

/// Run one `karucode plugin tool-output …` command.
pub async fn cli(args: ToolOutputArgs) -> anyhow::Result<()> {
    let config = open_config(args.config.as_deref())?;
    let base = server_url(args.url.as_deref(), &config);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("could not build the HTTP client: {e}"))?;

    let plugin = match (&args.plugin, args.command) {
        (Some(plugin), ToolOutputCommand::Show) => Some(plugin.clone()),
        (Some(plugin), _) => Some(plugin.clone()),
        (None, ToolOutputCommand::Show) => None,
        (None, _) => anyhow::bail!("this command needs a plugin name"),
    };

    let request = |method: reqwest::Method, path: String| {
        let mut builder = client.request(method, format!("{base}{path}"));
        if let Some(token) = config.admin_token.as_deref() {
            builder = builder.header(crate::settings::ADMIN_TOKEN_HEADER, token);
        }
        if let Some(token) = config.auth_token.as_deref() {
            builder = builder.bearer_auth(token);
        }
        // The CSRF guard on every mutating route wants the literal `web` client
        // header. The *surface* that answered travels in the body's `channel`
        // instead, which is what the audit record keeps.
        builder.header("x-karucode-client", CHANNEL_WEB)
    };

    match args.command {
        ToolOutputCommand::Show => {
            let Some(plugin) = plugin else {
                // Without a name: print every plugin that has a record, so an
                // operator can see what they have granted without hunting.
                let list: serde_json::Value =
                    request(reqwest::Method::GET, "/api/plugins".to_string())
                        .send()
                        .await
                        .map_err(network_error)?
                        .json()
                        .await
                        .map_err(network_error)?;
                let plugins = list.get("plugins").and_then(|v| v.as_array());
                let Some(plugins) = plugins else {
                    println!("the server did not answer with a plugin list");
                    return Ok(());
                };
                println!("{:<24} {:>10}  source", "plugin", "budget");
                for entry in plugins {
                    let Some(name) = entry.get("name").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let budget = entry.pointer("/tool_output/effective_bytes");
                    let granted = entry
                        .pointer("/tool_output/granted")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    println!(
                        "{:<24} {:>10}  {}",
                        name,
                        budget.and_then(|v| v.as_u64()).unwrap_or(0),
                        if granted {
                            "operator grant"
                        } else {
                            "limits.max_tool_output_bytes"
                        }
                    );
                }
                return Ok(());
            };
            let view = get_view(&request, &plugin).await?;
            print_view(&view);
            Ok(())
        }
        ToolOutputCommand::Request => {
            let plugin = plugin.expect("checked above");
            let bytes = args.bytes.ok_or_else(|| {
                anyhow::anyhow!(
                    "`plugin tool-output request` needs the exact value: `--bytes <n>`. Do not \
                     guess one - name the number you want and why."
                )
            })?;
            let reason = args.reason.trim();
            if reason.is_empty() {
                anyhow::bail!(
                    "`--reason` is required: the operator decides from it, so say what the plugin \
                     returns and why {bytes} bytes is the right number"
                );
            }
            let created: serde_json::Value = request(
                reqwest::Method::POST,
                format!("/api/plugins/{plugin}/tool-output/request"),
            )
            .json(&serde_json::json!({ "bytes": bytes, "reason": reason }))
            .send()
            .await
            .map_err(network_error)?
            .error_for_status()
            .map_err(http_error)?
            .json()
            .await
            .map_err(network_error)?;
            let global = created
                .get("global_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(24_000);
            println!(
                "Asked the operator: allow plugin `{plugin}` to return up to {bytes} bytes per \
                 tool result (the global default is {global})?\n\
                 Reason given: {reason}\n\
                 Waiting up to {}s for an answer; silence counts as no. Answer in Settings → \
                 Plugins & MCP.",
                PluginToolOutput::wait_secs(args.wait_secs)
            );

            // Poll for the answer. Silence, a dismissal and a timeout are all a
            // refusal, and the refusal is recorded so this is not asked again.
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(PluginToolOutput::wait_secs(args.wait_secs));
            loop {
                let view = get_view(&request, &plugin).await?;
                if let Some(decision) = view.decision.as_ref() {
                    match decision.granted_bytes() {
                        Some(granted) => {
                            println!(
                                "Granted: plugin `{plugin}` now keeps up to {granted} bytes per tool \
                                 result (decided by {} via {}, {}).",
                                decision.decided_by, decision.channel, decision.decided_at
                            );
                            return Ok(());
                        }
                        None => anyhow::bail!(
                            "Refused by {} ({}). The plugin keeps the global default of {} bytes. \
                             This answer is on record: do not ask again unless the operator clears \
                             it.",
                            decision.decided_by,
                            decision.channel,
                            view.global_bytes
                        ),
                    }
                }
                if std::time::Instant::now() >= deadline {
                    // The silence is the answer. Record it, so a later start (or a
                    // second attempt) sees the refusal instead of re-asking.
                    let _ = request(
                        reqwest::Method::POST,
                        format!("/api/plugins/{plugin}/tool-output/decision"),
                    )
                    .json(&serde_json::json!({
                        "decision": "refuse",
                        "channel": CHANNEL_TIMEOUT,
                        "reason": "no answer within the wait window",
                    }))
                    .send()
                    .await;
                    anyhow::bail!(
                        "No answer within the wait window, which counts as a no. The plugin keeps \
                         the global default of {} bytes and the refusal is on record; do not ask \
                         again in this run.",
                        view.global_bytes
                    );
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
        ToolOutputCommand::Decide => {
            let plugin = plugin.expect("checked above");
            require_terminal("plugin tool-output decide")?;
            let Some(grant) = args.decision else {
                anyhow::bail!("say which way: `--allow [--bytes <n>]` or `--refuse`");
            };
            let mut body = serde_json::json!({
                "decision": if grant { "grant" } else { "refuse" },
                "channel": CHANNEL_CLI,
            });
            if let Some(bytes) = args.bytes {
                body["bytes"] = serde_json::json!(bytes);
            }
            if !args.reason.trim().is_empty() {
                body["reason"] = serde_json::json!(args.reason.trim());
            }
            let response = request(
                reqwest::Method::POST,
                format!("/api/plugins/{plugin}/tool-output/decision"),
            )
            .json(&body)
            .send()
            .await
            .map_err(network_error)?;
            let view = decode_view(response).await?;
            print_view(&view);
            Ok(())
        }
        ToolOutputCommand::Clear => {
            let plugin = plugin.expect("checked above");
            require_terminal("plugin tool-output clear")?;
            let response = request(
                reqwest::Method::DELETE,
                format!("/api/plugins/{plugin}/tool-output"),
            )
            .send()
            .await
            .map_err(network_error)?;
            let view = decode_view(response).await?;
            println!(
                "Cleared the recorded decision for `{plugin}`. Its tools keep the global default \
                 until a new request is answered."
            );
            print_view(&view);
            Ok(())
        }
    }
}

fn network_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_connect() {
        anyhow::anyhow!(
            "could not reach the karucode server ({error}). Start it, or pass `--url \
             http://host:port`."
        )
    } else {
        anyhow::anyhow!("{error}")
    }
}

fn http_error(error: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

async fn get_view<F>(request: &F, plugin: &str) -> anyhow::Result<ToolOutputView>
where
    F: Fn(reqwest::Method, String) -> reqwest::RequestBuilder,
{
    let response = request(
        reqwest::Method::GET,
        format!("/api/plugins/{plugin}/tool-output"),
    )
    .send()
    .await
    .map_err(network_error)?;
    decode_view(response).await
}

/// Turn a response into a view, surfacing the server's own error message (which
/// is written to be actionable: it names the range, the ceiling or the decision
/// already on record).
async fn decode_view(response: reqwest::Response) -> anyhow::Result<ToolOutputView> {
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(network_error)?;
    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("the server refused the request");
        anyhow::bail!("{message}");
    }
    serde_json::from_value(body).map_err(|e| anyhow::anyhow!("unexpected answer: {e}"))
}

fn print_view(view: &ToolOutputView) {
    println!("plugin               {}", view.plugin);
    println!(
        "effective budget     {} bytes ({})",
        view.effective_bytes,
        if view.granted {
            "operator grant"
        } else {
            "global default"
        }
    );
    println!("global default       {} bytes", view.global_bytes);
    println!("ceiling              {} bytes", view.ceiling_bytes);
    if let Some(declared) = view.declared_bytes {
        println!("config narrowing     {declared} bytes ([[plugins]] tool_output_bytes)");
    }
    match &view.decision {
        Some(decision) => {
            println!(
                "decision             {} by {} via {} ({})",
                decision.decision.as_str(),
                decision.decided_by,
                decision.channel,
                decision.decided_at
            );
            if let Some(bytes) = decision.bytes {
                println!("granted              {bytes} bytes");
            }
            if !decision.reason.trim().is_empty() {
                println!("reason               {}", decision.reason);
            }
        }
        None => println!("decision             none - the global default applies"),
    }
    if let Some(pending) = &view.pending {
        println!(
            "waiting for          an answer to {} bytes ({})",
            pending.bytes, pending.reason
        );
    }
    if !view.history.is_empty() {
        println!("history:");
        for event in &view.history {
            println!(
                "  {}  {:<10} {} bytes  {}",
                event.at,
                event.kind,
                event
                    .bytes
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "-".into()),
                event.actor
            );
        }
    }
}
