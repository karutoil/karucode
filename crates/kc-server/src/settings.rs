//! Live settings: config file + environment, then per-install overrides from the
//! SQLite `settings` table.
//!
//! Precedence is defined by `kc_core::config`: (1) rows saved from the UI, (2)
//! environment, (3) config file, (4) defaults. [`Config::load`] already merges
//! (2)-(4); this module layers the persisted rows on top unless
//! `KARUCODE_LOCK_SETTINGS` removed that layer.
//!
//! The runtime reads settings through [`SettingsHandle`] on every step, so a
//! successful [`SettingsHandle::update`] hot-swaps the in-memory [`Config`] after
//! the rows were persisted. The API key is write-only: it is stored in the
//! database, kept in memory, and never returned by any getter used for JSON.

// @kc-doc: configuration
// @kc-title: Change karucode's configuration
// @kc-summary: The four config layers, which settings apply live, and how secrets and invalid values are handled.
// @kc-tags: config, settings, configuration, toml, env
// @kc-applies-to: env:KARUCODE_MALLOC_ARENA_MAX, env:KARUCODE_MALLOC_TRIM_SECS, file:~/.config/karucode/config.toml, env:KARUCODE_CONFIG, env:KARUCODE_ALLOW_TEMP_DIR, env:KARUCODE_LOCK_SETTINGS, env:KARUCODE_ADMIN_TOKEN, env:KARUCODE_PROJECT_ROOTS, env:KARUCODE_MAX_ITERATIONS, env:KARUCODE_MAX_RUN_SECS, env:KARUCODE_AUTO_CONTINUE, env:KARUCODE_MAX_CONTINUES, env:KARUCODE_CONTEXT_MAX_CHARS, env:KARUCODE_MAX_IMAGE_READ_BYTES, env:KARUCODE_MODEL_SUPPORTS_IMAGES, env:KARUCODE_PLUGINS_WATCH, env:KARUCODE_PLUGINS_WATCH_SECS, env:KARUCODE_NOTIFICATIONS, env:KARUCODE_NOTIFICATIONS_EVENTS, env:KARUCODE_NOTIFICATIONS_PLUGIN, env:KARUCODE_NOTIFICATIONS_MIN_RUN_SECS, env:KARUCODE_NOTIFICATIONS_SAY_BODY, env:KARUCODE_SUBAGENT_ALLOW_BACKGROUND, env:KARUCODE_SUBAGENT_MAX_BACKGROUND, env:KARUCODE_SUBAGENT_WAIT_TIMEOUT_SECS, env:KARUCODE_SUBAGENT_MAX_ITERATIONS, env:KARUCODE_SUBAGENT_MAX_CONTINUES, env:KARUCODE_SUBAGENT_TIMEOUT_SECS, env:KARUCODE_SUBAGENT_CONCURRENCY, env:KARUCODE_ALLOW_RECURSIVE_SUBAGENTS, config:[model], config:[limits], config:[approvals], config:[subagents], config:subagents.allow_background, config:subagents.max_background, config:subagents.wait_timeout_secs, config:subagents.max_concurrent, config:subagents.max_iterations, config:subagents.max_continues, config:subagents.max_depth, config:subagents.timeout_secs, config:[notifications], config:[[plugins]], config:[[mcp_servers]], config:plugins_watch, config:plugins_watch_secs, config:admin_token, config:project_roots, config:allow_temp_dir, config:model.supports_images, config:limits.max_image_read_bytes, config:notifications.enabled, config:notifications.min_run_secs, header:X-Karucode-Admin-Token, route:GET /api/config, route:PATCH /api/config, route:GET /api/stats, cmd:--config, cmd:--data-dir, ui:Settings
//
// # Where the settings come from
//
// Highest priority first:
//
// 1. **UI / database overrides** - what the Settings screen saved, in the
//    SQLite `settings` table.
// 2. **Environment** - `KARUCODE_*` variables (and `OPENAI_*` for the key).
// 3. **Config file** - `$KARUCODE_CONFIG`,
//    default `~/.config/karucode/config.toml`.
// 4. **Built-in defaults**.
//
// Command-line flags (`--host`, `--port`, `--workspace`, `--data-dir`,
// `--config`) are applied on top for the process they start. Start from
// `config.example.toml` in the repository - it lists every key with a comment.
//
// # Pinning a deployment
//
// Set `KARUCODE_LOCK_SETTINGS=1` to remove layer 1 entirely: the UI becomes
// read-only for settings and `PATCH /api/config` answers `403`. This is how you
// run karucode where the operator, not the browser, owns the model and keys.
//
// Every boolean in this file and in the environment takes the same spellings:
// `1/0`, `true/false`, `yes/no`, `on/off`, case-insensitive. Anything else is
// ignored with the previous value kept, so `KARUCODE_LOCK_SETTINGS=1` and
// `KARUCODE_LOCK_SETTINGS=true` are the same switch.
//
// # Which settings apply without a restart
//
// Model, sampling, limits, approvals and subagents are hot-swapped by a saved
// change. The plugin list is not part of `PATCH /api/config` - installing a
// plugin is not a settings tweak - so it has its own refresh:
//
// ```toml
// plugins_watch = true      # KARUCODE_PLUGINS_WATCH=1
// plugins_watch_secs = 2    # KARUCODE_PLUGINS_WATCH_SECS, clamped to 1..=600
// ```
//
// With `plugins_watch` on, the server re-reads `[[plugins]]` from this file and
// every `<data_dir>/plugins/*/plugin.toml` on a timer and applies additions,
// removals and rebuilt binaries without a restart. It is off by default because a
// reload replaces processes; `POST /api/plugins/reload` does the same thing on
// demand, and `POST /api/plugins/:name/reload` refreshes one plugin. Bind
// address, port, data directory and auth token still need a restart.
//
// # The admin gate
//
// `KARUCODE_LOCK_SETTINGS` is all-or-nothing. For the middle ground - the UI may
// tune limits and sampling, but not the security-relevant settings - set
// `KARUCODE_ADMIN_TOKEN` (or `admin_token` in the config file). A
// `PATCH /api/config` that changes any of these then also needs
// `X-Karucode-Admin-Token: <token>`:
//
// * `approvals` - the human gate itself;
// * `model.base_url`, `model.api_key` (including `clear_api_key`) - the endpoint
//   that receives every prompt and every command's output;
// * `providers` - the same, for every saved profile;
// * `active_provider`, `active_model` - which of those profiles is live.
//
// Everything else (`limits`, `subagents`, sampling) keeps working with the
// ordinary bearer token. A missing or wrong admin credential is
// `403 permission_denied`, and the change is not applied. The comparison is
// constant-time, and the token is never logged or echoed. Every authorised
// security change is logged at `info` with the field *names* only.
//
// With no admin token configured the gate is off and any API client can change
// those fields; the server logs a warning at startup when that is the case.
//
// Both credentials are validated: `auth_token` and `admin_token` must each be at
// least 16 characters, and neither may be a documented placeholder
// (`change-me…`, `changeme…`, `secret…`, `password…`, `test…`). A placeholder is
// published text - the 27-character `change-me-to-16-plus-chars` that used to
// ship in `.env.example` passed a length check and was a guessable working
// bearer token - so it is refused by name. Generate a real one:
//
// ```bash
// export KARUCODE_AUTH_TOKEN="$(openssl rand -hex 24)"
// export KARUCODE_ADMIN_TOKEN="$(openssl rand -hex 24)"
// ```
//
// # Project roots
//
// `project_roots` (`KARUCODE_PROJECT_ROOTS`, colon-separated) is an optional
// allowlist for where `POST /api/projects` may create a project. **Empty - the
// default - means no allowlist**: any existing directory is a valid project,
// which is what a single-operator install wants, since the obvious second
// project is a sibling checkout of the default workspace. Set it for a shared or
// remotely reachable deployment. Either way `/` itself is refused, so a request
// cannot make the whole filesystem the confinement root for every file tool.
//
// ```toml
// project_roots = ["/home/me/projects", "/srv/code"]
// ```
//
// # The temp directory
//
// `allow_temp_dir` (default `true`, or `KARUCODE_ALLOW_TEMP_DIR`) lets the file
// tools read and write the system temporary directory as well as the project.
// It is on by default because build output, screenshots, downloads and browser
// profiles land in `/tmp` and a path error there is noise, not protection. The
// root is `TMPDIR` when set and absolute, otherwise `/tmp`.
//
// ```toml
// allow_temp_dir = false      # project-only confinement
// ```
//
// This is unrelated to `project_roots`: that one gates where a project may be
// created, this one widens what a run's file tools may touch. Both are
// process-level, so changing either needs a restart.
//
// # What applies without a restart
//
// The runtime reads settings through one handle on every step, so a successful
// `PATCH /api/config` hot-swaps model, sampling, limits, approvals and subagent
// budgets for the next run. These are process-level and need a restart: `host`,
// `port`, `workspace` (the default workspace), `data_dir`, `auth_token`,
// `log_level`, plugin and MCP definitions (those are read at startup; enabling a
// plugin from the UI restarts that one process).
//
// # Secrets
//
// API keys are stored in the database and kept in memory, but every getter used
// for JSON and logging reports `api_key_set: true` instead of the value. The same
// rule applies to MCP `headers`. Never put a key in a command line: it shows up
// in `ps`.
//
// # Invalid values
//
// Validation runs before the value is persisted. A bad `PATCH /api/config`
// returns `400 bad_request` naming the field (`providers[0].base_url`, ...) and
// nothing is written, so the running configuration cannot be half-updated.
//
// # Long-horizon runs
//
// A run is not allowed to die just because it used up its first budget of model
// turns. The iteration budget belongs to a *segment*, and when a segment ends
// mid-task the runtime refreshes it and keeps going in the same session, run and
// transcript - it emits a notice, appends a `[runtime checkpoint]` user turn so
// the transcript explains the jump, and resets the counter.
//
// ```toml
// [limits]
// max_iterations = 200   # model turns per segment
// max_run_secs = 86400   # hard wall clock for the whole run (24h)
// auto_continue = true   # refresh the budget instead of stopping
// max_continues = 50     # how many refreshes one run may chain
// ```
//
// The effective ceiling for one run is about
// `(max_continues + 1) * max_iterations` model turns, or `max_run_secs`,
// whichever comes first. Set `auto_continue = false` (or `max_continues = 0`) to
// restore one-shot budgets; the run then stops with status `limit_reached`.
//
// Environment: `KARUCODE_MAX_ITERATIONS`, `KARUCODE_MAX_RUN_SECS`,
// `KARUCODE_AUTO_CONTINUE` (`1/0`, `true/false`, `yes/no`, `on/off`),
// `KARUCODE_MAX_CONTINUES`, `KARUCODE_CONTEXT_MAX_CHARS`.
//
// A budget that runs out is never a reason to re-send the prompt: the work is
// still in the transcript, and the checkpoint tells the model to continue rather
// than restart. Subagents follow the same rule with their own numbers:
// `subagents.max_iterations` per segment, refreshed up to
// `subagents.max_continues` times, inside `subagents.timeout_secs`.
//
// ```toml
// [subagents]
// max_concurrent = 3      # subagent loops running at once (0 = no ceiling)
// max_background = 8      # live detached agents per session (0 = no ceiling)
// allow_background = true # permitted to spawn with `background = true`
// wait_timeout_secs = 600 # default bound on `wait_for_agents`
// max_iterations = 200    # model turns per subagent segment
// max_continues = 60      # about 12,200 turns before it must report back
// timeout_secs = 14400    # 4h wall clock for one delegated task
// ```
//
// `max_concurrent` bounds real parallelism, not call sites: a `spawn_agents`
// fan-out or a `background: true` spawn runs beside its caller, so several loops
// can be live at once. `max_background` is a separate ceiling on *detached*
// agents in one session, because each one keeps a provider stream open after the
// turn that started it has ended. A spawn past a finite limit is refused by name,
// never quietly queued. `wait_timeout_secs` bounds `wait_for_agents`: a wait that
// runs out reports which agents are still working instead of hanging.
//
// **`max_concurrent = 0` and `max_background = 0` mean no ceiling**, for an
// install that wants the harness out of the model's way: every spawn is granted
// a slot immediately and a `spawn_agents` fan-out starts all of its children.
// Each live loop is a provider stream and a transcript, so an unbounded setting
// makes cost the only limit - a model that fans out two hundred ways bills for
// two hundred loops. `max_depth` is the other kind of zero: `0` forbids
// delegation outright, and the field holds at most 255 generations.
//
// `max_continues = 0` restores the one-shot budget (the subagent stops at
// `max_iterations`), and `limits.auto_continue = false` switches continuation off
// for runs and subagents alike. Environment:
// `KARUCODE_SUBAGENT_CONCURRENCY`, `KARUCODE_SUBAGENT_MAX_BACKGROUND`,
// `KARUCODE_SUBAGENT_ALLOW_BACKGROUND`, `KARUCODE_SUBAGENT_WAIT_TIMEOUT_SECS`,
// `KARUCODE_SUBAGENT_MAX_ITERATIONS`, `KARUCODE_SUBAGENT_MAX_CONTINUES`,
// `KARUCODE_SUBAGENT_TIMEOUT_SECS`, `KARUCODE_ALLOW_RECURSIVE_SUBAGENTS`.
//
// `limits.context_max_chars` decides how much conversation is sent to the model.
// Leave it at `0` (the default) and the budget is **derived from the resolved
// model**: its `context_window` minus the room its answer needs, at four
// characters per token. A non-zero value is an explicit ceiling for the whole
// install and can only lower that derived budget. When it overflows, the oldest
// exchanges are replaced by one compaction notice
// that keeps a one-line digest of what they said (including which files each
// turn read); the persisted transcript keeps every message. The original user
// turn and the exchange in flight are never compacted away. `limits.max_tool_calls_per_turn`
// is a runaway-batch guard, not a work budget - calls past it are skipped with a
// placeholder result. How compaction interacts with provider prompt caching is
// documented under `prompt-cache`.
//
// # Images (vision models)
//
// `read_file` can show a model an image, and this is the switch that decides
// whether it does. Nothing guesses it from the model id - an endpoint will
// accept a request carrying an image for a text-only model and either drop it or
// fail the whole turn, and a model that "read" an image it never saw will
// describe it confidently.
//
// ```toml
// [model]
// supports_images = true        # KARUCODE_MODEL_SUPPORTS_IMAGES=1
//
// [limits]
// max_image_read_bytes = 4194304   # 4 MiB, base64'd into the transcript
// ```
//
// With `supports_images` on, `read_file` on a PNG/JPEG/GIF/WebP base64-encodes
// the file and attaches it to the tool result. With it off, the tool refuses with
// a sentence the model is expected to repeat ("I cannot see images"), which is
// the whole point: a text-only model must say so rather than invent a
// description. A `vision = true|false` on a `[[providers.models]]` entry overrides
// the install-wide flag for that model, so one session can use a vision model
// while the default stays text-only. It is resolved for the model a session will
// actually call: pinning a session to a vision model is enough, even when the
// profile's default model is text-only. Formats the vision APIs reject (BMP, ICO,
// TIFF, AVIF, HEIC) are named in the refusal; SVG is text and is read as text.
//
// Both knobs are on the settings screen: "Send images to models" is the
// install-wide default, and each model row carries its own image setting
// (unknown / accepted / text only), prefilled from models.dev - `image` in
// `modalities.input` - whenever the model is picked from the catalog, added by
// id, or refreshed.
//
// The image bytes live in the persisted transcript and are resent on every later
// turn, which is why the cap exists. Count one image as about 8,000 characters of
// the context budget (`limits.context_max_chars`); the base64 length is not
// counted, because the provider bills the picture, not the encoding.
//
// # Resident memory
//
// A karucode server holds the transcript window, the event replay buffer and each
// plugin's JSON-RPC traffic, so its resident set follows what it has done, not
// only what it is doing. Two allocator knobs keep that from becoming a ratchet:
//
// ```bash
// KARUCODE_MALLOC_ARENA_MAX=2   # glibc arenas; default 2, 0/absent = 2
// KARUCODE_MALLOC_TRIM_SECS=60  # return free heap to the kernel; 0 disables
// ```
//
// glibc gives every thread that allocates its own arena (up to `8 * nproc`) and
// does not hand a freed page back to the kernel. On a 16-core box with six
// plugins and a dozen sessions that left tens of MiB of *empty* heap resident;
// capping the arenas and trimming while idle returns it. Both are process-wide
// and read once at startup, so changing them needs a restart. Set the trim
// interval to `0` if you would rather trade the memory back for a few
// microseconds every minute, or if you build against musl, where both knobs are
// no-ops. `scripts/measure_steady_memory.py` reproduces the numbers on your own
// machine.
//
// # Minimal config
//
// ```toml
// host = "127.0.0.1"
// port = 8787
// workspace = "/home/me/project"
//
// [model]
// base_url = "http://127.0.0.1:4000/v1"
// model = "gpt-4o-mini"
//
// [approvals]
// mode = "ask"
// allow_commands = ["rg", "git status", "cargo test"]
// ```
//
// # Gotchas
//
// * Environment beats the config file, so an exported `KARUCODE_MODEL` silently
//   wins over `[[providers]]`. `GET /api/config` shows the effective value.
// * The data directory is forced to mode `0700` at startup because the database
//   holds keys.
// * `limits.context_max_chars` compacts old turns into a digest; the persisted
//   transcript is untouched and nothing is sent to the model to summarise.
// * The settings file is TOML. A malformed file stops startup with the path and
//   the parse error rather than falling back to defaults.
//
// Full reference: `config.example.toml`, `.env.example`, `docs/API.md`.

