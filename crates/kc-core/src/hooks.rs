use async_trait::async_trait;

/// Lifecycle hooks implemented by the plugin host (and by tests). Kept in core
/// so the runtime never depends on the plugin host directly.
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Text blocks appended to the system prompt for this session.
    async fn before_prompt(&self, session_id: &str, workspace: &str) -> Vec<String> {
        let _ = (session_id, workspace);
        Vec::new()
    }

    async fn on_run_end(&self, session_id: &str, run_id: &str, status: &str) {
        let _ = (session_id, run_id, status);
    }

    async fn on_tool_result(&self, session_id: &str, tool: &str, ok: bool) {
        let _ = (session_id, tool, ok);
    }
}

pub struct NoHooks;

#[async_trait]
impl Hooks for NoHooks {}
