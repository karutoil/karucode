use crate::config::{ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
use async_trait::async_trait;

/// Live, possibly-mutable settings source. Implemented by the server over the
/// config file/env defaults plus per-install overrides persisted in SQLite, so
/// the runtime always reads current values instead of a startup snapshot.
#[async_trait]
pub trait Settings: Send + Sync {
    async fn model(&self) -> ModelConfig;
    async fn limits(&self) -> Limits;
    async fn approval_policy(&self) -> ApprovalPolicy;
    async fn subagent_config(&self) -> SubagentConfig;
}
