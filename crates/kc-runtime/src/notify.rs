//! Turning runtime moments into notifications.
//!
//! Three things decide whether a notification is emitted, and they are checked in
//! this order at the moment the event would go out — never at startup, so a
//! settings change takes effect on the next run:
//!
//! 1. **Config.** `[notifications] enabled` and the per-kind switches, read from
//!    the live settings source. A disabled server emits nothing at all.
//! 2. **Length.** `min_run_secs` drops a ping for a two-second question.
//! 3. **The moment.** Only *terminal* states notify. A run parked on an approval
//!    notifies when it parks; a run that the user cancelled does not (they pressed
//!    Escape, they know).
//!
//! Nothing here blocks a run: the payload is built from values already in hand, and
//! `emit` is the same fire-and-forget broadcast the rest of the loop uses. The
//! `on_notify` plugin fan-out that follows it is spawned, not awaited.
//!
//! ## Why the runtime, not the browser
//!
//! A browser only sees the session it has open. A run that finishes in another
//! project, in another tab, or while the window is behind something else would
//! never produce a notification — which is exactly the case this feature exists
//! for. The server reports every terminal run; the UI decides which ones are worth
//! interrupting you for.

use crate::events::EmitCtx;
use crate::RuntimeInner;
use kc_core::config::NotificationsConfig;
use kc_core::events::Event;
use kc_core::notify::{Notify, NotifyKind, RUNTIME_SOURCE};
use kc_core::store::RunStatus;
use std::sync::Arc;

/// How much of the answer is offered as the notification body.
///
/// `Notify::new` caps the field at [`kc_core::notify::MAX_NOTIFY_BODY_CHARS`]
/// anyway; this is the narrower "first line of the answer" slice, because a body
/// that starts mid-paragraph is worse than a short one.
const BODY_PREVIEW_CHARS: usize = 200;

/// What the runtime knows about a finished run.
pub(crate) struct RunFinished<'a> {
    pub run_id: &'a str,
    pub status: RunStatus,
    pub error: Option<&'a str>,
    /// The last assistant text the loop produced, used for the body.
    pub answer: &'a str,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    /// Whether the human asked for this run to stop. A user's own cancellation is
    /// never worth a notification.
    pub cancelled_by_user: bool,
}

/// Emit the notification for a terminal run, if the configuration wants one.
///
/// Returns the notification that was emitted, so a caller (and a test) can assert
/// on it without subscribing to the broadcast channel.
pub(crate) async fn run_finished(
    inner: &Arc<RuntimeInner>,
    ctx: &EmitCtx,
    session_title: &str,
    run: RunFinished<'_>,
) -> Option<Notify> {
    let config = inner.deps.settings.notify_config().await;
    if !config.enabled {
        return None;
    }

    let kind = kind_for(run.status, run.cancelled_by_user)?;
    if !config.allows(kind) {
        return None;
    }
    let elapsed_secs = ((run.finished_at_ms - run.started_at_ms).max(0) / 1000) as u64;
    if !config.run_is_long_enough(elapsed_secs) {
        return None;
    }

    let title = title_for(kind);
    let body = body_for(&config, &run, session_title);
    let notification = Notify::new(
        kind,
        format!("run:{}", run.run_id),
        title,
        body,
        ctx.session_id.clone(),
        session_title,
        RUNTIME_SOURCE,
    )
    .sticky(kind == NotifyKind::NeedsInput);
    emit(inner, ctx, notification)
}

/// Emit the notification for a pending approval.
///
/// Called only for the top-level (app) approval card: a subagent's approval is
/// raised *through* its parent's card, and notifying twice for one card would be
/// a bug rather than a feature.
pub(crate) async fn approval_requested(
    inner: &Arc<RuntimeInner>,
    ctx: &EmitCtx,
    approval_id: &str,
    approval_title: &str,
    approval_detail: &str,
) -> Option<Notify> {
    let config = inner.deps.settings.notify_config().await;
    if !config.allows(NotifyKind::NeedsInput) {
        return None;
    }
    let title = session_title(inner, &ctx.session_id).await;
    // The approval's own title is the most useful thing to show ("Write a file"),
    // and its detail names the target. Together they are what the card says.
    let body = if approval_detail.trim().is_empty() {
        approval_title.to_string()
    } else {
        format!("{approval_title}: {approval_detail}")
    };
    let notification = Notify::new(
        NotifyKind::NeedsInput,
        format!("approval:{approval_id}"),
        "Approval needed",
        body,
        ctx.session_id.clone(),
        title,
        RUNTIME_SOURCE,
    )
    // An approval that scrolls away before it is read is worse than no
    // notification: it also has a timeout, after which it silently expires.
    .sticky(true);
    emit(inner, ctx, notification)
}

