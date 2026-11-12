//! Notification payloads: the one signal that says "look at this".
//!
//! A notification is a *hint about state*, never the state itself. It is emitted
//! after the thing it describes has been persisted, it is rendered by surfaces the
//! server does not control (an OS notification, a toast, a sound cue), and losing
//! one must never break a run.
//!
//! Two rules shape the types below and are enforced by [`Notify::new`], so no call
//! site can bypass them:
//!
//! * **Plain text, capped.** `title` and `body` are drawn by the operating system
//!   and by a toast inside the app's own origin. They are never Markdown, never
//!   HTML, and are truncated on the way in — a plugin cannot push a 60 KB tool
//!   result into a desktop notification, and a prompt-injected answer cannot paint
//!   arbitrary-length text on a lock screen.
//! * **A closed set of kinds.** The sound cue table, the per-event settings toggles
//!   and the settings UI all key off [`NotifyKind`], so a plugin may choose among
//!   four values but cannot invent its own label.
//!
//! The kind is the *reason a human is being interrupted*, not the event that
//! produced it: a run that hit an iteration limit is [`NotifyKind::NeedsInput`]
//! ("wants to continue, may not"), not an error.

// @kc-doc: notifications
// @kc-title: Get told when a run finishes or needs an answer
// @kc-summary: karucode pings you when a run settles, parks on an approval or fails — with a sound per kind, desktop notifications, an unread badge, and four independent ways to turn it off.
// @kc-tags: notifications, sounds, alerts, approvals, unread, plugin notify, desktop
// @kc-applies-to: config:[notifications], config:notifications.enabled, config:notifications.run_finished, config:notifications.needs_input, config:notifications.error, config:notifications.info, config:notifications.plugin, config:notifications.min_run_secs, config:notifications.say_body, config:notifications.ephemeral_sessions, env:KARUCODE_NOTIFICATIONS, env:KARUCODE_NOTIFICATIONS_EVENTS, env:KARUCODE_NOTIFICATIONS_PLUGIN, env:KARUCODE_NOTIFICATIONS_MIN_RUN_SECS, env:KARUCODE_NOTIFICATIONS_SAY_BODY, env:KARUCODE_NOTIFICATIONS_EPHEMERAL_SESSIONS, route:GET /api/config, route:PATCH /api/config, route:GET /api/events, event:notification, method:notify, route:GET /api/update, ui:Settings > Updates, hook:on_notify, ui:SettingsSheet, ui:SessionsRail, ui:ToastRegion, ui:NotificationCenter, ui:ProjectRail, file:web/src/state/notifier.ts, file:web/src/state/notifications.test.ts, file:web/src/components/NotificationCenter.tsx, file:web/src/lib/notifySound.ts, file:web/src/lib/notifyDesktop.ts
//
// A long run is exactly the case where you stop watching. The agent works for
// minutes, you switch windows, and without a signal the only way to find out it
// finished is to come back and look. The same signal covers the opposite case: a
// run parked on an approval, which will sit there until it times out.
//
// Notifications are **on by default**. `min_run_secs = 0` means every run notifies,
// including a two-second question.
//
// # What notifies
//
// karucode has **runs**, not turns: one run is a prompt plus every iteration, tool
// call and auto-continue it spawns. Notifying per model turn would ping on every
// tool loop, so the signal is about states a human cares about.
//
// | Moment | kind | cue |
// | --- | --- | --- |
// | the run completed | `run_finished` | two rising notes |
// | an approval is pending | `needs_input` | double ping, and it stays on screen |
// | the run stopped at a configured limit | `needs_input` | double ping |
// | the run failed, or a restart interrupted it | `error` | low buzz |
// | a subagent failed | `error` | low buzz |
// | a plugin raised one | whatever it chose (`info` by default) | one tick for `info` |
// | a newer karucode build is available | `info` | one tick for `info` |
// | **you** cancelled the run | *nothing* | — |
// | a temporary (ephemeral) session did any of the above | *nothing* by default | — |
//
// A cancellation you asked for needs no ping: you pressed Escape. A cancellation
// nobody asked for (a shutdown) still notifies. And a temporary session is
// quiet by default: see `notifications.ephemeral_sessions` below.
//
// The update notice is raised once per build by the background update check, with
// id `update:<build>`, so a check every six hours does not repeat it. It obeys the
// same `enabled` / `info` switches as everything else (the server decides before
// emitting), and it never installs anything: it says a newer build exists and
// points at **Settings → Updates**. See the `updates` topic.
//
// # Four ways to turn it off
//
// They answer different questions, so all four exist.
//
// 1. **`enabled = false`** (or `KARUCODE_NOTIFICATIONS=0`) stops the server emitting
//    the `notification` event **at all**. Nothing downstream has to cooperate, and a
//    browser cannot accidentally ignore the switch.
// 2. **The per-event switches** keep run-finished pings and drop the rest.
// 3. **`plugin = false`** refuses anything a plugin raises, enforced in the plugin
//    host so a plugin cannot bypass it by sending anyway.
// 4. **This browser's preferences** (sound on/off, volume, desktop notifications)
//    in Settings → Notifications silence a *surface*, not the signal. They are
//    stored in `localStorage`, not on the server, because they describe the device.
//
// # Temporary sessions
//
// A temporary (ephemeral) session — one created with `"ephemeral": true` by an
// API client, or the helper session a plugin spawns — is **silent by default**:
// `notifications.ephemeral_sessions = false` (the shipped default). Every
// producer respects it: a run that finishes, an approval that parks, a failed
// subagent, and a plugin notice that names the session.
//
// The reasoning is about *what a notification is for*. A notification says "look
// at this session"; a temporary session retires itself the moment its work ends,
// so by the time you look, the Open button points at a row that no longer
// exists. And the client that created it (the program driving the API, the
// plugin that owns the helper) is already holding the answer it asked for — a
// ping is noise twice over: once for existing, once for arriving too late.
//
// Turn it on with `ephemeral_sessions = true`, `PATCH /api/config`
// (`{"notifications": {"ephemeral_sessions": true}}`), the **Temporary chats**
// toggle in Settings → Notifications, or `KARUCODE_NOTIFICATIONS_EPHEMERAL_SESSIONS=1`
// in the environment. The notify-frame rules above are unchanged; only the
// session's ephemerality decides.
//
// # Configuration
//
// ```toml
// [notifications]
// enabled = true             # master switch
// run_finished = true
// needs_input = true
// error = true
// info = true                # plugin info notifications
// plugin = true              # may plugins raise notifications at all
// min_run_secs = 0           # skip runs shorter than this
// say_body = true            # include the start of the answer, or title only
// ephemeral_sessions = false # may temporary (ephemeral) sessions notify
// ```
//
// The same settings are editable in the UI and through `PATCH /api/config`
// (`{"notifications": {"enabled": false}}`), and `GET /api/config` reports them
// under `notifications` along with `max_min_run_secs` and the list of `kinds`.
//
// Environment overrides, for a locked deployment:
//
// ```bash
// KARUCODE_NOTIFICATIONS=0                      # silence everything
// KARUCODE_NOTIFICATIONS_EVENTS=run_finished,needs_input
// KARUCODE_NOTIFICATIONS_EVENTS=error,!info     # a leading ! or - turns one off
// KARUCODE_NOTIFICATIONS_PLUGIN=0
// KARUCODE_NOTIFICATIONS_MIN_RUN_SECS=30
// KARUCODE_NOTIFICATIONS_SAY_BODY=0
// KARUCODE_NOTIFICATIONS_EPHEMERAL_SESSIONS=1  # let temporary chats notify
// ```
//
// An unknown event name in `KARUCODE_NOTIFICATIONS_EVENTS` fails at startup with
// the list it expected, rather than silently giving you everything or nothing.
//
// `min_run_secs` is capped at 3600 (an hour); past that the setting is a typo.
//
// # Sounds
//
// Each kind has its own cue, distinguishable by *shape* rather than only pitch so it
// is recognisable on a laptop speaker: a rise for finished, a repeat for needs-input,
// a dull low tone for an error, one tick for info.
//
// Two properties matter:
//
// * **The cue does not depend on the notification permission.** Browsers keep the
//   two completely separate: `Notification.permission` governs only the OS
//   notification, and the audio plays regardless. So a browser that has never been
//   asked — or that has refused — still makes a noise.
// * **There are no audio files.** The page's Content-Security-Policy has no
//   `media-src`, so it falls back to `default-src 'self'` and a `blob:` or `data:`
//   audio URL is refused by the browser (`NotSupportedError`). Each cue is therefore
//   synthesized with Web Audio: a couple of oscillators with an envelope, which also
//   means a test can render the cue and measure it.
//
// Browsers block audio until the page has been interacted with once. karucode resumes
// its audio context on your first click or keypress; if a cue is due before that, you
// get one hint toast rather than silence that looks like a bug.
//
// # Desktop notifications
//
// Best-effort, and off until you ask for it:
//
// * they need a **secure context** — `https:`, `http://localhost` or
//   `http://127.0.0.1`. A LAN address over plain HTTP cannot use the API at all, and
//   Settings says so instead of failing silently. karucode speaks plain HTTP and
//   expects TLS in front of it; see the `security` topic.
// * permission is requested **only** from the button in Settings, from that click.
//   Nothing prompts on page load, because Firefox (≥72) and Safari refuse a prompt
//   without a gesture.
// * they are shown only when the tab is hidden **or** the notification is about a
//   session you are not looking at. A run finishing in another project notifies even
//   with the tab in front — that is the case the feature exists for.
// * the OS sound is suppressed when karucode already played a cue, so one event never
//   makes two noises.
// * a notification replaces the previous one of the same kind rather than stacking,
//   and clicking it brings the window forward and opens that session.
// * most mobile browsers cannot show page-initiated notifications at all (they need a
//   service worker, which karucode does not register). There the in-app badge, the
//   title counter and the sound still work, and Settings says "limited".
//
// # In-app
//
// A toast names the session and offers **Open**: the click jumps there. A session you
// were not watching also gets an unread count in the rail, and the tab title carries
// the total (`(2) Fix the login redirect — karucode`), which is the only thing visible
// from another tab. Opening the session clears its count.
//
// Every notification is also kept in the **notification centre** — a list behind the
// bell in the chat header, newest first, with the unread ones marked and **Mark all
// read** / **Clear** above them. It exists because none of the other surfaces persist:
// a toast is gone in seconds, a desktop notification can be refused, and a dot on a
// session row says *that* something happened without saying what. Each entry names the
// session and the project, and opening one switches project first when the session
// lives somewhere else.
//
// The centre, the per-session dot, the count on a project's tile in the rail and the
// title counter are all derived from that one list, so they cannot disagree and marking
// something read clears all of them. The list is kept in this browser's `localStorage`
// (bounded to 100 entries and a week), because the server does not store notifications:
// they describe live state, and the server would have to grow a table to remember them.
//
// Each payload carries the id of the project its session belongs to (`project_id`,
// omitted when the server could not read the session). That is what makes a badge on a
// project you are *not* looking at possible: the browser only ever holds the open
// project's sessions, so it cannot work out the owner itself.
//
// # Plugins
//
// Plugins cannot reach a browser — no route mounting, no script execution — so a
// plugin asks for a human's attention over its own protocol with a `notify`
// notification frame:
//
// ```json
// {"jsonrpc":"2.0","method":"notify",
//  "params":{"kind":"needs_input","title":"Deploy needs a yes",
//            "body":"staging is approved, production is not","sticky":true}}
// ```
//
// `kind` is one of `run_finished`, `needs_input`, `error`, `info`; `title` is
// required; `body`, `session_id`, `sound` and `sticky` are optional. The host
// validates the frame — an unknown kind, an empty title or junk params are dropped,
// text is truncated, and a plugin raising more than one notification per second
// (burst of five) has the extra ones dropped — so a plugin cannot flood a desktop.
//
// From the SDK:
//
// ```rust,ignore
// use sdk::{notify, NotifyKind};
//
// notify(NotifyKind::NeedsInput, "Deploy needs a yes", "staging is approved")?;
// ```
//
// The other direction is the **`on_notify`** hook: a plugin that advertises it is
// called with `{session_id, run_id, kind, title, body, source}` for every
// notification the runtime emits, so a plugin can bridge karucode to ntfy, Pushover or
// a desktop daemon without karucode growing that integration. The call is
// fire-and-forget with a 5 s budget and never fails a run.
//
// ```rust,ignore
// PluginBuilder::new("bridge", "0.1.0")
//     .on_notify(|payload| { /* forward it somewhere */ serde_json::json!({}) })
//     .run()
// ```
//
// # What it will not do
//
// * A notification never contains HTML or Markdown, and `say_body = false` keeps model
//   output off a lock screen entirely. Errors are the exception: the error text is
//   karucode's own and always travels, because "Run failed" alone is not actionable.
// * A notification is not an approval channel. Nothing can be approved from one; the
//   approval card is still the only control.
// * A plugin cannot set a URL, an icon or an action on a notification.

