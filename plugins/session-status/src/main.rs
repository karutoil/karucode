//! `session-status`: the reference plugin for the extensibility surface.
//!
//! It exists to prove — by existing — that a third-party plugin can do what this
//! harness's own features do:
//!
//! * **remember** something per session, durably, across restarts and upgrades
//!   (`plugin/state/*`, §R2);
//! * **offer** it as slash commands that cost no tokens (`/plugin-session-status-complete`,
//!   §R3) and as chips drawn on the rail and the session header (§R5);
//! * **shape the rail** so open work sorts first (`before_session_list`, §R4);
//! * **keep a run going** while the session is still open (`on_stop`, §R1);
//! * **expose** the whole thing to the model as a tool, so the agent can mark a
//!   session done itself instead of asking a human to type a command.
//!
//! Nothing here is privileged. Every API it uses is documented in
//! `docs/PLUGINS.md` and available to any plugin.
//!
//! # What "open" and "completed" mean
//!
//! A session starts **open** — that is the absence of a stored value, so an
//! upgrade or a fresh install costs no migration. Marking it completed writes
//! `{"status":"completed","note":"…","at":<unix>}` under the session scope, which
//! only this plugin can read. `reopen` deletes the key, returning the session to
//! its default.
//!
//! # Install
//!
//! ```toml
//! [[plugins]]
//! name = "session-status"
//! command = ["target/debug/session-status"]
//! ```
//!
//! Then `/plugin-session-status-complete`, or right-click a row in the rail.

use kc_plugin_sdk::{
    hooks, setting_bool, setting_number, setting_text, settings, state_scope, HostClient,
    PluginBuilder, PluginSetting, PluginSettingKind, PluginToolResult, PluginToolResultExt,
    ToolRequest, UiActionRequest, UiActionResult, UiActionResultExt,
};
use serde_json::{json, Value};

/// The session-state key holding this plugin's one value.
const KEY: &str = "status";
/// The command token this plugin hosts, spelled out for the docs and tests.
const COMPLETE_COMMAND: &str = "plugin-session-status-complete";
const REOPEN_COMMAND: &str = "plugin-session-status-reopen";

