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
    /// Settings this plugin declares (§S1). The host validates them before the
    /// plugin is marked ready, the operator edits them in Settings → Plugins &
    /// MCP, and the plugin reads the effective values from
    /// `KARUCODE_PLUGIN_SETTINGS`. Additive: a plugin that declares none keeps
    /// working unchanged.
    #[serde(default)]
    pub settings: Vec<PluginSetting>,
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
/// One `replace` contribution: a built-in element's own text, replaced (§R6).
pub const UI_KIND_REPLACE: &str = "replace";

/// Every contribution kind this build understands.
///
/// A contribution of another kind is skipped with a reason rather than refused
/// outright: a plugin written for a newer host should lose one button, not fail.
pub const UI_KINDS: &[&str] = &[
    UI_KIND_BUTTON,
    UI_KIND_COMMAND,
    UI_KIND_KEYBINDING,
    UI_KIND_HIDE,
    UI_KIND_REPLACE,
];

/// The tones a badge or chip may use.
///
/// Closed on purpose: the app owns the palette (and the theme owns the colours),
/// so a plugin picks a *role* - done, active, warn - and never a hex value. An
/// unknown tone draws `neutral` rather than failing.
pub const UI_TONES: &[&str] = &["neutral", "accent", "success", "warn", "danger", "muted"];
/// The tone drawn when a badge names none, or names one this build does not have.
pub const UI_TONE_DEFAULT: &str = "neutral";
/// Cap on a badge's text, in bytes. It sits on a session row beside the title.
pub const MAX_UI_BADGE_BYTES: usize = 24;

/// The composer's action row, left of the model pins.
pub const UI_SLOT_COMPOSER_TOOLBAR: &str = "composer.toolbar";
/// The results inspector's header, beside **Context**.
pub const UI_SLOT_INSPECTOR_HEADER: &str = "inspector.header";
/// The session header row above the transcript.
pub const UI_SLOT_SESSION_HEADER: &str = "session.header";
/// The workspace status bar at the bottom of the window.
pub const UI_SLOT_STATUS_BAR: &str = "statusbar.item";
/// A small chip drawn on a row of the project rail's session list.
pub const UI_SLOT_SESSION_ROW_BADGE: &str = "session.row.badge";
/// An item in a rail session row's right-click menu.
pub const UI_SLOT_SESSION_ROW_MENU: &str = "session.row.menu";
/// A chip in the open session's header, beside its title.
pub const UI_SLOT_SESSION_HEADER_CHIP: &str = "session.header.chip";

/// Every slot a `button` may target.
///
/// Closed on purpose: the app decides where a control can land, so no plugin can
/// mint a position and no CSS escaping is involved. Adding a slot is a one-line
/// change here plus the mount point in the frontend.
///
/// The three `session.*` entries are what makes a workflow plugin possible
/// without shipping markup: a badge is text plus a tone, a menu item is a label
/// plus an action, and a header chip is the same. All three are drawn by the app.
pub const UI_SLOTS: &[&str] = &[
    UI_SLOT_COMPOSER_TOOLBAR,
    UI_SLOT_INSPECTOR_HEADER,
    UI_SLOT_SESSION_HEADER,
    UI_SLOT_STATUS_BAR,
    UI_SLOT_SESSION_ROW_BADGE,
    UI_SLOT_SESSION_ROW_MENU,
    UI_SLOT_SESSION_HEADER_CHIP,
];

/// Harness identifiers of built-in UI that a plugin may hide.
///
/// An **allowlist**. Hiding is app-wide (every session, every project) and a
/// plugin could otherwise remove the control that stops a runaway run or answers
/// an approval, so only cosmetic and navigational elements are listed. Anything
/// not named here is refused with the list, which teaches the author instead of
/// silently doing nothing.
///
/// The list is the app's element registry: every entry maps to a documented
/// mount point in the web UI, and a contribution naming one the running build
/// does not draw is accepted and simply matches nothing - the same graceful
/// degradation as an icon name the app does not have.
pub const UI_HIDEABLE: &[&str] = &[
    UI_HIDE_COMPOSER_ATTACH,
    UI_HIDE_COMPOSER_MODEL,
    UI_HIDE_INSPECTOR_TAB_WORKSPACE,
    UI_HIDE_INSPECTOR_TAB_CHANGES,
    UI_HIDE_INSPECTOR_TAB_SESSION,
    UI_HIDE_INSPECTOR_TAB_AGENTS,
    UI_HIDE_INSPECTOR_CONTEXT,
    UI_HIDE_RAIL_FOOTER,
    UI_HIDE_SESSION_HEADER_ACTIONS,
    UI_HIDE_NOTIFICATIONS,
    UI_HIDE_STATUS_BAR,
    UI_HIDE_COMMAND_PALETTE,
];

/// Elements a plugin may **replace the text of**, rather than hide.
///
/// A narrower catalogue than [`UI_HIDEABLE`] and for a different reason: text
/// substitution is how an organisation renames the product in its own
/// deployment ("Session" → "Thread", "Project" → "Workspace") without forking.
/// Nothing that states a safety property is listed - a plugin must not be able to
/// relabel "Approve" or rewrite the unsandboxed-shell warning.
pub const UI_REPLACEABLE: &[&str] = &[
    UI_REPLACE_APP_NAME,
    UI_REPLACE_SESSION_LABEL,
    UI_REPLACE_PROJECT_LABEL,
    UI_REPLACE_NEW_SESSION_LABEL,
    UI_REPLACE_GREETING,
];

