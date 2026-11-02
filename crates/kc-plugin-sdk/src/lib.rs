//! SDK for out-of-process karucode plugins.
//!
//! A plugin is a normal executable that speaks newline-delimited JSON-RPC 2.0 on
//! stdin/stdout (protocol version [`PLUGIN_PROTOCOL_VERSION`], defined in
//! [`kc_core::plugin`] and shared by the host and this SDK). It never links the
//! harness: the only coupling is the wire protocol, so anything that can spawn a
//! process and write JSON can be a plugin.
//!
//! # Minimal plugin
//!
//! ```no_run
//! use kc_plugin_sdk::{
//!     hooks, PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest,
//! };
//! use serde_json::json;
//!
//! fn main() -> std::io::Result<()> {
//!     PluginBuilder::new("greeter", "0.1.0")
//!         .description("Greets people")
//!         .tool(
//!             "hello",
//!             "Greet someone.",
//!             json!({"type": "object", "properties": {"name": {"type": "string"}}}),
//!             |req: ToolRequest| {
//!                 let name = req.arguments.get("name").and_then(|v| v.as_str()).unwrap_or("world");
//!                 PluginToolResult::ok(format!("Hello, {name}!"))
//!             },
//!         )
//!         .hook(hooks::BEFORE_PROMPT, |_payload| json!("The greeter plugin is active."))
//!         .run()
//! }
//! ```
//!
//! # Rules
//!
//! * **stdout is the protocol.** One JSON object per line, nothing else. Write
//!   logs to stderr; the host drains them into its own log at debug level.
//! * Answer every request with the same `id`, either `result` or `error`.
//! * `initialize` must return a manifest whose `name` matches the name the host
//!   was configured with and whose `protocol` is `"1"`.
//! * Only advertise a hook you handle; the host only calls advertised hooks.
//! * Plugins run with the server's OS permissions. `permissions` is declarative
//!   and used by the host when deciding which tools to expose; it is not a
//!   sandbox.
//!
//! [`PluginBuilder`] implements the loop for you; [`Plugin::handle_request`] is
//! available if you want to embed the protocol in your own runtime.

