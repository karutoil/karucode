//! Plugin and MCP protocol types (versioned, JSON-shaped).
//!
//! Plugins are separate processes speaking newline-delimited JSON-RPC 2.0 over
//! stdin/stdout. The core supervises them, so a panicking or hanging plugin
//! cannot take the harness down. This is *process* isolation, not a sandbox: a
//! plugin runs with the server's OS permissions.

use serde::{Deserialize, Serialize};

pub const PLUGIN_PROTOCOL_VERSION: &str = "1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "protocol_version")]
    pub protocol: String,
    /// Capabilities the plugin provides.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Permissions the plugin declares; the core records them and refuses tools
    /// whose origin was not declared. Not an OS sandbox.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Lifecycle hooks the plugin wants to receive.
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub tool_prefix: Option<String>,
    /// Panels this plugin contributes to the web UI's inspector.
    ///
    /// Every field is defaulted, so a manifest that declares none keeps working
    /// unchanged: the panel capability is additive, not a protocol bump.
    #[serde(default)]
    pub panels: Vec<PluginPanel>,
    /// Declarative UI contributions: buttons in named slots, command-palette
    /// entries, and built-in elements the plugin asks the app to hide.
    ///
    /// Like `panels`, this is additive: an older host ignores an unknown field,
    /// and a plugin that declares none changes nothing.
    #[serde(default)]
    pub ui: Vec<PluginUiContribution>,
}

fn protocol_version() -> String {
    PLUGIN_PROTOCOL_VERSION.into()
}

/// One panel a plugin contributes to the inspector.
///
/// The host never interprets [`PluginPanelData::data`]: it is opaque JSON the
/// frontend renders with a component registered for that panel type. That is
/// what keeps a plugin from executing code in the app's origin — the property
/// the CSP and `frame-ancestors 'none'` exist to protect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginPanel {
    /// Stable id, unique within the plugin. Hosted as `plugin__<plugin>__<id>`.
    #[serde(default)]
    pub id: String,
    /// Tab label shown in the inspector.
    #[serde(default)]
    pub title: String,
    /// Optional leading glyph name from the app's icon set.
    #[serde(default)]
    pub icon: Option<String>,
    /// How the payload is produced: [`PANEL_SOURCE_TOOL`] (the plugin may run
    /// `tool` to build it) or [`PANEL_SOURCE_STATIC`] (a document it keeps). Both
    /// are answered by `panels/read`; the value is documentation for the host and
    /// the settings screen.
    #[serde(default = "panel_source_tool")]
    pub source: String,
    /// For `source = "tool"`, the tool a plugin may run to build the payload.
    #[serde(default)]
    pub tool: Option<String>,
    /// Suggested polling interval in ms; 0 means "only on explicit refresh".
    /// The host clamps it (see [`clamp_refresh_ms`]).
    #[serde(default)]
    pub refresh_ms: u64,
}

fn panel_source_tool() -> String {
    PANEL_SOURCE_TOOL.to_string()
}

impl Default for PluginPanel {
    /// A declaration with the documented defaults: `source = "tool"`, no icon,
    /// no tool, no polling. Hand-written rather than derived so `..Default::default()`
    /// cannot produce a `source` that the host refuses.
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            icon: None,
            source: panel_source_tool(),
            tool: None,
            refresh_ms: 0,
        }
    }
}

/// `source` value: the payload is built by running the named tool.
pub const PANEL_SOURCE_TOOL: &str = "tool";
/// `source` value: the payload is a document the plugin already keeps.
pub const PANEL_SOURCE_STATIC: &str = "static";

/// Prefix of every hosted panel id, mirroring the tool prefix.
pub const PANEL_PREFIX: &str = "plugin__";

/// The id a panel is hosted under: `plugin__<plugin>__<panel>`.
///
/// The prefix keeps a plugin panel from colliding with a built-in inspector tab
/// (`workspace`, `changes`, `commands`, `agents`) or with another plugin's panel.
pub fn hosted_panel_id(plugin: &str, panel: &str) -> String {
    format!("{PANEL_PREFIX}{plugin}__{panel}")
}

/// Whether `id` is usable as a panel id.
///
/// A panel id ends up in a URL path segment (`GET /api/plugins/:name/panels/:id`)
/// and in a DOM id, so it is restricted to `a-z`, `0-9`, `-`, `_` and `.`; it may
/// not be empty or contain whitespace, `/`, `?`, `#` or `%`.
pub fn valid_panel_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
}