use serde::{Deserialize, Serialize};

/// Maximum characters kept in [`Notify::title`].
///
/// Sized for a phone lock screen: two short lines. Longer text is truncated on a
/// character boundary, so a multi-byte codepoint is never split.
pub const MAX_NOTIFY_TITLE_CHARS: usize = 120;

/// Maximum characters kept in [`Notify::body`].
///
/// Roughly six lines. The body is meant to be the *first line of the answer* or an
/// approval's title — enough to decide whether to switch to the window, not enough
/// to be a second copy of the transcript.
pub const MAX_NOTIFY_BODY_CHARS: usize = 400;

/// Maximum characters kept in a plugin-supplied `sound` name.
pub const MAX_NOTIFY_SOUND_CHARS: usize = 32;

/// Why a human is being interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    /// The run settled successfully and the answer is ready.
    RunFinished,
    /// Something is waiting on a human: a pending approval, or a run that stopped
    /// because it reached a configured limit and may continue.
    NeedsInput,
    /// The run failed, was interrupted by a restart, or a subagent failed.
    Error,
    /// Neutral: a plugin chose to say something. The only kind with no built-in
    /// producer, so it can never be noisy on its own.
    Info,
}

impl NotifyKind {
    /// The wire spelling. Stable: it appears in SSE payloads, in the plugin
    /// protocol and in `[notifications]` event names.
    pub fn as_str(&self) -> &'static str {
        match self {
            NotifyKind::RunFinished => "run_finished",
            NotifyKind::NeedsInput => "needs_input",
            NotifyKind::Error => "error",
            NotifyKind::Info => "info",
        }
    }

    /// Parse a wire spelling. `None` for anything else, so an unknown value from a
    /// plugin or a settings row is refused rather than silently replaced.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "run_finished" | "run-finished" => Some(NotifyKind::RunFinished),
            "needs_input" | "needs-input" => Some(NotifyKind::NeedsInput),
            "error" => Some(NotifyKind::Error),
            "info" => Some(NotifyKind::Info),
            _ => None,
        }
    }

    /// Every kind, in the order the settings screen lists them.
    pub const ALL: &'static [NotifyKind] = &[
        NotifyKind::RunFinished,
        NotifyKind::NeedsInput,
        NotifyKind::Error,
        NotifyKind::Info,
    ];
}