/// The composer's paperclip / attach-files button.
pub const UI_HIDE_COMPOSER_ATTACH: &str = "composer.attach";
/// The composer's model / provider picker.
pub const UI_HIDE_COMPOSER_MODEL: &str = "composer.model";
/// The results inspector's **Files** tab.
pub const UI_HIDE_INSPECTOR_TAB_WORKSPACE: &str = "inspector.tab.workspace";
/// The results inspector's **Git** tab.
pub const UI_HIDE_INSPECTOR_TAB_CHANGES: &str = "inspector.tab.changes";
/// The results inspector's **Session usage** tab.
pub const UI_HIDE_INSPECTOR_TAB_SESSION: &str = "inspector.tab.session";
/// The results inspector's **Agents** tab.
pub const UI_HIDE_INSPECTOR_TAB_AGENTS: &str = "inspector.tab.agents";
/// The **Context** button in the session header.
///
/// The wire name predates the move: the control used to live in the results
/// header and now sits in the session header, so hiding it works whether the
/// results panel is docked, popped out or closed.
pub const UI_HIDE_INSPECTOR_CONTEXT: &str = "inspector.context";
/// The rail's footer (theme and settings buttons).
pub const UI_HIDE_RAIL_FOOTER: &str = "rail.footer";
/// The session header's action buttons, beside the title.
pub const UI_HIDE_SESSION_HEADER_ACTIONS: &str = "session.header.actions";
/// The notification bell and its panel.
pub const UI_HIDE_NOTIFICATIONS: &str = "notifications";
/// The workspace status bar.
pub const UI_HIDE_STATUS_BAR: &str = "statusbar";
/// The `Ctrl/Cmd+K` command palette entry point.
///
/// The chord itself stays bound (it is the app's key map); this removes the
/// visible affordance, which is what a deployment with its own launcher wants.
pub const UI_HIDE_COMMAND_PALETTE: &str = "command-palette";

/// The product name drawn in the rail's lockup.
pub const UI_REPLACE_APP_NAME: &str = "app.name";
/// The word used for a conversation ("Session").
pub const UI_REPLACE_SESSION_LABEL: &str = "label.session";
/// The word used for a workspace root ("Project").
pub const UI_REPLACE_PROJECT_LABEL: &str = "label.project";
/// The "New session" button label.
pub const UI_REPLACE_NEW_SESSION_LABEL: &str = "label.new-session";
/// The empty-state greeting above the composer.
pub const UI_REPLACE_GREETING: &str = "empty.greeting";

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

// ---------------------------------------------------------------------------
// Settings a plugin declares (phase 2, §S1)
//
// A plugin used to be configurable only by hand-editing `[[plugins]]` in
// config.toml: `KARUCODE_PLUGIN_SETTINGS` existed, but nothing could write it and
// nothing told the operator what was configurable. These types are that
// declaration. The host validates them before a plugin is marked ready, and the
// web UI renders a form from them - so a plugin gets settings in the product
// without a line of app code per plugin.
// ---------------------------------------------------------------------------

/// Most settings one plugin may declare. More than this is a configuration
/// surface nobody fills in; the plugin should read a file or use `plugin/state/*`.
pub const MAX_PLUGIN_SETTINGS: usize = 32;
/// Cap on a setting key: `[a-z0-9._-]`, short enough to read in a URL.
pub const MAX_SETTING_KEY_BYTES: usize = 48;
/// Cap on a setting's label (what the field is called in the UI).
pub const MAX_SETTING_LABEL_BYTES: usize = 80;
/// Cap on a setting's help text, in bytes.
pub const MAX_SETTING_HELP_BYTES: usize = 400;
/// Cap on a `text` setting's value, and on each `select` option, in bytes.
pub const MAX_SETTING_TEXT_BYTES: usize = 2_000;
/// Most options a `select` may offer.
pub const MAX_SETTING_OPTIONS: usize = 32;
/// Cap on a number of settings a plugin may mark `secret`.
pub const MAX_SECRET_SETTINGS: usize = 8;

/// The kind of value a setting holds.
///
/// Deliberately a closed set: each one has a renderer, a validator and a JSON
/// encoding, so an unknown type is a manifest error rather than a field the UI
/// silently drops. `secret` is a `text` whose value is never echoed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSettingKind {
    Bool,
    Text,
    Number,
    Select,
    Secret,
}

impl PluginSettingKind {
    /// Every kind, for validation and for the docs.
    pub const ALL: [PluginSettingKind; 5] = [
        PluginSettingKind::Bool,
        PluginSettingKind::Text,
        PluginSettingKind::Number,
        PluginSettingKind::Select,
        PluginSettingKind::Secret,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PluginSettingKind::Bool => "bool",
            PluginSettingKind::Text => "text",
            PluginSettingKind::Number => "number",
            PluginSettingKind::Select => "select",
            PluginSettingKind::Secret => "secret",
        }
    }

    /// Whether a value of this kind is a number, which is the only kind that
    /// carries `min`/`max`.
    pub fn is_number(self) -> bool {
        matches!(self, PluginSettingKind::Number)
    }
}

/// One option of a `select` setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSettingOption {
    /// The value written to the config.
    pub value: String,
    /// What the operator reads. Defaults to `value`.
    #[serde(default)]
    pub label: String,
}