use crate::error::ApiError;
use async_trait::async_trait;
use kc_core::config::{
    ApprovalMode, ApprovalPolicy, Config, Limits, ModelConfig, ModelEntry, ProviderProfile,
    SessionModelSelection, SubagentConfig, TerminalConfig, TitlesConfig, MAX_TERMINALS_HINT,
    MAX_TERMINAL_SCROLLBACK, MAX_TITLE_CHARS_HINT, MIN_TERMINAL_SCROLLBACK,
};
use kc_core::error::{Error, Result};
use kc_core::settings::Settings;
use kc_core::store::Store;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tracing::warn;

/// Settings keys persisted in the `settings` table. Kept in one place so the
/// read and write paths cannot drift.
mod keys {
    pub const MODEL_PROVIDER: &str = "model.provider";
    pub const MODEL_BASE_URL: &str = "model.base_url";
    pub const MODEL_NAME: &str = "model.model";
    pub const MODEL_API_KEY: &str = "model.api_key";
    pub const MODEL_TEMPERATURE: &str = "model.temperature";
    pub const MODEL_SEND_TEMPERATURE: &str = "model.send_temperature";
    pub const MODEL_MAX_TOKENS: &str = "model.max_tokens";
    pub const MODEL_TOP_P: &str = "model.top_p";
    pub const MODEL_REQUEST_TIMEOUT: &str = "model.request_timeout_secs";
    pub const MODEL_PARALLEL_TOOL_CALLS: &str = "model.parallel_tool_calls";
    pub const MODEL_SUPPORTS_IMAGES: &str = "model.supports_images";
    pub const MODEL_ACTIVE_PROVIDER: &str = "model.active_provider";
    pub const MODEL_ACTIVE_MODEL: &str = "model.active_model";
    /// JSON array of [`ProviderProfile`]s, keys included. Written as one row so
    /// a profile and its models can never be torn apart across rows.
    pub const PROVIDERS: &str = "model.providers";

    pub const LIMITS_MAX_ITERATIONS: &str = "limits.max_iterations";
    pub const LIMITS_MAX_RUN_SECS: &str = "limits.max_run_secs";
    pub const LIMITS_AUTO_CONTINUE: &str = "limits.auto_continue";
    pub const LIMITS_MAX_CONTINUES: &str = "limits.max_continues";
    pub const LIMITS_MAX_TOOL_OUTPUT_BYTES: &str = "limits.max_tool_output_bytes";
    pub const LIMITS_MAX_COMMAND_OUTPUT_BYTES: &str = "limits.max_command_output_bytes";
    pub const LIMITS_COMMAND_TIMEOUT_SECS: &str = "limits.command_timeout_secs";
    pub const LIMITS_MAX_COMMAND_TIMEOUT_SECS: &str = "limits.max_command_timeout_secs";
    pub const LIMITS_MAX_FILE_READ_BYTES: &str = "limits.max_file_read_bytes";
    pub const LIMITS_MAX_IMAGE_READ_BYTES: &str = "limits.max_image_read_bytes";
    pub const LIMITS_SEARCH_MAX_RESULTS: &str = "limits.search_max_results";
    pub const LIMITS_CONTEXT_MAX_CHARS: &str = "limits.context_max_chars";
    pub const LIMITS_MAX_TOOL_CALLS_PER_TURN: &str = "limits.max_tool_calls_per_turn";

    pub const APPROVALS_MODE: &str = "approvals.mode";
    pub const APPROVALS_ALLOW_COMMANDS: &str = "approvals.allow_commands";
    pub const APPROVALS_AUTO_APPROVE_PATHS: &str = "approvals.auto_approve_paths";
    pub const APPROVALS_TIMEOUT_SECS: &str = "approvals.timeout_secs";

    pub const SUBAGENTS_MAX_CONCURRENT: &str = "subagents.max_concurrent";
    pub const SUBAGENTS_MAX_ITERATIONS: &str = "subagents.max_iterations";
    pub const SUBAGENTS_MAX_CONTINUES: &str = "subagents.max_continues";
    pub const SUBAGENTS_ALLOW_RECURSIVE: &str = "subagents.allow_recursive";
    pub const SUBAGENTS_MAX_DEPTH: &str = "subagents.max_depth";
    pub const SUBAGENTS_TIMEOUT_SECS: &str = "subagents.timeout_secs";
    pub const SUBAGENTS_ALLOW_BACKGROUND: &str = "subagents.allow_background";
    pub const SUBAGENTS_MAX_BACKGROUND: &str = "subagents.max_background";
    pub const SUBAGENTS_WAIT_TIMEOUT_SECS: &str = "subagents.wait_timeout_secs";

    pub const TITLES_ENABLED: &str = "titles.enabled";
    pub const TITLES_PROVIDER: &str = "titles.provider";
    pub const TITLES_MODEL: &str = "titles.model";
    pub const TITLES_MAX_CHARS: &str = "titles.max_chars";
    pub const TITLES_TIMEOUT_SECS: &str = "titles.timeout_secs";

    pub const NOTIFICATIONS_ENABLED: &str = "notifications.enabled";
    pub const NOTIFICATIONS_RUN_FINISHED: &str = "notifications.run_finished";
    pub const NOTIFICATIONS_NEEDS_INPUT: &str = "notifications.needs_input";
    pub const NOTIFICATIONS_ERROR: &str = "notifications.error";
    pub const NOTIFICATIONS_INFO: &str = "notifications.info";
    pub const NOTIFICATIONS_PLUGIN: &str = "notifications.plugin";
    pub const NOTIFICATIONS_MIN_RUN_SECS: &str = "notifications.min_run_secs";
    pub const NOTIFICATIONS_SAY_BODY: &str = "notifications.say_body";
}

// ---------------------------------------------------------------------------
// PATCH payloads (docs/API.md `PATCH /api/config`)
// ---------------------------------------------------------------------------

/// Distinguishes "field absent" (`None`) from "present and null" (`Some(None)`).
fn double_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPatch {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    /// Send a temperature at all. Default stays whatever was stored (off).
    pub send_temperature: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub max_tokens: Option<Option<u32>>,
    #[serde(default, deserialize_with = "double_option")]
    pub top_p: Option<Option<f32>>,
    pub request_timeout_secs: Option<u64>,
    pub parallel_tool_calls: Option<bool>,
    /// Whether the model accepts image input (`read_file` on a PNG/JPEG/...).
    pub supports_images: Option<bool>,
    /// Write-only. Empty strings are treated as absent.
    pub api_key: Option<String>,
    /// Erase the stored API key (wins over `api_key`).
    pub clear_api_key: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsPatch {
    pub max_iterations: Option<u32>,
    pub max_run_secs: Option<u64>,
    pub auto_continue: Option<bool>,
    pub max_continues: Option<u32>,
    pub max_tool_output_bytes: Option<usize>,
    pub max_command_output_bytes: Option<usize>,
    pub command_timeout_secs: Option<u64>,
    pub max_command_timeout_secs: Option<u64>,
    pub max_file_read_bytes: Option<usize>,
    pub max_image_read_bytes: Option<usize>,
    pub search_max_results: Option<usize>,
    pub context_max_chars: Option<usize>,
    pub max_tool_calls_per_turn: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalsPatch {
    pub mode: Option<String>,
    pub allow_commands: Option<Vec<String>>,
    pub auto_approve_paths: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentsPatch {
    pub max_concurrent: Option<usize>,
    pub max_iterations: Option<u32>,
    pub max_continues: Option<u32>,
    pub allow_recursive: Option<bool>,
    pub max_depth: Option<u8>,
    pub timeout_secs: Option<u64>,
    pub allow_background: Option<bool>,
    pub max_background: Option<usize>,
    pub wait_timeout_secs: Option<u64>,
}

/// `[titles]` section of `PATCH /api/config`.
///
/// `provider`/`model` are plain strings, not `Option<Option<String>>`: an empty
/// string is the documented spelling of "inherit the session's model", which is
/// exactly what `TitlesConfig` stores, so there is nothing to distinguish.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitlesPatch {
    pub enabled: Option<bool>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_chars: Option<usize>,
    pub timeout_secs: Option<u64>,
}

/// `[notifications]` section of `PATCH /api/config`.
///
/// Every field is optional, so the settings screen can flip one switch without
/// sending the rest. `min_run_secs` is the only numeric bound; the boolean
/// switches have no invalid value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsPatch {
    pub enabled: Option<bool>,
    pub run_finished: Option<bool>,
    pub needs_input: Option<bool>,
    pub error: Option<bool>,
    pub info: Option<bool>,
    pub plugin: Option<bool>,
    pub min_run_secs: Option<u64>,
    pub say_body: Option<bool>,
}

/// One provider profile to create or update. Fields left out keep their current
/// value, so the settings screen can save just the model list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPatch {
    pub id: String,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    /// Write-only; an empty string is treated as absent.
    pub api_key: Option<String>,
    /// Erase the stored key (wins over `api_key`).
    pub clear_api_key: Option<bool>,
    /// Replaces the whole model list when present.
    pub models: Option<Vec<ModelEntry>>,
    pub request_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersPatch {
    #[serde(default)]
    pub upsert: Vec<ProviderPatch>,
    #[serde(default)]
    pub delete: Vec<String>,
}

/// Which profile/model the runtime should use. This is install-wide (not per
/// project), matching the rest of the settings model.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivePatch {
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Body of `PATCH /api/config`. Every section is optional; unknown fields are
/// rejected so a typo in the browser cannot silently do nothing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPatch {
    pub model: Option<ModelPatch>,
    pub providers: Option<ProvidersPatch>,
    pub active: Option<ActivePatch>,
    pub limits: Option<LimitsPatch>,
    pub approvals: Option<ApprovalsPatch>,
    pub subagents: Option<SubagentsPatch>,
    pub titles: Option<TitlesPatch>,
    pub notifications: Option<NotificationsPatch>,
}

impl ConfigPatch {
    /// The security-relevant fields this patch would change, by documented name.
    ///
    /// These are the settings that decide *whether a human is asked* and *where
    /// the prompts go*: `approvals` (the human gate), `model.base_url` /
    /// `model.api_key` (the endpoint that receives every prompt and every
    /// command's output), `providers` (the same, for every saved profile) and
    /// `active_provider` / `active_model` (which of those profiles is live).
    /// `project_roots` and `admin_token` are listed for completeness: they are
    /// not part of [`ConfigPatch`] (the API cannot change them), but a future
    /// field must not silently skip the gate.
    ///
    /// An empty result means the patch is safe to apply with the ordinary bearer
    /// token alone.
    pub fn security_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.approvals.is_some() {
            fields.push("approvals");
        }
        if let Some(model) = &self.model {
            if model.base_url.is_some() {
                fields.push("model.base_url");
            }
            // `clear_api_key` erases the credential, so it is as sensitive as
            // setting one.
            if model.api_key.is_some() || model.clear_api_key.is_some() {
                fields.push("model.api_key");
            }
        }
        if self.providers.is_some() {
            fields.push("providers");
        }
        if let Some(active) = &self.active {
            if active.provider.is_some() {
                fields.push("active_provider");
            }
            if active.model.is_some() {
                fields.push("active_model");
            }
        }
        fields
    }
}

/// Header that carries the admin credential for security-relevant settings
/// changes. Deliberately *not* `Authorization`: the bearer token is the ordinary
/// API credential, and the admin token is a second, stronger one.
pub const ADMIN_TOKEN_HEADER: &str = "x-karucode-admin-token";

/// Constant-time byte comparison. `crate::app` has an identical private helper
/// for the bearer token; it is not reachable from here, so this mirrors it
/// (length check first, then XOR-accumulate) rather than comparing with `==`,
/// which would leak the matching prefix length through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Enforce the admin gate for a settings patch.
///
/// * `admin_token = None` - no gate (the historical behaviour); the caller logs
///   a warning once at startup.
/// * `admin_token = Some(_)` and the patch touches no security field - allowed
///   with the ordinary credential.
/// * `admin_token = Some(_)` and the patch touches a security field - the
///   `X-Karucode-Admin-Token` header must match, otherwise `403`.
///
/// Returns the names of the security fields that were authorised, so the caller
/// can log them (names only, never values).
pub fn authorize_patch(
    config: &Config,
    patch: &ConfigPatch,
    presented: Option<&str>,
) -> Result<Vec<&'static str>> {
    let fields = patch.security_fields();
    if fields.is_empty() {
        return Ok(fields);
    }
    let Some(expected) = config.admin_token.as_deref().filter(|t| !t.is_empty()) else {
        // No admin token configured: the gate is off. Startup already warned.
        return Ok(fields);
    };
    let presented = presented.map(str::trim).unwrap_or_default();
    if presented.is_empty() {
        return Err(Error::PermissionDenied(format!(
            "changing {} requires the admin credential: send `X-Karucode-Admin-Token` \
             (KARUCODE_ADMIN_TOKEN)",
            fields.join(", ")
        )));
    }
    if !constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        return Err(Error::PermissionDenied(
            "the admin credential is not valid for this change".into(),
        ));
    }
    Ok(fields)
}

// ---------------------------------------------------------------------------
// SettingsHandle
// ---------------------------------------------------------------------------

/// Live settings source. Cheap to clone via `Arc`; the runtime holds the trait
/// object and always sees the current values.
pub struct SettingsHandle {
    config: RwLock<Config>,
    store: Arc<dyn Store>,
    /// Serialises read-modify-write updates so two concurrent PATCH requests
    /// cannot interleave their persisted rows.
    update_lock: tokio::sync::Mutex<()>,
}

// A manual Debug impl so accidentally logging a handle cannot print the key.
impl std::fmt::Debug for SettingsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = self.read();
        f.debug_struct("SettingsHandle")
            .field("workspace", &cfg.workspace_root)
            .field("model", &cfg.model.model)
            .field("base_url", &cfg.model.base_url)
            .field("api_key_set", &api_key_set(&cfg.model))
            .field("lock_settings", &cfg.lock_settings)
            .finish()
    }
}

