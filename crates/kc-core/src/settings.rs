use crate::config::{
    ApprovalPolicy, Limits, ModelConfig, ModelOverrideJudgement, NotificationsConfig,
    SessionModelSelection, SubagentConfig,
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

    /// Change the install-wide `subagents.worktrees` default, on behalf of the
    /// top-level agent (`subagent_worktrees` with `scope: "global"`).
    ///
    /// A *write* is deliberately not part of the read surface this trait was
    /// built for, so it is defaulted: an embedding or a test that has no settings
    /// persistence must not silently accept a change it cannot keep. The server
    /// implements it over the same validated, persisted update
    /// `PATCH /api/config` uses, which means `lock_settings` refuses it too and a
    /// failed write changes nothing.
    async fn set_subagent_worktrees(&self, on: bool) -> Result<(), String> {
        let _ = on;
        Err(
            "this deployment cannot change install settings from a tool; \
             change `subagents.worktrees` in the config file, or use \
             `scope: \"session\"` to change only this conversation"
                .into(),
        )
    }

    /// Live `[skills]` settings: the switches, the hand-added skills and the ids
    /// the user turned off.
    ///
    /// The runtime and the `skill` tool both resolve skills through this, so a
    /// skill added in the UI is usable on the very next run without a restart. A
    /// simple embedding or a test that does not care keeps the empty default,
    /// which is "skills on, nothing found".
    async fn skills(&self) -> crate::skill::SkillsConfig {
        crate::skill::SkillsConfig::default()
    }

    /// Live `[prompt]` settings: how this install lets the system prompt be
    /// changed.
    ///
    /// Defaulted to "nothing changes", so an embedding or a test written before
    /// the section existed keeps the exact prompt it had. The runtime reads it
    /// once per run, because the text is `message[0]` of every request in that
    /// run and re-reading it per turn would move the cached prefix.
    async fn prompt(&self) -> crate::config::PromptConfig {
        crate::config::PromptConfig::default()
    }

    /// Resolve the `model` argument of `spawn_agent`/`spawn_agents` - one id
    /// that came from *outside* the config, typed by the caller and derived
    /// by nothing - against the models this install actually made available.
    ///
    /// A valid id is one we created and made available in karucode: a model
    /// **any** profile declares resolves onto that profile's endpoint and
    /// per-model metadata, even when it is not the session's own profile.
    /// `selection` is the session's pin, so the id a pinned profile actually
    /// sends is recognized too, and the historical pass-through on installs
    /// where nobody declared a catalog keeps working. The default returns
    /// [`ModelOverrideJudgement::Unjudged`] ("cannot judge, refuse nothing"),
    /// so an embedding or a test that keeps the single-model behaviour must
    /// not start failing spawns because it has no catalog to judge against.
    async fn resolve_requested_model(
        &self,
        selection: Option<&SessionModelSelection>,
        requested: &str,
    ) -> ModelOverrideJudgement {
        let _ = (selection, requested);
        ModelOverrideJudgement::Unjudged {
            requested: String::new(),
        }
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
