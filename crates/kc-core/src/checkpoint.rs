//! The rewind journal: before-images of every file a run changed.
//!
//! A rewind is only honest if it can put the workspace back. Truncating the
//! transcript is a database write; undoing an `edit_file` is not, because the
//! bytes it replaced are gone. So every mutating file operation records what the
//! file looked like *before* it ran, and a rewind replays those records in
//! reverse.
//!
//! Three things shape this module:
//!
//! * **The journal is per file operation, not per run.** One assistant turn can
//!   write the same file four times; replaying a run-level "before" snapshot
//!   would lose the intermediate states a second rewind needs. Each row is the
//!   before-image of one tool call.
//! * **A create must be undoable too.** "This file did not exist" is not the
//!   absence of a record, it is a record with `existed = false`. Otherwise a
//!   rewind leaves the file the agent created behind and the workspace no longer
//!   matches the transcript it is being rewound to.
//! * **The cap is per file, and a capped snapshot is not a snapshot.** A file
//!   over [`MAX_SNAPSHOT_BYTES`] is recorded as *not captured*; the rewind then
//!   reports it as skipped instead of restoring a truncated file over a good one.
//!   A half-restored file is worse than an unrestored one.
//!
//! Content is stored as text. File tools already refuse binary targets
//! (`write_file` writes text, `edit_file` requires valid UTF-8), so a non-UTF-8
//! before-image can only come from a file the agent then overwrote blindly -
//! which is recorded as not captured rather than mangled through a lossy
//! conversion.

use serde::{Deserialize, Serialize};

/// Largest file this journal keeps a before-image of.
///
/// `edit_file` already refuses targets over 8 MiB; `write_file` is capped at
/// 2 MiB per call but may overwrite any size of existing file. 4 MiB covers
/// every ordinary source file and keeps one rewind from holding hundreds of
/// megabytes in SQLite.
pub const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

/// What an undo of one journal row did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoOutcome {
    /// The file was restored to the bytes it had before the tool ran.
    Restored,
    /// The tool created the file, so the undo removed it.
    Removed,
    /// The before-image was never captured (binary, or over the size cap).
    NotCaptured,
    /// Restoring failed (the path escaped the workspace, or the write failed).
    Failed,
}

impl UndoOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            UndoOutcome::Restored => "restored",
            UndoOutcome::Removed => "removed",
            UndoOutcome::NotCaptured => "not_captured",
            UndoOutcome::Failed => "failed",
        }
    }

    /// True when the workspace is what it was before the tool ran.
    pub fn is_clean(&self) -> bool {
        matches!(self, UndoOutcome::Restored | UndoOutcome::Removed)
    }
}

/// The before-image of one path, as recorded by one mutating file tool.
///
/// `path` is workspace-relative and uses `/` separators (see
/// [`kc_core::paths::Workspace::relative`]), so a journal stays meaningful when
/// the project moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCheckpoint {
    /// Database id. `0` for a row that has not been written yet.
    #[serde(default)]
    pub id: i64,
    pub session_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The tool call that changed the file, so a rewind can name it.
    pub call_id: String,
    /// The tool that changed it (`write_file`, `edit_file`, ...).
    pub tool: String,
    pub path: String,
    /// False when the tool created the file; undoing then removes it.
    pub existed: bool,
    /// The bytes the file had before the tool ran. Empty when `existed` is false
    /// or when the before-image was too large or not text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// True when the file existed but its bytes are not in `content`.
    #[serde(default)]
    pub capture_failed: bool,
    /// The message id the tool result was persisted as. This is what places a
    /// journal row on the transcript timeline: everything with a larger id
    /// happened *after* it.
    #[serde(default)]
    pub message_id: i64,
    pub created_at: i64,
}

impl FileCheckpoint {
    /// How many bytes of `content` this row stores.
    pub fn bytes(&self) -> usize {
        self.content.len()
    }

    /// Whether this row could be undone. A row with a failed capture cannot.
    pub fn undoable(&self) -> bool {
        !self.capture_failed
    }
}

/// One file as a rewind will leave it, for the preview the UI shows before
/// anything is touched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindFile {
    /// Workspace-relative path, `/`-separated.
    pub path: String,
    /// The earliest before-image of this path in the rewound region. That is the
    /// state the preview describes: undoing in reverse ends here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<String>,
    /// True when the rewind removes the file (the agent created it).
    pub removes_file: bool,
    /// How many journal rows in the region touched this path.
    pub changes: usize,
    /// Names of the tools that touched it, in the order they ran.
    pub tools: Vec<String>,
    /// True when at least one row for this path has no before-image, so the
    /// rewind cannot fully restore it.
    #[serde(default)]
    pub incomplete: bool,
}

/// Everything `POST /api/sessions/:id/rewind` will do, without doing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindPreview {
    pub session_id: String,
    /// The message the rewind keeps last.
    pub message_id: i64,
    /// How many messages the rewind hides from the transcript.
    pub messages_removed: usize,
    /// Every file the rewind touches, sorted by path.
    pub files: Vec<RewindFile>,
    /// True when some touched file has no complete before-image. Reverting is
    /// still offered, but the client must say what stays behind.
    #[serde(default)]
    pub incomplete: bool,
    /// Tool calls in the region that cannot be undone by the journal at all -
    /// `run_command`, a plugin tool, an MCP tool. Their effects stay, and the
    /// UI says so rather than implying a clean revert.
    #[serde(default)]
    pub opaque_tools: Vec<String>,
}