/// Source string used for notifications the runtime emits itself.
///
/// A plugin-raised notification carries the plugin's name here instead, which is
/// what lets the UI say where it came from without a second lookup.
pub const RUNTIME_SOURCE: &str = "runtime";

/// One notification.
///
/// Constructed with [`Notify::new`], which applies the caps; the fields are public
/// so a caller can adjust `sound`/`sticky` afterwards without a builder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notify {
    /// Stable per source moment (`run:<run_id>`, `approval:<approval_id>`,
    /// `plugin:<name>:<n>`). The client dedupes on this, so a replayed frame after
    /// a reconnect cannot produce a second sound or a second OS notification.
    pub id: String,
    pub kind: NotifyKind,
    pub title: String,
    pub body: String,
    pub session_id: String,
    /// The session's title at emit time, so a notification is readable on its own
    /// ("Fix the login redirect" rather than "karucode").
    pub session_title: String,
    /// The project the session belongs to, so the UI can count unread news per
    /// project without asking who owns a session it has never listed.
    ///
    /// Empty when nothing named a session (a plugin notice that is not about one)
    /// or when the session could not be read; the client treats that as "unknown
    /// project" and shows the notification in the global list only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    /// [`RUNTIME_SOURCE`] or the name of the plugin that raised it.
    pub source: String,
    /// A cue override. `None` means "the kind's default cue". The UI owns the
    /// table, so an unknown name falls back to the kind's default rather than
    /// going silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// Keep the desktop notification on screen until it is dismissed. Set for
    /// approvals: a notification that disappears before it is read is worse than
    /// none.
    #[serde(default)]
    pub sticky: bool,
}