/// Clamp a plugin's suggested `refresh_ms`.
///
/// A plugin must not be able to make the UI hammer the server: anything between
/// 1 and 999 becomes 1000, and anything above 300 000 becomes 300 000. 0 stays 0
/// and means "only on explicit refresh".
pub fn clamp_refresh_ms(ms: u64) -> u64 {
    const MIN_NON_ZERO: u64 = 1_000;
    const MAX: u64 = 300_000;
    if ms == 0 {
        return 0;
    }
    ms.clamp(MIN_NON_ZERO, MAX)
}

// ===========================================================================
// Declarative UI contributions
// ===========================================================================
//
// A plugin cannot ship markup or script: the frontend is a compile-time snapshot
// embedded in the binary and the CSP protects the app's origin. What it *can* do
// is describe intent in its manifest — "put this button here", "add this palette
// command", "hide that built-in control" — and let the app draw it with its own
// components. Every string below is a name the app recognises; nothing here is
// HTML, a URL, a CSS selector or code.
//
// The three pieces are deliberately separate:
//
// * **shape** ([`PluginUiContribution`]) — what the plugin declares;
// * **catalogues** ([`UI_SLOTS`], [`UI_HIDEABLE`]) — the closed set of places the
//   app exposes, so a plugin can never invent a mount point;
// * **behaviour** (`ui/action`) — what happens when a person activates it, with
//   the host validating the answer before the browser reads a single field.

/// One `button` contribution: a labelled control in a named slot.
pub const UI_KIND_BUTTON: &str = "button";
/// One `command` contribution: an entry in the command palette.
pub const UI_KIND_COMMAND: &str = "command";
/// One `keybinding` contribution: a keyboard shortcut for an action. Not drawn —
/// listed in the settings screen and matched by the app's key handler.
pub const UI_KIND_KEYBINDING: &str = "keybinding";
/// One `hide` contribution: a built-in element the app should not draw.
pub const UI_KIND_HIDE: &str = "hide";

/// Every contribution kind this build understands.
///
/// A contribution of another kind is skipped with a reason rather than refused
/// outright: a plugin written for a newer host should lose one button, not fail.
pub const UI_KINDS: &[&str] = &[
    UI_KIND_BUTTON,
    UI_KIND_COMMAND,
    UI_KIND_KEYBINDING,
    UI_KIND_HIDE,
];

/// The composer's action row, left of the model pins.
pub const UI_SLOT_COMPOSER_TOOLBAR: &str = "composer.toolbar";
/// The results inspector's header, beside **Context**.
pub const UI_SLOT_INSPECTOR_HEADER: &str = "inspector.header";
/// The session header row above the transcript.
pub const UI_SLOT_SESSION_HEADER: &str = "session.header";

/// Every slot a `button` may target.
///
/// Closed on purpose: the app decides where a control can land, so no plugin can
/// mint a position and no CSS escaping is involved. Adding a slot is a one-line
/// change here plus the mount point in the frontend.
pub const UI_SLOTS: &[&str] = &[
    UI_SLOT_COMPOSER_TOOLBAR,
    UI_SLOT_INSPECTOR_HEADER,
    UI_SLOT_SESSION_HEADER,
];

/// Harness identifiers of built-in UI that a plugin may hide.
///
/// An **allowlist**, and a short one. Hiding is app-wide (every session, every
/// project) and a plugin could otherwise remove the control that stops a runaway
/// run or answers an approval, so only cosmetic and navigational elements are
/// listed. Anything not named here is refused with the list, which teaches the
/// author instead of silently doing nothing.
pub const UI_HIDEABLE: &[&str] = &[
    UI_HIDE_COMPOSER_ATTACH,
    UI_HIDE_INSPECTOR_TAB_WORKSPACE,
    UI_HIDE_INSPECTOR_TAB_CHANGES,
    UI_HIDE_INSPECTOR_TAB_SESSION,
    UI_HIDE_INSPECTOR_TAB_AGENTS,
    UI_HIDE_INSPECTOR_CONTEXT,
];

