//! Core types, interfaces and configuration for the karucode agent harness.
//!
//! This crate is deliberately dependency-light: it defines the contracts that the
//! store, provider, tools, runtime, plugin host and server all share. Everything
//! else in the workspace depends on this crate, never the other way around.

pub mod approval;
pub mod config;
pub mod error;
pub mod events;
pub mod hooks;
pub mod ids;
pub mod message;
pub mod metrics;
pub mod notify;
pub mod paths;
pub mod plugin;
pub mod prompt;
pub mod provider;
pub mod settings;
pub mod store;
pub mod tool;

/// Source text of [`plugin`], so tooling can emit a dependency-free copy of the
/// protocol types (`karucode plugin init`).
///
/// It lives here rather than inside `plugin.rs` on purpose: an `include_str!` in
/// that file would be copied along with it and would then try to include itself
/// in the generated module.
pub const PLUGIN_PROTOCOL_SOURCE: &str = include_str!("plugin.rs");

pub use error::{Error, Result};
pub use events::{Event, EventEnvelope, EventSink};
pub use ids::{AgentId, ApprovalId, RunId, SessionId};
pub use message::{Message, Role, ToolCall};
pub use notify::{Notify, NotifyKind};
pub use paths::Workspace;
pub use settings::Settings;
pub use tool::{
    Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolRegistry, ToolSpec,
};