impl Notify {
    /// Build a notification, applying the documented caps.
    ///
    /// Every character-level field goes through [`truncate_chars`], and `title` is
    /// stripped of control characters: a newline in a title breaks the layout of
    /// some OS notification surfaces, and a `\r` can be used to overwrite text.
    pub fn new(
        kind: NotifyKind,
        id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
        session_id: impl Into<String>,
        session_title: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            id: truncate_chars(&id.into(), MAX_NOTIFY_TITLE_CHARS),
            kind,
            title: truncate_chars(&single_line(&title.into()), MAX_NOTIFY_TITLE_CHARS),
            body: truncate_chars(&body.into(), MAX_NOTIFY_BODY_CHARS),
            session_id: session_id.into(),
            session_title: truncate_chars(
                &single_line(&session_title.into()),
                MAX_NOTIFY_TITLE_CHARS,
            ),
            project_id: String::new(),
            source: truncate_chars(&source.into(), MAX_NOTIFY_TITLE_CHARS),
            sound: None,
            sticky: false,
        }
    }

    /// Name the project the notifying session belongs to.
    ///
    /// Separate from [`Notify::new`] so the seven required arguments stay as they
    /// were: attribution is a refinement, and a call site that does not know the
    /// project (a plugin describing no session at all) must still be able to build
    /// a notification.
    pub fn with_project(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = truncate_chars(project_id.into().trim(), MAX_NOTIFY_TITLE_CHARS);
        self
    }

    /// Set the cue override, capped. Over-long names are truncated rather than
    /// rejected: a wrong cue name is a cosmetic problem.
    pub fn with_sound(mut self, sound: Option<String>) -> Self {
        self.sound = sound.map(|s| truncate_chars(s.trim(), MAX_NOTIFY_SOUND_CHARS));
        self
    }

    /// Keep the OS notification on screen until dismissed.
    pub fn sticky(mut self, sticky: bool) -> Self {
        self.sticky = sticky;
        self
    }

    /// Whether this notification is about the given session.
    pub fn is_for(&self, session_id: &str) -> bool {
        self.session_id == session_id
    }
}

