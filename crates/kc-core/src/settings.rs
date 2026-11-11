use crate::config::{
    ApprovalPolicy, Limits, ModelConfig, NotificationsConfig, SessionModelSelection, SubagentConfig,
};
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

    /// The model ids this install can actually send, for validating an id that
    /// did not come from the config: the `model` argument of
    /// `spawn_agent`/`spawn_agents`, which the caller types and nothing else
    /// checks.
    ///
    /// `selection` is the session's pin, because a pinned session resolves
    /// against *its* profile rather than the install-wide one. The default is
    /// empty, which means "cannot judge, refuse nothing" - an embedding or a
    /// test that keeps the single-model behaviour must not start failing spawns
    /// because it has no catalog to check against.
    async fn known_models(&self, selection: Option<&SessionModelSelection>) -> Vec<String> {
        let _ = selection;
        Vec::new()
    }

    /// Live `[notifications]` settings.
    ///
    /// Defaulted to "everything on" so the test and embedding implementations that
    /// existed before notifications keep compiling unchanged; the server overrides
    /// it with the real, live values. The runtime reads this *at the moment it
    /// would emit*, so flipping the switch takes effect on the next run rather
    /// than at the next restart.
    async fn notify_config(&self) -> NotificationsConfig {
        NotificationsConfig::default()
    }
}