/// The composer's paperclip / attach-files button.
pub const UI_HIDE_COMPOSER_ATTACH: &str = "composer.attach";
/// The results inspector's **Files** tab.
pub const UI_HIDE_INSPECTOR_TAB_WORKSPACE: &str = "inspector.tab.workspace";
/// The results inspector's **Git** tab.
pub const UI_HIDE_INSPECTOR_TAB_CHANGES: &str = "inspector.tab.changes";
/// The results inspector's **Session usage** tab.
pub const UI_HIDE_INSPECTOR_TAB_SESSION: &str = "inspector.tab.session";
/// The results inspector's **Agents** tab.
pub const UI_HIDE_INSPECTOR_TAB_AGENTS: &str = "inspector.tab.agents";
/// The results inspector's **Context** button.
pub const UI_HIDE_INSPECTOR_CONTEXT: &str = "inspector.context";

/// Most contributions one plugin may add. A manifest is a manifest, not a UI
/// framework: a plugin that needs more than this is a page, and pages belong in a
/// panel.
pub const MAX_UI_CONTRIBUTIONS: usize = 64;
/// Cap on a button or command label, in bytes.
pub const MAX_UI_LABEL_BYTES: usize = 80;
/// Cap on a tooltip / description, in bytes.
pub const MAX_UI_TITLE_BYTES: usize = 200;
/// Cap on a `message` contribution's body, in bytes.
pub const MAX_UI_MESSAGE_BYTES: usize = 4_000;
/// Cap on the toast text a `ui/action` may return, in bytes.
pub const MAX_UI_RESULT_MESSAGE_BYTES: usize = 2_000;
/// Cap on how many panels one action may ask the UI to refresh.
pub const MAX_UI_RESULT_REFRESH: usize = 16;
/// Cap on a `keys` spec, in bytes. A shortcut is a shortcut, not a chord book.
pub const MAX_UI_KEYS_BYTES: usize = 32;

/// Shortcuts the app itself already owns, in canonical form.
///
/// A plugin may not take one: the user learned these, the workbench depends on
/// them, and a silently double-bound key is a bug report nobody can explain. The
/// list is deliberately short and explicit — it is the app's own key map, and a
/// contribution naming one of these is skipped with a note rather than silently
/// shadowed.
pub const UI_RESERVED_KEYS: &[&str] = &[
    "mod+k",       // command palette
    "mod+b",       // sessions rail
    "mod+n",       // new session
    "mod+j",       // session context
    "mod+shift+t", // theme studio
    "mod+enter",   // send / queue
];
// `escape` and `enter` are not listed: every plugin binding must carry `mod`,
// `ctrl` or `alt`, so a bare key can never collide with the app's own. That rule
// is what keeps the reservation list short and honest.

/// Canonical modifier order, so two spellings of one chord compare equal.
const UI_MODIFIER_ORDER: &[&str] = &["mod", "ctrl", "alt", "shift"];

/// Named keys a binding may use after its modifiers.
///
/// A single letter or digit is also allowed. Anything else (a function key, a
/// media key, an arrow) is refused for now: the set is closed so the app can say
/// *why*, and adding one is a one-line change here.
const UI_NAMED_KEYS: &[&str] = &["enter", "escape", "space", "/", "[", "]", ",", ".", "-"];

/// Normalise a `keys` spec, or return `None` when it is not usable.
///
/// Accepted shape: `+`-separated, case-insensitive, at least one of `mod` (Ctrl on
/// Windows/Linux, Cmd on macOS), `ctrl` or `alt`, followed by exactly one key —
/// a single `a-z`/`0-9` character or one of [`UI_NAMED_KEYS`]. `shift` alone is not
/// enough: that is just typing.
///
/// The result is canonical (`mod+shift+f`), so the host, the plugin and the
/// browser can compare chords as plain strings.
pub fn normalise_ui_keys(spec: &str) -> Option<String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_UI_KEYS_BYTES {
        return None;
    }
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key: Option<String> = None;
    for part in trimmed.split('+') {
        let part = part.trim().to_ascii_lowercase();
        if part.is_empty() {
            return None;
        }
        if UI_MODIFIER_ORDER.contains(&part.as_str()) {
            if let Some(canonical) = UI_MODIFIER_ORDER.iter().find(|m| **m == part) {
                if !mods.contains(canonical) {
                    mods.push(*canonical);
                }
            }
            continue;
        }
        if key.is_some() {
            // Two non-modifier keys: a chord, not a shortcut.
            return None;
        }
        let usable = (part.len() == 1 && part.chars().all(|c| c.is_ascii_alphanumeric()))
            || UI_NAMED_KEYS.contains(&part.as_str());
        if !usable {
            return None;
        }
        key = Some(part);
    }
    let key = key?;
    if !mods.iter().any(|m| matches!(*m, "mod" | "ctrl" | "alt")) {
        return None;
    }
    mods.sort_by_key(|m| {
        UI_MODIFIER_ORDER
            .iter()
            .position(|o| o == m)
            .unwrap_or(usize::MAX)
    });
    let mut parts: Vec<String> = mods.into_iter().map(str::to_string).collect();
    parts.push(key);
    Some(parts.join("+"))
}