impl SettingsHandle {
    /// Create the handle and layer persisted rows over the file/env config.
    ///
    /// Failure policy, by layer:
    ///
    /// * the **file + environment** layer is validated first and a bad value
    ///   aborts startup — that is operational configuration and a torn value is
    ///   worth failing loudly for;
    /// * each **persisted row** is then applied on its own and kept only if the
    ///   resulting config is still valid. A stale or hand-edited row (an old
    ///   `model.base_url`, a limit that is out of range, a value written by a
    ///   previous build) is warned about and ignored, so it can never prevent
    ///   the server from starting: the file/env value for that key stays live;
    /// * an unreadable settings table is a hard error, because silently running
    ///   on stale defaults would be worse.
    pub async fn new(config: Config, store: Arc<dyn Store>) -> Result<Arc<Self>> {
        // Layer 3/2 (file, environment) and the CLI overrides on top of it.
        let mut config = config;
        // A config from the file/env path is already normalised; a config built
        // in code (tests, embedders) may not be. Normalising here means every
        // caller sees the same provider-profile invariants.
        let legacy_config = config.providers.is_empty();
        config.normalize_providers();
        validate_settings(&config)?;
        if !config.lock_settings {
            let rows = store
                .all_settings()
                .await
                .map_err(|e| Error::Store(format!("could not read persisted settings: {e}")))?;
            apply_rows_resilient(&mut config, &rows, legacy_config);
        }
        // Warn once, here, because this runs exactly once per process: without
        // an admin token any bearer-token holder can turn approvals off and
        // repoint the model endpoint, which is the whole human gate.
        if config
            .admin_token
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            warn!(
                "no admin token configured (KARUCODE_ADMIN_TOKEN): any API client can change \
                 approvals, the model endpoint and provider credentials via PATCH /api/config"
            );
        }
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            store,
            update_lock: tokio::sync::Mutex::new(()),
        }))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        // A poisoned lock still contains a consistent snapshot; refusing to read
        // settings would take the whole server down.
        self.config.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Copy of the current config. Contains the API key: never serialise this
    /// directly; use `model.public_view()` for the browser.
    pub fn snapshot(&self) -> Config {
        self.read().clone()
    }

    pub fn is_locked(&self) -> bool {
        self.read().lock_settings
    }

    pub fn auth_token(&self) -> Option<String> {
        self.read().auth_token.clone()
    }

    /// Every secret that must not appear in an outgoing error message.
    pub fn secrets(&self) -> Vec<String> {
        let cfg = self.read();
        let mut secrets: Vec<String> = Vec::new();
        secrets.push(cfg.model.api_key.clone().unwrap_or_default());
        if let Some(token) = &cfg.auth_token {
            secrets.push(token.clone());
        }
        for profile in &cfg.providers {
            if let Some(key) = &profile.api_key {
                secrets.push(key.clone());
            }
        }
        secrets.into_iter().filter(|s| s.len() >= 4).collect()
    }

    pub fn redact(&self, text: &str) -> String {
        let secrets = self.secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        crate::error::redact(text, &refs)
    }

    /// Apply a validated patch: persist changed keys first, then swap the
    /// in-memory config. When a write fails the previous values are restored
    /// best-effort and the old config stays live, so a failed PATCH is a no-op.
    pub async fn update(&self, patch: &ConfigPatch) -> Result<Config> {
        if self.is_locked() {
            return Err(Error::PermissionDenied(
                "settings are locked (KARUCODE_LOCK_SETTINGS); change them in the config file or environment"
                    .into(),
            ));
        }

        let _update = self.update_lock.lock().await;
        let old = self.snapshot();
        let mut next = old.clone();
        apply_patch(&mut next, patch)?;
        validate_settings(&next)?;

        let old_rows = rows_for(&old);
        let new_rows = rows_for(&next);
        let mut changed: Vec<(&'static str, String, String)> = Vec::new();
        for (key, value) in &new_rows {
            let previous = old_rows.get(key).cloned().unwrap_or_default();
            if &previous != value {
                changed.push((key, previous, value.clone()));
            }
        }

        let mut written: Vec<(&'static str, String)> = Vec::new();
        for (key, previous, value) in &changed {
            if let Err(e) = self.store.set_setting(key, value).await {
                // Roll back what we already wrote so the DB keeps matching the
                // still-live config.
                for (written_key, written_previous) in written.into_iter().rev() {
                    if let Err(rollback) =
                        self.store.set_setting(written_key, &written_previous).await
                    {
                        warn!(
                            key = written_key,
                            "could not roll back a settings write: {rollback}"
                        );
                    }
                }
                return Err(Error::Store(format!(
                    "could not persist setting `{key}`: {e}"
                )));
            }
            written.push((key, previous.clone()));
        }

        *self.config.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        Ok(next)
    }

    /// Persist and apply an empty API key (the `clear_api_key` flow).
    pub async fn clear_api_key(&self) -> Result<Config> {
        self.update(&ConfigPatch {
            model: Some(ModelPatch {
                clear_api_key: Some(true),
                ..ModelPatch::default()
            }),
            ..ConfigPatch::default()
        })
        .await
    }

    /// Browser-safe config document (`GET/PATCH /api/config`).
    pub async fn public_config(&self) -> serde_json::Value {
        let cfg = self.snapshot();
        let effective = cfg.effective_model();
        serde_json::json!({
            "workspace": cfg.workspace_root.to_string_lossy(),
            "model": public_model(&effective),
            "providers": cfg.providers.iter().map(|p| p.public_view()).collect::<Vec<_>>(),
            "active": {
                "provider": cfg.active_provider,
                "model": cfg.active_model,
            },
            "limits": cfg.limits,
            "approvals": cfg.approvals,
            "subagents": cfg.subagents,
            "titles": {
                "enabled": cfg.titles.enabled,
                "provider": cfg.titles.provider,
                "model": cfg.titles.model,
                "max_chars": cfg.titles.max_chars,
                "timeout_secs": cfg.titles.timeout_secs,
                // What a run would actually ask. `null` provider/model means
                // "inherits the session's model", and the settings screen shows
                // the resolved name so the inheritance is not a mystery.
                "resolved": resolved_title_model(&cfg).map(|model| serde_json::json!({
                    "provider": model.provider,
                    "model": model.model,
                    "inherited": cfg.titles.inherits_session_model(),
                })),
            },
            // Notification routing. Every switch is here rather than in the
            // browser so a client cannot accidentally ignore one: `enabled =
            // false` means the server sends no `notification` frame at all.
            "notifications": public_notifications(&cfg.notifications),
            // Plugin hot-reload settings: read-only here (they are not part of
            // `PATCH /api/config`), but the settings sheet shows what the server
            // is actually watching on, which is the difference between "my edit
            // is loading" and "my edit will never load".
            "plugins_watch": cfg.plugins_watch,
            "plugins_watch_secs": cfg.plugins_watch_secs,
            // The browser terminal. `shell` is resolved so the UI can say what
            // would actually be spawned, and `enabled` is what hides the tab: a
            // deployment that must not offer a shell sets KARUCODE_TERMINALS=0.
            "terminals": {
                "enabled": cfg.terminals.enabled,
                "shell": cfg.terminals.shell_program(),
                "scrollback_bytes": cfg.terminals.replay_capacity(),
                "max_terminals": cfg.terminals.terminal_budget(),
                "idle_timeout_secs": cfg.terminals.idle_timeout_secs,
            },
            "settings_locked": cfg.lock_settings,
            "version": crate::VERSION,
        })
    }
}

/// The `notifications` slice of `GET /api/config`.
///
/// Extracted from `public_config` so a test can assert the wire shape without a
/// database, and so the settings UI has one place to read the switch names from.
pub fn public_notifications(
    notifications: &kc_core::config::NotificationsConfig,
) -> serde_json::Value {
    serde_json::json!({
        "enabled": notifications.enabled,
        "run_finished": notifications.run_finished,
        "needs_input": notifications.needs_input,
        "error": notifications.error,
        "info": notifications.info,
        "plugin": notifications.plugin,
        "min_run_secs": notifications.min_run_secs,
        "say_body": notifications.say_body,
        // The documented ceiling, so the settings screen can validate without
        // hard-coding a number that lives in core.
        "max_min_run_secs": kc_core::config::MAX_MIN_RUN_SECS,
        // The kinds the server can emit. A client should render a kind it does
        // not know as `info` rather than dropping the notification.
        "kinds": kc_core::notify::NotifyKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
    })
}

#[async_trait]
impl Settings for SettingsHandle {
    async fn model(&self) -> ModelConfig {
        self.read().effective_model()
    }

    async fn resolve_model(&self, selection: Option<&SessionModelSelection>) -> ModelConfig {
        resolve_session_model(&self.snapshot(), selection)
    }

    async fn limits(&self) -> Limits {
        self.read().limits.clone()
    }

    async fn approval_policy(&self) -> ApprovalPolicy {
        self.read().approvals.clone()
    }

    async fn subagent_config(&self) -> SubagentConfig {
        self.read().subagents.clone()
    }

    async fn notify_config(&self) -> kc_core::config::NotificationsConfig {
        self.read().notifications.clone()
    }
}

/// Resolve a session's model selection against the current profiles.
///
/// `None` is the install-wide default. A selection whose profile (or model) was
/// deleted after it was saved falls back to the default / the profile's first
/// model with a warning instead of failing the run: settings can change while a
/// session sits idle, and a stale pin must not make the session unusable.
pub fn resolve_session_model(
    config: &Config,
    selection: Option<&SessionModelSelection>,
) -> ModelConfig {
    let Some(selection) = selection else {
        return config.effective_model();
    };
    let provider_id = selection.provider.trim();
    let Some(profile) = config
        .providers
        .iter()
        .find(|profile| profile.id == provider_id)
    else {
        warn!(
            provider = provider_id,
            "the session's provider profile no longer exists; using the install-wide default model"
        );
        return config.effective_model();
    };

    let mut model = config.model_for(profile, &selection.model);
    let model_id = selection.model.trim();
    let known =
        profile.models.is_empty() || profile.models.iter().any(|entry| entry.id == model_id);
    if !known {
        // An unknown id was never looked up in the profile's model list, so the
        // per-model metadata above is missing: fall back to the profile default
        // (which also resets `supports_images` to that model's declaration).
        model = config.model_for_profile(profile);
        warn!(
            provider = %profile.id,
            model = model_id,
            "the session's model is no longer offered by its provider; using the provider's default model"
        );
    }

    if let Some(effort) = selection
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
    {
        // The provider forwards `extra_body` verbatim, which is how every
        // OpenAI-compatible endpoint spells per-request reasoning effort.
        model.extra_body.insert(
            "reasoning_effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    }
    model
}

/// Resolve the model that writes **session titles**.
///
/// Three cases, in order:
///
/// 1. `[titles] provider` + `model` both set: that profile and that model.
///    A profile/model that no longer exists falls back to case 2 (a stale title
///    model must not silently disable the feature).
/// 2. `[titles] provider` alone: that profile's default model.
/// 3. nothing set (the default): the install-wide default model. This is what
///    "inherit" means on the server side; a session with a *pin* inherits its own
///    pin instead, which the caller resolves with [`resolved_title_model_for_session`].
pub fn resolved_title_model(config: &Config) -> Option<ModelConfig> {
    let profile_id = config.titles.provider_id();
    let model_id = config.titles.model_id();
    match (profile_id, model_id) {
        (Some(profile_id), model) => {
            match config
                .providers
                .iter()
                .find(|profile| profile.id == profile_id)
            {
                Some(profile) => Some(match model {
                    Some(model_id) => config.model_for(profile, model_id),
                    None => config.model_for_profile(profile),
                }),
                None => {
                    warn!(
                        provider = profile_id,
                        "titles.provider no longer exists; titles follow the default model"
                    );
                    Some(config.effective_model())
                }
            }
        }
        (None, Some(model_id)) => {
            // A model id with no provider: use the active profile, which is
            // where the user's other model choices live.
            Some(match config.active_profile() {
                Some(profile) => config.model_for(profile, model_id),
                None => {
                    let mut model = config.effective_model();
                    model.model = model_id.to_string();
                    model
                }
            })
        }
        (None, None) => Some(config.effective_model()),
    }
}

/// The model that writes titles **for one session**.
///
/// An explicit `[titles]` model always wins. Otherwise the session's own pin is
/// inherited, so a session running a local 3B model titles itself with that model
/// rather than reaching for the install-wide default it is not otherwise using.
pub fn resolved_title_model_for_session(
    config: &Config,
    selection: Option<&SessionModelSelection>,
) -> Option<ModelConfig> {
    if config.titles.inherits_session_model() {
        return Some(resolve_session_model(config, selection));
    }
    resolved_title_model(config)
}

/// Validate a session's model selection against the live provider profiles.
///
/// Runs at the HTTP boundary so an unknown profile or a model the profile does
/// not offer is a `400 bad_request` naming the field, not a run that silently
/// uses something else.
pub fn validate_session_model(config: &Config, selection: &SessionModelSelection) -> Result<()> {
    selection.validate_shape()?;
    let provider_id = selection.provider.trim();
    let profile = config
        .providers
        .iter()
        .find(|profile| profile.id == provider_id)
        .ok_or_else(|| {
            Error::BadRequest(format!(
                "model.provider `{provider_id}` does not match any configured provider"
            ))
        })?;
    let model_id = selection.model.trim();
    if !profile.models.is_empty() && !profile.models.iter().any(|entry| entry.id == model_id) {
        return Err(Error::BadRequest(format!(
            "model.model `{model_id}` is not in the model list of provider `{}`",
            profile.id
        )));
    }
    Ok(())
}

/// Browser-safe model view. Rounds the f32 tunables so `0.2f32` does not appear
/// as `0.20000000298023224` in JSON (the documented shape shows `0.2`).
pub fn public_model(model: &ModelConfig) -> serde_json::Value {
    let round = |value: f32| ((value as f64) * 10_000.0).round() / 10_000.0;
    serde_json::json!({
        "provider": model.provider,
        "base_url": model.base_url,
        "model": model.model,
        "temperature": round(model.temperature),
        "send_temperature": model.send_temperature,
        "max_tokens": model.max_tokens,
        "top_p": model.top_p.map(round),
        "request_timeout_secs": model.request_timeout_secs,
        "parallel_tool_calls": model.parallel_tool_calls,
        "supports_images": model.supports_images,
        "api_key_set": api_key_set(model),
    })
}

pub fn api_key_set(model: &ModelConfig) -> bool {
    model
        .api_key
        .as_deref()
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Patch application + validation
// ---------------------------------------------------------------------------

fn apply_patch(config: &mut Config, patch: &ConfigPatch) -> Result<()> {
    let mut legacy_connection = false;
    if let Some(model) = &patch.model {
        // `clear_api_key` wins over a value in the same request so the browser
        // can always express "forget it".
        if model.clear_api_key == Some(true) {
            config.model.api_key = Some(String::new());
        }
        if let Some(provider) = &model.provider {
            // Store the canonical (trimmed) id: it was checked against the
            // factory's `supported()` list at the HTTP boundary.
            let trimmed = provider.trim();
            if !trimmed.is_empty() {
                config.model.provider = trimmed.to_string();
            }
        }
        if let Some(base_url) = &model.base_url {
            config.model.base_url = base_url.clone();
        }
        if let Some(name) = &model.model {
            config.model.model = name.clone();
        }
        if let Some(send) = model.send_temperature {
            config.model.send_temperature = send;
        }
        if let Some(temperature) = model.temperature {
            config.model.temperature = temperature;
        }
        if let Some(max_tokens) = model.max_tokens {
            config.model.max_tokens = max_tokens;
        }
        if let Some(top_p) = model.top_p {
            config.model.top_p = top_p;
        }
        if let Some(timeout) = model.request_timeout_secs {
            config.model.request_timeout_secs = timeout;
        }
        if let Some(parallel) = model.parallel_tool_calls {
            config.model.parallel_tool_calls = parallel;
        }
        if let Some(images) = model.supports_images {
            config.model.supports_images = images;
        }
        if model.clear_api_key != Some(true) {
            if let Some(key) = &model.api_key {
                let key = key.trim();
                if !key.is_empty() {
                    config.model.api_key = Some(key.to_string());
                }
            }
        }
        // The legacy flat fields still work for scripts, but they describe the
        // active profile now: mirror them onto it below so a later
        // `normalize_providers` cannot silently undo the patch.
        legacy_connection = model.provider.is_some()
            || model.base_url.is_some()
            || model.model.is_some()
            || model.api_key.is_some()
            || model.clear_api_key.is_some()
            || model.request_timeout_secs.is_some();
    }

    if let Some(providers) = &patch.providers {
        for id in &providers.delete {
            let id = id.trim();
            if id.is_empty() {
                return Err(Error::BadRequest(
                    "providers.delete may not contain an empty id".into(),
                ));
            }
            config.providers.retain(|profile| profile.id != id);
            if config.active_provider.as_deref() == Some(id) {
                config.active_provider = None;
            }
        }
        for incoming in &providers.upsert {
            let id = incoming.id.trim();
            if id.is_empty() {
                return Err(Error::BadRequest(
                    "providers.upsert[].id may not be empty".into(),
                ));
            }
            let index = config.providers.iter().position(|profile| profile.id == id);
            let mut profile = match index {
                Some(index) => config.providers[index].clone(),
                None => ProviderProfile {
                    id: id.to_string(),
                    name: id.to_string(),
                    ..ProviderProfile::default()
                },
            };
            if let Some(name) = &incoming.name {
                profile.name = name.trim().to_string();
            }
            if let Some(provider) = &incoming.provider {
                profile.provider = provider.trim().to_string();
            }
            if let Some(base_url) = &incoming.base_url {
                profile.base_url = base_url.trim().to_string();
            }
            // `clear_api_key` wins over a value in the same request.
            if incoming.clear_api_key == Some(true) {
                profile.api_key = Some(String::new());
            } else if let Some(key) = &incoming.api_key {
                let key = key.trim();
                if !key.is_empty() {
                    profile.api_key = Some(key.to_string());
                }
            }
            if let Some(models) = &incoming.models {
                profile.models = models.clone();
            }
            if let Some(timeout) = incoming.request_timeout_secs {
                profile.request_timeout_secs = timeout;
            }
            match index {
                Some(index) => config.providers[index] = profile,
                None => config.providers.push(profile),
            }
        }
    }

    if let Some(active) = &patch.active {
        if let Some(provider) = &active.provider {
            let provider = provider.trim();
            if !provider.is_empty() {
                config.active_provider = Some(provider.to_string());
            }
        }
        if let Some(model) = &active.model {
            let model = model.trim();
            config.active_model = (!model.is_empty()).then(|| model.to_string());
        }
    }

    if legacy_connection {
        let model_patch = patch.model.as_ref();
        let active = config.active_provider.clone();
        if let Some(profile) = config
            .providers
            .iter_mut()
            .find(|profile| Some(profile.id.as_str()) == active.as_deref())
        {
            if model_patch.and_then(|m| m.provider.as_ref()).is_some() {
                profile.provider = config.model.provider.clone();
            }
            if model_patch.and_then(|m| m.base_url.as_ref()).is_some() {
                profile.base_url = config.model.base_url.clone();
            }
            if model_patch.and_then(|m| m.request_timeout_secs).is_some() {
                profile.request_timeout_secs = config.model.request_timeout_secs;
            }
            if model_patch.and_then(|m| m.clear_api_key) == Some(true) {
                profile.api_key = Some(String::new());
            } else if let Some(key) = model_patch.and_then(|m| m.api_key.as_deref()) {
                if !key.trim().is_empty() {
                    profile.api_key = Some(key.trim().to_string());
                }
            }
            if let Some(model_id) = model_patch.and_then(|m| m.model.as_deref()) {
                let model_id = model_id.trim();
                if !profile.models.iter().any(|model| model.id == model_id) {
                    profile.models.push(ModelEntry::new(model_id));
                }
                config.active_model = Some(model_id.to_string());
            }
        }
    }

    if let Some(limits) = &patch.limits {
        if let Some(v) = limits.max_iterations {
            config.limits.max_iterations = v;
        }
        if let Some(v) = limits.max_run_secs {
            config.limits.max_run_secs = v;
        }
        if let Some(v) = limits.auto_continue {
            config.limits.auto_continue = v;
        }
        if let Some(v) = limits.max_continues {
            config.limits.max_continues = v;
        }
        if let Some(v) = limits.max_tool_output_bytes {
            config.limits.max_tool_output_bytes = v;
        }
        if let Some(v) = limits.max_command_output_bytes {
            config.limits.max_command_output_bytes = v;
        }
        if let Some(v) = limits.command_timeout_secs {
            config.limits.command_timeout_secs = v;
        }
        if let Some(v) = limits.max_command_timeout_secs {
            config.limits.max_command_timeout_secs = v;
        }
        if let Some(v) = limits.max_file_read_bytes {
            config.limits.max_file_read_bytes = v;
        }
        if let Some(v) = limits.max_image_read_bytes {
            config.limits.max_image_read_bytes = v;
        }
        if let Some(v) = limits.search_max_results {
            config.limits.search_max_results = v;
        }
        if let Some(v) = limits.context_max_chars {
            config.limits.context_max_chars = v;
        }
        if let Some(v) = limits.max_tool_calls_per_turn {
            config.limits.max_tool_calls_per_turn = v;
        }
    }

    if let Some(approvals) = &patch.approvals {
        if let Some(mode) = &approvals.mode {
            let parsed = ApprovalMode::parse(mode).ok_or_else(|| {
                Error::BadRequest(format!(
                    "approvals.mode must be one of ask, ask_commands, auto, read_only (got `{mode}`)"
                ))
            })?;
            config.approvals.mode = parsed;
        }
        if let Some(list) = &approvals.allow_commands {
            config.approvals.allow_commands = clean_list("approvals.allow_commands", list)?;
        }
        if let Some(list) = &approvals.auto_approve_paths {
            config.approvals.auto_approve_paths = clean_list("approvals.auto_approve_paths", list)?;
        }
        if let Some(timeout) = approvals.timeout_secs {
            config.approvals.timeout_secs = timeout;
        }
    }

    if let Some(subagents) = &patch.subagents {
        if let Some(v) = subagents.max_concurrent {
            config.subagents.max_concurrent = v;
        }
        if let Some(v) = subagents.max_iterations {
            config.subagents.max_iterations = v;
        }
        if let Some(v) = subagents.max_continues {
            config.subagents.max_continues = v;
        }
        if let Some(v) = subagents.allow_recursive {
            config.subagents.allow_recursive = v;
        }
        if let Some(v) = subagents.max_depth {
            config.subagents.max_depth = v;
        }
        if let Some(v) = subagents.timeout_secs {
            config.subagents.timeout_secs = v;
        }
        if let Some(v) = subagents.allow_background {
            config.subagents.allow_background = v;
        }
        if let Some(v) = subagents.max_background {
            config.subagents.max_background = v;
        }
        if let Some(v) = subagents.wait_timeout_secs {
            config.subagents.wait_timeout_secs = v;
        }
    }

    if let Some(titles) = &patch.titles {
        if let Some(v) = titles.enabled {
            config.titles.enabled = v;
        }
        if let Some(v) = &titles.provider {
            config.titles.provider = v.trim().to_string();
        }
        if let Some(v) = &titles.model {
            config.titles.model = v.trim().to_string();
        }
        if let Some(v) = titles.max_chars {
            config.titles.max_chars = v;
        }
        if let Some(v) = titles.timeout_secs {
            config.titles.timeout_secs = v;
        }
        // `enabled` and the model ids are checked where the config is validated;
        // the content of the ids is free-form here because a provider profile can
        // be renamed in the same patch.
    }

    if let Some(notifications) = &patch.notifications {
        let target = &mut config.notifications;
        if let Some(v) = notifications.enabled {
            target.enabled = v;
        }
        if let Some(v) = notifications.run_finished {
            target.run_finished = v;
        }
        if let Some(v) = notifications.needs_input {
            target.needs_input = v;
        }
        if let Some(v) = notifications.error {
            target.error = v;
        }
        if let Some(v) = notifications.info {
            target.info = v;
        }
        if let Some(v) = notifications.plugin {
            target.plugin = v;
        }
        if let Some(v) = notifications.min_run_secs {
            target.min_run_secs = v;
        }
        if let Some(v) = notifications.say_body {
            target.say_body = v;
        }
    }

    // Repair a missing selection (first profile added, active profile deleted)
    // and mirror the active connection into the flat section. An explicit but
    // wrong selection is an error, not something to silently rewrite.
    let selection_dangling = config.active_provider.is_none();
    if selection_dangling {
        config.normalize_providers();
    }
    if !config.providers.is_empty() {
        validate_active_selection(config)?;
    }
    config.normalize_providers();
    Ok(())
}

fn clean_list(field: &str, list: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Err(Error::BadRequest(format!(
                "{field} may not contain empty entries"
            )));
        }
        if trimmed.contains('\0') {
            return Err(Error::BadRequest(format!(
                "{field} may not contain NUL bytes"
            )));
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// Validate every tunable that the UI can change. Called for the merged config,
/// so a patch can never produce a config that startup validation would reject.
pub fn validate_settings(config: &Config) -> Result<()> {
    // Validate the config as given. Load-time paths normalise first (see
    // `Config::normalize_providers`), an API patch is checked strictly against
    // the profile list by `apply_patch`, and a config with no profiles at all
    // is the legacy flat shape, which `validate_profiles` skips.
    validate_model(&config.model)?;
    validate_profiles(config)?;
    // The runtime reads the effective model, so that is what must be valid too
    // (it catches an active profile with no models and an empty flat fallback).
    validate_model(&config.effective_model())?;
    // An unknown provider in the file, environment or a profile would otherwise
    // start a server that silently speaks a different wire format.
    let supported =
        kc_core::provider::ProviderFactory::supported(&kc_provider::ProviderFactoryImpl);
    check_provider_id(&config.model.provider, &supported)?;
    validate_limits(&config.limits)?;
    validate_approvals(&config.approvals)?;
    validate_subagents(&config.subagents)?;
    validate_titles(&config.titles)?;
    validate_notifications(&config.notifications)?;
    validate_terminals(&config.terminals)
}

/// Validate `[notifications]`.
///
/// Only one value can be out of range. It is still checked here rather than in the
/// patch handler so a bad TOML file or a stale settings row is caught too — with
/// the same message the API would return.
fn validate_notifications(notifications: &kc_core::config::NotificationsConfig) -> Result<()> {
    if notifications.min_run_secs > kc_core::config::MAX_MIN_RUN_SECS {
        return Err(Error::BadRequest(format!(
            "notifications.min_run_secs must be at most {} (got {})",
            kc_core::config::MAX_MIN_RUN_SECS,
            notifications.min_run_secs
        )));
    }
    Ok(())
}

/// Longest a terminal may sit with no connected browser before its shell is
/// reaped, in seconds (30 days). Past this the setting is a typo, not a policy.
const MAX_TERMINAL_IDLE_SECS: u64 = 30 * 24 * 3600;

/// Validate `[terminals]`.
///
/// The shell is checked here rather than at spawn time: a bad `terminals.shell`
/// would otherwise surface as a websocket that closes with a message, every time
/// someone opens the tab. An absolute path must exist and be executable; a bare
/// name must resolve on `PATH` the way the child's `exec` would.
///
/// `enabled = false` still validates the rest: the values are what the settings
/// screen shows, and re-enabling later must not revive a broken shell.
fn validate_terminals(terminals: &TerminalConfig) -> Result<()> {
    if terminals.scrollback_bytes < MIN_TERMINAL_SCROLLBACK
        || terminals.scrollback_bytes > MAX_TERMINAL_SCROLLBACK
    {
        return Err(Error::BadRequest(format!(
            "terminals.scrollback_bytes must be between {MIN_TERMINAL_SCROLLBACK} and \
             {MAX_TERMINAL_SCROLLBACK} (got {})",
            terminals.scrollback_bytes
        )));
    }
    if terminals.max_terminals == 0 || terminals.max_terminals > MAX_TERMINALS_HINT {
        return Err(Error::BadRequest(format!(
            "terminals.max_terminals must be between 1 and {MAX_TERMINALS_HINT} (got {})",
            terminals.max_terminals
        )));
    }
    if terminals.idle_timeout_secs > MAX_TERMINAL_IDLE_SECS {
        return Err(Error::BadRequest(format!(
            "terminals.idle_timeout_secs must be at most {MAX_TERMINAL_IDLE_SECS} (0 keeps a shell \
             until the server restarts)"
        )));
    }
    let shell = terminals.shell.trim();
    if shell.is_empty() {
        return Ok(());
    }
    if shell.chars().any(|c| c.is_control()) {
        return Err(Error::BadRequest(
            "terminals.shell may not contain control characters".into(),
        ));
    }
    // A shell is one executable, not a command line: `"bash -l"` would be a
    // confusing strerror from `exec`, and a space in a path is better handled by
    // an absolute path than by surprise word-splitting.
    if shell.contains(char::is_whitespace) {
        return Err(Error::BadRequest(
            "terminals.shell must be one executable path or name, without arguments".into(),
        ));
    }
    if shell.contains('/') {
        let path = std::path::Path::new(shell);
        let meta = std::fs::metadata(path).map_err(|e| {
            Error::BadRequest(format!("terminals.shell `{shell}` is not usable: {e}"))
        })?;
        if !meta.is_file() {
            return Err(Error::BadRequest(format!(
                "terminals.shell `{shell}` is not a regular file"
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return Err(Error::BadRequest(format!(
                    "terminals.shell `{shell}` is not executable"
                )));
            }
        }
    }
    Ok(())
}

/// Longest a title request may wait. A title nobody sees for a minute is worse
/// than the heuristic title it was going to replace.
const MAX_TITLE_TIMEOUT_SECS: u64 = 120;

/// Validate `[titles]`.
///
/// **`enabled` and the model ids are deliberately not required to resolve here.**
/// Titles are a convenience; an install whose title model was deleted should keep
/// running (and fall back), temporarily disable titles (which is what the user
/// asked for), or rename the profile (which is what validation would forbid).
/// What *is* validated is what a bad value would make dangerous: a title budget
/// of zero (an empty title), a run-away `max_chars`, and a timeout of zero (an
/// instant, always-failing request).
fn validate_titles(titles: &TitlesConfig) -> Result<()> {
    if titles.max_chars < 8 || titles.max_chars > MAX_TITLE_CHARS_HINT {
        return Err(Error::BadRequest(format!(
            "titles.max_chars must be between 8 and {MAX_TITLE_CHARS_HINT} (got {})",
            titles.max_chars
        )));
    }
    if titles.timeout_secs == 0 {
        return Err(Error::BadRequest(
            "titles.timeout_secs must be at least 1".into(),
        ));
    }
    if titles.timeout_secs > MAX_TITLE_TIMEOUT_SECS {
        return Err(Error::BadRequest(format!(
            "titles.timeout_secs must be at most {MAX_TITLE_TIMEOUT_SECS}"
        )));
    }
    for (field, value) in [("provider", &titles.provider), ("model", &titles.model)] {
        if value.len() > 200 {
            return Err(Error::BadRequest(format!(
                "titles.{field} is longer than 200 characters"
            )));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::BadRequest(format!(
                "titles.{field} may not contain control characters"
            )));
        }
    }
    Ok(())
}

/// Bound on how many endpoints one install may configure. Generous for a real
/// setup, small enough that `GET /api/config` stays a cheap document.
pub const MAX_PROFILES: usize = 16;
/// Bound on the curated model list of one profile.
pub const MAX_MODELS_PER_PROFILE: usize = 64;
const MAX_PROFILE_ID_CHARS: usize = 64;
const MAX_PROFILE_NAME_CHARS: usize = 80;
const MAX_MODEL_ID_CHARS: usize = 200;
const MAX_MODEL_NAME_CHARS: usize = 200;
/// Tokens. Far above any current model, low enough to catch a unit mistake
/// (writing 128 (thousands) or the context in characters).
const MAX_CONTEXT_TOKENS: u64 = 100_000_000;
/// USD per million tokens.
const MAX_MODEL_COST: f64 = 1_000_000.0;

fn validate_profiles(config: &Config) -> Result<()> {
    if config.providers.len() > MAX_PROFILES {
        return Err(Error::BadRequest(format!(
            "at most {MAX_PROFILES} provider profiles are supported (got {})",
            config.providers.len()
        )));
    }
    let mut seen: Vec<&str> = Vec::new();
    for profile in &config.providers {
        validate_provider(profile)?;
        if seen.contains(&profile.id.as_str()) {
            return Err(Error::BadRequest(format!(
                "provider id `{}` is used more than once",
                profile.id
            )));
        }
        seen.push(profile.id.as_str());
    }
    if !config.providers.is_empty() {
        validate_active_selection(config)?;
    }
    Ok(())
}

fn validate_active_selection(config: &Config) -> Result<()> {
    let active_id = config.active_provider.as_deref().ok_or_else(|| {
        Error::BadRequest("active.provider is required when provider profiles exist".into())
    })?;
    let profile = config
        .providers
        .iter()
        .find(|profile| profile.id == active_id)
        .ok_or_else(|| {
            Error::BadRequest(format!(
                "active.provider `{active_id}` does not match any configured provider"
            ))
        })?;
    match config.active_model.as_deref() {
        Some(model) => {
            if !profile.models.is_empty() && !profile.models.iter().any(|entry| entry.id == model) {
                return Err(Error::BadRequest(format!(
                    "active.model `{model}` is not in the model list of provider `{}`",
                    profile.id
                )));
            }
        }
        None => {
            if !profile.models.is_empty() {
                return Err(Error::BadRequest(
                    "active.model is required when the active provider has models".into(),
                ));
            }
        }
    }
    Ok(())
}

/// One profile: id, endpoint, library and the tunables that live per endpoint.
pub fn validate_provider(profile: &ProviderProfile) -> Result<()> {
    let id = profile.id.trim();
    if id.is_empty() {
        return Err(Error::BadRequest("providers[].id may not be empty".into()));
    }
    if id.len() > MAX_PROFILE_ID_CHARS {
        return Err(Error::BadRequest(format!(
            "provider id `{id}` is longer than {MAX_PROFILE_ID_CHARS} characters"
        )));
    }
    let first_ok = id
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false);
    if !first_ok
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(Error::BadRequest(format!(
            "provider id `{id}` may only contain letters, digits, '.', '_' and '-', and must start with a letter or digit"
        )));
    }
    if profile.name.chars().any(char::is_control) {
        return Err(Error::BadRequest(format!(
            "provider `{id}` name may not contain control characters"
        )));
    }
    if profile.name.trim().chars().count() > MAX_PROFILE_NAME_CHARS {
        return Err(Error::BadRequest(format!(
            "provider `{id}` name is longer than {MAX_PROFILE_NAME_CHARS} characters"
        )));
    }
    let supported =
        kc_core::provider::ProviderFactory::supported(&kc_provider::ProviderFactoryImpl);
    check_provider_id(&profile.provider, &supported)?;
    validate_base_url(&profile.base_url)?;
    if profile.request_timeout_secs == 0 || profile.request_timeout_secs > 3600 {
        return Err(Error::BadRequest(format!(
            "provider `{id}` request_timeout_secs must be between 1 and 3600 (got {})",
            profile.request_timeout_secs
        )));
    }
    if profile.models.len() > MAX_MODELS_PER_PROFILE {
        return Err(Error::BadRequest(format!(
            "provider `{id}` has more than {MAX_MODELS_PER_PROFILE} models"
        )));
    }
    let mut seen: Vec<&str> = Vec::new();
    for model in &profile.models {
        validate_model_entry(id, model)?;
        if seen.contains(&model.id.trim()) {
            return Err(Error::BadRequest(format!(
                "provider `{id}` lists model `{}` more than once",
                model.id.trim()
            )));
        }
        seen.push(model.id.trim());
    }
    for (name, value) in &profile.extra_headers {
        if name.trim().is_empty()
            || name.contains('\n')
            || name.contains('\r')
            || value.contains('\n')
            || value.contains('\r')
        {
            return Err(Error::BadRequest(format!(
                "provider `{id}` extra_headers contains an invalid header name or value"
            )));
        }
    }
    Ok(())
}

fn validate_model_entry(provider_id: &str, model: &ModelEntry) -> Result<()> {
    let id = model.id.trim();
    if id.is_empty() {
        return Err(Error::BadRequest(format!(
            "provider `{provider_id}` has a model with an empty id"
        )));
    }
    if id.len() > MAX_MODEL_ID_CHARS {
        return Err(Error::BadRequest(format!(
            "provider `{provider_id}` model id is longer than {MAX_MODEL_ID_CHARS} characters"
        )));
    }
    if id.chars().any(char::is_control) {
        return Err(Error::BadRequest(format!(
            "provider `{provider_id}` model `{id}` id may not contain control characters"
        )));
    }
    if let Some(name) = &model.name {
        if name.chars().any(char::is_control) {
            return Err(Error::BadRequest(format!(
                "provider `{provider_id}` model `{id}` name may not contain control characters"
            )));
        }
        if name.trim().chars().count() > MAX_MODEL_NAME_CHARS {
            return Err(Error::BadRequest(format!(
                "provider `{provider_id}` model `{id}` name is longer than {MAX_MODEL_NAME_CHARS} characters"
            )));
        }
    }
    for (field, value) in [
        ("context_window", model.context_window),
        ("max_output_tokens", model.max_output_tokens),
    ] {
        if let Some(value) = value {
            if value == 0 || value > MAX_CONTEXT_TOKENS {
                return Err(Error::BadRequest(format!(
                    "provider `{provider_id}` model `{id}` {field} must be between 1 and {MAX_CONTEXT_TOKENS} (got {value})"
                )));
            }
        }
    }
    for (field, value) in [
        ("cost_input", model.cost_input),
        ("cost_output", model.cost_output),
    ] {
        if let Some(value) = value {
            if !value.is_finite() || !(0.0..=MAX_MODEL_COST).contains(&value) {
                return Err(Error::BadRequest(format!(
                    "provider `{provider_id}` model `{id}` {field} must be between 0 and {MAX_MODEL_COST} USD per million tokens (got {value})"
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_model(model: &ModelConfig) -> Result<()> {
    if model.provider.trim().is_empty() {
        return Err(Error::BadRequest("model.provider may not be empty".into()));
    }
    validate_base_url(&model.base_url)?;
    if model.model.trim().is_empty() {
        return Err(Error::BadRequest("model.model may not be empty".into()));
    }
    if !model.temperature.is_finite() || !(0.0..=2.0).contains(&model.temperature) {
        return Err(Error::BadRequest(format!(
            "model.temperature must be between 0 and 2 (got {})",
            model.temperature
        )));
    }
    if let Some(max_tokens) = model.max_tokens {
        if max_tokens == 0 {
            return Err(Error::BadRequest(
                "model.max_tokens must be greater than 0 (or null for the provider default)".into(),
            ));
        }
        if max_tokens > 2_000_000 {
            return Err(Error::BadRequest(format!(
                "model.max_tokens must be at most 2000000 (got {max_tokens})"
            )));
        }
    }
    if let Some(top_p) = model.top_p {
        if !top_p.is_finite() || !(0.0..=1.0).contains(&top_p) {
            return Err(Error::BadRequest(format!(
                "model.top_p must be between 0 and 1 (got {top_p})"
            )));
        }
    }
    if model.request_timeout_secs == 0 {
        return Err(Error::BadRequest(
            "model.request_timeout_secs must be at least 1".into(),
        ));
    }
    Ok(())
}

/// Parse-level check for the endpoint. Scheme + non-empty authority, no control
/// characters. We deliberately do not resolve DNS here.
pub fn validate_base_url(raw: &str) -> Result<()> {
    let url = raw.trim();
    if url.is_empty() {
        return Err(Error::BadRequest(
            "model.base_url may not be empty; use an OpenAI-compatible API root such as http://127.0.0.1:4000/v1"
                .into(),
        ));
    }
    if url.contains('\n') || url.contains('\r') || url.contains('\0') || url.contains(' ') {
        return Err(Error::BadRequest(
            "model.base_url may not contain whitespace or control characters".into(),
        ));
    }
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| {
            Error::BadRequest(format!(
                "model.base_url must start with http:// or https:// (got `{url}`)"
            ))
        })?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(Error::BadRequest(format!(
            "model.base_url is missing a host (got `{url}`)"
        )));
    }
    validate_authority(authority, url)
}

/// Host/port check without DNS. Accepts IPv6 literals in brackets, IDN hosts and
/// an optional numeric port; rejects empty hosts and bogus ports.
fn validate_authority(authority: &str, url: &str) -> Result<()> {
    // Credentials in a base URL would be echoed by `GET /api/config`; the API
    // key belongs in `model.api_key`.
    if authority.contains('@') {
        return Err(Error::BadRequest(
            "model.base_url may not embed credentials; set model.api_key instead".into(),
        ));
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (inside, after) = rest.split_once(']').ok_or_else(|| {
            Error::BadRequest(format!(
                "model.base_url has an unterminated IPv6 host (got `{url}`)"
            ))
        })?;
        if inside.is_empty() {
            return Err(Error::BadRequest(format!(
                "model.base_url is missing a host (got `{url}`)"
            )));
        }
        match after {
            "" => (inside, None),
            suffix => match suffix.strip_prefix(':') {
                Some(port) => (inside, Some(port)),
                None => {
                    return Err(Error::BadRequest(format!(
                        "model.base_url has an invalid IPv6 host or port (got `{url}`)"
                    )))
                }
            },
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.trim().is_empty() {
        return Err(Error::BadRequest(format!(
            "model.base_url is missing a host (got `{url}`)"
        )));
    }
    if !authority.starts_with('[') && host.contains(':') {
        return Err(Error::BadRequest(format!(
            "model.base_url has an unbracketed IPv6 host; write it as http://[::1]:port/v1 (got `{url}`)"
        )));
    }
    if let Some(port) = port {
        if port.is_empty() || port.parse::<u16>().is_err() {
            return Err(Error::BadRequest(format!(
                "model.base_url has an invalid port `{port}` (expected 1-65535)"
            )));
        }
    }
    Ok(())
}

/// Inclusive numeric bounds for every tunable the UI may change. A PATCH that
/// leaves a value outside its range is rejected with `400 bad_request` naming
/// the field and the bounds — never silently clamped, because a silent clamp
/// hides the mistake from the operator. The upper bounds are the resource
/// limits themselves: `limits.max_file_read_bytes = 4_000_000_000` would make a
/// single read allocate gigabytes and OOM the process.
const LIMITS_BOUNDS: &[(&str, u64, u64)] = &[
    ("limits.max_file_read_bytes", 1, 5_000_000),
    // An image is base64-encoded into the transcript (a third larger) and resent
    // every turn, so the ceiling is the same order as a file read.
    ("limits.max_image_read_bytes", 1, 20_000_000),
    ("limits.max_tool_output_bytes", 256, 1_000_000),
    ("limits.max_command_output_bytes", 256, 1_000_000),
    // 0 = derive the budget from the resolved model's context window; any
    // non-zero value is an explicit ceiling for the install.
    ("limits.context_max_chars", 0, 4_000_000),
    ("limits.max_iterations", 1, 10_000),
    ("limits.max_run_secs", 10, 604_800),
    ("limits.max_continues", 0, 1_000),
    ("limits.command_timeout_secs", 1, 3_600),
    ("limits.max_command_timeout_secs", 1, 3_600),
    ("limits.search_max_results", 1, 2_000),
    ("limits.max_tool_calls_per_turn", 1, 256),
];

const SUBAGENT_BOUNDS: &[(&str, u64, u64)] = &[
    // `0` is the documented "no ceiling": every spawn gets a slot immediately.
    // The finite ceiling is generous on purpose - a fan-out of 64 is a normal
    // review sweep - and what stops a runaway is the cost of 64 provider streams,
    // not a number in this table.
    ("subagents.max_concurrent", 0, 1024),
    ("subagents.max_iterations", 1, 10_000),
    ("subagents.max_continues", 0, 1_000),
    // `0` forbids delegation; the field is a `u8`, so 255 is its own ceiling and
    // is "no practical limit" for nesting.
    ("subagents.max_depth", 0, u8::MAX as u64),
    // Background agents are live loops with a provider stream each, so they get
    // the same ceiling and the same `0` = unlimited meaning as `max_concurrent`.
    ("subagents.max_background", 0, 1024),
    ("subagents.wait_timeout_secs", 1, 3_600),
];

fn check_bounds(name: &str, value: u64, min: u64, max: u64) -> Result<()> {
    if value < min || value > max {
        return Err(Error::BadRequest(format!(
            "{name} must be between {min} and {max} (got {value})"
        )));
    }
    Ok(())
}

fn validate_limits(limits: &Limits) -> Result<()> {
    let values: [(&str, u64); 12] = [
        (
            "limits.max_file_read_bytes",
            limits.max_file_read_bytes as u64,
        ),
        (
            "limits.max_image_read_bytes",
            limits.max_image_read_bytes as u64,
        ),
        (
            "limits.max_tool_output_bytes",
            limits.max_tool_output_bytes as u64,
        ),
        (
            "limits.max_command_output_bytes",
            limits.max_command_output_bytes as u64,
        ),
        ("limits.context_max_chars", limits.context_max_chars as u64),
        ("limits.max_iterations", u64::from(limits.max_iterations)),
        ("limits.max_run_secs", limits.max_run_secs),
        ("limits.max_continues", u64::from(limits.max_continues)),
        ("limits.command_timeout_secs", limits.command_timeout_secs),
        (
            "limits.max_command_timeout_secs",
            limits.max_command_timeout_secs,
        ),
        (
            "limits.search_max_results",
            limits.search_max_results as u64,
        ),
        (
            "limits.max_tool_calls_per_turn",
            limits.max_tool_calls_per_turn as u64,
        ),
    ];
    for ((name, value), (expected, min, max)) in values.iter().zip(LIMITS_BOUNDS) {
        debug_assert_eq!(*name, *expected, "bounds table and values disagree");
        check_bounds(name, *value, *min, *max)?;
    }
    Ok(())
}

fn validate_approvals(policy: &ApprovalPolicy) -> Result<()> {
    if policy.timeout_secs == 0 {
        return Err(Error::BadRequest(
            "approvals.timeout_secs must be at least 1".into(),
        ));
    }
    clean_list("approvals.allow_commands", &policy.allow_commands)?;
    clean_list("approvals.auto_approve_paths", &policy.auto_approve_paths)?;
    Ok(())
}

fn validate_subagents(subagents: &SubagentConfig) -> Result<()> {
    // Three values are meaningful at zero, and they do not agree with each other
    // on purpose: `max_concurrent = 0` and `max_background = 0` are *no ceiling*
    // (the widest setting there is), `max_depth = 0` is the narrowest (delegation
    // is forbidden), and `max_continues = 0` is the one-shot budget. A single
    // "zero means X" rule across the section would be wrong in one direction or
    // the other, so each field documents its own.
    let values: [(&str, u64); 6] = [
        ("subagents.max_concurrent", subagents.max_concurrent as u64),
        (
            "subagents.max_iterations",
            u64::from(subagents.max_iterations),
        ),
        (
            "subagents.max_continues",
            u64::from(subagents.max_continues),
        ),
        ("subagents.max_depth", u64::from(subagents.max_depth)),
        ("subagents.max_background", subagents.max_background as u64),
        ("subagents.wait_timeout_secs", subagents.wait_timeout_secs),
    ];
    for ((name, value), (expected, min, max)) in values.iter().zip(SUBAGENT_BOUNDS) {
        debug_assert_eq!(*name, *expected, "bounds table and values disagree");
        check_bounds(name, *value, *min, *max)?;
    }
    // A subagent may run for hours (a whole coding task), so the ceiling is a
    // week, not an hour: the realistic bound is `timeout_secs` itself.
    check_bounds("subagents.timeout_secs", subagents.timeout_secs, 1, 604_800)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Persistence rows
// ---------------------------------------------------------------------------

/// Apply persisted rows one at a time, keeping each row only when the whole
/// config stays valid. A row that fails validation is warned about (naming the
/// key) and skipped, so its previous — file/env — value survives.
///
/// Validation runs per row, not once at the end, precisely so that one bad row
/// cannot take the process down; the aggregate cannot be invalid either, because
/// the starting config is validated and every accepted row keeps it valid.
fn apply_rows_resilient(config: &mut Config, rows: &[(String, String)], legacy_config: bool) {
    // The provider list and its selection are one logical unit: a profile row
    // applied without its selection (or vice versa) is not a state anyone
    // wrote. Collect them and apply the unit last, so repairing the selection
    // never sees a half-applied library.
    let mut provider_rows: Vec<&(String, String)> = Vec::new();
    for row in rows {
        if matches!(
            row.0.as_str(),
            keys::PROVIDERS | keys::MODEL_ACTIVE_PROVIDER | keys::MODEL_ACTIVE_MODEL
        ) {
            provider_rows.push(row);
            continue;
        }
        let previous = config.clone();
        apply_row(config, &row.0, &row.1);
        if let Err(error) = validate_settings(config) {
            warn!(
                key = row.0.as_str(),
                %error,
                "ignoring a persisted settings row that is no longer valid; keeping the configured value"
            );
            *config = previous;
        }
    }
    let previous = config.clone();
    for row in provider_rows {
        apply_row(config, &row.0, &row.1);
    }
    // A database written before provider profiles existed only has the flat
    // `model.*` rows. Re-synthesize the profile from those rows so an upgraded
    // install keeps the endpoint and key it was actually using. A config that
    // already declared profiles keeps them: those are authoritative.
    if legacy_config && !rows.iter().any(|(key, _)| key == keys::PROVIDERS) {
        config.providers.clear();
    }
    config.normalize_providers();
    if let Err(error) = validate_settings(config) {
        warn!(
            %error,
            "ignoring persisted provider profiles that are no longer valid; keeping the configured value"
        );
        *config = previous;
        config.normalize_providers();
    }
}

fn rows_for(config: &Config) -> BTreeMap<&'static str, String> {
    let mut rows = BTreeMap::new();
    let model = &config.model;
    rows.insert(keys::MODEL_PROVIDER, model.provider.clone());
    rows.insert(keys::MODEL_BASE_URL, model.base_url.clone());
    rows.insert(keys::MODEL_NAME, model.model.clone());
    rows.insert(
        keys::MODEL_API_KEY,
        model.api_key.clone().unwrap_or_default(),
    );
    rows.insert(keys::MODEL_TEMPERATURE, model.temperature.to_string());
    rows.insert(
        keys::MODEL_SEND_TEMPERATURE,
        model.send_temperature.to_string(),
    );
    rows.insert(
        keys::MODEL_MAX_TOKENS,
        model.max_tokens.map(|v| v.to_string()).unwrap_or_default(),
    );
    rows.insert(
        keys::MODEL_TOP_P,
        model.top_p.map(|v| v.to_string()).unwrap_or_default(),
    );
    rows.insert(
        keys::MODEL_REQUEST_TIMEOUT,
        model.request_timeout_secs.to_string(),
    );
    rows.insert(
        keys::MODEL_PARALLEL_TOOL_CALLS,
        model.parallel_tool_calls.to_string(),
    );
    rows.insert(
        keys::MODEL_SUPPORTS_IMAGES,
        model.supports_images.to_string(),
    );
    rows.insert(
        keys::PROVIDERS,
        serde_json::to_string(&config.providers).unwrap_or_else(|_| "[]".into()),
    );
    rows.insert(
        keys::MODEL_ACTIVE_PROVIDER,
        config.active_provider.clone().unwrap_or_default(),
    );
    rows.insert(
        keys::MODEL_ACTIVE_MODEL,
        config.active_model.clone().unwrap_or_default(),
    );

    let limits = &config.limits;
    rows.insert(
        keys::LIMITS_MAX_ITERATIONS,
        limits.max_iterations.to_string(),
    );
    rows.insert(keys::LIMITS_MAX_RUN_SECS, limits.max_run_secs.to_string());
    rows.insert(keys::LIMITS_AUTO_CONTINUE, limits.auto_continue.to_string());
    rows.insert(keys::LIMITS_MAX_CONTINUES, limits.max_continues.to_string());
    rows.insert(
        keys::LIMITS_MAX_TOOL_OUTPUT_BYTES,
        limits.max_tool_output_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_COMMAND_OUTPUT_BYTES,
        limits.max_command_output_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_COMMAND_TIMEOUT_SECS,
        limits.command_timeout_secs.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_COMMAND_TIMEOUT_SECS,
        limits.max_command_timeout_secs.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_FILE_READ_BYTES,
        limits.max_file_read_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_IMAGE_READ_BYTES,
        limits.max_image_read_bytes.to_string(),
    );
    rows.insert(
        keys::LIMITS_SEARCH_MAX_RESULTS,
        limits.search_max_results.to_string(),
    );
    rows.insert(
        keys::LIMITS_CONTEXT_MAX_CHARS,
        limits.context_max_chars.to_string(),
    );
    rows.insert(
        keys::LIMITS_MAX_TOOL_CALLS_PER_TURN,
        limits.max_tool_calls_per_turn.to_string(),
    );

    let approvals = &config.approvals;
    rows.insert(keys::APPROVALS_MODE, approvals.mode.as_str().to_string());
    rows.insert(
        keys::APPROVALS_ALLOW_COMMANDS,
        serde_json::to_string(&approvals.allow_commands).unwrap_or_else(|_| "[]".into()),
    );
    rows.insert(
        keys::APPROVALS_AUTO_APPROVE_PATHS,
        serde_json::to_string(&approvals.auto_approve_paths).unwrap_or_else(|_| "[]".into()),
    );
    rows.insert(
        keys::APPROVALS_TIMEOUT_SECS,
        approvals.timeout_secs.to_string(),
    );

    let subagents = &config.subagents;
    rows.insert(
        keys::SUBAGENTS_MAX_CONCURRENT,
        subagents.max_concurrent.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_MAX_ITERATIONS,
        subagents.max_iterations.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_MAX_CONTINUES,
        subagents.max_continues.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_ALLOW_RECURSIVE,
        subagents.allow_recursive.to_string(),
    );
    rows.insert(keys::SUBAGENTS_MAX_DEPTH, subagents.max_depth.to_string());
    rows.insert(
        keys::SUBAGENTS_TIMEOUT_SECS,
        subagents.timeout_secs.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_ALLOW_BACKGROUND,
        subagents.allow_background.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_MAX_BACKGROUND,
        subagents.max_background.to_string(),
    );
    rows.insert(
        keys::SUBAGENTS_WAIT_TIMEOUT_SECS,
        subagents.wait_timeout_secs.to_string(),
    );

    let titles = &config.titles;
    rows.insert(keys::TITLES_ENABLED, titles.enabled.to_string());
    rows.insert(keys::TITLES_PROVIDER, titles.provider.clone());
    rows.insert(keys::TITLES_MODEL, titles.model.clone());
    rows.insert(keys::TITLES_MAX_CHARS, titles.max_chars.to_string());
    rows.insert(keys::TITLES_TIMEOUT_SECS, titles.timeout_secs.to_string());

    let notifications = &config.notifications;
    rows.insert(
        keys::NOTIFICATIONS_ENABLED,
        notifications.enabled.to_string(),
    );
    rows.insert(
        keys::NOTIFICATIONS_RUN_FINISHED,
        notifications.run_finished.to_string(),
    );
    rows.insert(
        keys::NOTIFICATIONS_NEEDS_INPUT,
        notifications.needs_input.to_string(),
    );
    rows.insert(keys::NOTIFICATIONS_ERROR, notifications.error.to_string());
    rows.insert(keys::NOTIFICATIONS_INFO, notifications.info.to_string());
    rows.insert(keys::NOTIFICATIONS_PLUGIN, notifications.plugin.to_string());
    rows.insert(
        keys::NOTIFICATIONS_MIN_RUN_SECS,
        notifications.min_run_secs.to_string(),
    );
    rows.insert(
        keys::NOTIFICATIONS_SAY_BODY,
        notifications.say_body.to_string(),
    );

    rows
}

fn parse_value<T: std::str::FromStr>(key: &str, value: &str) -> Option<T> {
    match value.trim().parse::<T>() {
        Ok(v) => Some(v),
        Err(_) => {
            // Never log the value: it may be a secret on a mistyped row.
            warn!(key, "ignoring a settings row that is not a valid number");
            None
        }
    }
}

fn parse_bool(key: &str, value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            warn!(key, "ignoring a settings row that is not a valid boolean");
            None
        }
    }
}

fn parse_list(key: &str, value: &str) -> Option<Vec<String>> {
    if value.trim().is_empty() {
        return Some(Vec::new());
    }
    if let Ok(list) = serde_json::from_str::<Vec<String>>(value) {
        return Some(list);
    }
    // Tolerate a hand-edited comma separated list.
    let list: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        warn!(
            key,
            "ignoring a settings row that is not a valid string list"
        );
        None
    } else {
        Some(list)
    }
}

#[cfg(test)]
fn apply_rows(config: &mut Config, rows: &[(String, String)]) {
    for (key, value) in rows {
        apply_row(config, key, value);
    }
    config.normalize_providers();
}

fn apply_row(config: &mut Config, key: &str, value: &str) {
    match key {
        keys::MODEL_PROVIDER => config.model.provider = value.to_string(),
        keys::MODEL_BASE_URL => config.model.base_url = value.to_string(),
        keys::MODEL_NAME => config.model.model = value.to_string(),
        keys::MODEL_API_KEY => config.model.api_key = Some(value.to_string()),
        keys::PROVIDERS => {
            if value.trim().is_empty() {
                return;
            }
            match serde_json::from_str::<Vec<ProviderProfile>>(value) {
                Ok(providers) => config.providers = providers,
                Err(e) => {
                    // Never log the value: it holds API keys.
                    warn!(key, error = %e, "ignoring a provider profile row that is not valid JSON");
                }
            }
        }
        keys::MODEL_ACTIVE_PROVIDER => {
            config.active_provider = (!value.trim().is_empty()).then(|| value.trim().to_string());
        }
        keys::MODEL_ACTIVE_MODEL => {
            config.active_model = (!value.trim().is_empty()).then(|| value.trim().to_string());
        }
        keys::MODEL_SEND_TEMPERATURE => {
            if let Ok(v) = value.parse::<bool>() {
                config.model.send_temperature = v;
            }
        }
        keys::MODEL_TEMPERATURE => {
            if let Some(v) = parse_value::<f32>(key, value) {
                config.model.temperature = v;
            }
        }
        keys::MODEL_MAX_TOKENS => {
            if value.trim().is_empty() {
                config.model.max_tokens = None;
            } else if let Some(v) = parse_value::<u32>(key, value) {
                config.model.max_tokens = Some(v);
            }
        }
        keys::MODEL_TOP_P => {
            if value.trim().is_empty() {
                config.model.top_p = None;
            } else if let Some(v) = parse_value::<f32>(key, value) {
                config.model.top_p = Some(v);
            }
        }
        keys::MODEL_REQUEST_TIMEOUT => {
            if let Some(v) = parse_value::<u64>(key, value) {
                config.model.request_timeout_secs = v;
            }
        }
        keys::MODEL_PARALLEL_TOOL_CALLS => {
            if let Some(v) = parse_bool(key, value) {
                config.model.parallel_tool_calls = v;
            }
        }
        keys::MODEL_SUPPORTS_IMAGES => {
            if let Some(v) = parse_bool(key, value) {
                config.model.supports_images = v;
            }
        }

        keys::LIMITS_MAX_ITERATIONS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_iterations = v;
            }
        }
        keys::LIMITS_MAX_RUN_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_run_secs = v;
            }
        }
        keys::LIMITS_AUTO_CONTINUE => {
            if let Some(v) = parse_bool(key, value) {
                config.limits.auto_continue = v;
            }
        }
        keys::LIMITS_MAX_CONTINUES => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_continues = v;
            }
        }
        keys::LIMITS_MAX_TOOL_OUTPUT_BYTES => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_tool_output_bytes = v;
            }
        }
        keys::LIMITS_MAX_COMMAND_OUTPUT_BYTES => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_command_output_bytes = v;
            }
        }
        keys::LIMITS_COMMAND_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.command_timeout_secs = v;
            }
        }
        keys::LIMITS_MAX_COMMAND_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_command_timeout_secs = v;
            }
        }
        keys::LIMITS_MAX_FILE_READ_BYTES => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_file_read_bytes = v;
            }
        }
        keys::LIMITS_MAX_IMAGE_READ_BYTES => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_image_read_bytes = v;
            }
        }
        keys::LIMITS_SEARCH_MAX_RESULTS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.search_max_results = v;
            }
        }
        keys::LIMITS_CONTEXT_MAX_CHARS => {
            if let Some(v) = parse_value(key, value) {
                config.limits.context_max_chars = v;
            }
        }
        keys::LIMITS_MAX_TOOL_CALLS_PER_TURN => {
            if let Some(v) = parse_value(key, value) {
                config.limits.max_tool_calls_per_turn = v;
            }
        }

        keys::APPROVALS_MODE => match ApprovalMode::parse(value) {
            Some(mode) => config.approvals.mode = mode,
            None => warn!(
                key,
                "ignoring an unknown approval mode in the settings table"
            ),
        },
        keys::APPROVALS_ALLOW_COMMANDS => {
            if let Some(list) = parse_list(key, value) {
                config.approvals.allow_commands = list;
            }
        }
        keys::APPROVALS_AUTO_APPROVE_PATHS => {
            if let Some(list) = parse_list(key, value) {
                config.approvals.auto_approve_paths = list;
            }
        }
        keys::APPROVALS_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.approvals.timeout_secs = v;
            }
        }

        keys::SUBAGENTS_MAX_CONCURRENT => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.max_concurrent = v;
            }
        }
        keys::SUBAGENTS_MAX_ITERATIONS => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.max_iterations = v;
            }
        }
        keys::SUBAGENTS_MAX_CONTINUES => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.max_continues = v;
            }
        }
        keys::SUBAGENTS_ALLOW_RECURSIVE => {
            if let Some(v) = parse_bool(key, value) {
                config.subagents.allow_recursive = v;
            }
        }
        keys::SUBAGENTS_ALLOW_BACKGROUND => {
            if let Some(v) = parse_bool(key, value) {
                config.subagents.allow_background = v;
            }
        }
        keys::SUBAGENTS_MAX_BACKGROUND => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.max_background = v;
            }
        }
        keys::SUBAGENTS_WAIT_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.wait_timeout_secs = v;
            }
        }
        keys::SUBAGENTS_MAX_DEPTH => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.max_depth = v;
            }
        }
        keys::SUBAGENTS_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.subagents.timeout_secs = v;
            }
        }

        keys::TITLES_ENABLED => {
            if let Some(v) = parse_bool(key, value) {
                config.titles.enabled = v;
            }
        }
        keys::TITLES_PROVIDER => config.titles.provider = value.trim().to_string(),
        keys::TITLES_MODEL => config.titles.model = value.trim().to_string(),
        keys::TITLES_MAX_CHARS => {
            if let Some(v) = parse_value(key, value) {
                config.titles.max_chars = v;
            }
        }
        keys::TITLES_TIMEOUT_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.titles.timeout_secs = v;
            }
        }

        keys::NOTIFICATIONS_ENABLED => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.enabled = v;
            }
        }
        keys::NOTIFICATIONS_RUN_FINISHED => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.run_finished = v;
            }
        }
        keys::NOTIFICATIONS_NEEDS_INPUT => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.needs_input = v;
            }
        }
        keys::NOTIFICATIONS_ERROR => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.error = v;
            }
        }
        keys::NOTIFICATIONS_INFO => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.info = v;
            }
        }
        keys::NOTIFICATIONS_PLUGIN => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.plugin = v;
            }
        }
        keys::NOTIFICATIONS_MIN_RUN_SECS => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.min_run_secs = v;
            }
        }
        keys::NOTIFICATIONS_SAY_BODY => {
            if let Some(v) = parse_value(key, value) {
                config.notifications.say_body = v;
            }
        }

        // Unknown keys belong to a newer build; ignore instead of failing.
        _ => {}
    }
}