/// Emit the notification for a subagent that failed.
///
/// A subagent that succeeded is noise — the parent run reports its own completion.
///
/// Takes the session id and the event sink rather than an [`EmitCtx`]: a subagent
/// runs off a [`kc_core::tool::ToolCtx`], which carries the parent session's sink
/// and no run context of its own.
pub(crate) async fn agent_failed(
    inner: &Arc<RuntimeInner>,
    session_id: &str,
    events: &kc_core::events::EventSink,
    agent_id: &str,
    agent_name: &str,
    error: Option<&str>,
) -> Option<Notify> {
    let config = inner.deps.settings.notify_config().await;
    if !config.allows(NotifyKind::Error) {
        return None;
    }
    let notification = Notify::new(
        NotifyKind::Error,
        format!("agent:{agent_id}"),
        format!("Subagent failed: {agent_name}"),
        error.unwrap_or("The subagent stopped without a reason."),
        session_id,
        session_title(inner, session_id).await,
        RUNTIME_SOURCE,
    );
    Some(emit_with(inner, session_id, None, events, notification))
}

/// The session's title, for the notification's `session_title` field.
///
/// Read at emit time rather than carried around: a session can be renamed while a
/// run is in flight, and the current name is the one worth showing. A missing row
/// is not an error — the run works — so an empty title falls back to the caller's
/// wording.
async fn session_title(inner: &Arc<RuntimeInner>, session_id: &str) -> String {
    inner
        .deps
        .store
        .get_session(session_id)
        .await
        .ok()
        .flatten()
        .map(|session| session.title)
        .unwrap_or_default()
}

/// Publish one notification and hand it to the plugin `on_notify` hooks.
///
/// The hook fan-out is spawned on the current runtime so a slow plugin can never
/// delay the end of a run — the same rule `[titles]` follows.
fn emit(inner: &Arc<RuntimeInner>, ctx: &EmitCtx, notification: Notify) -> Option<Notify> {
    let sink = ctx.sink();
    Some(emit_with(
        inner,
        &ctx.session_id,
        Some(&ctx.run_id),
        &sink,
        notification,
    ))
}

/// The shared tail of every notification: publish it and hand it to `on_notify`.
fn emit_with(
    inner: &Arc<RuntimeInner>,
    session_id: &str,
    run_id: Option<&str>,
    events: &kc_core::events::EventSink,
    notification: Notify,
) -> Notify {
    let payload = serde_json::json!({
        "session_id": session_id,
        "run_id": run_id,
        "kind": notification.kind.as_str(),
        "title": notification.title,
        "body": notification.body,
        "source": notification.source,
    });
    events(Event::Notification {
        notification: notification.clone(),
    });
    let hooks = Arc::clone(&inner.deps.hooks);
    tokio::spawn(async move {
        if let Some(notifier) = hooks.notifier() {
            notifier.on_notify(payload).await;
        }
    });
    notification
}

/// The kind a terminal status maps to, or `None` for "do not notify".
///
/// Deliberately exhaustive over [`RunStatus`]: adding a status should force a
/// decision here rather than silently defaulting to a ping.
fn kind_for(status: RunStatus, cancelled_by_user: bool) -> Option<NotifyKind> {
    match status {
        RunStatus::Completed => Some(NotifyKind::RunFinished),
        RunStatus::Failed => Some(NotifyKind::Error),
        // A restart interrupted it: the user was not watching and needs to know
        // their work stopped.
        RunStatus::Interrupted => Some(NotifyKind::Error),
        // It wants to continue and is not allowed to. That is a request for input.
        RunStatus::LimitReached => Some(NotifyKind::NeedsInput),
        // Cancelled by the user: they know, they pressed the button. A cancellation
        // they did not ask for (shutdown, timeout) is not representable as
        // `Cancelled` here — the callers pass `Interrupted` or `Failed` for those.
        RunStatus::Cancelled => {
            if cancelled_by_user {
                None
            } else {
                Some(NotifyKind::NeedsInput)
            }
        }
        RunStatus::Running => None,
    }
}

fn title_for(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::RunFinished => "Run finished",
        NotifyKind::NeedsInput => "Waiting for you",
        NotifyKind::Error => "Run failed",
        NotifyKind::Info => "karucode",
    }
}

/// The body: the first line of the answer when `say_body` is on, and a status
/// sentence otherwise.
///
/// A failure always names the error, whether or not `say_body` is set: a
/// notification that says only "Run failed" is not actionable, and the error text
/// is karucode's own, not model output.
fn body_for(config: &NotificationsConfig, run: &RunFinished<'_>, session_title: &str) -> String {
    let subject = if session_title.trim().is_empty() {
        "this session".to_string()
    } else {
        format!("“{session_title}”")
    };
    if let Some(error) = run.error.map(str::trim).filter(|e| !e.is_empty()) {
        return first_line(error, BODY_PREVIEW_CHARS);
    }
    match run.status {
        RunStatus::LimitReached => {
            format!("{subject} stopped at a configured limit and can continue.")
        }
        RunStatus::Cancelled => format!("{subject} was cancelled."),
        _ if !config.say_body => String::new(),
        _ => {
            let answer = first_line(run.answer, BODY_PREVIEW_CHARS);
            if answer.is_empty() {
                format!("{subject} is done.")
            } else {
                answer
            }
        }
    }
}