/// Whether a normalised spec collides with a shortcut the app already owns.
pub fn ui_keys_reserved(spec: &str) -> bool {
    UI_RESERVED_KEYS.contains(&spec)
}

/// One declarative UI contribution from a plugin manifest.
///
/// Every field is a plain string the host validates against a catalogue; nothing
/// is interpreted as markup, a path or a selector. `kind` decides which of the
/// other fields matter:
///
/// | `kind` | required | ignored |
/// | --- | --- | --- |
/// | `button` | `id`, `label`, `slot`, and exactly one of `action` / `message` | `target`, `keys` |
/// | `command` | `id`, `label`, and exactly one of `action` / `message` | `slot`, `target`, `keys` |
/// | `keybinding` | `id`, `label`, `keys`, and exactly one of `action` / `message` | `slot`, `target` |
/// | `hide` | `target` from [`UI_HIDEABLE`] | `label`, `action`, `message`, `keys` |
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginUiContribution {
    /// [`UI_KIND_BUTTON`], [`UI_KIND_COMMAND`] or [`UI_KIND_HIDE`].
    #[serde(default)]
    pub kind: String,
    /// Stable id, unique within the plugin across every contribution. Hosted as
    /// `plugin__<plugin>__<id>`.
    #[serde(default)]
    pub id: String,
    /// For `button`: one of [`UI_SLOTS`].
    #[serde(default)]
    pub slot: Option<String>,
    /// What the person sees. Required for `button` and `command`.
    #[serde(default)]
    pub label: String,
    /// Optional tooltip.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional glyph name from the app's icon set.
    #[serde(default)]
    pub icon: Option<String>,
    /// For `button`/`command`: the plugin RPC action to run on activation.
    #[serde(default)]
    pub action: Option<String>,
    /// For `button`/`command`: a user message to send on activation, for plugins
    /// that do not want to answer `ui/action` at all.
    #[serde(default)]
    pub message: Option<String>,
    /// For `keybinding`: the shortcut, e.g. `mod+shift+f`. Canonicalised by
    /// [`normalise_ui_keys`].
    #[serde(default)]
    pub keys: Option<String>,
    /// For `hide`: one of [`UI_HIDEABLE`].
    #[serde(default)]
    pub target: Option<String>,
}

/// Whether `id` is usable as a contribution id.
///
/// It ends up in a DOM id and in a URL path segment
/// (`POST /api/plugins/:name/ui/actions/:action`), so it is restricted to
/// `a-z`, `0-9`, `-`, `_` and `.` — the same rule as a panel id.
pub fn valid_ui_id(id: &str) -> bool {
    valid_panel_id(id)
}

/// Whether `kind` is one this build understands.
pub fn ui_kind_supported(kind: &str) -> bool {
    UI_KINDS.contains(&kind)
}

/// Whether `slot` is a place the app mounts controls.
pub fn known_ui_slot(slot: &str) -> bool {
    UI_SLOTS.contains(&slot)
}

/// Whether `target` is a built-in element a plugin may ask the app to hide.
pub fn hideable_ui_target(target: &str) -> bool {
    UI_HIDEABLE.contains(&target)
}

/// The id a contribution is hosted under: `plugin__<plugin>__<id>`.
///
/// The prefix keeps a plugin's button from colliding with a built-in one or with
/// another plugin's contribution.
pub fn hosted_ui_id(plugin: &str, id: &str) -> String {
    format!("{PANEL_PREFIX}{plugin}__{id}")
}