// @kc-doc: plugins
// @kc-title: Write and install a plugin
// @kc-summary: Build an out-of-process plugin that adds tools, hooks and inspector panels, register it, and debug it when it fails to start.
// @kc-tags: plugins, extension, sdk, tools, hooks, jsonrpc, panels, notifications
// @kc-applies-to: config:[[plugins]], file:~/.local/share/karucode/plugins/<name>/plugin.toml, dir:plugins/example-hello, tool:plugin__<plugin>__<tool>, cmd:cargo build -p example-hello, cmd:karucode plugin init, cmd:karucode plugin sdk, cmd:karucode plugin sdk --check, file:src/sdk.rs, file:src/protocol.rs, doc:docs/PLUGINS.md, env:KARUCODE_PLUGIN_NAME, env:KARUCODE_PLUGIN_PROTOCOL, env:KARUCODE_PLUGIN_PERMISSIONS, env:KARUCODE_PLUGIN_SETTINGS, env:KARUCODE_PLUGINS_WATCH, env:KARUCODE_PLUGINS_WATCH_SECS, config:panels, config:plugins_watch, config:plugins_watch_secs, config:notifications.plugin, route:GET /api/plugins, route:POST /api/plugins/reload, route:POST /api/plugins/:name/reload, route:POST /api/plugins/:name/enable, route:GET /api/plugins/:name/panels/:panel, config:plugin_panels, method:notify, hook:on_notify, fn:PluginBuilder::panel, fn:PluginBuilder::on_notify, fn:notify
//
// A plugin is a separate executable that speaks newline-delimited JSON-RPC 2.0 on
// stdin/stdout, protocol version `1`. It never links the harness, so it can be
// written in Rust with this SDK, in Python, in Go or in shell. The host spawns,
// supervises and restarts it; a crash or hang fails that plugin only.
//
// # 1. Write it with the Rust SDK
//
// ```bash
// karucode plugin init ./wordy     # the name comes from the directory
// karucode plugin init ./wordy --name wordy        # explicit name
// karucode plugin init ./wordy --force             # replace an existing scaffold
// cd wordy && cargo build --release
// ```
//
// The scaffolder writes the SDK **into** the plugin (`src/sdk.rs` and
// `src/protocol.rs` are copies of this build), so a Rust plugin depends on
// nothing from karucode: there is no path into a checkout and no published crate
// to wait for. Edit `src/main.rs`:
//
// ```rust
// use sdk::{PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest};
// use serde_json::json;
//
// fn main() -> std::io::Result<()> {
//     PluginBuilder::new("wordy", "0.1.0")
//         .description("Counts words")
//         .tool(
//             "count",
//             "Count the words in `text`.",
//             json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
//             |req: ToolRequest| {
//                 let text = req.str_arg("text");
//                 if text.is_empty() {
//                     return PluginToolResult::err("`text` is required");
//                 }
//                 PluginToolResult::ok(format!("{} words", text.split_whitespace().count()))
//             },
//         )
//         .hook(sdk::hooks::BEFORE_PROMPT, |_payload| json!("The wordy plugin is active."))
//         .run()
// }
// ```
//
// Add a tool with `.tool(…)`, a hook with `.hook(hooks::BEFORE_PROMPT, …)`, a
// panel with `.panel(…)`, and declare what it touches with
// `.permission(permissions::FILES_WRITE)`. After upgrading karucode, refresh the
// vendored copy with `karucode plugin sdk <dir>`; `--check` exits non-zero when it
// is stale, writes nothing and starts no server, so a plugin's CI can guard it.
//
// Inside this repository `plugins/example-hello` is a workspace member and uses
// `kc-plugin-sdk` by relative path (`cargo build -p example-hello`). Copy its
// sources if you like, never its `Cargo.toml`: that path only resolves from
// inside this checkout.
//
// # 2. Register it
//
// Either in the config file:
//
// ```toml
// [[plugins]]
// name = "example-hello"            # must match the plugin's manifest name
// command = ["/abs/path/to/target/release/example-hello"]
// enabled = true
// permissions = ["files.read"]
// env = { RUST_LOG = "debug" }
// settings = { greeting = "hi" }
// ```
//
// or as a manifest the host discovers on disk at
// `<data_dir>/plugins/<name>/plugin.toml` (same keys, `command` required). A
// duplicate name fails the slot with an error naming both sources instead of
// silently running one of them. Either source can be added while the server is
// running — see §4.
//
// # 3. Run and inspect
//
// ```bash
// curl -s localhost:8787/api/plugins | jq        # status, tools, error, restart_count
// curl -s -X POST localhost:8787/api/plugins/example-hello/enable \
//      -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{"enabled":true}'
// ```
//
// Status is `stopped`, `starting`, `ready`, `failed` or `disabled`. Enabling resets
// the restart budget.
//
// # 4. Install, upgrade and remove a plugin without a restart
//
// The host re-reads its sources on demand, so none of the three needs a server
// restart:
//
// ```bash
// # install: drop a manifest into the data dir, then apply it
// curl -s -X POST localhost:8787/api/plugins/reload -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{}' | jq .report
// # { "added": ["wordy"], "removed": [], "restarted": [], "unchanged": ["example-hello"] }
//
// # upgrade: rebuild the binary in place, then replace just that process
// curl -s -X POST localhost:8787/api/plugins/wordy/reload -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{}' | jq .status
//
// # remove: delete <data_dir>/plugins/wordy (or its [[plugins]] entry) and rescan
// curl -s -X POST localhost:8787/api/plugins/reload -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{}' | jq .report.removed
// ```
//
// `POST /api/plugins/reload` is a **rescan**: it re-reads the `[[plugins]]` entries
// from the config file plus every `<data_dir>/plugins/*/plugin.toml`, then adds new
// names, drops names that disappeared and replaces names whose config or files
// changed. Everything unchanged is left running untouched, so it is safe to call
// on a hunch. `POST /api/plugins/:name/reload` replaces one plugin's process and
// answers with its diagnostics.
//
// "Files changed" includes a **rebuilt binary**: the host fingerprints each
// command argument that resolves to a file, so a recompiled `target/release/wordy`
// with an identical command line still counts as an upgrade. A runtime
// enable/disable survives a rescan — the config file decides what a *new* plugin
// starts as, not whether an existing one that you switched off comes back.
//
// Both reload routes rebuild the tool registry before they answer, so the model
// sees the new tools on its very next call. A run that is already in flight keeps
// the registry snapshot it started with; a call to a replaced plugin fails with
// "was stopped" rather than hanging, and the next run is clean.
//
// To skip the curl entirely, let the server watch its own sources:
//
// ```toml
// # ~/.config/karucode/config.toml
// plugins_watch = true     # or KARUCODE_PLUGINS_WATCH=1
// plugins_watch_secs = 2   # or KARUCODE_PLUGINS_WATCH_SECS, clamped to 1..=600
// ```
//
// It polls (one `read_dir` plus a `stat` per command) instead of using inotify, so
// it also works over a network mount or a container bind mount, and a write that
// lands while the server is busy is still applied. Off by default: a reload
// replaces processes, so opting in is deliberate.
//
// # 5. Debug it without the server
//
// The protocol is just lines of JSON, so drive it by hand:
//
// ```bash
// printf '%s\n' \
//   '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"1"}}' \
//   '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
//   | ./target/release/example-hello
// ```
//
// # Panels (a tab in the inspector)
//
// A plugin can contribute a tab to the web UI's inspector. It supplies **data**,
// never markup: the payload is opaque JSON the host forwards untouched, and the
// frontend renders it with a component registered for that panel type, or as
// read-only JSON when none is. That is what keeps a plugin from executing code
// in the app's origin, so the CSP and `frame-ancestors 'none'` keep their
// meaning.
//
// ```rust
// use sdk::{PanelRequest, PluginBuilder};
// use serde_json::json;
//
// fn main() -> std::io::Result<()> {
//     PluginBuilder::new("wordy", "0.1.0")
//         .panel("words", "Words", |_req: PanelRequest| {
//             json!({ "words": 12, "files": ["src/main.rs"] })
//         })
//         .run()
// }
// ```
//
// In a hand-written plugin, declare the panel in the manifest and answer
// `panels/read`:
//
// ```json
// {"id":"words","title":"Words","icon":"list","source":"static","refresh_ms":5000}
// ```
//
// ```json
// {"jsonrpc":"2.0","id":9,"method":"panels/read",
//  "params":{"panel":"words","session_id":"s_1","project":"p_1","workspace":"/w"}}
// {"jsonrpc":"2.0","id":9,"result":{"title":"Words","data":{"words":12}}}
// ```
//
// * the panel id is hosted as `plugin__<plugin>__<id>`, so it cannot collide with
//   a built-in tab or another plugin's panel. Ids are lowercase `a-z`, `0-9`,
//   `-`, `_`, `.`; anything else, a duplicate id, an unknown `source` or a `tool`
//   the plugin does not advertise in `tools/list` fails the handshake and shows
//   up in the plugin diagnostics;
// * `data` is any JSON. `title` is optional — the manifest's title is used when
//   it is missing;
// * `icon` is a *name* from the app's icon set (`check`, `list`, `eye`, `folder`,
//   `git`, `terminal`, `bolt`, `spark`, …); an unknown name draws a neutral
//   glyph. The app decides what the name looks like, which is why a plugin cannot
//   ship an SVG;
// * `refresh_ms` is a hint. 0 (the default) means "read when the tab is opened
//   or refreshed"; anything else is clamped to 1000..=300000 so a plugin cannot
//   make the UI hammer the server;
// * the payload is capped at 256 KiB when rendered. Do real work in a tool the
//   agent calls, not in a panel handler that runs every few seconds.
//
// The tab is listed in `GET /api/config` under `plugin_panels` and read over
// `GET /api/plugins/:name/panels/:panel?project=&session=`; a plugin that
// declares no panels changes nothing, which is why this is additive and did not
// bump the protocol version.
//
// # The call context
//
// Since this revision, `tools/call` also carries
// `context: {session_id, run_id, agent_id, workspace, project, depth}`, so a
// workspace-scoped plugin knows which project a call belongs to instead of
// remembering the last `before_prompt`. The SDK exposes it as
// `ToolRequest::context()`, `::workspace()` and `::session_id()`. A plugin from
// before this field simply ignores it; on an older host `context()` returns
// `None`.
//
// # Permissions
//
// `permissions` is a declarative list (`files.read`, `files.write`, `shell`,
// `network`, `session.read`, or free-form). It is **fail-closed**: an entry that
// lists no permissions grants none, whatever the manifest asks for. To honour the
// manifest's own declaration, opt in explicitly with `permissions = ["*"]` or
// `settings.trust = true`. A `mutating` tool without `files.write`, or a `shell`
// tool without `shell`, is not exposed at all.
//
// A tool's declared `class` is a **hint only**. Every plugin tool is exposed as
// `external`, so it always goes through the approval policy — a plugin cannot
// exempt itself by declaring `read_only`. To let a specific tool skip the prompt,
// list it in the plugin's `settings.read_only_tools = ["tool", …]`.
//
// Those two keys, `trust` and `read_only_tools`, are **operator-only**: they are
// honoured from the `[[plugins]]` entry in `config.toml` and ignored when a
// `plugin.toml` in the plugin's own directory sets them. A plugin ships that file,
// so letting it decide its own approval exemptions would defeat the policy; the
// host logs the ignore and repeats it in the plugin's `error` field rather than
// obeying it silently. Every other `settings` key still reaches the plugin.
//
// # Tool names
//
// A tool is exposed as `plugin__<plugin>__<tool>`. The prefix prevents collisions
// with first-party tools, but **not** between two plugins: a plugin `a` with a
// tool `b__c` and a plugin `a__b` with a tool `c` both produce
// `plugin__a__b__c`. The first in discovery order keeps the name; any later
// claimant's tool is not exposed, and the reason is logged and reported in that
// plugin's `error` field. A collision never silently replaces another plugin.
//
// The plugin process gets a **cleared environment** plus an allowlist (`PATH`,
// `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TERM`, `TMPDIR`, `PWD`, `TZ`), the
// `KARUCODE_PLUGIN_*` variables and your `env` table (which wins). Credentials
// such as `KARUCODE_AUTH_TOKEN`, `OPENAI_API_KEY`, `GITHUB_TOKEN` and anything
// matching `*_API_KEY`/`*_TOKEN`/`*_SECRET`/`*_PASSWORD`/`AWS_*` are never
// inherited; pass a plugin its own credential through `env` if it needs one.
//
// It is **not** a sandbox. The plugin runs with the server's OS permissions and
// can do anything that user can do. Install plugins you trust, exactly like shell
// scripts.
//
// # Gotchas
//
// * stdout is the protocol. Anything else printed there is logged and ignored;
//   logs belong on stderr.
// * `initialize` must report the configured name and protocol `"1"`, or the
//   plugin fails with an error naming both values.
// * A dead process is restarted once per failed call, at most three times per
//   plugin per server lifetime; after that calls fail with an explanatory message
//   until you fix it (or reload the plugin, which resets the budget).
// * A replaced binary or an edited manifest needs a reload, not a restart:
//   `POST /api/plugins/:name/reload`, or `plugins_watch = true` to have the server
//   notice on its own.
// * Results are truncated and rendered as text. A tool result still cannot
//   inject UI; a panel contributes **data** (rendered by a first-party
//   component, or as read-only JSON) and never markup or script.
// * A panel id that is not lowercase `a-z0-9-_.`, a duplicate id, an unknown
//   `source`, or a `source = "tool"` panel naming a tool the plugin does not
//   advertise all fail the handshake; the reason is in `PluginDiagnostics.error`
//   and `GET /api/plugins`.
// * Hook output is capped at 4 KiB per contribution and 16 KiB in total before it
//   reaches the system prompt; anything longer is cut with `[… truncated]`.
// * A plugin that breaks the framing contract (a line over 8 MiB, a read error) is
//   killed and marked `failed`; it is not left running.
// * `before_prompt` output is placed in the system prompt and re-requested on
//   **every model call**. Return the same text each time: a timestamp, counter
//   or random value there changes message 0 and makes the provider re-bill the
//   whole context instead of reading it from the prompt cache. Notes from several
//   plugins are joined in plugin order, not in the order the processes answer.
//
// Full reference: `docs/PLUGINS.md` (wire protocol, hooks, timeouts, error codes).

