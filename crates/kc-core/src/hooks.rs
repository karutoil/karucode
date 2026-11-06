use async_trait::async_trait;
use serde_json::Value;

/// Lifecycle hooks implemented by the plugin host (and by tests). Kept in core
/// so the runtime never depends on the plugin host directly.
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Text blocks appended to the system prompt for this session.
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        let _ = (session_id, workspace);
        Vec::new()
    }

    /// Text blocks appended to the system prompt, with the user turn being
    /// answered.
    ///
    /// `turn` is the text of the user message this run is answering, or `None`
    /// when the run was started without one. It exists so a source can retrieve
    /// *against the question* instead of memoising one block per session: a
    /// knowledge-graph recall that cannot see the turn can only ever return the
    /// same static notes.
    ///
    /// Defaults to [`Hooks::before_prompt`], so a source that does not care -
    /// and every existing implementation - behaves exactly as before. Callers
    /// call this method, not `before_prompt`, so an override here is honoured
    /// without changing the older signature.
    async fn before_prompt_for_turn(
        &self,
        session_id: &str,
        workspace: &str,
        turn: Option<&str>,
    ) -> Vec<String> {
        let _ = turn;
        self.before_prompt(session_id, workspace).await
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        let _ = (session_id, run_id, status);
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        let _ = (session_id, tool, ok);
    }

    /// The notification observer, if this source can observe notifications.
    ///
    /// `on_notify` lives on its own trait rather than on [`Hooks`] because it is
    /// the one hook the runtime calls **after** the run has already finished, from
    /// a spawned task: the caller must be able to hold the observer without
    /// holding the hook source, and a source that does not implement it costs
    /// nothing at the call site.
    ///
    /// `None` (the default) means "nothing observes notifications", which is what
    /// every test embedding and every non-plugin source wants.
    fn notifier(&self) -> Option<std::sync::Arc<dyn NotificationObserver>> {
        None
    }
}

/// Something that wants to hear about every notification the runtime emits.
///
/// Implemented by the plugin host, which fans the call out to plugins that
/// advertise `on_notify`. This is the "out" half of the plugin notification story:
/// plugins raise notifications over `notify`, and observe everyone else's through
/// this hook, so a plugin can bridge karucode to ntfy, Pushover, a desktop notifier
/// or anything else without karucode growing those integrations.
#[async_trait]
pub trait NotificationObserver: Send + Sync {
    /// Called once per emitted notification with
    /// `{session_id, run_id, kind, title, body, source}`.
    ///
    /// The runtime spawns this and does not wait for it: a plugin that hangs must
    /// not delay a run, and its 5 s budget is the plugin host's problem.
    async fn on_notify(&self, payload: Value);
}

pub struct NoHooks;

#[async_trait]
impl Hooks for NoHooks {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hook_source_observes_no_notifications() {
        // Registering a listener must be opt-in: every existing embedding keeps
        // compiling and keeps costing nothing.
        assert!(NoHooks.notifier().is_none());
    }
}
