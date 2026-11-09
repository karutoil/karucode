//! Core types, interfaces and configuration for the karucode agent harness.
//!
//! This crate is deliberately dependency-light: it defines the contracts that the
//! store, provider, tools, runtime, plugin host and server all share. Everything
//! else in the workspace depends on this crate, never the other way around.

pub mod approval;
pub mod checkpoint;
pub mod cidr;
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
pub mod queue;
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

pub use approval::{ApprovalDecision, ApprovalKind, ApprovalRequest};
pub use checkpoint::{FileCheckpoint, RewindFile, RewindPreview, UndoOutcome};
pub use cidr::{cloudflare_ranges, CidrRange, CidrSet};
pub use error::{Error, Result};
pub use events::{Event, EventEnvelope, EventSink};
pub use ids::{AgentId, ApprovalId, RunId, SessionId};
pub use message::{Message, Role, ToolCall};
pub use notify::{Notify, NotifyKind};
pub use paths::Workspace;
pub use queue::{
    AttachmentMode, QueuedAttachment, QueuedKind, QueuedMessage, MAX_QUEUED_ATTACHMENTS,
};
pub use settings::Settings;
pub use store::{RewindPoint, StoredMessage};
pub use tool::{
    capture_file, closest_tool_name, repair_arguments, CapturedFile, JournalHandle, Tool,
    ToolCallRequest, ToolClass, ToolCtx, ToolOrigin, ToolOutcome, ToolRegistry, ToolSpec,
    FAILURE_HINT_MAX_BYTES,
};