/// One setting a plugin declares.
///
/// Every field beyond `key`, `type` and `default` is presentation or validation:
/// the host stores the value and checks it against this declaration, and the UI
/// draws the field. Nothing here runs code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginSetting {
    /// Stable key, `[a-z0-9._-]`, unique within the plugin. This is what appears
    /// in `KARUCODE_PLUGIN_SETTINGS` and in the settings API.
    pub key: String,
    /// The field's label in the UI.
    pub label: String,
    /// What kind of value it holds.
    #[serde(rename = "type")]
    pub kind: PluginSettingKind,
    /// The value when the operator has set nothing.
    #[serde(default)]
    pub default: serde_json::Value,
    /// One or two sentences under the field.
    #[serde(default)]
    pub help: String,
    /// For `number`: the accepted range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// For `select`: the offered values.
    #[serde(default)]
    pub options: Vec<PluginSettingOption>,
}

/// Why a declared setting was refused.
///
/// Returned rather than logged so the manifest error can name the field: a plugin
/// author reading "setting `foo`: default is not a number" fixes it in seconds,
/// and one reading "invalid manifest" does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingProblem {
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for SettingProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "setting `{}`: {}", self.key, self.message)
        }
    }
}

/// Is `key` a legal setting key: `[a-z0-9._-]`, 1..=[`MAX_SETTING_KEY_BYTES`]?
pub fn valid_setting_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_SETTING_KEY_BYTES
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Check one declared setting, including that its default matches its type.
pub fn validate_setting(setting: &PluginSetting) -> Result<(), SettingProblem> {
    let problem = |message: String| {
        Err(SettingProblem {
            key: setting.key.clone(),
            message,
        })
    };
    if !valid_setting_key(&setting.key) {
        return problem(format!(
            "the key must be ascii lowercase letters, digits, `.`, `_` or `-`, 1 to \
             {MAX_SETTING_KEY_BYTES} bytes"
        ));
    }
    let label = setting.label.trim();
    if label.is_empty() {
        return problem("a label is required: it is what the field is called".into());
    }
    if label.len() > MAX_SETTING_LABEL_BYTES {
        return problem(format!(
            "the label is longer than {MAX_SETTING_LABEL_BYTES} bytes"
        ));
    }
    if setting.help.len() > MAX_SETTING_HELP_BYTES {
        return problem(format!(
            "the help text is longer than {MAX_SETTING_HELP_BYTES} bytes"
        ));
    }
    if let (Some(min), Some(max)) = (setting.min, setting.max) {
        if !min.is_finite() || !max.is_finite() || min > max {
            return problem("`min` must be a finite number no greater than `max`".into());
        }
    }
    if setting.kind != PluginSettingKind::Number && (setting.min.is_some() || setting.max.is_some())
    {
        return problem(format!(
            "`min`/`max` only apply to a `number` setting, not `{}`",
            setting.kind.as_str()
        ));
    }
    if setting.kind != PluginSettingKind::Select && !setting.options.is_empty() {
        return problem(format!(
            "`options` only apply to a `select` setting, not `{}`",
            setting.kind.as_str()
        ));
    }
    if setting.kind == PluginSettingKind::Select {
        if setting.options.is_empty() {
            return problem("a `select` needs at least one option".into());
        }
        if setting.options.len() > MAX_SETTING_OPTIONS {
            return problem(format!("more than {MAX_SETTING_OPTIONS} options"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for option in &setting.options {
            if option.value.trim().is_empty() || option.value.len() > MAX_SETTING_TEXT_BYTES {
                return problem("every option needs a non-empty value".into());
            }
            if option.label.len() > MAX_SETTING_LABEL_BYTES {
                return problem(format!(
                    "an option label is longer than {MAX_SETTING_LABEL_BYTES} bytes"
                ));
            }
            if !seen.insert(option.value.as_str()) {
                return problem(format!("two options share the value `{}`", option.value));
            }
        }
    }
    // The default must be a value the setting could actually hold: a form that
    // offers a default the validator would reject is a form that cannot be saved.
    match check_setting_value(setting, &setting.default) {
        Ok(()) => Ok(()),
        Err(message) => problem(format!("the default is invalid: {message}")),
    }
}

/// Check that `value` is acceptable for `setting`.
///
/// This is the single place a stored value is judged - the settings API, the
/// config loader and the UI all defer to it, so they cannot disagree about what
/// a legal value is.
pub fn check_setting_value(
    setting: &PluginSetting,
    value: &serde_json::Value,
) -> Result<(), String> {
    let kind = setting.kind;
    fn require_text<'v>(
        setting: &PluginSetting,
        value: &'v serde_json::Value,
    ) -> Result<&'v str, String> {
        let text = value
            .as_str()
            .ok_or_else(|| format!("`{}` expects a string", setting.key))?;
        if text.len() > MAX_SETTING_TEXT_BYTES {
            return Err(format!(
                "the value is longer than {MAX_SETTING_TEXT_BYTES} bytes"
            ));
        }
        Ok(text)
    }
    match kind {
        PluginSettingKind::Bool => {
            if !value.is_boolean() {
                return Err(format!("`{}` expects true or false", setting.key));
            }
        }
        PluginSettingKind::Text | PluginSettingKind::Secret => {
            require_text(setting, value)?;
        }
        PluginSettingKind::Number => {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("`{}` expects a number", setting.key))?;
            if !number.is_finite() {
                return Err(format!("`{}` must be a finite number", setting.key));
            }
            if let Some(min) = setting.min {
                if number < min {
                    return Err(format!("`{}` must be at least {min}", setting.key));
                }
            }
            if let Some(max) = setting.max {
                if number > max {
                    return Err(format!("`{}` must be at most {max}", setting.key));
                }
            }
        }
        PluginSettingKind::Select => {
            let text = require_text(setting, value)?;
            if !setting.options.iter().any(|o| o.value == text) {
                let offered: Vec<&str> = setting.options.iter().map(|o| o.value.as_str()).collect();
                return Err(format!(
                    "`{}` must be one of [{}]",
                    setting.key,
                    offered.join(", ")
                ));
            }
        }
    }
    Ok(())
}

