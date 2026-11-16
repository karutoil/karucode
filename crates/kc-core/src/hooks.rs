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

    /// The gate before a tool call runs: block it, or rewrite its arguments.
    ///
    /// This is the one hook that can *change* what the agent does rather than
    /// observe it, and it is deliberately narrow:
    ///
    /// * a block becomes the tool's result - the tool never runs, and the model
    ///   is told why, so it can adapt instead of retrying blindly;
    /// * a rewrite replaces the arguments **before** the harness validates them
    ///   against the tool's own JSON Schema, so a plugin cannot smuggle a shape
    ///   the tool did not agree to.
    ///
    /// A gate can never *widen* permission: it may say "ask the person" where the
    /// policy would have allowed a call, and never the reverse. The approval
    /// policy remains the security gate; this is a policy hook for plugin
    /// authoring, not a way around the human.
    ///
    /// The default allows everything, so every existing embedding and every
    /// plugin that does not advertise `on_tool_call` behaves exactly as before.
    async fn on_tool_call(&self, _call: &ToolCallGate) -> GateDecision {
        GateDecision::Allow
    }

    /// The gate after a tool ran, before the model sees the result.
    ///
    /// `output` is what the tool produced, before truncation, so a replacement is
    /// bounded by the same budget the real output would have hit. A plugin that
    /// returns `None` changes nothing.
    async fn on_tool_result_gate(&self, _result: &ToolResultGate) -> Option<String> {
        None
    }

    /// Text to append to the system prompt for one model request.
    ///
    /// The assembled system prompt is passed in and is **not** modifiable: a
    /// replacement would let a plugin erase the harness's own rules, including the
    /// statement that shell commands are unsandboxed. Appending is enough for
    /// retrieval, a house style, or "this project is on freeze".
    async fn on_model_request(&self, _request: &ModelRequestGate) -> Option<String> {
        None
    }

    /// Asked when a run is about to end: may it stop, or is something unfinished?
    ///
    /// `on_stop` is what makes a workflow plugin possible - a session that is
    /// still `open` can ask for one more turn instead of going quiet. The runtime
    /// bounds it: `limits.max_stop_continuations` (3 by default) per run, and a
    /// `continue` that would exceed it ends the run with a notice rather than
    /// looping.
    async fn on_stop(&self, _stop: &StopGate) -> Option<StopDecision> {
        None
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

/// What a plugin may do about a tool call it was shown, before it runs.
///
/// The three answers map to the only three safe things a policy hook can do:
/// let it through, stop it with a reason the model can read, or change the
/// arguments to something the harness will validate again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Run the call as the model wrote it.
    Allow,
    /// Do not run it. `reason` becomes the tool result, so the model learns why.
    Block { reason: String },
    /// Run a different call: the harness re-validates `arguments` against the
    /// tool's JSON Schema before anything is approved or executed.
    Rewrite { arguments: serde_json::Value },
}

/// One tool call, as the `on_tool_call` gate sees it.
///
/// Everything here is already the harness's own view of the call: the tool name,
/// its class (so a plugin can gate `shell` without pattern-matching a name), and
/// the parsed arguments.
#[derive(Debug, Clone)]
pub struct ToolCallGate {
    pub session_id: String,
    pub run_id: String,
    pub tool: String,
    pub class: String,
    pub origin: String,
    pub arguments: serde_json::Value,
}

/// One finished tool result, as the `on_tool_result` gate sees it.
#[derive(Debug, Clone)]
pub struct ToolResultGate {
    pub session_id: String,
    pub run_id: String,
    pub tool: String,
    pub ok: bool,
    pub output: String,
    pub truncated: bool,
}

/// One model request, as the `on_model_request` gate sees it.
#[derive(Debug, Clone)]
pub struct ModelRequestGate {
    pub session_id: String,
    pub run_id: String,
    /// The assembled system prompt. Read-only by design: a plugin may append
    /// through [`Hooks::on_model_request`] but may not remove anything.
    pub system: String,
    /// How many messages the request carries.
    pub messages: usize,
    /// Tool names exposed for this request.
    pub tools: Vec<String>,
}

/// One run that is about to end, as the `on_stop` gate sees it.
#[derive(Debug, Clone)]
pub struct StopGate {
    pub session_id: String,
    pub run_id: String,
    /// The status the run would be persisted with if it stopped now.
    pub status: String,
    /// How many times a plugin has already kept this run going.
    pub continuations: u32,
    pub max_continuations: u32,
}

/// A plugin's answer to [`StopGate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopDecision {
    /// Keep the run alive for one more turn.
    pub keep_going: bool,
    /// The note the next turn starts with. Bounded by the caller.
    pub reason: String,
}

impl StopDecision {
    pub fn stop() -> Self {
        Self {
            keep_going: false,
            reason: String::new(),
        }
    }
    pub fn keep_going(reason: impl Into<String>) -> Self {
        Self {
            keep_going: true,
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hook_source_observes_no_notifications() {
        // Registering a listener must be opt-in: every existing embedding keeps
        // compiling and keeps costing nothing.
        assert!(NoHooks.notifier().is_none());
    }

    /// The gate defaults are what keep the new hooks additive: a source that
    /// implements none of them must behave exactly like every source written
    /// before they existed.
    ///
    /// `kc-core` deliberately has no async runtime as a dependency, so the
    /// futures are polled once here: every default body returns immediately, and a
    /// `Pending` would itself be the bug this test exists to catch.
    #[test]
    fn the_default_gates_change_nothing() {
        use std::future::Future;
        use std::pin::pin;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        fn now<F: Future>(fut: F) -> F::Output {
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                RawWaker::new(std::ptr::null(), &VTABLE)
            }
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
            // SAFETY: the vtable's functions never dereference the null data
            // pointer, which is the whole contract of a no-op waker.
            let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
            let mut cx = Context::from_waker(&waker);
            let mut fut = pin!(fut);
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(value) => value,
                Poll::Pending => panic!("a default gate hook must not wait on anything"),
            }
        }

        let call = ToolCallGate {
            session_id: "s".into(),
            run_id: "r".into(),
            tool: "run_command".into(),
            class: "shell".into(),
            origin: "builtin".into(),
            arguments: serde_json::json!({"command": "rm -rf /"}),
        };
        assert_eq!(now(NoHooks.on_tool_call(&call)), GateDecision::Allow);

        let result = ToolResultGate {
            session_id: "s".into(),
            run_id: "r".into(),
            tool: "read_file".into(),
            ok: true,
            output: "text".into(),
            truncated: false,
        };
        assert!(now(NoHooks.on_tool_result_gate(&result)).is_none());

        let request = ModelRequestGate {
            session_id: "s".into(),
            run_id: "r".into(),
            system: "rules".into(),
            messages: 2,
            tools: vec!["read_file".into()],
        };
        assert!(now(NoHooks.on_model_request(&request)).is_none());

        let stop = StopGate {
            session_id: "s".into(),
            run_id: "r".into(),
            status: "completed".into(),
            continuations: 0,
            max_continuations: 3,
        };
        assert!(now(NoHooks.on_stop(&stop)).is_none());
    }
}