/// Truncate to `max` **characters** (not bytes), so a multi-byte codepoint is
/// never split in half.
pub fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    // Trim a trailing partial word so the cut reads as deliberate rather than
    // broken; only when there is whitespace to cut at.
    if let Some(index) = out.rfind(char::is_whitespace) {
        if index > max / 2 {
            out.truncate(index);
        }
    }
    out.push('…');
    out
}

/// Collapse every control character and newline into a single space and trim.
///
/// A title is one line by definition; `body` keeps its newlines (a toast renders
/// them), but the title and the session title are flattened.
fn single_line(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_space = false;
    for ch in value.chars() {
        if ch.is_control() || ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_refuse_unknown_values() {
        for kind in NotifyKind::ALL {
            assert_eq!(NotifyKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(
            NotifyKind::parse("needs-input"),
            Some(NotifyKind::NeedsInput)
        );
        assert_eq!(
            NotifyKind::parse(" RUN_FINISHED "),
            Some(NotifyKind::RunFinished)
        );
        assert_eq!(NotifyKind::parse("critical"), None);
        assert_eq!(NotifyKind::parse(""), None);
        // The wire spelling is what the docs and the plugin protocol promise.
        let json = serde_json::to_value(NotifyKind::NeedsInput).unwrap();
        assert_eq!(json, serde_json::json!("needs_input"));
    }

    #[test]
    fn a_notification_truncates_title_and_body_on_char_boundaries() {
        // Multi-byte characters: a byte-slice implementation would panic here.
        let title = "é".repeat(MAX_NOTIFY_TITLE_CHARS + 50);
        let body = "漢".repeat(MAX_NOTIFY_BODY_CHARS + 50);
        let notify = Notify::new(
            NotifyKind::Info,
            "id",
            title,
            body,
            "s_1",
            "session",
            RUNTIME_SOURCE,
        );
        assert_eq!(notify.title.chars().count(), MAX_NOTIFY_TITLE_CHARS + 1); // + ellipsis
        assert_eq!(notify.body.chars().count(), MAX_NOTIFY_BODY_CHARS + 1);
        assert!(notify.title.ends_with('…'));
    }

    #[test]
    fn a_short_value_is_left_alone() {
        let notify = Notify::new(
            NotifyKind::Info,
            "id",
            "Run finished",
            "All done.",
            "s_1",
            "Fix login",
            RUNTIME_SOURCE,
        );
        assert_eq!(notify.title, "Run finished");
        assert_eq!(notify.body, "All done.");
        assert!(!notify.title.contains('…'));
    }

    #[test]
    fn titles_are_flattened_to_one_line_but_bodies_keep_their_breaks() {
        let notify = Notify::new(
            NotifyKind::Info,
            "id",
            "Run\nfinished\r\u{0007}  now",
            "first line\nsecond line",
            "s_1",
            "Fix\nlogin",
            RUNTIME_SOURCE,
        );
        assert_eq!(notify.title, "Run finished now");
        assert_eq!(notify.session_title, "Fix login");
        // The body is allowed to keep line breaks: a toast draws them.
        assert_eq!(notify.body, "first line\nsecond line");
    }

    #[test]
    fn a_sound_name_is_capped_and_trimmed() {
        let notify = Notify::new(NotifyKind::Info, "id", "t", "b", "s_1", "s", RUNTIME_SOURCE)
            .with_sound(Some("  a-very-long-cue-name-that-goes-on-and-on  ".into()));
        let sound = notify.sound.unwrap();
        assert!(sound.chars().count() <= MAX_NOTIFY_SOUND_CHARS + 1);
        assert!(!sound.starts_with(' '));
    }

    #[test]
    fn a_project_id_is_optional_and_omitted_when_unknown() {
        // A notification about no session, or one whose session could not be read,
        // carries no project: the key is absent rather than an empty string, so a
        // client can tell "unknown" from a project whose id is somehow blank.
        let unattributed = Notify::new(NotifyKind::Info, "i", "t", "b", "", "", RUNTIME_SOURCE);
        let json = serde_json::to_value(&unattributed).unwrap();
        assert!(json.get("project_id").is_none());

        let attributed = unattributed.clone().with_project("  p_docs  ");
        assert_eq!(attributed.project_id, "p_docs");
        let json = serde_json::to_value(&attributed).unwrap();
        assert_eq!(json["project_id"], serde_json::json!("p_docs"));
        // A payload from before this field existed still parses.
        let back: Notify = serde_json::from_value(serde_json::to_value(&unattributed).unwrap())
            .expect("an older payload without project_id must parse");
        assert_eq!(back.project_id, "");
    }

    #[test]
    fn sticky_and_source_survive_a_round_trip() {
        let notify = Notify::new(
            NotifyKind::NeedsInput,
            "approval:a_1",
            "Approval needed",
            "write_file docs/plan.md",
            "s_1",
            "Plan the work",
            "kc-todo",
        )
        .sticky(true);
        let json = serde_json::to_value(&notify).unwrap();
        assert_eq!(json["kind"], serde_json::json!("needs_input"));
        assert_eq!(json["source"], serde_json::json!("kc-todo"));
        assert_eq!(json["sticky"], serde_json::json!(true));
        // `sound: None` is omitted rather than serialised as null.
        assert!(json.get("sound").is_none());
        let back: Notify = serde_json::from_value(json).unwrap();
        assert_eq!(back, notify);
        assert!(back.is_for("s_1"));
        assert!(!back.is_for("s_2"));
    }

    #[test]
    fn an_older_payload_without_sound_or_sticky_still_parses() {
        // Protocol versions later than this one may send more fields; a payload
        // that predates them may send fewer.
        let notify: Notify = serde_json::from_str(
            r#"{"id":"i","kind":"info","title":"t","body":"b",
                "session_id":"s","session_title":"st","source":"runtime"}"#,
        )
        .unwrap();
        assert_eq!(notify.sound, None);
        assert!(!notify.sticky);
    }

    #[test]
    fn truncation_does_not_split_a_word_when_it_can_avoid_it() {
        let text = format!("{} end", "word ".repeat(40));
        let cut = truncate_chars(&text, 30);
        assert!(cut.chars().count() <= 31);
        assert!(cut.ends_with('…'));
        // No dangling half word plus a space before the ellipsis.
        assert!(!cut.contains(" …"));
    }

    #[test]
    fn every_kind_is_listed_once_for_the_settings_screen() {
        let mut seen: Vec<&str> = NotifyKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(seen.len(), 4);
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "a kind is listed twice");
        // `info` is the only kind with no built-in producer.
        assert!(NotifyKind::ALL.contains(&NotifyKind::Info));
    }
}
