use crate::config::{ApprovalPolicy, Limits, ModelConfig, SessionModelSelection, SubagentConfig};
use async_trait::async_trait;

/// Live, possibly-mutable settings source. Implemented by the server over the
/// config file/env defaults plus per-install overrides persisted in SQLite, so
/// the runtime always reads current values instead of a startup snapshot.
#[async_trait]
pub trait Settings: Send + Sync {
    async fn model(&self) -> ModelConfig;
    /// Effective model for a session given its per-session selection: the
    /// install-wide default when `selection` is `None`, otherwise the selection
    /// resolved against the currently configured profiles. The default
    /// implementation ignores the selection, so simple embeddings and tests keep
    /// the single-model behaviour.
    async fn resolve_model(&self, selection: Option<&SessionModelSelection>) -> ModelConfig {
        let _ = selection;
        self.model().await
    }
    async fn limits(&self) -> Limits;
    async fn approval_policy(&self) -> ApprovalPolicy;
    async fn subagent_config(&self) -> SubagentConfig;
}