/// Convert an update error into the HTTP shape (403 for locked settings).
pub fn update_error(error: Error) -> ApiError {
    ApiError::from(error)
}

/// Reject a provider id that the running factory cannot build.
///
/// The set of ids belongs to the factory ([`kc_core::provider::ProviderFactory::supported`]),
/// not to this module, so the check is exposed for the HTTP boundary: without it
/// `model.provider` would accept anything and the UI selector could claim a
/// backend that does not exist. The id is compared trimmed, so a copy-pasted
/// value with surrounding spaces is accepted (it is stored trimmed by the
/// caller) but never a different spelling.
pub fn check_provider_id(provider: &str, supported: &[String]) -> Result<()> {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return Err(Error::BadRequest("model.provider may not be empty".into()));
    }
    if supported.iter().any(|id| id == trimmed) {
        return Ok(());
    }
    Err(Error::BadRequest(format!(
        "model.provider must be one of {} (got `{trimmed}`)",
        supported.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::config::Config;

    fn test_config() -> Config {
        Config {
            lock_settings: true,
            ..Config::default()
        }
    }

    fn config_with_admin(token: Option<&str>) -> Config {
        Config {
            admin_token: token.map(str::to_string),
            ..Config::default()
        }
    }

    fn approvals_patch(mode: &str) -> ConfigPatch {
        ConfigPatch {
            approvals: Some(ApprovalsPatch {
                mode: Some(mode.to_string()),
                ..ApprovalsPatch::default()
            }),
            ..ConfigPatch::default()
        }
    }

    fn limits_patch() -> ConfigPatch {
        ConfigPatch {
            limits: Some(LimitsPatch {
                max_iterations: Some(42),
                ..LimitsPatch::default()
            }),
            ..ConfigPatch::default()
        }
    }

    #[test]
    fn security_fields_are_recognised() {
        assert_eq!(approvals_patch("auto").security_fields(), vec!["approvals"]);
        assert!(limits_patch().security_fields().is_empty());

        let endpoint = ConfigPatch {
            model: Some(ModelPatch {
                base_url: Some("https://attacker.example/v1".into()),
                ..ModelPatch::default()
            }),
            ..ConfigPatch::default()
        };
        assert_eq!(endpoint.security_fields(), vec!["model.base_url"]);

        let key = ConfigPatch {
            model: Some(ModelPatch {
                api_key: Some("sk-x".into()),
                ..ModelPatch::default()
            }),
            ..ConfigPatch::default()
        };
        assert_eq!(key.security_fields(), vec!["model.api_key"]);

        // Erasing the key is as sensitive as setting one.
        let clear = ConfigPatch {
            model: Some(ModelPatch {
                clear_api_key: Some(true),
                ..ModelPatch::default()
            }),
            ..ConfigPatch::default()
        };
        assert_eq!(clear.security_fields(), vec!["model.api_key"]);

        let providers = ConfigPatch {
            providers: Some(ProvidersPatch::default()),
            ..ConfigPatch::default()
        };
        assert_eq!(providers.security_fields(), vec!["providers"]);

        let active = ConfigPatch {
            active: Some(ActivePatch {
                provider: Some("p".into()),
                model: Some("m".into()),
            }),
            ..ConfigPatch::default()
        };
        assert_eq!(
            active.security_fields(),
            vec!["active_provider", "active_model"]
        );
    }

    #[test]
    fn without_an_admin_token_the_gate_is_off() {
        let config = config_with_admin(None);
        assert!(authorize_patch(&config, &approvals_patch("auto"), None).is_ok());
        // An empty token is treated as unset rather than as "match the empty
        // string", which would be a trivially bypassable gate.
        let empty = config_with_admin(Some(""));
        assert!(authorize_patch(&empty, &approvals_patch("auto"), None).is_ok());
    }

    #[test]
    fn a_security_patch_without_the_admin_token_is_refused() {
        let config = config_with_admin(Some("s3cret-admin"));
        let error = authorize_patch(&config, &approvals_patch("auto"), None).unwrap_err();
        assert!(matches!(error, Error::PermissionDenied(_)), "{error:?}");
        assert!(error.to_string().contains("X-Karucode-Admin-Token"));
        // The message names the field but never the credential.
        assert!(error.to_string().contains("approvals"));
        assert!(!error.to_string().contains("s3cret-admin"));
    }

    #[test]
    fn a_wrong_admin_token_is_refused() {
        let config = config_with_admin(Some("s3cret-admin"));
        let error =
            authorize_patch(&config, &approvals_patch("auto"), Some("s3cret-admi")).unwrap_err();
        assert!(matches!(error, Error::PermissionDenied(_)), "{error:?}");
        assert!(!error.to_string().contains("s3cret-admin"));
    }

    #[test]
    fn the_admin_token_authorises_a_security_patch() {
        let config = config_with_admin(Some("s3cret-admin"));
        let fields = authorize_patch(&config, &approvals_patch("auto"), Some("s3cret-admin"))
            .expect("authorised");
        assert_eq!(fields, vec!["approvals"]);
        // Surrounding whitespace from a header value is tolerated.
        assert!(authorize_patch(&config, &approvals_patch("auto"), Some(" s3cret-admin ")).is_ok());
    }

    #[test]
    fn a_non_security_patch_needs_no_admin_token() {
        let config = config_with_admin(Some("s3cret-admin"));
        let fields = authorize_patch(&config, &limits_patch(), None).expect("allowed");
        assert!(fields.is_empty());
    }

    #[test]
    fn the_admin_comparison_is_exact_and_constant_time() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn every_limit_is_bounded_and_names_the_field() {
        // (mutation, expected field name in the error)
        let cases: Vec<(&str, u64, u64)> = vec![
            ("limits.max_file_read_bytes", 0, 5_000_001),
            ("limits.max_tool_output_bytes", 255, 1_000_001),
            ("limits.max_command_output_bytes", 255, 1_000_001),
            // 0 is the "derive it from the model" sentinel, so only the high end
            // is out of range.
            ("limits.context_max_chars", 4_000_001, 4_000_001),
            ("limits.max_iterations", 0, 10_001),
            ("limits.max_run_secs", 9, 604_801),
            ("limits.max_continues", 1_001, 1_001),
            ("limits.command_timeout_secs", 0, 3_601),
            ("limits.max_command_timeout_secs", 0, 3_601),
            ("limits.search_max_results", 0, 2_001),
            ("limits.max_tool_calls_per_turn", 0, 257),
        ];
        for (field, low, high) in cases {
            for value in [low, high] {
                let mut cfg = Config::default();
                set_limit(&mut cfg, field, value);
                let error = validate_settings(&cfg).unwrap_err().to_string();
                assert!(
                    error.contains(field),
                    "{field}={value} should be rejected naming the field, got: {error}"
                );
                assert!(
                    error.contains("must be between"),
                    "{field}={value} should describe the bounds, got: {error}"
                );
            }
        }
    }

    #[test]
    fn subagent_bounds_are_enforced_and_depth_zero_is_allowed() {
        let cases: Vec<(&str, u64)> = vec![
            ("subagents.max_concurrent", 1_025),
            ("subagents.max_iterations", 0),
            ("subagents.max_iterations", 10_001),
            ("subagents.max_continues", 1_001),
            ("subagents.max_background", 1_025),
            ("subagents.timeout_secs", 0),
            ("subagents.timeout_secs", 604_801),
        ];
        for (field, value) in cases {
            let mut cfg = Config::default();
            match field {
                "subagents.max_concurrent" => cfg.subagents.max_concurrent = value as usize,
                "subagents.max_iterations" => cfg.subagents.max_iterations = value as u32,
                "subagents.max_continues" => cfg.subagents.max_continues = value as u32,
                "subagents.max_background" => cfg.subagents.max_background = value as usize,
                "subagents.timeout_secs" => cfg.subagents.timeout_secs = value,
                _ => cfg.subagents.max_depth = value as u8,
            }
            let error = validate_settings(&cfg).unwrap_err().to_string();
            assert!(error.contains(field), "{field}={value}: {error}");
        }

        // `max_depth` is a `u8`, so the type is the ceiling and `256` cannot be
        // constructed here at all - a JSON body claiming it fails deserialisation
        // before validation runs (see `tests/api.rs`). What this test owns is the
        // other end: `0` and `255` are both valid.
        let mut cfg = Config::default();
        cfg.subagents.max_depth = u8::MAX;
        assert!(validate_settings(&cfg).is_ok(), "max_depth = 255 is valid");

        let mut cfg = Config::default();
        cfg.subagents.max_depth = 0;
        assert!(validate_settings(&cfg).is_ok(), "max_depth = 0 is valid");

        // `max_continues = 0` is the documented one-shot budget, not an error.
        let mut cfg = Config::default();
        cfg.subagents.max_continues = 0;
        assert!(
            validate_settings(&cfg).is_ok(),
            "max_continues = 0 restores the one-shot behaviour"
        );
    }

    /// The "unlimited" spelling has to survive the whole stack, because three
    /// different readers depend on it agreeing: the settings API, the limiter and
    /// the background check. A clamp anywhere in the middle turns the widest
    /// setting an operator can ask for into the narrowest one silently.
    #[test]
    fn unbounded_subagents_are_a_supported_setting() {
        let mut cfg = Config::default();
        cfg.subagents.max_concurrent = 0;
        cfg.subagents.max_background = 0;
        cfg.subagents.max_depth = u8::MAX;
        cfg.subagents.allow_recursive = true;
        assert!(
            validate_settings(&cfg).is_ok(),
            "0 concurrency, 0 background and max depth are all valid: {:?}",
            validate_settings(&cfg)
        );

        // And the settings round-trip keeps them: a clamp on the write path would
        // make the form lie about what it saved.
        let rows: Vec<(String, String)> = rows_for(&cfg)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let mut restored = Config::default();
        apply_rows(&mut restored, &rows);
        assert_eq!(restored.subagents.max_concurrent, 0);
        assert_eq!(restored.subagents.max_background, 0);
        assert_eq!(restored.subagents.max_depth, u8::MAX);
    }

    #[test]
    fn default_settings_satisfy_the_bounds() {
        assert!(validate_settings(&Config::default()).is_ok());
    }

    fn set_limit(cfg: &mut Config, field: &str, value: u64) {
        match field {
            "limits.max_file_read_bytes" => cfg.limits.max_file_read_bytes = value as usize,
            "limits.max_tool_output_bytes" => cfg.limits.max_tool_output_bytes = value as usize,
            "limits.max_command_output_bytes" => {
                cfg.limits.max_command_output_bytes = value as usize
            }
            "limits.context_max_chars" => cfg.limits.context_max_chars = value as usize,
            "limits.max_iterations" => cfg.limits.max_iterations = value as u32,
            "limits.max_run_secs" => cfg.limits.max_run_secs = value,
            "limits.max_continues" => cfg.limits.max_continues = value as u32,
            "limits.command_timeout_secs" => cfg.limits.command_timeout_secs = value,
            "limits.max_command_timeout_secs" => cfg.limits.max_command_timeout_secs = value,
            "limits.search_max_results" => cfg.limits.search_max_results = value as usize,
            "limits.max_tool_calls_per_turn" => cfg.limits.max_tool_calls_per_turn = value as usize,
            other => panic!("unknown limit {other}"),
        }
    }

    #[test]
    fn only_supported_provider_ids_are_accepted() {
        let supported = vec!["openai_compatible".to_string()];
        assert!(check_provider_id("openai_compatible", &supported).is_ok());
        assert!(check_provider_id("  openai_compatible  ", &supported).is_ok());

        let err = check_provider_id("anthropic", &supported)
            .unwrap_err()
            .to_string();
        assert!(err.contains("model.provider"), "{err}");
        assert!(err.contains("openai_compatible"), "{err}");

        assert!(check_provider_id("", &supported).is_err());
        assert!(check_provider_id("OpenAI_Compatible", &supported).is_err());
        // A factory that can build more than one provider lists them all.
        let more = vec!["openai_compatible".to_string(), "echo".to_string()];
        assert!(check_provider_id("echo", &more).is_ok());
        let err = check_provider_id("nope", &more).unwrap_err().to_string();
        assert!(err.contains("openai_compatible, echo"), "{err}");
    }

    #[test]
    fn invalid_persisted_rows_are_skipped_individually() {
        let mut cfg = Config::default();
        let base_url = cfg.model.base_url.clone();
        apply_rows_resilient(
            &mut cfg,
            &[
                // Out-of-range limit and a bogus URL: both must be ignored ...
                ("limits.max_iterations".into(), "99999".into()),
                ("model.base_url".into(), "ftp://nope".into()),
                // ... while a valid row still applies.
                ("model.temperature".into(), "0.5".into()),
                ("limits.search_max_results".into(), "7".into()),
            ],
            true,
        );
        assert_eq!(
            cfg.limits.max_iterations,
            Limits::default().max_iterations,
            "invalid row ignored"
        );
        assert_eq!(cfg.model.base_url, base_url, "invalid row ignored");
        assert!((cfg.model.temperature - 0.5).abs() < f32::EPSILON);
        assert_eq!(cfg.limits.search_max_results, 7);
        // The result is a config `SettingsHandle::update` would also accept.
        assert!(validate_settings(&cfg).is_ok());
    }

    #[test]
    fn unparsable_persisted_rows_are_ignored() {
        let mut cfg = Config::default();
        apply_rows_resilient(
            &mut cfg,
            &[
                ("model.temperature".into(), "hot".into()),
                ("approvals.mode".into(), "whatever".into()),
                ("some.future.key".into(), "x".into()),
            ],
            true,
        );
        assert_eq!(cfg.model.temperature, 0.2);
        assert_eq!(cfg.approvals.mode, ApprovalMode::Ask);
    }

    #[test]
    fn patch_validates_temperature_range() {
        let mut cfg = test_config();
        let patch = ConfigPatch {
            model: Some(ModelPatch {
                temperature: Some(2.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        let err = validate_settings(&cfg).unwrap_err();
        assert!(err.to_string().contains("temperature"));
    }

    #[test]
    fn patch_rejects_zero_max_tokens() {
        let mut cfg = test_config();
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    max_tokens: Some(Some(0)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(validate_settings(&cfg).is_err());
    }

    #[test]
    fn patch_clears_max_tokens_with_null() {
        let mut cfg = test_config();
        cfg.model.max_tokens = Some(100);
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    max_tokens: Some(None),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.model.max_tokens, None);
    }

    #[test]
    fn base_url_validation_checks_scheme_host_and_port() {
        assert!(validate_base_url("http://127.0.0.1:4000/v1").is_ok());
        assert!(validate_base_url("https://gateway.example.com/v1").is_ok());
        assert!(validate_base_url("http://[::1]:4000/v1").is_ok());
        assert!(validate_base_url("https://例え.jp/v1").is_ok());
        assert!(validate_base_url("http://host:notaport/v1").is_err());
        assert!(validate_base_url("http://:4000/v1").is_err());
        assert!(validate_base_url("http://[::1/v1").is_err());
        assert!(validate_base_url("http://[::1]x/v1").is_err());
        assert!(validate_base_url("http://host:70000/v1").is_err());
        assert!(validate_base_url("http:///v1").is_err());
        assert!(validate_base_url("host:4000").is_err());
        assert!(validate_base_url("http://a b/v1").is_err());
        assert!(validate_base_url("http://user:pass@host/v1").is_err());
        assert!(validate_base_url("http://::1:4000/v1").is_err());
    }

    #[test]
    fn patch_rejects_bad_base_url_and_mode() {
        let mut cfg = test_config();
        let err = apply_patch(
            &mut cfg,
            &ConfigPatch {
                model: Some(ModelPatch {
                    base_url: Some("ftp://example.com".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .and_then(|_| validate_settings(&cfg))
        .unwrap_err();
        assert!(err.to_string().contains("http://"));

        let err = apply_patch(
            &mut cfg,
            &ConfigPatch {
                approvals: Some(ApprovalsPatch {
                    mode: Some("sometimes".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("approvals.mode"));
    }

    #[test]
    fn patch_accepts_zero_concurrency_as_unlimited() {
        let mut cfg = test_config();
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                subagents: Some(SubagentsPatch {
                    max_concurrent: Some(0),
                    max_background: Some(0),
                    max_depth: Some(u8::MAX),
                    allow_recursive: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            validate_settings(&cfg).is_ok(),
            "0 concurrency means no ceiling, not an invalid value: {:?}",
            validate_settings(&cfg)
        );
        assert_eq!(cfg.subagents.max_concurrent, 0);
        assert_eq!(cfg.subagents.max_background, 0);
        assert_eq!(cfg.subagents.max_depth, u8::MAX);
    }

    #[test]
    fn patch_rejects_concurrency_over_the_finite_ceiling() {
        let mut cfg = test_config();
        apply_patch(
            &mut cfg,
            &ConfigPatch {
                subagents: Some(SubagentsPatch {
                    max_concurrent: Some(1_025),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let error = validate_settings(&cfg).unwrap_err().to_string();
        assert!(error.contains("subagents.max_concurrent"), "{error}");
    }

    #[test]
    fn provider_profiles_and_selection_roundtrip_through_rows() {
        let mut cfg = test_config();
        cfg.providers = vec![
            ProviderProfile {
                id: "gateway".into(),
                name: "Gateway".into(),
                base_url: "https://gw.example/v1".into(),
                api_key: Some("sk-gateway".into()),
                models: vec![
                    ModelEntry {
                        id: "model-a".into(),
                        name: Some("Model A".into()),
                        context_window: Some(128_000),
                        max_output_tokens: Some(16_384),
                        tool_call: Some(true),
                        ..ModelEntry::default()
                    },
                    ModelEntry::new("model-b"),
                ],
                ..ProviderProfile::default()
            },
            ProviderProfile {
                id: "local".into(),
                name: "Local".into(),
                base_url: "http://127.0.0.1:8000/v1".into(),
                models: vec![ModelEntry::new("tiny")],
                ..ProviderProfile::default()
            },
        ];
        cfg.active_provider = Some("gateway".into());
        cfg.active_model = Some("model-b".into());
        cfg.normalize_providers();
        assert!(validate_settings(&cfg).is_ok());

        let rows: Vec<(String, String)> = rows_for(&cfg)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert!(rows.iter().any(|(k, _)| k == keys::PROVIDERS));
        // The profile row is the only place the key is stored.
        assert!(!rows
            .iter()
            .any(|(k, v)| k == keys::MODEL_API_KEY && v.contains("sk-gateway")));

        let mut restored = Config::default();
        apply_rows(&mut restored, &rows);
        assert_eq!(restored.providers.len(), 2);
        assert_eq!(restored.active_provider.as_deref(), Some("gateway"));
        assert_eq!(restored.active_model.as_deref(), Some("model-b"));
        assert_eq!(
            restored.effective_model().api_key.as_deref(),
            Some("sk-gateway")
        );
        assert_eq!(
            restored.providers[0].models[0].context_window,
            Some(128_000)
        );
        assert_eq!(restored.effective_model().base_url, "https://gw.example/v1");
    }

    #[test]
    fn a_new_profile_can_be_added_and_selected_in_one_patch() {
        let mut cfg = test_config();
        cfg.normalize_providers();
        let patch = ConfigPatch {
            providers: Some(ProvidersPatch {
                upsert: vec![ProviderPatch {
                    id: "gateway".into(),
                    name: Some("My gateway".into()),
                    base_url: Some("https://gw.example/v1".into()),
                    api_key: Some("sk-secret".into()),
                    models: Some(vec![ModelEntry::new("model-a")]),
                    ..ProviderPatch::default()
                }],
                delete: Vec::new(),
            }),
            active: Some(ActivePatch {
                provider: Some("gateway".into()),
                model: Some("model-a".into()),
            }),
            ..ConfigPatch::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        assert!(validate_settings(&cfg).is_ok());
        assert_eq!(cfg.active_provider.as_deref(), Some("gateway"));
        let effective = cfg.effective_model();
        assert_eq!(effective.base_url, "https://gw.example/v1");
        assert_eq!(effective.api_key.as_deref(), Some("sk-secret"));
        assert_eq!(effective.model, "model-a");

        // A second patch that only replaces the model list keeps the key.
        let patch = ConfigPatch {
            providers: Some(ProvidersPatch {
                upsert: vec![ProviderPatch {
                    id: "gateway".into(),
                    models: Some(vec![ModelEntry::new("model-a"), ModelEntry::new("model-b")]),
                    ..ProviderPatch::default()
                }],
                ..ProvidersPatch::default()
            }),
            ..ConfigPatch::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        let profile = cfg.active_profile().unwrap();
        assert_eq!(profile.models.len(), 2);
        assert_eq!(profile.api_key.as_deref(), Some("sk-secret"));
    }

    #[test]
    fn deleting_the_active_profile_reselects_another() {
        let mut cfg = test_config();
        cfg.providers = vec![
            ProviderProfile {
                id: "a".into(),
                models: vec![ModelEntry::new("ma")],
                ..ProviderProfile::default()
            },
            ProviderProfile {
                id: "b".into(),
                models: vec![ModelEntry::new("mb")],
                ..ProviderProfile::default()
            },
        ];
        cfg.active_provider = Some("a".into());
        cfg.active_model = Some("ma".into());
        cfg.normalize_providers();

        apply_patch(
            &mut cfg,
            &ConfigPatch {
                providers: Some(ProvidersPatch {
                    delete: vec!["a".into()],
                    ..ProvidersPatch::default()
                }),
                ..ConfigPatch::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.active_provider.as_deref(), Some("b"));
        assert_eq!(cfg.active_model.as_deref(), Some("mb"));
    }

    #[test]
    fn provider_validation_rejects_bad_shapes_and_names_the_field() {
        let mut cfg = test_config();
        cfg.providers = vec![ProviderProfile {
            id: "bad id".into(),
            ..ProviderProfile::default()
        }];
        cfg.active_provider = Some("bad id".into());
        let err = validate_settings(&cfg).unwrap_err().to_string();
        assert!(err.contains("letters, digits"), "{err}");

        let mut cfg = test_config();
        cfg.providers = vec![ProviderProfile {
            id: "dup".into(),
            models: vec![ModelEntry::new("m"), ModelEntry::new("m")],
            ..ProviderProfile::default()
        }];
        cfg.active_provider = Some("dup".into());
        cfg.active_model = Some("m".into());
        let err = validate_settings(&cfg).unwrap_err().to_string();
        assert!(err.contains("more than once"), "{err}");

        let mut cfg = test_config();
        cfg.providers = vec![ProviderProfile {
            id: "ctx".into(),
            models: vec![ModelEntry {
                id: "m".into(),
                context_window: Some(0),
                ..ModelEntry::default()
            }],
            ..ProviderProfile::default()
        }];
        cfg.active_provider = Some("ctx".into());
        cfg.active_model = Some("m".into());
        let err = validate_settings(&cfg).unwrap_err().to_string();
        assert!(err.contains("context_window"), "{err}");

        let mut cfg = test_config();
        cfg.providers = vec![
            ProviderProfile {
                id: "dup-id".into(),
                ..ProviderProfile::default()
            },
            ProviderProfile {
                id: "dup-id".into(),
                ..ProviderProfile::default()
            },
        ];
        cfg.active_provider = Some("dup-id".into());
        let err = validate_settings(&cfg).unwrap_err().to_string();
        assert!(err.contains("used more than once"), "{err}");

        // An unknown selection is rejected rather than silently repaired.
        let mut cfg = test_config();
        cfg.providers = vec![ProviderProfile {
            id: "p".into(),
            models: vec![ModelEntry::new("m")],
            ..ProviderProfile::default()
        }];
        cfg.active_provider = Some("p".into());
        cfg.active_model = Some("other".into());
        let err = validate_settings(&cfg).unwrap_err().to_string();
        assert!(err.contains("active.model"), "{err}");
    }

    fn selection(provider: &str, model: &str, effort: Option<&str>) -> SessionModelSelection {
        SessionModelSelection {
            provider: provider.into(),
            model: model.into(),
            reasoning_effort: effort.map(str::to_string),
        }
    }

    fn two_profile_config() -> Config {
        let mut cfg = test_config();
        cfg.model.temperature = 0.4;
        cfg.providers = vec![
            ProviderProfile {
                id: "gateway".into(),
                base_url: "https://gw.example/v1".into(),
                api_key: Some("sk-gateway".into()),
                models: vec![ModelEntry::new("model-a"), ModelEntry::new("model-b")],
                ..ProviderProfile::default()
            },
            ProviderProfile {
                id: "local".into(),
                base_url: "http://127.0.0.1:8000/v1".into(),
                models: vec![ModelEntry::new("tiny")],
                ..ProviderProfile::default()
            },
        ];
        cfg.active_provider = Some("gateway".into());
        cfg.active_model = Some("model-a".into());
        cfg.normalize_providers();
        cfg
    }

    #[test]
    fn no_selection_is_the_install_wide_default() {
        let cfg = two_profile_config();
        let resolved = resolve_session_model(&cfg, None);
        assert_eq!(resolved.model, "model-a");
        assert_eq!(resolved.base_url, "https://gw.example/v1");
        assert_eq!(resolved.api_key.as_deref(), Some("sk-gateway"));
        // Sampling stays global for every session.
        assert!((resolved.temperature - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn a_session_selection_resolves_to_its_own_profile_and_model() {
        let cfg = two_profile_config();
        // The other profile, which is not the install-wide active one.
        let resolved = resolve_session_model(&cfg, Some(&selection("local", "tiny", None)));
        assert_eq!(resolved.model, "tiny");
        assert_eq!(resolved.base_url, "http://127.0.0.1:8000/v1");
        assert!(resolved.api_key.is_none());
        // Selecting another model on the active profile does not touch the
        // install-wide active selection.
        let resolved = resolve_session_model(&cfg, Some(&selection("gateway", "model-b", None)));
        assert_eq!(resolved.model, "model-b");
        assert_eq!(cfg.active_model.as_deref(), Some("model-a"));
    }

    #[test]
    fn reasoning_effort_rides_in_extra_body() {
        let cfg = two_profile_config();
        let resolved = resolve_session_model(&cfg, Some(&selection("local", "tiny", Some("high"))));
        assert_eq!(
            resolved.extra_body.get("reasoning_effort"),
            Some(&serde_json::Value::String("high".into()))
        );
        // A blank effort means "use the endpoint default", not `""`.
        let resolved = resolve_session_model(&cfg, Some(&selection("local", "tiny", Some("  "))));
        assert!(!resolved.extra_body.contains_key("reasoning_effort"));
    }

    #[test]
    fn a_stale_selection_falls_back_instead_of_failing_the_run() {
        let cfg = two_profile_config();
        // Profile deleted after the selection was saved.
        let resolved = resolve_session_model(&cfg, Some(&selection("gone", "model-x", None)));
        assert_eq!(resolved.model, "model-a", "falls back to the default");
        assert_eq!(resolved.base_url, "https://gw.example/v1");
        // Model removed from an existing profile: use that profile's default.
        let resolved = resolve_session_model(&cfg, Some(&selection("gateway", "removed", None)));
        assert_eq!(resolved.model, "model-a");
        assert_eq!(resolved.base_url, "https://gw.example/v1");
    }

    /// The reported bug, end to end at the resolver: a session pinned to a
    /// vision model must be able to read an image even though the profile's
    /// default model (and the install-wide flag) say text-only. `read_file`
    /// reads `ToolCtx::supports_images`, which the runtime fills from exactly
    /// this `ModelConfig`.
    #[test]
    fn a_session_pinned_to_a_vision_model_can_see_images() {
        let mut cfg = two_profile_config();
        // The profile's *first* model is the install-wide default: text-only.
        cfg.model.supports_images = false;
        cfg.providers[0].models.push(ModelEntry {
            id: "gpt-4o".into(),
            name: Some("GPT-4o".into()),
            context_window: Some(128_000),
            vision: Some(true),
            ..ModelEntry::default()
        });
        cfg.normalize_providers();

        // The install-wide default stays text-only.
        assert!(!resolve_session_model(&cfg, None).supports_images);
        assert!(
            !resolve_session_model(&cfg, Some(&selection("gateway", "model-a", None)))
                .supports_images
        );

        // The pin is what the session runs, so it is what decides.
        let pinned = resolve_session_model(&cfg, Some(&selection("gateway", "gpt-4o", None)));
        assert_eq!(pinned.model, "gpt-4o");
        assert!(
            pinned.supports_images,
            "a session pinned to a vision model must be allowed to read images"
        );
        // The per-model metadata follows the pin, not the profile default.
        assert_eq!(pinned.context_window, Some(128_000));

        // A stale pin still resolves to the default model *and* its capabilities.
        let stale = resolve_session_model(&cfg, Some(&selection("gateway", "deleted", None)));
        assert_eq!(stale.model, "model-a");
        assert!(!stale.supports_images);
    }

    #[test]
    fn session_selection_validation_names_the_field() {
        let cfg = two_profile_config();
        assert!(validate_session_model(&cfg, &selection("local", "tiny", Some("high"))).is_ok());

        let err = validate_session_model(&cfg, &selection("nope", "tiny", None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("model.provider"), "{err}");

        let err = validate_session_model(&cfg, &selection("local", "model-b", None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("model.model"), "{err}");

        let err = validate_session_model(&cfg, &selection("local", "tiny", Some("extreme")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("reasoning_effort"), "{err}");

        let mut blank = selection("local", "tiny", None);
        blank.provider = "  ".into();
        assert!(validate_session_model(&cfg, &blank).is_err());
    }

    #[test]
    fn api_key_is_cleared_and_never_in_public_config() {
        let mut cfg = test_config();
        cfg.model.api_key = Some("sk-secret".into());
        let patch = ConfigPatch {
            model: Some(ModelPatch {
                clear_api_key: Some(true),
                api_key: Some("sk-other".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_patch(&mut cfg, &patch).unwrap();
        assert_eq!(cfg.model.api_key.as_deref(), Some(""));
        assert!(!api_key_set(&cfg.model));
    }

    #[test]
    fn rows_roundtrip_every_tunable() {
        let mut cfg = Config::default();
        cfg.model.temperature = 0.75;
        cfg.model.max_tokens = Some(512);
        cfg.model.top_p = Some(0.5);
        cfg.limits.max_iterations = 41;
        cfg.approvals.mode = ApprovalMode::AskCommands;
        cfg.approvals.allow_commands = vec!["cargo test".into(), "rg".into()];
        cfg.subagents.max_depth = 2;
        let rows: Vec<(String, String)> = rows_for(&cfg)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        let mut restored = Config::default();
        apply_rows(&mut restored, &rows);
        assert!((restored.model.temperature - 0.75).abs() < f32::EPSILON);
        assert_eq!(restored.model.max_tokens, Some(512));
        assert_eq!(restored.model.top_p, Some(0.5));
        assert_eq!(restored.limits.max_iterations, 41);
        assert_eq!(restored.approvals.mode, ApprovalMode::AskCommands);
        assert_eq!(restored.approvals.allow_commands.len(), 2);
        assert_eq!(restored.subagents.max_depth, 2);
    }

    #[test]
    fn invalid_rows_are_ignored_not_fatal() {
        let mut cfg = Config::default();
        apply_rows(
            &mut cfg,
            &[
                ("model.temperature".into(), "hot".into()),
                ("limits.max_iterations".into(), "-3".into()),
                ("approvals.mode".into(), "whatever".into()),
                ("model.api_key".into(), "sk-x".into()),
            ],
        );
        assert_eq!(cfg.model.temperature, 0.2);
        assert_eq!(cfg.limits.max_iterations, Limits::default().max_iterations);
        assert_eq!(cfg.approvals.mode, ApprovalMode::Ask);
        assert_eq!(cfg.model.api_key.as_deref(), Some("sk-x"));
    }

    #[test]
    fn public_model_rounds_floats_and_hides_the_key() {
        let model = ModelConfig {
            temperature: 0.2,
            top_p: Some(0.95),
            api_key: Some("sk-secret".into()),
            ..ModelConfig::default()
        };
        let value = public_model(&model);
        assert_eq!(value["temperature"], serde_json::json!(0.2));
        assert_eq!(value["top_p"], serde_json::json!(0.95));
        assert_eq!(value["api_key_set"], serde_json::json!(true));
        assert!(!value.to_string().contains("sk-secret"));
    }

    #[test]
    fn public_config_hides_the_key() {
        let cfg = Config {
            lock_settings: false,
            model: ModelConfig {
                api_key: Some("sk-super-secret".into()),
                ..ModelConfig::default()
            },
            ..Config::default()
        };
        let text = serde_json::to_string(&cfg.model.public_view()).unwrap();
        assert!(!text.contains("sk-super-secret"));
        assert!(text.contains("\"api_key_set\":true"));
    }

    // -----------------------------------------------------------------------
    // [notifications]
    // -----------------------------------------------------------------------

    fn notifications_patch() -> ConfigPatch {
        ConfigPatch {
            notifications: Some(NotificationsPatch {
                enabled: Some(false),
                min_run_secs: Some(30),
                ..NotificationsPatch::default()
            }),
            ..ConfigPatch::default()
        }
    }

    #[test]
    fn a_notifications_patch_applies_and_leaves_the_rest_alone() {
        let mut config = Config::default();
        apply_patch(&mut config, &notifications_patch()).unwrap();
        assert!(!config.notifications.enabled);
        assert_eq!(config.notifications.min_run_secs, 30);
        // Fields the patch did not name keep their current value.
        assert!(config.notifications.plugin);
        assert!(config.notifications.run_finished);
        assert!(config.notifications.say_body);
    }

    #[test]
    fn a_notifications_patch_is_not_a_security_sensitive_change() {
        // Silencing a ping is not the human gate and does not repoint a provider,
        // so it must not need the admin token.
        assert!(notifications_patch().security_fields().is_empty());
        let config = Config {
            admin_token: Some("s3cret-admin".into()),
            ..Config::default()
        };
        assert!(authorize_patch(&config, &notifications_patch(), None).is_ok());
    }

    #[test]
    fn an_out_of_range_minimum_run_length_is_refused_by_name() {
        let mut config = Config::default();
        config.notifications.min_run_secs = kc_core::config::MAX_MIN_RUN_SECS + 1;
        let error = validate_settings(&config).unwrap_err().to_string();
        assert!(error.contains("notifications.min_run_secs"), "{error}");
        assert!(error.contains("at most"), "{error}");

        // The ceiling itself is accepted.
        config.notifications.min_run_secs = kc_core::config::MAX_MIN_RUN_SECS;
        assert!(validate_settings(&config).is_ok());
    }

    #[test]
    fn notification_settings_survive_a_write_and_read_round_trip() {
        // The rows are the only thing between a UI save and the next restart, so a
        // switch that is written but not read back is a silent data loss.
        let mut config = Config::default();
        config.notifications = kc_core::config::NotificationsConfig {
            enabled: false,
            run_finished: false,
            needs_input: true,
            error: true,
            info: false,
            plugin: false,
            min_run_secs: 45,
            say_body: false,
        };
        let rows: Vec<(String, String)> = rows_for(&config)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let mut restored = Config::default();
        apply_rows(&mut restored, &rows);
        assert_eq!(restored.notifications, config.notifications);
    }

    #[test]
    fn every_notification_row_is_persisted() {
        let config = Config::default();
        let rows = rows_for(&config);
        for key in [
            keys::NOTIFICATIONS_ENABLED,
            keys::NOTIFICATIONS_RUN_FINISHED,
            keys::NOTIFICATIONS_NEEDS_INPUT,
            keys::NOTIFICATIONS_ERROR,
            keys::NOTIFICATIONS_INFO,
            keys::NOTIFICATIONS_PLUGIN,
            keys::NOTIFICATIONS_MIN_RUN_SECS,
            keys::NOTIFICATIONS_SAY_BODY,
        ] {
            assert!(
                rows.contains_key(key),
                "{key} is missing from the persisted rows"
            );
        }
    }

    #[test]
    fn public_config_reports_every_notification_switch() {
        let json = public_notifications(&kc_core::config::NotificationsConfig::default());
        assert_eq!(json["enabled"], serde_json::json!(true));
        assert_eq!(json["run_finished"], serde_json::json!(true));
        assert_eq!(json["needs_input"], serde_json::json!(true));
        assert_eq!(json["error"], serde_json::json!(true));
        assert_eq!(json["info"], serde_json::json!(true));
        assert_eq!(json["plugin"], serde_json::json!(true));
        assert_eq!(json["min_run_secs"], serde_json::json!(0));
        assert_eq!(json["say_body"], serde_json::json!(true));
        // The ceiling and the kind list come from core, so the UI cannot drift.
        assert_eq!(
            json["max_min_run_secs"],
            serde_json::json!(kc_core::config::MAX_MIN_RUN_SECS)
        );
        let kinds = json["kinds"].as_array().unwrap();
        assert_eq!(kinds.len(), 4);
        assert!(kinds.iter().any(|k| k == "needs_input"));
    }

    #[test]
    fn an_unknown_notifications_field_is_rejected_by_the_api() {
        // `deny_unknown_fields`: a typo in the browser must fail loudly instead of
        // silently doing nothing.
        let error = serde_json::from_value::<ConfigPatch>(serde_json::json!({
            "notifications": { "enabeld": false }
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("enabeld"), "{error}");
    }

    #[test]
    fn a_stale_notification_row_is_ignored_rather_than_breaking_startup() {
        // A row written by a newer build, or corrupted, must not stop the server.
        let mut config = Config::default();
        apply_rows(
            &mut config,
            &[
                (keys::NOTIFICATIONS_ENABLED.to_string(), "not-a-bool".into()),
                (keys::NOTIFICATIONS_MIN_RUN_SECS.to_string(), "abc".into()),
            ],
        );
        assert!(
            config.notifications.enabled,
            "the default survives a bad row"
        );
        assert_eq!(config.notifications.min_run_secs, 0);
    }
}

#[cfg(test)]
mod provider_startup_tests {
    use super::*;

    fn config_with_provider(provider: &str) -> Config {
        Config {
            model: ModelConfig {
                provider: provider.into(),
                ..ModelConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn startup_refuses_an_unknown_provider_from_the_file_or_env() {
        let message = validate_settings(&config_with_provider("anthropic_native"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("anthropic_native"), "{message}");
        assert!(message.contains("openai_compatible"), "{message}");
    }

    #[test]
    fn startup_accepts_the_implemented_provider() {
        assert!(validate_settings(&config_with_provider("openai_compatible")).is_ok());
    }
}
