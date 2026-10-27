//! Subagent spawning, concurrency limits and status reporting.

use kc_core::config::SubagentConfig;

pub struct SubagentLimiter {
    pub config: SubagentConfig,
}

impl SubagentLimiter {
    pub fn new(config: SubagentConfig) -> Self {
        Self { config }
    }
}