// Everything below the marker is copied verbatim into a scaffolded plugin by
// `karucode plugin init` (see `src/embed.rs`). Keep it free of harness-only
// dependencies, and refer to the wire types as `kc_core::plugin` — the emitter
// rewrites that path to the plugin's own `crate::protocol` module.
mod embed;
pub use embed::{
    files, name_from_path, protocol_source, sdk_source, validate_name, EmbedError,
    SDK_SOURCE_MARKER,
};

// ---- BEGIN EMBEDDED SDK SOURCE ----
pub use kc_core::plugin::*;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

/// One `tools/call` invocation handed to a tool handler.
pub struct ToolRequest {
    pub name: String,
    pub arguments: Value,
    /// Session/workspace the call belongs to. `None` on a host older than the
    /// `context` parameter, and on a call the host made without one.
    pub context: Option<ToolCallContext>,
}

impl ToolRequest {
    /// String argument (empty string when absent).
    pub fn str_arg(&self, key: &str) -> &str {
        self.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }

    /// Integer argument (0 when absent or not an integer).
    pub fn int_arg(&self, key: &str) -> i64 {
        self.arguments
            .get(key)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }

    /// Session/workspace the call belongs to. `None` on an older host.
    ///
    /// Prefer this over remembering the workspace from the last `before_prompt`:
    /// that guess cross-talks between concurrent sessions.
    pub fn context(&self) -> Option<&ToolCallContext> {
        self.context.as_ref()
    }