/// The first non-empty line, truncated to `max` characters.
///
/// Model output starts with whatever it starts with — often a code fence or a
/// blank line — so leading whitespace is skipped rather than rendered.
fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    kc_core::notify::truncate_chars(line, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_runs_notify_and_user_cancellations_do_not() {
        assert_eq!(
            kind_for(RunStatus::Completed, false),
            Some(NotifyKind::RunFinished)
        );
        assert_eq!(kind_for(RunStatus::Cancelled, true), None);
        // A cancellation nobody asked for still deserves attention.
        assert_eq!(
            kind_for(RunStatus::Cancelled, false),
            Some(NotifyKind::NeedsInput)
        );
    }

    #[test]
    fn a_limit_reached_run_asks_for_input_rather_than_reporting_an_error() {
        // This is the distinction that makes the kind useful: the run did not fail,
        // it is waiting to be allowed to continue.
        assert_eq!(
            kind_for(RunStatus::LimitReached, false),
            Some(NotifyKind::NeedsInput)
        );
    }

    #[test]
    fn failures_and_interruptions_are_errors_and_a_running_status_is_silent() {
        assert_eq!(kind_for(RunStatus::Failed, false), Some(NotifyKind::Error));
        assert_eq!(
            kind_for(RunStatus::Interrupted, false),
            Some(NotifyKind::Error)
        );
        assert_eq!(kind_for(RunStatus::Running, false), None);
    }

    #[test]
    fn the_body_prefers_the_first_answer_line_and_skips_leading_blanks() {
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::Completed,
            error: None,
            answer: "\n\n   Here is what I changed.\nAnd some more detail.\n",
            started_at_ms: 0,
            finished_at_ms: 5_000,
            cancelled_by_user: false,
        };
        let config = NotificationsConfig::default();
        assert_eq!(
            body_for(&config, &run, "Fix the bug"),
            "Here is what I changed."
        );
    }

    #[test]
    fn say_body_off_leaves_the_body_empty_for_a_plain_success() {
        // The point of turning it off is to keep model output off a lock screen.
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::Completed,
            error: None,
            answer: "secret-looking model output",
            started_at_ms: 0,
            finished_at_ms: 1_000,
            cancelled_by_user: false,
        };
        let config = NotificationsConfig {
            say_body: false,
            ..NotificationsConfig::default()
        };
        let body = body_for(&config, &run, "S");
        assert_eq!(body, "");
        assert!(!body.contains("secret"));
    }

    #[test]
    fn a_failure_always_carries_the_error_even_when_say_body_is_off() {
        // "Run failed" with no reason is not actionable, and an error string is
        // karucode's own text rather than model output.
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::Failed,
            error: Some("the provider returned 401"),
            answer: "",
            started_at_ms: 0,
            finished_at_ms: 1_000,
            cancelled_by_user: false,
        };
        let config = NotificationsConfig {
            say_body: false,
            ..NotificationsConfig::default()
        };
        assert_eq!(body_for(&config, &run, "S"), "the provider returned 401");
    }

    #[test]
    fn an_empty_answer_falls_back_to_a_sentence_that_names_the_session() {
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::Completed,
            error: None,
            answer: "   \n\n ",
            started_at_ms: 0,
            finished_at_ms: 1_000,
            cancelled_by_user: false,
        };
        let config = NotificationsConfig::default();
        assert_eq!(
            body_for(&config, &run, "Refactor the parser"),
            "“Refactor the parser” is done."
        );
        // A session with no title at all still reads as a sentence.
        assert_eq!(body_for(&config, &run, "  "), "this session is done.");
    }

    #[test]
    fn a_limit_reached_body_explains_what_would_happen_next() {
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::LimitReached,
            error: None,
            answer: "half an answer",
            started_at_ms: 0,
            finished_at_ms: 1_000,
            cancelled_by_user: false,
        };
        let body = body_for(&NotificationsConfig::default(), &run, "S");
        assert!(body.contains("limit"), "{body}");
        assert!(body.contains("continue"), "{body}");
    }

    #[test]
    fn a_long_first_line_is_truncated_rather_than_sent_whole() {
        let long = "x".repeat(BODY_PREVIEW_CHARS * 3);
        let run = RunFinished {
            run_id: "r_1",
            status: RunStatus::Completed,
            error: None,
            answer: &long,
            started_at_ms: 0,
            finished_at_ms: 1_000,
            cancelled_by_user: false,
        };
        let body = body_for(&NotificationsConfig::default(), &run, "S");
        assert!(body.chars().count() <= BODY_PREVIEW_CHARS + 1);
    }

    #[test]
    fn titles_come_from_the_kind_not_the_status() {
        assert_eq!(title_for(NotifyKind::RunFinished), "Run finished");
        assert_eq!(title_for(NotifyKind::NeedsInput), "Waiting for you");
        assert_eq!(title_for(NotifyKind::Error), "Run failed");
    }
}