/// Response payload of a `ui/action` request.
///
/// Everything here is a *hint for the app*, validated by the host before the
/// browser sees it: a toast, a set of panels to re-read, and at most one URL to
/// open in a new tab. There is deliberately no field that carries markup, a
/// script, a selector or a style.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PluginUiActionResult {
    /// `false` renders the message as an error. Defaults to `true`, so a plugin
    /// that answers `{"message": "done"}` is a success.
    #[serde(default = "ui_result_ok")]
    pub ok: bool,
    /// Shown to the person who clicked.
    #[serde(default)]
    pub message: String,
    /// Bare panel ids of this plugin to re-read after the action.
    #[serde(default)]
    pub refresh: Vec<String>,
    /// A URL to open in a new tab. Only `http` and `https` are accepted.
    #[serde(default)]
    pub open_url: Option<String>,
}

fn ui_result_ok() -> bool {
    true
}

/// Whether a URL is one the UI may open: absolute `http`/`https`, nothing else.
///
/// A plugin cannot hand the app a `javascript:`, `file:` or `data:` URL, and the
/// frontend re-checks this before calling `window.open`.
pub fn allowed_ui_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !lower.contains(char::is_whitespace)
        && url.len() <= 2_048
}

/// The method a host uses to run a contributed action inside its plugin.
pub const UI_ACTION_METHOD: &str = "ui/action";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    #[serde(default = "empty_schema")]
    pub parameters: serde_json::Value,
    /// Optional side-effect class: read_only | mutating | shell | external.
    #[serde(default)]
    pub class: Option<String>,
}