    /// The workspace root the call belongs to, when the host sent one.
    pub fn workspace(&self) -> Option<&str> {
        self.context()
            .map(|c| c.workspace.as_str())
            .filter(|w| !w.is_empty())
    }

    /// The session the call belongs to, when the host sent one.
    pub fn session_id(&self) -> Option<&str> {
        self.context()
            .map(|c| c.session_id.as_str())
            .filter(|s| !s.is_empty())
    }
}

/// One `panels/read` invocation handed to a panel handler.
///
/// `data` is opaque to the host: whatever the handler returns is forwarded to
/// the browser untouched and rendered with a component registered for that
/// panel type, so a panel cannot run code in the app's origin.
pub struct PanelRequest {
    /// The bare panel id from the manifest.
    pub panel: String,
    pub session_id: String,
    pub project: Option<String>,
    /// Absolute path of the project root the panel was requested for.
    pub workspace: String,
}

/// Constructors for [`PluginToolResult`].
///
/// `PluginToolResult` is a wire type shared with the host, so the ergonomic
/// constructors arrive as an extension trait instead of inherent methods. Bring
/// it into scope (`use kc_plugin_sdk::PluginToolResultExt;`) and
/// `PluginToolResult::ok("done")` works.
pub trait PluginToolResultExt: Sized {
    /// Successful result.
    fn ok(content: impl Into<String>) -> Self;
    /// Failed result: the harness shows `content` as the failure reason. It is
    /// not a transport error, so the plugin stays connected.
    fn err(content: impl Into<String>) -> Self;
    /// Replace the content of an existing result.
    fn content(self, content: impl Into<String>) -> Self;
    /// Attach structured metadata (echoed back in `ToolOutcome::meta`).
    fn with_meta(self, meta: Value) -> Self;
}

impl PluginToolResultExt for PluginToolResult {
    fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            meta: None,
        }
    }

    fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            meta: None,
        }
    }

    fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    fn with_meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }
}

type ToolHandler = Box<dyn Fn(ToolRequest) -> PluginToolResult + Send + Sync>;
type HookHandler = Box<dyn Fn(Value) -> Value + Send + Sync>;
type PanelHandler = Box<dyn Fn(PanelRequest) -> Value + Send + Sync>;

/// Why a human is being interrupted, from a plugin's point of view.
///
/// Mirrors the host's `NotifyKind` over the wire. It is redefined here rather
/// than re-exported because the SDK is copied *verbatim* into a scaffolded plugin,
/// which must not depend on the harness crates. The wire spellings are pinned by a
/// test, so the two definitions cannot drift silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// A run settled successfully.
    RunFinished,
    /// Something waits on a human (an approval, a run that may continue).
    NeedsInput,
    /// Something failed.
    Error,
    /// Neutral. The only kind a plugin can use without claiming urgency.
    Info,
}

impl NotifyKind {
    /// The wire spelling, matching the host's `NotifyKind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            NotifyKind::RunFinished => "run_finished",
            NotifyKind::NeedsInput => "needs_input",
            NotifyKind::Error => "error",
            NotifyKind::Info => "info",
        }
    }
}

/// Ask a human to look at something.
///
/// A plugin cannot reach a browser — the host serves no plugin routes and runs no
/// plugin code in the app's origin — so this frame is the only way to say "look at
/// this" from inside a plugin. It is a JSON-RPC *notification* (no `id`, no reply):
/// fire it from a tool handler, a hook or a background thread and do not wait.
///
/// ```no_run
/// use kc_plugin_sdk::{notify, NotifyKind};
///
/// // Anything that can write a line to stdout works, including from a thread.
/// # fn main() -> std::io::Result<()> {
/// notify(NotifyKind::NeedsInput, "Deploy needs a yes", "staging is approved")?;
/// # Ok(())
/// # }
/// ```
///
/// The host validates the frame before it reaches a person: an unknown `kind`, an
/// empty title or junk params are dropped, the title and body are truncated, and a
/// plugin raising more than one notification per second (burst five) has the extra
/// ones dropped. Which is to say: nothing here can be used to flood a desktop.
///
/// Writing goes to stdout, which is the protocol channel. A plugin that logs with
/// `println!` corrupts the protocol; log with `eprintln!`.
pub fn notify(
    kind: NotifyKind,
    title: impl AsRef<str>,
    body: impl AsRef<str>,
) -> std::io::Result<()> {
    notify_with(NotifyFrame {
        kind,
        title: title.as_ref().to_string(),
        body: body.as_ref().to_string(),
        session_id: None,
        sound: None,
        sticky: false,
    })
}

/// One `notify` frame, for the fields [`notify`] does not take.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyFrame {
    pub kind: NotifyKind,
    pub title: String,
    pub body: String,
    /// The session it is about. `None` lets the host use the session it invoked
    /// the plugin for, when there is one.
    pub session_id: Option<String>,
    /// A cue name. `None` means the kind's default; an unknown name falls back to
    /// the default rather than going silent.
    pub sound: Option<String>,
    /// Keep the OS notification on screen until dismissed.
    pub sticky: bool,
}

impl NotifyFrame {
    /// The exact JSON written to the protocol channel.
    ///
    /// Exposed so a plugin that speaks the protocol by hand (not through this SDK)
    /// can build the same frame, and so tests can assert on it without stdout.
    pub fn to_value(&self) -> Value {
        let mut params = json!({
            "kind": self.kind.as_str(),
            "title": self.title,
            "body": self.body,
        });
        let object = params.as_object_mut().expect("json object");
        if let Some(session_id) = &self.session_id {
            object.insert("session_id".into(), json!(session_id));
        }
        if let Some(sound) = &self.sound {
            object.insert("sound".into(), json!(sound));
        }
        if self.sticky {
            object.insert("sticky".into(), json!(true));
        }
        json!({"jsonrpc": "2.0", "method": methods::NOTIFY, "params": params})
    }
}