/// Validate a whole declared set: bounds, uniqueness, and each setting's own rules.
pub fn validate_settings(settings: &[PluginSetting]) -> Result<(), SettingProblem> {
    if settings.len() > MAX_PLUGIN_SETTINGS {
        return Err(SettingProblem {
            key: String::new(),
            message: format!("a plugin may declare at most {MAX_PLUGIN_SETTINGS} settings"),
        });
    }
    let secrets = settings
        .iter()
        .filter(|s| s.kind == PluginSettingKind::Secret)
        .count();
    if secrets > MAX_SECRET_SETTINGS {
        return Err(SettingProblem {
            key: String::new(),
            message: format!("a plugin may declare at most {MAX_SECRET_SETTINGS} secrets"),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for setting in settings {
        validate_setting(setting)?;
        if !seen.insert(setting.key.as_str()) {
            return Err(SettingProblem {
                key: setting.key.clone(),
                message: "declared twice".into(),
            });
        }
    }
    Ok(())
}

/// Apply an operator's values over a plugin's declared defaults.
///
/// Values that do not fit the declaration are **dropped rather than applied**: the
/// plugin then sees its default, which is the safe direction. The settings API
/// refuses such a write in the first place, so this only fires for a config file
/// edited by hand - and a hand-edited typo must not stop a plugin from loading.
pub fn effective_settings(
    settings: &[PluginSetting],
    configured: &std::collections::BTreeMap<String, serde_json::Value>,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut out = std::collections::BTreeMap::new();
    for setting in settings {
        let value = configured
            .get(&setting.key)
            .filter(|value| check_setting_value(setting, value).is_ok())
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        out.insert(setting.key.clone(), value);
    }
    // Keys the plugin never declared are kept: a newer plugin installed on an
    // older declaration would otherwise lose its operator's values on load.
    for (key, value) in configured {
        if !out.contains_key(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

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
    /// For a `session.row.badge` (§R5): the chip's text. Short by design - it is
    /// drawn on a 26 px row next to the session's own title.
    #[serde(default)]
    pub text: Option<String>,
    /// For a `session.row.badge`: one of [`UI_TONES`]. Defaults to `neutral`.
    #[serde(default)]
    pub tone: Option<String>,
    /// For `command`: the slash command's summary line, shown in the `/` menu and
    /// in `GET /api/commands` if this contribution is also a command (§R3).
    #[serde(default)]
    pub summary: Option<String>,
    /// For a session-state command: the state keys it writes, so the UI can
    /// refresh exactly those instead of every panel. Informational.
    #[serde(default)]
    pub sets: Vec<String>,
    /// For `replace` (§R6): the literal text to put in place of the element's own.
    #[serde(default)]
    pub replace: Option<String>,
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

/// Whether `target` is an element whose text a plugin may replace (§R6).
pub fn replaceable_ui_target(target: &str) -> bool {
    UI_REPLACEABLE.contains(&target)
}

/// Whether `tone` is a badge tone this build draws.
pub fn known_ui_tone(tone: &str) -> bool {
    UI_TONES.contains(&tone)
}

/// The tone a badge is actually drawn with: its own if known, else `neutral`.
pub fn ui_tone_or_default(tone: Option<&str>) -> &'static str {
    match tone {
        Some(tone) if known_ui_tone(tone) => UI_TONES
            .iter()
            .find(|t| **t == tone)
            .copied()
            .unwrap_or(UI_TONE_DEFAULT),
        _ => UI_TONE_DEFAULT,
    }
}

/// Whether `slot` holds a chip/badge, which carries `text` instead of a label.
pub fn ui_slot_is_badge(slot: &str) -> bool {
    slot == UI_SLOT_SESSION_ROW_BADGE || slot == UI_SLOT_SESSION_HEADER_CHIP
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
    /// Plugin-owned durable state (host → plugin: none; the plugin asks the host).
    ///
    /// The four calls are `plugin/state/get`, `plugin/state/set`,
    /// `plugin/state/delete` and `plugin/state/list`. They are requests the
    /// **plugin** sends to the host, over the same stdio channel tools and
    /// `notify` already use, and the host answers with a JSON-RPC response. A
    /// plugin therefore keeps its own data without growing a database of its own,
    /// and without the harness ever exposing another session's rows.
    pub const STATE_GET: &str = "plugin/state/get";
    pub const STATE_SET: &str = "plugin/state/set";
    pub const STATE_DELETE: &str = "plugin/state/delete";
    pub const STATE_LIST: &str = "plugin/state/list";
    /// Read this plugin's **effective settings** (§S2): what the operator set,
    /// over the defaults its own manifest declared.
    ///
    /// Read-only by design. A plugin must not be able to change its own settings,
    /// or it could quietly relax a guard rail the operator switched on. Settings
    /// are operator data; the plugin is a consumer.
    pub const SETTINGS_GET: &str = "plugin/settings/get";
}

/// Scopes a plugin may store state in.
///
/// * `session` — one conversation. The host fills in the session id, so a plugin
///   cannot name a session it was not asked about; deleting a session deletes its
///   rows.
/// * `project` — one project, visible to every session in it.
/// * `install` — this server, every project. The natural home for "the operator
///   turned this plugin off", not for per-session workflow state.
pub mod state_scope {
    pub const SESSION: &str = "session";
    pub const PROJECT: &str = "project";
    pub const INSTALL: &str = "install";
    /// Values the **operator** set through the settings API (§S2).
    ///
    /// Deliberately absent from [`ALL`]: `valid_state_scope` gates every
    /// `plugin/state/*` call, so a plugin cannot read or write this scope with
    /// the plugin-facing API. Its only writer is the server, on behalf of the
    /// person at the keyboard - otherwise a plugin could relax a guard rail the
    /// operator switched on.
    pub const SETTING: &str = "setting";
    pub const ALL: &[&str] = &[SESSION, PROJECT, INSTALL];
}

/// Longest key a plugin may store under, in bytes.
///
/// Keys are sorted and shown in a panel and in `GET /api/plugins/:name/state`, so
/// they stay short and boring: `[a-z0-9._-]`, which makes them stable across
/// languages and impossible to confuse with a path.
pub const MAX_STATE_KEY_BYTES: usize = 120;
/// Largest single value a plugin may store, in bytes (JSON, serialised).
///
/// This is *data*, so the bound is generous next to a tool result: a workflow
/// plugin keeping a per-session note does not need more, and a plugin that wants
/// to keep a database is a plugin that should be writing a file.
pub const MAX_STATE_VALUE_BYTES: usize = 64 * 1024;
/// Rows one plugin may hold per scope, so a leak is bounded even under the byte cap.
pub const MAX_STATE_KEYS_PER_SCOPE: usize = 4_096;
/// Total bytes one plugin may hold in one scope, across every key.
///
/// Per scope rather than per plugin so a workflow plugin keeping a little state
/// per session is not punished for having many sessions, while a single runaway
/// scope still has a ceiling. The write that would cross it is refused before it
/// lands, with a message naming the limit.
pub const MAX_STATE_BYTES_PER_PLUGIN: i64 = 8 * 1024 * 1024;
/// Rows a `plugin/state/list` answer returns at most.
pub const MAX_STATE_LIST: usize = 512;

/// Whether `key` is a usable plugin state key.
pub fn valid_state_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_STATE_KEY_BYTES
        && key.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
}

/// Whether `scope` is one [`state_scope::ALL`] names.
pub fn valid_state_scope(scope: &str) -> bool {
    state_scope::ALL.contains(&scope)
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
    /// Asked when a run is about to end. May keep it going for one more turn.
    pub const ON_STOP: &str = "on_stop";
    /// Called when the operator draws a session list, to shape how it is shown.
    pub const BEFORE_SESSION_LIST: &str = "before_session_list";

    pub const IMPLEMENTED: &[&str] = &[
        BEFORE_PROMPT,
        ON_RUN_END,
        ON_TOOL_RESULT,
        ON_NOTIFY,
        ON_TOOL_CALL,
        ON_MODEL_REQUEST,
        ON_STOP,
        BEFORE_SESSION_LIST,
    ];
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

/// Per-plugin tool-output budgets: how many bytes of one tool result a single
/// plugin may return to the model.
///
/// The global bound is `limits.max_tool_output_bytes` (24,000 bytes by default)
/// and it stays the default for every plugin with no grant. A plugin may be
/// given its own budget - higher or lower - and that grant is a **record of an
/// operator decision**, never something a plugin can create for itself:
///
/// * a `plugin.toml` in the plugin's own directory cannot set it: the key is
///   ignored there (with a warning) and stripped out of an installed manifest,
///   exactly like `settings.trust` and `settings.read_only_tools`;
/// * `[[plugins]] tool_output_bytes` in the operator's `config.toml` may only
///   *narrow* a grant - on its own it grants nothing, so an agent that edits its
///   own config still gets the global default;
/// * the only writer of a grant is the operator-facing decision (the web UI or
///   an authenticated route), and every decision records who, when and why.
///
/// There is no environment variable, plugin setting or manifest key that creates
/// a grant. Values are rejected outside [`MIN_BYTES`]`..=`[`MAX_BYTES`] at
/// request time and clamped again when read, so lowering
/// `limits.max_plugin_tool_output_bytes` retires an older, larger grant.
/// The durable backend behind `plugin/state/*` (§R2).
///
/// The plugin host only needs somewhere to read and write a JSON string under
/// `(plugin, scope, owner, key)`; the server implements this over `kc-store`, so
/// `kc-plugin-host` never depends on the store and a test can pass an in-memory
/// map instead. The trait speaks plain strings and counts; every bound (value
/// size, row count, byte budget) is enforced by the caller, which is what keeps
/// one hostile plugin from growing the database without limit.
#[async_trait::async_trait]
pub trait PluginStateBackend: Send + Sync {
    /// Read one value.
    async fn get(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
    ) -> crate::Result<Option<String>>;
    /// Write one value, returning the row count in that scope after the write.
    async fn set(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
        value: &str,
    ) -> crate::Result<i64>;
    /// Delete one value, reporting whether it existed.
    async fn delete(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
    ) -> crate::Result<bool>;
    /// List `(key, value)` pairs for one scope, ordered by key, capped by `limit`.
    async fn list(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        prefix: &str,
        limit: usize,
    ) -> crate::Result<Vec<(String, String)>>;
    /// Total stored bytes for one plugin in one scope (the budget check).
    async fn bytes(&self, plugin: &str, scope: &str, owner: &str) -> crate::Result<i64>;
}

/// One session, as a `before_session_list` hook sees it (§R4).
///
/// A compact summary, never a transcript: enough to decide "this is open, that is
/// done" without handing a plugin the conversation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionListEntry {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    /// The session's latest run status (`completed`, `failed`, …), or empty.
    pub status: String,
}

/// How a plugin wants one session drawn in the rail (§R4).
///
/// Every field is a hint the app validates: `sort_key` reorders within a group,
/// `group` is a small integer bucket (0 first — a workflow plugin puts open work
/// there), and `badges` are short chips. `None` means "draw it as usual".
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SessionListAnnotation {
    pub id: String,
    /// Lower sorts first within its group. `None` keeps `updated_at` ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<i64>,
    /// The bucket this row belongs to; `None` keeps it in the default bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<i32>,
    /// Chips drawn on the row. Each is `{text, tone}`; the app caps the count.
    #[serde(default)]
    pub badges: Vec<SessionBadge>,
}

/// One chip a plugin asks the app to draw beside a session (§R4/R5).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SessionBadge {
    pub text: String,
    /// One of [`UI_TONES`]; an unknown tone draws `neutral`.
    #[serde(default)]
    pub tone: String,
}

/// Rows one `before_session_list` answer may carry.
pub const MAX_SESSION_ANNOTATIONS: usize = 512;
/// Chips drawn per row: three is already a busy row.
pub const MAX_SESSION_BADGES: usize = 3;

pub mod tool_output {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::RwLock;

    /// Smallest budget a grant may carry. It is the floor the runtime already
    /// keeps for any tool result (`TURN_RESULT_FLOOR_BYTES`), so a smaller grant
    /// would only starve the model of the result it asked for.
    pub const MIN_BYTES: usize = 2_000;
    /// Largest budget a grant may carry. The turn's share of the context window
    /// and the 8 MiB protocol framing cap both bite long before this; a bigger
    /// number would only be a promise the host cannot keep.
    pub const MAX_BYTES: usize = 1_000_000;
    /// How long a request waits for the operator before the silence is read as
    /// "no". Silence, a dismissal and a timeout are all a refusal.
    pub const DEFAULT_WAIT_SECS: u64 = 180;
    /// Ceiling for a caller-requested wait, so a blocked plugin install cannot
    /// park a run for an hour.
    pub const MAX_WAIT_SECS: u64 = 900;

    /// Clamp a byte value into the range a grant may carry.
    pub fn clamp_bytes(bytes: usize) -> usize {
        bytes.clamp(MIN_BYTES, MAX_BYTES)
    }

    /// The operator's answer, as stored.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Decision {
        Granted,
        Refused,
    }

    impl Decision {
        pub fn as_str(self) -> &'static str {
            match self {
                Decision::Granted => "granted",
                Decision::Refused => "refused",
            }
        }

        pub fn parse(raw: &str) -> Option<Self> {
            match raw.trim().to_ascii_lowercase().as_str() {
                "granted" | "grant" | "allow" | "allowed" | "yes" => Some(Decision::Granted),
                "refused" | "refuse" | "deny" | "denied" | "no" => Some(Decision::Refused),
                _ => None,
            }
        }
    }

    /// One recorded decision: the audit record behind a grant or a refusal.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Record {
        pub plugin: String,
        pub decision: Decision,
        /// The granted value. `None` on a refusal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bytes: Option<usize>,
        /// What was asked for, so a refusal still says what it refused.
        pub requested_bytes: usize,
        #[serde(default)]
        pub reason: String,
        /// Who decided: `operator` for a person, `no answer` for a timeout or a
        /// dismissal. Never an agent identity - no agent path writes a grant.
        pub decided_by: String,
        /// Which surface answered: `web`, `cli-tty` or `api`.
        pub channel: String,
        /// Unix milliseconds.
        pub decided_at: i64,
    }

    impl Record {
        /// The bytes this record grants, if any. A refusal grants nothing.
        pub fn granted_bytes(&self) -> Option<usize> {
            match self.decision {
                Decision::Granted => self.bytes.map(clamp_bytes),
                Decision::Refused => None,
            }
        }
    }

    /// A request waiting for the operator to answer.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Request {
        pub plugin: String,
        pub bytes: usize,
        #[serde(default)]
        pub reason: String,
        pub requested_at: i64,
    }

    /// One line of the append-only audit trail.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Event {
        pub plugin: String,
        /// `requested`, `granted`, `refused` or `cleared`.
        pub kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bytes: Option<usize>,
        pub requested_bytes: usize,
        #[serde(default)]
        pub reason: String,
        /// Who caused it: an agent (a request) or the operator (a decision).
        pub actor: String,
        /// Unix milliseconds.
        pub at: i64,
    }

    /// The grants in force, shared between the server (which writes them) and
    /// the plugin host (which reads them on every tool call).
    ///
    /// Reads are lock-cheap and lock-tiny: a tool call does one map lookup, so a
    /// decision takes effect on the very next call without a reload.
    #[derive(Debug, Default)]
    pub struct Grants {
        inner: RwLock<BTreeMap<String, Record>>,
    }

    impl Grants {
        pub fn new() -> Self {
            Self::default()
        }

        /// Replace the grant in force for `plugin`. A refusal is stored too, so
        /// the host can tell "never asked" from "the operator said no".
        pub fn set(&self, record: Record) {
            let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
            guard.insert(record.plugin.clone(), record);
        }

        /// Forget the decision for `plugin`, allowing a fresh request.
        pub fn clear(&self, plugin: &str) {
            let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
            guard.remove(plugin);
        }

        pub fn record(&self, plugin: &str) -> Option<Record> {
            self.inner
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(plugin)
                .cloned()
        }

        /// The budget the host applies to this plugin's tool results, or `None`
        /// when the global `limits.max_tool_output_bytes` still governs it.
        ///
        /// `declared` is the operator's `[[plugins]] tool_output_bytes`: it can
        /// only narrow what the decision granted, never widen it. The ceiling in
        /// `limits.max_plugin_tool_output_bytes` is applied by the runtime, which
        /// is the side that holds the live [`crate::config::Limits`].
        pub fn effective_bytes(&self, plugin: &str, declared: Option<usize>) -> Option<usize> {
            let record = self.record(plugin)?;
            let granted = record.granted_bytes()?;
            let narrowed = declared.map_or(granted, |declared| granted.min(declared));
            Some(clamp_bytes(narrowed))
        }
    }
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
    /// The text a person typed after a slash command, when the action was run
    /// from the composer (§R3) rather than from a clicked control.
    ///
    /// It is the command's argument, so a plugin can implement `/todo add the
    /// release note` without a second round trip. Empty for a button click, and
    /// always empty for a tool call.
    #[serde(default)]
    pub args: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grant map is the one place the effective budget comes from, so its
    /// three answers are pinned here: a grant applies, a refusal (and an absent
    /// decision) falls back to the global default, and the operator's own
    /// `tool_output_bytes` can only narrow what they granted.
    #[test]
    fn grants_apply_narrow_refuse_and_clear() {
        use tool_output::{Decision, Grants};

        let grants = Grants::new();
        // Nothing recorded: the global `limits.max_tool_output_bytes` governs.
        assert_eq!(grants.effective_bytes("kc-codegraph", None), None);

        grants.set(tool_output::Record {
            plugin: "kc-codegraph".into(),
            decision: Decision::Granted,
            bytes: Some(400_000),
            requested_bytes: 400_000,
            reason: "whole-feature bundles".into(),
            decided_by: "operator".into(),
            channel: "web".into(),
            decided_at: 1,
        });
        assert_eq!(
            grants.effective_bytes("kc-codegraph", None),
            Some(400_000),
            "the grant raises the ceiling above the 24,000 default"
        );
        assert_eq!(
            grants.effective_bytes("kc-codegraph", Some(50_000)),
            Some(50_000),
            "the operator's config value narrows their own grant"
        );
        assert_eq!(
            grants.effective_bytes("kc-codegraph", Some(900_000)),
            Some(400_000),
            "and it can never widen it"
        );

        // A refusal is a decision too, and it grants nothing.
        grants.set(tool_output::Record {
            plugin: "wordy".into(),
            decision: Decision::Refused,
            bytes: None,
            requested_bytes: 120_000,
            reason: String::new(),
            decided_by: "operator".into(),
            channel: "web".into(),
            decided_at: 2,
        });
        assert_eq!(
            grants.effective_bytes("wordy", None),
            None,
            "a refusal falls back to the global default"
        );

        // Clearing reopens the question rather than restoring the old grant.
        grants.clear("kc-codegraph");
        assert_eq!(grants.effective_bytes("kc-codegraph", None), None);
    }

    #[test]
    fn a_granted_value_is_clamped_to_the_accepted_range() {
        use tool_output::{Decision, Grants, MAX_BYTES, MIN_BYTES};
        let grants = Grants::new();
        let record = |bytes: usize| tool_output::Record {
            plugin: "p".into(),
            decision: Decision::Granted,
            bytes: Some(bytes),
            requested_bytes: bytes,
            reason: String::new(),
            decided_by: "operator".into(),
            channel: "web".into(),
            decided_at: 1,
        };
        grants.set(record(0));
        assert_eq!(
            grants.effective_bytes("p", None),
            Some(MIN_BYTES),
            "a stored zero cannot starve a result to nothing"
        );
        grants.set(record(usize::MAX));
        assert_eq!(
            grants.effective_bytes("p", None),
            Some(MAX_BYTES),
            "and an absurd value cannot promise more than the host will send"
        );
    }

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

    // ---- declared settings (phase 2, §S1) ----

    fn setting(key: &str, kind: PluginSettingKind, default: serde_json::Value) -> PluginSetting {
        PluginSetting {
            key: key.into(),
            label: key.into(),
            kind,
            default,
            help: String::new(),
            min: None,
            max: None,
            options: Vec::new(),
        }
    }

    #[test]
    fn a_well_formed_setting_manifest_is_accepted() {
        let settings = vec![
            setting("collapse", PluginSettingKind::Bool, serde_json::json!(true)),
            setting(
                "label",
                PluginSettingKind::Text,
                serde_json::json!("Completed"),
            ),
            PluginSetting {
                min: Some(0.0),
                max: Some(120.0),
                ..setting("after", PluginSettingKind::Number, serde_json::json!(0))
            },
            PluginSetting {
                options: vec![
                    PluginSettingOption {
                        value: "bottom".into(),
                        label: "Bottom".into(),
                    },
                    PluginSettingOption {
                        value: "top".into(),
                        label: "Top".into(),
                    },
                ],
                ..setting(
                    "place",
                    PluginSettingKind::Select,
                    serde_json::json!("bottom"),
                )
            },
        ];
        assert!(validate_settings(&settings).is_ok());
    }

    #[test]
    fn a_bad_setting_is_refused_with_the_reason_named() {
        // The point of a SettingProblem is that the author can fix it from the
        // message alone, so each case asserts the message says what is wrong.
        let cases: Vec<(PluginSetting, &str)> = vec![
            (
                PluginSetting {
                    key: "Not A Key".into(),
                    ..setting("ok", PluginSettingKind::Bool, serde_json::json!(true))
                },
                "key must be",
            ),
            // A default that does not match the declared type: the form would
            // open on a value it then refuses to save.
            (
                setting("flag", PluginSettingKind::Bool, serde_json::json!("yes")),
                "default is invalid",
            ),
            (
                setting("n", PluginSettingKind::Number, serde_json::json!(true)),
                "default is invalid",
            ),
            (
                PluginSetting {
                    min: Some(10.0),
                    max: Some(1.0),
                    ..setting("r", PluginSettingKind::Number, serde_json::json!(5))
                },
                "min",
            ),
            // min/max on a non-number, options on a non-select: silently ignoring
            // either would leave the author believing the field was constrained.
            (
                PluginSetting {
                    min: Some(0.0),
                    ..setting("t", PluginSettingKind::Text, serde_json::json!("x"))
                },
                "only apply to a `number`",
            ),
            (
                PluginSetting {
                    options: vec![PluginSettingOption {
                        value: "a".into(),
                        label: String::new(),
                    }],
                    ..setting("b", PluginSettingKind::Bool, serde_json::json!(true))
                },
                "only apply to a `select`",
            ),
            (
                setting("s", PluginSettingKind::Select, serde_json::json!("a")),
                "at least one option",
            ),
        ];
        for (bad, expected) in cases {
            let error = validate_setting(&bad).expect_err("should be refused");
            assert!(
                error.message.contains(expected),
                "expected {expected:?} in {:?}",
                error.message
            );
        }
    }

    #[test]
    fn a_select_only_accepts_its_declared_options() {
        let select = PluginSetting {
            options: vec![PluginSettingOption {
                value: "bottom".into(),
                label: "Bottom".into(),
            }],
            ..setting(
                "place",
                PluginSettingKind::Select,
                serde_json::json!("bottom"),
            )
        };
        assert!(check_setting_value(&select, &serde_json::json!("bottom")).is_ok());
        let error = check_setting_value(&select, &serde_json::json!("middle")).unwrap_err();
        assert!(
            error.contains("bottom"),
            "the reason lists the options: {error}"
        );
    }

    #[test]
    fn a_number_is_bounded_by_its_range() {
        let number = PluginSetting {
            min: Some(0.0),
            max: Some(10.0),
            ..setting("n", PluginSettingKind::Number, serde_json::json!(5))
        };
        assert!(check_setting_value(&number, &serde_json::json!(0)).is_ok());
        assert!(check_setting_value(&number, &serde_json::json!(10.0)).is_ok());
        assert!(check_setting_value(&number, &serde_json::json!(10.5)).is_err());
        assert!(check_setting_value(&number, &serde_json::json!(-1)).is_err());
        // JSON has no NaN literal, but a computed value can still be absurd.
        assert!(check_setting_value(&number, &serde_json::json!("5")).is_err());
    }

    #[test]
    fn duplicate_and_excessive_settings_are_refused() {
        let twice = vec![
            setting("dup", PluginSettingKind::Bool, serde_json::json!(true)),
            setting("dup", PluginSettingKind::Text, serde_json::json!("x")),
        ];
        assert!(validate_settings(&twice)
            .unwrap_err()
            .message
            .contains("twice"));

        let many: Vec<PluginSetting> = (0..=MAX_PLUGIN_SETTINGS)
            .map(|i| {
                setting(
                    &format!("k{i}"),
                    PluginSettingKind::Bool,
                    serde_json::json!(true),
                )
            })
            .collect();
        assert!(validate_settings(&many).is_err());
    }

    #[test]
    fn effective_settings_fall_back_to_defaults_and_keep_unknown_keys() {
        let declared = vec![
            setting("collapse", PluginSettingKind::Bool, serde_json::json!(true)),
            setting(
                "label",
                PluginSettingKind::Text,
                serde_json::json!("Completed"),
            ),
        ];
        let mut configured = std::collections::BTreeMap::new();
        // A legal override, an illegal one, and a key the plugin never declared.
        configured.insert("collapse".to_string(), serde_json::json!(false));
        configured.insert("label".to_string(), serde_json::json!(42));
        configured.insert("future_key".to_string(), serde_json::json!("kept"));

        let effective = effective_settings(&declared, &configured);
        assert_eq!(effective.get("collapse"), Some(&serde_json::json!(false)));
        // An illegal value falls back to the default rather than reaching the
        // plugin: a hand-edited config typo must not break a plugin's assumptions.
        assert_eq!(
            effective.get("label"),
            Some(&serde_json::json!("Completed"))
        );
        // A key a newer build knows about survives a load by an older declaration.
        assert_eq!(
            effective.get("future_key"),
            Some(&serde_json::json!("kept"))
        );
    }

    #[test]
    fn a_manifest_may_declare_settings_and_an_old_one_need_not() {
        let with: PluginManifest = serde_json::from_value(serde_json::json!({
            "name": "p", "version": "1",
            "settings": [
                {"key": "collapse", "label": "Collapse", "type": "bool", "default": true}
            ]
        }))
        .expect("manifest");
        assert_eq!(with.settings.len(), 1);
        assert_eq!(with.settings[0].kind, PluginSettingKind::Bool);

        let without: PluginManifest =
            serde_json::from_value(serde_json::json!({"name": "p", "version": "1"})).expect("old");
        assert!(without.settings.is_empty(), "settings are additive");
    }
}
