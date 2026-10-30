//! Bridging plugin-raised notifications onto the event stream.
//!
//! A plugin cannot reach a browser: there is no route mounting, no asset serving
//! and no script execution, which is the same property that keeps a panel data-
//! only. So the only way a plugin can ask a human to look at something is to say
//! so over its own protocol (`{"method":"notify"}`) and let the *server* publish it.
//!
//! This module is that bridge, and it is deliberately thin:
//!
//! 1. [`spawn`] drains the host's notification channel.
//! 2. Each notice is checked against the live `[notifications]` config — the same
//!    `notifications.plugin` switch that guards the hook path, so a plugin cannot
//!    bypass a disabled switch by sending frames anyway.
//! 3. An accepted notice becomes an [`Event::Notification`] with `source` set to
//!    the plugin's name, published through `Runtime::emit` so it shares the one
//!    monotonic sequence, the replay ring and the lag marker with everything else.
//!
//! One thing it must never do: fail, block or panic. It runs forever in its own
//! task, and a bad notice is logged and skipped.

use crate::state::AppState;
use kc_core::events::Event;
use kc_core::notify::{Notify, RUNTIME_SOURCE};
use kc_core::settings::Settings as _;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, warn};

/// Drain plugin notifications into the event stream for the life of the process.
///
/// The task stops when the channel closes (every plugin slot dropped) or the
/// runtime shuts down, whichever happens first.
pub fn spawn(state: AppState, mut rx: UnboundedReceiver<kc_plugin_host::PluginNotice>) {
    tokio::spawn(async move {
        while let Some(notice) = rx.recv().await {
            // Read the config per notice rather than once: the switch can be
            // flipped from the settings screen while the server is running.
            let config = state.settings().notify_config().await;
            if !config.allows_plugin(notice.kind) {
                debug!(
                    plugin = %notice.plugin,
                    kind = notice.kind.as_str(),
                    "plugin notification refused by [notifications]"
                );
                continue;
            }
            let session_id = if notice.session_id.is_empty() {
                // A notice not about a particular session still has to be
                // attributable. The UI treats an empty session as "no jump target"
                // rather than inventing one.
                String::new()
            } else {
                notice.session_id.clone()
            };
            let title = session_title(&state, &session_id).await;
            let notification = Notify::new(
                notice.kind,
                // `<plugin>:<title>` rather than a counter: a plugin that raises the
                // same notification twice is a duplicate, not two events, and the
                // client dedupes on this id.
                format!("plugin:{}:{}", notice.plugin, notice.title),
                notice.title,
                notice.body,
                session_id,
                title,
                notice.plugin,
            )
            .with_sound(notice.sound)
            .sticky(notice.sticky);

            // `source` is the plugin's name, so the UI can say where it came from.
            debug_assert_ne!(notification.source, RUNTIME_SOURCE);
            let session_for_envelope =
                (!notification.session_id.is_empty()).then(|| notification.session_id.clone());
            state.runtime().emit(
                session_for_envelope.as_deref(),
                None,
                None,
                Event::Notification { notification },
            );
        }
        debug!("plugin notification channel closed");
    });
}

/// The session title for a notice, when it named a session.
///
/// A lookup failure is not an error: the notification is still worth showing, and
/// the client falls back to the plugin's own title.
async fn session_title(state: &AppState, session_id: &str) -> String {
    if session_id.is_empty() {
        return String::new();
    }
    match state.store().get_session(session_id).await {
        Ok(Some(session)) => session.title,
        Ok(None) => {
            warn!(session = %session_id, "a plugin notified about a session that does not exist");
            String::new()
        }
        Err(e) => {
            warn!(session = %session_id, error = %e, "could not read a session title for a plugin notification");
            String::new()
        }
    }
}