/// Write one fully-specified `notify` frame.
///
/// Flushes immediately: a notification that sits in a buffer because the plugin is
/// still working is a notification that arrives too late to be useful.
pub fn notify_with(frame: NotifyFrame) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", frame.to_value())?;
    stdout.flush()
}

struct ToolEntry {
    def: PluginToolDef,
    handler: ToolHandler,
}

/// A plugin assembled by [`PluginBuilder`], ready to serve requests.
pub struct Plugin {
    manifest: PluginManifest,
    tools: BTreeMap<String, ToolEntry>,
    hooks: BTreeMap<String, HookHandler>,
    panels: BTreeMap<String, PanelHandler>,
    shutdown: bool,
}

impl Plugin {
    /// Manifest reported to the host by `initialize`.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// True once a `shutdown` request was handled.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Handle one decoded JSON-RPC message.
    ///
    /// Returns the response object for requests, and `None` for notifications
    /// (no `id`). This is the whole protocol surface; [`Plugin::run`] is just
    /// stdin/stdout plumbing around it.
    pub fn handle_request(&mut self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (e.g. `notifications/initialized`) get no response.
        let id = id?;

        let response = match method.as_str() {
            methods::INITIALIZE => Ok(json!(self.manifest)),
            methods::TOOLS_LIST => {
                let list: Vec<PluginToolDef> = self.tools.values().map(|t| t.def.clone()).collect();
                Ok(json!({ "tools": list }))
            }
            methods::TOOLS_CALL => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                // Additive: an older host sends no `context`, and a malformed
                // one counts as absent rather than failing the call.
                let context = params
                    .get("context")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok());
                match self.tools.get(&name) {
                    Some(entry) => {
                        let result = (entry.handler)(ToolRequest {
                            name: name.clone(),
                            arguments,
                            context,
                        });
                        Ok(serde_json::to_value(result).unwrap_or_else(
                            |_| json!({"ok": false, "content": "serialisation error"}),
                        ))
                    }
                    None => Err(PluginError::new(-32601, format!("unknown tool `{name}`"))),
                }
            }
            methods::PANELS_READ => {
                let panel = params
                    .get("panel")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let request = PanelRequest {
                    panel: panel.clone(),
                    session_id: params
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    project: params
                        .get("project")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    workspace: params
                        .get("workspace")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                };
                match self.panels.get(&panel) {
                    Some(handler) => {
                        let data = handler(request);
                        Ok(json!({
                            "title": self
                                .manifest
                                .panels
                                .iter()
                                .find(|p| p.id == panel)
                                .map(|p| p.title.clone()),
                            "data": data,
                        }))
                    }
                    // An unknown panel is caller error: `-32602` is the
                    // documented "invalid params" answer.
                    None => Err(PluginError::new(
                        -32602,
                        format!("this plugin declares no panel `{panel}`"),
                    )),
                }
            }
            methods::HOOKS_RUN => {
                let hook = params
                    .get("hook")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                match self.hooks.get(&hook) {
                    Some(handler) => Ok(json!({ "output": handler(payload) })),
                    None => Ok(json!({ "output": null })),
                }
            }
            methods::SHUTDOWN => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            other => Err(PluginError::new(
                -32601,
                format!("unknown method `{other}`"),
            )),
        };

        Some(match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        })
    }

    /// Serve requests on stdin/stdout until EOF or `shutdown`.
    pub fn run(&mut self) -> std::io::Result<()> {
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
                Ok(value) => value,
                Err(e) => {
                    // A malformed line is not recoverable per-request (we cannot
                    // know its id), so report and keep going.
                    let error = json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": {"code": -32700, "message": format!("parse error: {e}")},
                    });
                    writeln!(stdout, "{error}")?;
                    stdout.flush()?;
                    continue;
                }
            };
            let response = self.handle_request(request);
            if let Some(response) = response {
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
            }
            if self.shutdown {
                return Ok(());
            }
        }
    }
}

/// Builds a [`Plugin`]: declares the manifest, the tools and the hooks.
pub struct PluginBuilder {
    manifest: PluginManifest,
    tools: BTreeMap<String, ToolEntry>,
    hooks: BTreeMap<String, HookHandler>,
    panels: BTreeMap<String, PanelHandler>,
}

