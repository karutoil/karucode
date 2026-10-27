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
pub mod paths;
pub mod plugin;
pub mod prompt;
pub mod provider;
pub mod settings;
pub mod store;
pub mod tool;

pub use error::{Error, Result};
pub use events::{Event, EventEnvelope, EventSink};
pub use ids::{AgentId, ApprovalId, RunId, SessionId};
pub use message::{Message, Role, ToolCall};
pub use paths::Workspace;
pub use settings::Settings;
pub use tool::{
    Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolRegistry, ToolSpec,
};