fn main() -> std::io::Result<()> {
    let host = HostClient::global();
    // The operator's choices, over this plugin's own declared defaults. Read once
    // at start: the host restarts the plugin when a setting is saved, so a fresh
    // process is how a new value arrives.
    // Read inside the handler, not here: `settings()` talks to the host, and the
    // request/response loop that carries that conversation only runs once
    // `.run()` is entered. Calling it in `main` would fail silently and hand back
    // the defaults, which looks correct until someone changes a setting.
    let configured = || {
        let values = settings();
        (
            setting_bool(&values, "collapse_completed", true),
            setting_text(&values, "group_label", "Completed").to_string(),
            setting_number(&values, "after_minutes", 0.0),
        )
    };

    // Reading one session's status is used by the tool, the commands, the badge
    // hook and the stop gate, so it is written once here.
    let read = {
        let host = host.clone();
        move |session_id: &str| -> Option<Value> {
            host.state_get(state_scope::SESSION, session_id, "", KEY)
                .ok()
                .flatten()
        }
    };

    let is_completed = {
        let read = read.clone();
        move |session_id: &str| -> bool {
            read(session_id)
                .and_then(|v| {
                    v.get("status")
                        .and_then(Value::as_str)
                        .map(|s| s == "completed")
                })
                .unwrap_or(false)
        }
    };

    // `mark` is shared by the slash commands and the tool: one code path, so a
    // click, a `/`-command and a tool call can never disagree.
    let mark = {
        let host = host.clone();
        move |session_id: &str, completed: bool, note: &str| -> Result<(), String> {
            if session_id.is_empty() {
                return Err(
                    "this action needs a session; open one, or pass session_id to the tool"
                        .to_string(),
                );
            }
            if completed {
                host.state_set(
                    state_scope::SESSION,
                    session_id,
                    "",
                    KEY,
                    json!({
                        "status": "completed",
                        "note": note,
                        "at": unix_now(),
                    }),
                )
            } else {
                host.state_delete(state_scope::SESSION, session_id, "", KEY)
                    .map(|_| ())
            }
        }
    };

    let complete_action = {
        let mark = mark.clone();
        move |req: UiActionRequest| -> UiActionResult {
            // A slash command passes the typed remainder as `args`; a click passes
            // nothing, which is fine — the note is optional.
            match mark(&req.session_id, true, req.args.trim()) {
                Ok(()) => UiActionResult::ok(if req.args.trim().is_empty() {
                    format!("Marked this session completed. `/{REOPEN_COMMAND}` undoes it.")
                } else {
                    format!(
                        "Marked this session completed: {}. `/{REOPEN_COMMAND}` undoes it.",
                        req.args.trim()
                    )
                })
                .refresh("session-status"),
                Err(e) => UiActionResult::err(e),
            }
        }
    };

    let reopen_action = {
        let mark = mark.clone();
        move |req: UiActionRequest| -> UiActionResult {
            match mark(&req.session_id, false, "") {
                Ok(()) => UiActionResult::ok(format!(
                    "Reopened this session. `/{COMPLETE_COMMAND}` marks it done again."
                ))
                .refresh("session-status"),
                Err(e) => UiActionResult::err(e),
            }
        }
    };

    let read_for_chip = read.clone();

    PluginBuilder::new("session-status", "0.1.0")
        .description("Track whether a session is open or completed, and draw that in the rail")
        .setting(PluginSetting {
            key: "collapse_completed".into(),
            label: "Fold completed sessions away".into(),
            kind: PluginSettingKind::Bool,
            default: json!(true),
            help: "Finished sessions move into their own group at the bottom of the rail \
                   instead of staying in the date list."
                .into(),
            min: None,
            max: None,
            options: Vec::new(),
        })
        .setting(PluginSetting {
            key: "group_label".into(),
            label: "Group label".into(),
            kind: PluginSettingKind::Text,
            default: json!("Completed"),
            help: "What the folded group is called in the rail.".into(),
            min: None,
            max: None,
            options: Vec::new(),
        })
        .setting(PluginSetting {
            key: "after_minutes".into(),
            label: "Wait before folding".into(),
            kind: PluginSettingKind::Number,
            default: json!(0),
            help: "Minutes to leave a finished session where it is before it files itself \
                   away. 0 folds it immediately."
                .into(),
            min: Some(0.0),
            max: Some(20_160.0),
            options: Vec::new(),
        })
        // Declaring the hooks is what makes the host call them; an undeclared hook
        // is never invoked, even though the handler would work.
        .hook(hooks::BEFORE_SESSION_LIST, {
            move |payload: Value| -> Value {
                let read = read.clone();
                let (collapse_completed, group_label, after_minutes) = configured();
                let sessions = payload
                    .get("sessions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let annotated: Vec<Value> = sessions
                    .into_iter()
                    .filter_map(|session| {
                        let id = session.get("id")?.as_str()?.to_string();
                        // No stored value IS the open state: a session starts open,
                        // so nothing was ever written for it. `None` here must not
                        // drop the row - the rail would lose sessions it never
                        // annotated, which is the whole list on a fresh install.
                        let completed = read(&id);
                        let finished_at = completed
                            .as_ref()
                            .filter(|v| v.get("status").and_then(Value::as_str) == Some("completed"))
                            .and_then(|v| v.get("at"))
                            .and_then(Value::as_u64);
                        let Some(finished_at) = finished_at else {
                            return Some(json!({
                                "id": id,
                                "group": 0,
                                "badges": [{"text": "open", "tone": "accent"}],
                            }));
                        };
                        // `after_minutes` is the grace period: a session finished a
                        // moment ago stays where the person is still looking, and
                        // files itself away later. 0 groups immediately.
                        let elapsed_minutes = (unix_now().saturating_sub(finished_at)) as f64 / 60.0;
                        let settled = elapsed_minutes >= after_minutes;
                        if !collapse_completed || !settled {
                            // Not filed away yet: keep its place and its chip, so a
                            // session does not vanish from under the pointer.
                            return Some(json!({
                                "id": id,
                                "group": 0,
                                "badges": [{"text": "completed", "tone": "muted"}],
                            }));
                        }
                        Some(json!({
                            "id": id,
                            // Group 1: the rail draws a lower group below group 0 and
                            // folds a `collapsed` group shut, so finished work files
                            // itself away without disappearing.
                            "group": 1,
                            "collapsed": true,
                            "group_label": group_label,
                            "badges": [{"text": "completed", "tone": "muted"}],
                        }))
                    })
                    .collect();
                json!({ "sessions": annotated })
            }
        })
        // The stop gate is what makes "open" mean something: a run may not quietly
        // end while the session is still marked open. It is bounded by the host
        // (`limits.max_stop_continuations`), so a plugin cannot hold a run hostage.
        .on_stop({
            let is_completed = is_completed.clone();
            move |payload: Value| -> Value {
                let session_id = payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let continuations = payload
                    .get("continuations")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                // Only on the first ask: nagging every turn would burn the whole
                // continuation budget on the same message.
                if continuations > 0 || session_id.is_empty() || is_completed(session_id) {
                    return Value::Null;
                }
                json!({
                    "reason": format!(
                        "This session is still marked **open**. If the work is finished, mark it \
                         completed with `/{COMPLETE_COMMAND}` (or `session_status_complete`), and \
                         say what was done. Otherwise keep going."
                    )
                })
            }
        })
        // A button on the session header. `message`-free: it runs the action.
        .button_with(
            "session.header",
            "mark-completed",
            "Mark completed",
            Some("check"),
            Some("Mark this session completed"),
            complete_action.clone(),
        )
        // A chip on every rail row: clicking it toggles the session's state, which
        // is the whole feature reachable from the list itself.
        .chip_with(
            "session.row.badge",
            "toggle",
            "✓",
            "accent",
            Some("Completed — click to reopen"),
            {
                let read = read_for_chip;
                let complete = complete_action.clone();
                let reopen = reopen_action.clone();
                move |req: UiActionRequest| -> UiActionResult {
                    let done = read(&req.session_id)
                        .and_then(|v| {
                            v.get("status")
                                .and_then(Value::as_str)
                                .map(|s| s == "completed")
                        })
                        .unwrap_or(false);
                    if done {
                        reopen(req)
                    } else {
                        complete(req)
                    }
                }
            },
        )
        // The slash commands: `/plugin-session-status-complete <note>`.
        .command_with(
            "complete",
            "Mark this session completed",
            Some("check"),
            Some("Mark the current session completed"),
            complete_action,
        )
        .command_with(
            "reopen",
            "Reopen this session",
            Some("rotate"),
            Some("Clear the completed mark"),
            reopen_action,
        )
        // …and the same thing for the model, so the agent can finish the job it was
        // asked to do without a human typing anything.
        .tool(
            "session_status_complete",
            "Mark a session completed (or reopen it). Use this when the work the person \
             asked for in this session is finished.",
            json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "The session to mark. Defaults to the session this call runs in."
                    },
                    "note": {
                        "type": "string",
                        "description": "One line on what was finished."
                    },
                    "reopen": {
                        "type": "boolean",
                        "description": "Clear the mark instead of setting it."
                    }
                }
            }),
            {
                let mark = mark.clone();
                move |req: ToolRequest| -> PluginToolResult {
                    let session_id = req
                        .context
                        .as_ref()
                        .map(|c| c.session_id.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| req.str_arg("session_id").to_string());
                    let reopen = req.arguments.get("reopen").and_then(Value::as_bool).unwrap_or(false);
                    let note = req.str_arg("note");
                    match mark(&session_id, !reopen, note) {
                        Ok(()) if reopen => PluginToolResult::ok("Reopened the session.")
                            .with_meta(json!({"status": "open"})),
                        Ok(()) => PluginToolResult::ok(format!(
                            "Marked session `{session_id}` completed{}.",
                            if note.is_empty() {
                                String::new()
                            } else {
                                format!(": {note}")
                            }
                        ))
                        .with_meta(json!({"status": "completed"})),
                        Err(e) => PluginToolResult::err(e),
                    }
                }
            },
        )
        .run()
}

/// Seconds since the epoch, without pulling in a date crate for one number.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