fn empty_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl PluginError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Messages sent from the plugin to the core.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InboundRaw {
    Response {
        id: u64,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<PluginError>,
    },
    Notification {
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
}

impl InboundRaw {
    /// Parse one inbound frame from the plugin process.
    pub fn parse(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

/// Well-known method names for both directions.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const HOOKS_RUN: &str = "hooks/run";
    pub const SHUTDOWN: &str = "shutdown";
    pub const LOG: &str = "log";
    /// Plugin asks a human to look at something (plugin → host, notification).
    ///
    /// Unlike [`LOG`] this reaches the UI: the host validates it and the server
    /// turns it into the same `notification` event the runtime emits.
    pub const NOTIFY: &str = "notify";
    /// Host asks the plugin for the payload of a panel it declared.
    pub const PANELS_READ: &str = "panels/read";
    /// Host asks the plugin to run a UI action it declared.
    ///
    /// The plugin answers with a [`PluginUiActionResult`]: a toast, panels to
    /// refresh, and optionally a URL to open. It runs because a person activated
    /// a control the plugin itself contributed — not on a model's initiative.
    pub const UI_ACTION: &str = super::UI_ACTION_METHOD;
}

/// Hooks the core can invoke on a plugin. Unsupported hooks are documented as
/// reserved; a plugin that does not advertise a hook never receives it.
pub mod hooks {
    pub const ON_RUN_START: &str = "on_run_start";
    pub const ON_RUN_END: &str = "on_run_end";
    pub const ON_TOOL_CALL: &str = "on_tool_call";
    pub const ON_TOOL_RESULT: &str = "on_tool_result";
    pub const ON_MODEL_REQUEST: &str = "on_model_request";
    pub const ON_MODEL_RESPONSE: &str = "on_model_response";
    /// Hooks that may contribute text to the system prompt.
    pub const BEFORE_PROMPT: &str = "before_prompt";
    pub const ON_SESSION_CREATE: &str = "on_session_create";
    /// Called for every notification the runtime emits, so a plugin can bridge
    /// karucode to an external notifier (ntfy, Pushover, a desktop daemon).
    pub const ON_NOTIFY: &str = "on_notify";

    pub const IMPLEMENTED: &[&str] = &[BEFORE_PROMPT, ON_RUN_END, ON_TOOL_RESULT, ON_NOTIFY];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Disabled,
    Starting,
    Ready,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostics {
    pub name: String,
    pub status: PluginStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restart_count: u32,
}

/// Reserved permission strings a plugin may declare.
pub mod permissions {
    pub const FILES_READ: &str = "files.read";
    pub const FILES_WRITE: &str = "files.write";
    pub const SHELL: &str = "shell";
    pub const NETWORK: &str = "network";
    pub const SESSION_READ: &str = "session.read";
}

/// Response payload of a `tools/call` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolResult {
    pub ok: bool,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

/// Response payload of a `hooks/run` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHookResult {
    #[serde(default)]
    pub output: Option<serde_json::Value>,
}

/// Response payload of a `panels/read` request.
///
/// `data` is opaque: the host forwards it to the browser untouched and the
/// frontend renders it with a registered component (or a generic JSON view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginPanelData {
    /// Title the plugin wants shown. Falls back to the manifest's `title`.
    #[serde(default)]
    pub title: Option<String>,
    /// The panel's payload. Any JSON; never interpreted by the host.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Context forwarded with a `panels/read` call and (since protocol v1 is
/// additive) with every `tools/call`, so a workspace-scoped plugin does not have
/// to guess the session or the project from the last `before_prompt`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolCallContext {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Absolute path of the run's project root.
    #[serde(default)]
    pub workspace: String,
    /// Project id, when the session belongs to one.
    #[serde(default)]
    pub project: Option<String>,
    /// Subagent nesting depth; 0 for the main agent.
    #[serde(default)]
    pub depth: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_defaults_protocol_version() {
        let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1.0"}"#).unwrap();
        assert_eq!(m.protocol, PLUGIN_PROTOCOL_VERSION);
        assert!(m.capabilities.is_empty());
    }

    #[test]
    fn inbound_response_and_notification_parse() {
        let v = serde_json::json!({"id": 3, "result": {"ok": true}});
        let m = InboundRaw::parse(v).unwrap();
        assert!(matches!(m, InboundRaw::Response { id: 3, .. }));

        let v = serde_json::json!({"method": "log", "params": {"message": "hi"}});
        let m = InboundRaw::parse(v).unwrap();
        assert!(matches!(m, InboundRaw::Notification { .. }));
    }

    #[test]
    fn plugin_error_serialises() {
        let e = PluginError::new(-32000, "boom");
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("boom"));
    }

    #[test]
    fn a_manifest_without_panels_keeps_working() {
        // The whole point of the capability being additive: a plugin that knows
        // nothing about panels is unaffected, and so is every stored manifest.
        let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1.0"}"#).unwrap();
        assert!(m.panels.is_empty());

        let round = serde_json::to_value(&m).unwrap();
        assert_eq!(round["panels"], serde_json::json!([]));
        let back: PluginManifest = serde_json::from_value(round).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_panel_declaration_defaults_every_field() {
        // `{"id": "todo"}` alone must be a valid declaration: a plugin should not
        // have to spell out `source`/`refresh_ms` to be understood.
        let panels: Vec<PluginPanel> =
            serde_json::from_str(r#"[{"id":"todo","title":"Todo"}]"#).unwrap();
        let panel = &panels[0];
        assert_eq!(panel.id, "todo");
        assert_eq!(panel.title, "Todo");
        assert_eq!(panel.source, PANEL_SOURCE_TOOL);
        assert_eq!(panel.icon, None);
        assert_eq!(panel.tool, None);
        assert_eq!(panel.refresh_ms, 0);

        let full: PluginPanel = serde_json::from_str(
            r#"{"id":"todo","title":"Todo","icon":"check","source":"static",
                "tool":"todo_list","refresh_ms":5000}"#,
        )
        .unwrap();
        assert_eq!(full.source, PANEL_SOURCE_STATIC);
        assert_eq!(full.tool.as_deref(), Some("todo_list"));
        assert_eq!(full.refresh_ms, 5000);

        let manifest: PluginManifest = serde_json::from_str(
            r#"{"name":"x","version":"1.0","panels":[{"id":"todo","title":"Todo"}]}"#,
        )
        .unwrap();
        assert_eq!(manifest.panels.len(), 1);
    }

    #[test]
    fn panel_ids_are_namespaced_and_validated() {
        assert_eq!(hosted_panel_id("kc-todo", "todo"), "plugin__kc-todo__todo");
        // The prefix is what makes a collision with a built-in tab impossible.
        for builtin in ["workspace", "changes", "commands", "agents"] {
            assert!(hosted_panel_id("p", builtin).starts_with(PANEL_PREFIX));
            assert_ne!(hosted_panel_id("p", builtin), builtin);
        }

        for good in ["todo", "tasks-2", "my_panel", "v1.2"] {
            assert!(valid_panel_id(good), "{good} must be accepted");
        }
        for bad in ["", "To do", "tasks/2", "a?b", "a#b", "100%", "Ünicode"] {
            assert!(!valid_panel_id(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn refresh_intervals_are_clamped() {
        // 0 means "only on explicit refresh" and stays 0.
        assert_eq!(clamp_refresh_ms(0), 0);
        // Anything impatient is raised, anything greedy is lowered.
        assert_eq!(clamp_refresh_ms(1), 1_000);
        assert_eq!(clamp_refresh_ms(999), 1_000);
        assert_eq!(clamp_refresh_ms(5_000), 5_000);
        assert_eq!(clamp_refresh_ms(10_000_000), 300_000);
    }

    #[test]
    fn panel_data_round_trips_and_keeps_opaque_json() {
        let value = serde_json::json!({
            "title": "Todo",
            "data": {"items": [{"id": "t2", "status": "done"}], "count": 1},
        });
        let parsed: PluginPanelData = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Todo"));
        assert_eq!(parsed.data["count"], serde_json::json!(1));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);

        // A payload with no title still parses; `data` defaults to null.
        let bare: PluginPanelData = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(bare.title, None);
        assert!(bare.data.is_null());
    }

    #[test]
    fn tool_call_context_survives_a_missing_new_field() {
        let ctx: ToolCallContext = serde_json::from_str(r#"{"session_id":"s_1"}"#).unwrap();
        assert_eq!(ctx.session_id, "s_1");
        assert_eq!(ctx.depth, 0);
        assert_eq!(ctx.project, None);
        // An older host that sends no context at all is representable.
        let none: Option<ToolCallContext> = None;
        assert!(none.is_none());
    }

    #[test]
    fn panels_read_is_a_well_known_method() {
        assert_eq!(methods::PANELS_READ, "panels/read");
    }

    #[test]
    fn ui_action_is_a_well_known_method() {
        assert_eq!(methods::UI_ACTION, "ui/action");
    }

    #[test]
    fn key_specs_normalise_to_one_canonical_spelling() {
        // Order and case do not matter; the canonical form always is
        // mod, ctrl, alt, shift, key.
        assert_eq!(
            normalise_ui_keys("Mod+Shift+F").as_deref(),
            Some("mod+shift+f")
        );
        assert_eq!(
            normalise_ui_keys("shift+mod+f").as_deref(),
            Some("mod+shift+f")
        );
        assert_eq!(normalise_ui_keys(" alt + g ").as_deref(), Some("alt+g"));
        assert_eq!(
            normalise_ui_keys("ctrl+alt+1").as_deref(),
            Some("ctrl+alt+1")
        );
        // `mod` twice is a typo, not a chord.
        assert_eq!(normalise_ui_keys("mod+mod+f").as_deref(), Some("mod+f"));
        // Named keys are allowed after a modifier.
        assert_eq!(normalise_ui_keys("mod+/").as_deref(), Some("mod+/"));
        assert_eq!(
            normalise_ui_keys("ctrl+enter").as_deref(),
            Some("ctrl+enter")
        );
    }

    #[test]
    fn unusable_key_specs_are_refused() {
        for bad in [
            "",                                       // nothing
            "f",                                      // a bare letter is typing, not a shortcut
            "shift+f",                                // shift alone is still typing
            "mod",                                    // no key
            "mod+shift",                              // modifiers only
            "mod+f+g",                                // two keys is a chord
            "mod+f1",                                 // function keys are not in the catalogue
            "mod+arrowup",                            // nor are arrows
            "mod+",                                   // trailing separator
            "+f",                                     // leading separator
            "mod++f",                                 // empty segment
            "mod+😀",                                 // one char, but not alphanumeric
            "mod+ffffffffffffffffffffffffffffffffff", // over the length cap
        ] {
            assert!(normalise_ui_keys(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_apps_own_shortcuts_are_reserved() {
        // Every reserved entry must itself be a spec a plugin *could* write, or the
        // reservation is meaningless.
        for reserved in UI_RESERVED_KEYS {
            let normalised = normalise_ui_keys(reserved);
            assert_eq!(
                normalised.as_deref(),
                Some(*reserved),
                "{reserved} is not canonical"
            );
            assert!(ui_keys_reserved(reserved));
        }
        // The ones a person would actually collide with, named one by one.
        for taken in ["mod+k", "mod+b", "mod+n", "mod+j", "mod+shift+t"] {
            assert!(ui_keys_reserved(taken), "{taken} must stay the app's");
        }
        assert!(!ui_keys_reserved("mod+shift+f"));
    }

    #[test]
    fn a_manifest_without_ui_still_parses() {
        // The whole point of the field being additive: an older manifest, or a
        // plugin that contributes no UI at all, is unchanged.
        let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1.0"}"#).unwrap();
        assert!(m.ui.is_empty());

        let with: PluginManifest = serde_json::from_str(
            r#"{"name":"x","version":"1.0","ui":[
                 {"kind":"button","id":"g","label":"Go","slot":"composer.toolbar","action":"go"},
                 {"kind":"hide","target":"inspector.tab.agents"}
               ]}"#,
        )
        .unwrap();
        assert_eq!(with.ui.len(), 2);
        assert_eq!(with.ui[0].slot.as_deref(), Some(UI_SLOT_COMPOSER_TOOLBAR));
        assert_eq!(
            with.ui[1].target.as_deref(),
            Some(UI_HIDE_INSPECTOR_TAB_AGENTS)
        );
    }

    #[test]
    fn ui_catalogues_are_closed() {
        assert!(ui_kind_supported(UI_KIND_BUTTON));
        assert!(ui_kind_supported(UI_KIND_COMMAND));
        assert!(ui_kind_supported(UI_KIND_KEYBINDING));
        assert!(ui_kind_supported(UI_KIND_HIDE));
        assert!(!ui_kind_supported("webview"));
        assert!(!ui_kind_supported(""));

        for slot in UI_SLOTS {
            assert!(known_ui_slot(slot), "{slot} must be a known slot");
        }
        assert!(!known_ui_slot("composer.everywhere"));
        assert!(!known_ui_slot(""));

        for target in UI_HIDEABLE {
            assert!(hideable_ui_target(target), "{target} must be hideable");
        }
        // Safety-critical controls are deliberately not in the catalogue.
        for refused in [
            "composer.stop",
            "composer.send",
            "approval.approve",
            "approval.deny",
            "session.delete",
        ] {
            assert!(
                !hideable_ui_target(refused),
                "{refused} must never be hideable"
            );
        }
    }

    #[test]
    fn ui_ids_follow_the_panel_rule() {
        assert!(valid_ui_id("new-task"));
        assert!(valid_ui_id("v1.2_beta"));
        for bad in ["", "New Task", "a/b", "a?b", "Ünicode"] {
            assert!(!valid_ui_id(bad), "{bad} must be refused");
        }
        assert_eq!(
            hosted_ui_id("kc-todo", "new-task"),
            "plugin__kc-todo__new-task"
        );
    }

    #[test]
    fn only_http_urls_may_be_opened() {
        assert!(allowed_ui_url("https://example.com/x?y=1"));
        assert!(allowed_ui_url("http://localhost:8080/panel"));
        assert!(allowed_ui_url("HTTPS://EXAMPLE.COM"));
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "//example.com",
            "https://example.com/ with space",
            "",
        ] {
            assert!(!allowed_ui_url(bad), "{bad} must be refused");
        }
        // A URL is bounded so a toast cannot become a payload.
        assert!(!allowed_ui_url(&format!(
            "https://example.com/{}",
            "a".repeat(2_100)
        )));
    }

    #[test]
    fn a_ui_action_result_defaults_to_success() {
        // `{"message": "done"}` is what a plugin actually writes; it must not read
        // as a failure just because `ok` was omitted.
        let parsed: PluginUiActionResult = serde_json::from_str(r#"{"message":"done"}"#).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.message, "done");
        assert!(parsed.refresh.is_empty());
        assert_eq!(parsed.open_url, None);

        let failure: PluginUiActionResult =
            serde_json::from_str(r#"{"ok":false,"message":"nope"}"#).unwrap();
        assert!(!failure.ok);

        // Round-trips, so the host and a hand-written plugin agree on the shape.
        let value = serde_json::json!({"ok": true, "message": "m", "refresh": ["todo"]});
        let parsed: PluginUiActionResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.refresh, vec!["todo".to_string()]);
    }
}