impl PluginBuilder {
    /// Start a plugin that reports `name`/`version` in its manifest. The name
    /// must match the `[[plugins]].name` (or `plugin.toml` name) the host is
    /// configured with.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            manifest: PluginManifest {
                name: name.into(),
                version: version.into(),
                description: String::new(),
                protocol: PLUGIN_PROTOCOL_VERSION.into(),
                capabilities: vec!["tools".into()],
                permissions: Vec::new(),
                hooks: Vec::new(),
                tool_prefix: None,
                panels: Vec::new(),
            },
            tools: BTreeMap::new(),
            hooks: BTreeMap::new(),
            panels: BTreeMap::new(),
        }
    }

    /// Human-readable description shown in diagnostics.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.manifest.description = description.into();
        self
    }

    /// Declare a permission (`files.read`, `files.write`, `shell`, `network`,
    /// `session.read`). Declarative: the host uses it to decide which tools to
    /// expose. It is **not** an OS sandbox.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.manifest.permissions.push(permission.into());
        self
    }

    /// Declare a capability (free-form; `tools` is declared by default).
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        let capability = capability.into();
        if !self.manifest.capabilities.contains(&capability) {
            self.manifest.capabilities.push(capability);
        }
        self
    }

    /// Register a tool with the default class (`external`, which always needs
    /// approval).
    pub fn tool(
        self,
        name: &str,
        description: &str,
        parameters: Value,
        handler: impl Fn(ToolRequest) -> PluginToolResult + Send + Sync + 'static,
    ) -> Self {
        self.tool_with_class(name, description, parameters, None, handler)
    }

    /// Register a tool with an explicit side-effect class:
    /// `read_only`, `mutating`, `shell` or `external`.
    ///
    /// `mutating` needs the `files.write` permission and `shell` needs the
    /// `shell` permission before the host exposes the tool.
    pub fn tool_with_class(
        mut self,
        name: &str,
        description: &str,
        parameters: Value,
        class: Option<&str>,
        handler: impl Fn(ToolRequest) -> PluginToolResult + Send + Sync + 'static,
    ) -> Self {
        self.tools.insert(
            name.to_string(),
            ToolEntry {
                def: PluginToolDef {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters,
                    class: class.map(|c| c.to_string()),
                },
                handler: Box::new(handler),
            },
        );
        self
    }

    /// Register a lifecycle hook. Only hooks listed in
    /// [`kc_core::plugin::hooks::IMPLEMENTED`] are called by the host; the
    /// others are reserved. The handler's return value becomes the hook's
    /// `output`.
    pub fn hook(
        mut self,
        name: &str,
        handler: impl Fn(Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        if !self.manifest.hooks.iter().any(|h| h == name) {
            self.manifest.hooks.push(name.to_string());
        }
        self.hooks.insert(name.to_string(), Box::new(handler));
        self
    }

    /// Observe every notification the runtime emits, so this plugin can bridge
    /// karucode to an external notifier (ntfy, Pushover, a desktop daemon, a web
    /// hook) without karucode growing that integration.
    ///
    /// The payload is `{session_id, run_id, kind, title, body, source}`. The call
    /// is fire-and-forget: the runtime does not wait for the answer, and a failing
    /// handler never fails a run.
    ///
    /// ```no_run
    /// # use kc_plugin_sdk::PluginBuilder;
    /// # use serde_json::{json, Value};
    /// # let builder = PluginBuilder::new("bridge", "0.1.0");
    /// builder.on_notify(|payload: Value| {
    ///     eprintln!("karucode says: {} {}", payload["kind"], payload["title"]);
    ///     json!({ "forwarded": true })
    /// });
    /// ```
    pub fn on_notify(self, handler: impl Fn(Value) -> Value + Send + Sync + 'static) -> Self {
        // The literal, not `kc_core::plugin::hooks::ON_NOTIFY`: this file is copied
        // verbatim into a scaffolded plugin, which must not depend on the harness
        // crates. A test pins the spelling against the host's constant.
        self.capability("notifications").hook("on_notify", handler)
    }

    /// Declare that this plugin raises notifications with [`notify`].
    ///
    /// Purely informational: the host accepts a `notify` frame whether or not the
    /// capability is declared. Declaring it lets the settings screen explain why a
    /// plugin has a notification switch.
    pub fn notifications(mut self) -> Self {
        self = self.capability("notifications");
        self
    }

    /// Contribute a panel to the web UI's inspector.
    ///
    /// `id` must be lowercase `a-z`, `0-9`, `-`, `_` or `.`. The handler's return
    /// value becomes the panel's `data`: any JSON, forwarded to the browser
    /// untouched and rendered with a component registered for that panel type
    /// (or as read-only JSON when the UI has none). A panel cannot run code in
    /// the app's origin.
    ///
    /// ```no_run
    /// # use kc_plugin_sdk::{PluginBuilder, PanelRequest};
    /// # use serde_json::json;
    /// # let builder = PluginBuilder::new("wordy", "0.1.0");
    /// builder.panel("words", "Words", |req: PanelRequest| {
    ///     json!({ "workspace": req.workspace, "words": 12 })
    /// });
    /// ```
    ///
    /// Trade-offs worth knowing:
    ///
    /// * the host calls this whenever the tab is opened or refreshed, so return
    ///   the payload cheaply — do the work in a tool the agent calls instead;
    /// * `refresh_ms` below 1000 is raised to 1000 and above 300 000 lowered, so
    ///   a plugin cannot make the UI hammer the server;
    /// * a panel that panics fails that read only; the host reports the error
    ///   next to the tab and the other panels keep working.
    pub fn panel(
        self,
        id: &str,
        title: &str,
        handler: impl Fn(PanelRequest) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.panel_with(id, title, None, 0, handler)
    }

    /// [`panel`](Self::panel) with an icon name and a suggested polling interval.
    pub fn panel_with(
        mut self,
        id: &str,
        title: &str,
        icon: Option<&str>,
        refresh_ms: u64,
        handler: impl Fn(PanelRequest) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.manifest.panels.retain(|p| p.id != id);
        self.manifest.panels.push(PluginPanel {
            id: id.to_string(),
            title: title.to_string(),
            icon: icon.map(str::to_string),
            source: PANEL_SOURCE_STATIC.to_string(),
            tool: None,
            refresh_ms,
        });
        if !self.manifest.capabilities.iter().any(|c| c == "panels") {
            self.manifest.capabilities.push("panels".into());
        }
        self.panels.insert(id.to_string(), Box::new(handler));
        self
    }

    /// Finish building without serving.
    pub fn build(self) -> Plugin {
        Plugin {
            manifest: self.manifest,
            tools: self.tools,
            hooks: self.hooks,
            panels: self.panels,
            shutdown: false,
        }
    }

    /// Serve requests on stdin/stdout until EOF or `shutdown`.
    pub fn run(self) -> std::io::Result<()> {
        self.build().run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin() -> Plugin {
        PluginBuilder::new("demo", "1.2.3")
            .description("test plugin")
            .permission(permissions::FILES_READ)
            .tool(
                "echo",
                "Echo.",
                json!({"type": "object", "properties": {"text": {"type": "string"}}}),
                |req: ToolRequest| PluginToolResult::ok(format!("echo: {}", req.str_arg("text"))),
            )
            .tool_with_class("deploy", "Deploy.", json!({}), Some("shell"), |_req| {
                PluginToolResult::err("not today")
            })
            .hook(hooks::BEFORE_PROMPT, |_payload| json!("demo note"))
            .build()
    }

    #[test]
    fn manifest_reflects_the_builder() {
        let plugin = plugin();
        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.description, "test plugin");
        assert_eq!(manifest.protocol, PLUGIN_PROTOCOL_VERSION);
        assert_eq!(manifest.permissions, vec!["files.read".to_string()]);
        assert_eq!(manifest.hooks, vec!["before_prompt".to_string()]);
        assert!(manifest.capabilities.contains(&"tools".to_string()));
    }

    #[test]
    fn initialize_returns_the_manifest() {
        let mut plugin = plugin();
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                                   "params": {"protocol": "1"}}))
            .unwrap();
        assert_eq!(response["id"], json!(1));
        assert_eq!(response["result"]["name"], json!("demo"));
    }

    #[test]
    fn tools_list_is_sorted_and_stable() {
        let mut plugin = plugin();
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["deploy", "echo"]);
        // The declared class travels with the definition.
        assert_eq!(response["result"]["tools"][0]["class"], json!("shell"));
        assert!(response["result"]["tools"][1]["class"].is_null());
    }

    #[test]
    fn tools_call_success_and_failure() {
        let mut plugin = plugin();
        let ok = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                   "params": {"name": "echo", "arguments": {"text": "hi"}}}))
            .unwrap();
        assert_eq!(ok["result"]["ok"], json!(true));
        assert_eq!(ok["result"]["content"], json!("echo: hi"));

        let failed = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                                   "params": {"name": "deploy", "arguments": {}}}))
            .unwrap();
        assert_eq!(failed["result"]["ok"], json!(false));
        assert_eq!(failed["result"]["content"], json!("not today"));
    }

    #[test]
    fn unknown_tool_and_method_are_jsonrpc_errors() {
        let mut plugin = plugin();
        let unknown_tool = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                                   "params": {"name": "nope", "arguments": {}}}))
            .unwrap();
        assert_eq!(unknown_tool["error"]["code"], json!(-32601));
        assert!(unknown_tool["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool `nope`"));

        let unknown_method = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 6, "method": "nope"}))
            .unwrap();
        assert_eq!(unknown_method["error"]["code"], json!(-32601));
    }

    #[test]
    fn hooks_return_output_and_unknown_hooks_return_null() {
        let mut plugin = plugin();
        let known = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 7, "method": "hooks/run",
                                   "params": {"hook": "before_prompt", "payload": {}}}))
            .unwrap();
        assert_eq!(known["result"]["output"], json!("demo note"));

        let unknown = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 8, "method": "hooks/run",
                                   "params": {"hook": "on_run_end", "payload": {}}}))
            .unwrap();
        assert!(unknown["result"]["output"].is_null());
    }

    #[test]
    fn notifications_get_no_response_and_shutdown_stops_the_loop() {
        let mut plugin = plugin();
        assert!(plugin
            .handle_request(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none());
        assert!(!plugin.is_shutdown());
        let response = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 9, "method": "shutdown"}))
            .unwrap();
        assert!(response["result"].is_null());
        assert!(plugin.is_shutdown());
    }

    #[test]
    fn result_constructors_are_complete() {
        let ok = PluginToolResult::ok("fine").with_meta(json!({"n": 1}));
        assert!(ok.ok);
        assert_eq!(ok.content, "fine");
        assert_eq!(ok.meta, Some(json!({"n": 1})));
        let err = PluginToolResult::err("broken");
        assert!(!err.ok);
        assert!(err.meta.is_none());
    }

    #[test]
    fn tool_request_argument_helpers() {
        let req = ToolRequest {
            name: "t".into(),
            arguments: json!({"text": "hi", "n": 4}),
            context: None,
        };
        assert_eq!(req.str_arg("text"), "hi");
        assert_eq!(req.str_arg("missing"), "");
        assert_eq!(req.int_arg("n"), 4);
        assert_eq!(req.int_arg("missing"), 0);
    }

    fn plugin_with_panel() -> Plugin {
        PluginBuilder::new("todo", "1.0.0")
            .panel_with("todo", "Todo", Some("check"), 5_000, |req: PanelRequest| {
                json!({
                    "session": req.session_id,
                    "project": req.project,
                    "workspace": req.workspace,
                    "items": [],
                })
            })
            .tool("echo", "Echo.", json!({}), |req: ToolRequest| {
                PluginToolResult::ok(match req.workspace() {
                    Some(workspace) => format!("workspace: {workspace}"),
                    None => "no context".to_string(),
                })
            })
            .build()
    }

    #[test]
    fn the_builder_declares_the_panel_and_the_capability() {
        let plugin = plugin_with_panel();
        let manifest = plugin.manifest();
        assert_eq!(manifest.panels.len(), 1);
        assert_eq!(manifest.panels[0].id, "todo");
        assert_eq!(manifest.panels[0].title, "Todo");
        assert_eq!(manifest.panels[0].icon.as_deref(), Some("check"));
        assert_eq!(manifest.panels[0].refresh_ms, 5_000);
        assert!(manifest.capabilities.contains(&"panels".to_string()));
    }

    #[test]
    fn panels_read_returns_the_handlers_data() {
        let mut plugin = plugin_with_panel();
        let response = plugin
            .handle_request(json!({
                "jsonrpc": "2.0", "id": 20, "method": "panels/read",
                "params": {"panel": "todo", "session_id": "s_1",
                           "project": "p_1", "workspace": "/w"},
            }))
            .unwrap();
        assert_eq!(response["result"]["title"], json!("Todo"));
        assert_eq!(response["result"]["data"]["session"], json!("s_1"));
        assert_eq!(response["result"]["data"]["project"], json!("p_1"));
        assert_eq!(response["result"]["data"]["workspace"], json!("/w"));
        assert_eq!(response["result"]["data"]["items"], json!([]));

        // An undeclared panel is `invalid params`, not a crash.
        let unknown = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 21, "method": "panels/read",
                                   "params": {"panel": "nope"}}))
            .unwrap();
        assert_eq!(unknown["error"]["code"], json!(-32602));
        assert!(unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no panel `nope`"));
    }

    #[test]
    fn tools_call_reads_the_forwarded_context() {
        let mut plugin = plugin_with_panel();
        let with = plugin
            .handle_request(json!({
                "jsonrpc": "2.0", "id": 22, "method": "tools/call",
                "params": {"name": "echo", "arguments": {},
                           "context": {"session_id": "s_1", "workspace": "/w", "depth": 0}},
            }))
            .unwrap();
        assert_eq!(with["result"]["content"], json!("workspace: /w"));

        // An older host sends no `context`: the handler still runs.
        let without = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 23, "method": "tools/call",
                                   "params": {"name": "echo", "arguments": {}}}))
            .unwrap();
        assert_eq!(without["result"]["content"], json!("no context"));

        // A malformed context counts as absent instead of failing the call.
        let malformed = plugin
            .handle_request(json!({"jsonrpc": "2.0", "id": 24, "method": "tools/call",
                                   "params": {"name": "echo", "arguments": {},
                                              "context": "not an object"}}))
            .unwrap();
        assert_eq!(malformed["result"]["content"], json!("no context"));
    }

    #[test]
    fn a_plugin_without_panels_declares_none() {
        let plugin = PluginBuilder::new("plain", "0.1.0").build();
        assert!(plugin.manifest().panels.is_empty());
        assert!(!plugin
            .manifest()
            .capabilities
            .contains(&"panels".to_string()));
    }

    #[test]
    fn a_notify_frame_has_the_documented_wire_shape() {
        let frame = NotifyFrame {
            kind: NotifyKind::NeedsInput,
            title: "Deploy needs a yes".into(),
            body: "staging is approved".into(),
            session_id: Some("s_1".into()),
            sound: Some("ping".into()),
            sticky: true,
        };
        let value = frame.to_value();
        assert_eq!(value["jsonrpc"], json!("2.0"));
        assert_eq!(value["method"], json!("notify"));
        // A notification: no id, so the host must not try to answer it.
        assert!(value.get("id").is_none());
        assert_eq!(value["params"]["kind"], json!("needs_input"));
        assert_eq!(value["params"]["title"], json!("Deploy needs a yes"));
        assert_eq!(value["params"]["body"], json!("staging is approved"));
        assert_eq!(value["params"]["session_id"], json!("s_1"));
        assert_eq!(value["params"]["sound"], json!("ping"));
        assert_eq!(value["params"]["sticky"], json!(true));
    }

    #[test]
    fn optional_notify_fields_are_omitted_rather_than_null() {
        // The host treats an absent field and a null one the same way, but omitting
        // keeps the frame small and the wire log readable.
        let value = NotifyFrame {
            kind: NotifyKind::Info,
            title: "t".into(),
            body: String::new(),
            session_id: None,
            sound: None,
            sticky: false,
        }
        .to_value();
        assert!(value["params"].get("session_id").is_none());
        assert!(value["params"].get("sound").is_none());
        assert!(value["params"].get("sticky").is_none());
        assert_eq!(value["params"]["kind"], json!("info"));
    }

    #[test]
    fn the_sdk_kind_spellings_match_the_host() {
        // The SDK is copied verbatim into a scaffolded plugin and cannot depend on
        // kc-core, so the two definitions are pinned here instead.
        assert_eq!(NotifyKind::RunFinished.as_str(), "run_finished");
        assert_eq!(NotifyKind::NeedsInput.as_str(), "needs_input");
        assert_eq!(NotifyKind::Error.as_str(), "error");
        assert_eq!(NotifyKind::Info.as_str(), "info");
        assert_eq!(
            kc_core::plugin::methods::NOTIFY,
            "notify",
            "the method name must match the host's"
        );
    }

    #[test]
    fn on_notify_advertises_the_hook_and_the_capability() {
        let plugin = PluginBuilder::new("bridge", "0.1.0")
            .on_notify(|_payload| json!({"forwarded": true}))
            .build();
        let manifest = plugin.manifest();
        assert!(manifest
            .hooks
            .contains(&kc_core::plugin::hooks::ON_NOTIFY.to_string()));
        assert!(manifest.capabilities.contains(&"notifications".to_string()));
        // Registering the same hook twice must not duplicate it in the manifest.
        let twice = PluginBuilder::new("bridge", "0.1.0")
            .on_notify(|_| json!({}))
            .on_notify(|_| json!({}))
            .build();
        assert_eq!(
            twice
                .manifest()
                .hooks
                .iter()
                .filter(|h| *h == kc_core::plugin::hooks::ON_NOTIFY)
                .count(),
            1
        );
    }

    #[test]
    fn an_on_notify_handler_answers_the_hook_request() {
        let mut plugin = PluginBuilder::new("bridge", "0.1.0")
            .on_notify(|payload| json!({"title": payload["title"]}))
            .build();
        let response = plugin
            .handle_request(json!({
                "jsonrpc": "2.0", "id": 5, "method": "hooks/run",
                "params": {"hook": "on_notify",
                           "payload": {"kind": "run_finished", "title": "Run finished"}}
            }))
            .expect("a response");
        assert_eq!(response["result"]["output"]["title"], json!("Run finished"));
    }

    #[test]
    fn declaring_notifications_is_additive_and_idempotent() {
        let manifest = PluginBuilder::new("p", "0.1.0")
            .notifications()
            .notifications()
            .build()
            .manifest()
            .clone();
        assert_eq!(
            manifest
                .capabilities
                .iter()
                .filter(|c| *c == "notifications")
                .count(),
            1
        );
    }
}
