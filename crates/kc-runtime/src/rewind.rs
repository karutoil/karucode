//! Applying a rewind: hide the transcript tail and put the workspace back.
//!
//! [`crate::Runtime::rewind`] is the entry point; this module holds the parts
//! that are worth testing on their own:
//!
//! * [`StoreJournal`] - the [`JournalHandle`] the file tools write through while
//!   a run is in flight. It captures the before-image *before* the write (the
//!   tool calls [`JournalHandle::record`] immediately before `write_atomic`), and
//!   stamps each row with the tool result's message id so a later rewind can
//!   place it on the transcript timeline.
//! * [`preview`] - what a rewind would do, computed from the same rows the undo
//!   uses. The preview and the action cannot disagree, which is the whole point
//!   of a confirmation dialog.
//! * [`revert`] - replay the journal backwards.
//!
//! # Why the journal is written by the tool, not the run loop
//!
//! The run loop knows a call is about to happen but not what it will touch: the
//! path is inside the arguments and only the tool knows how to resolve it
//! (containment, symlinks, parent creation). Reading the file in the loop would
//! duplicate that resolution and would still race the write. So the tool records
//! immediately before it writes, and everything else consumes the journal.

use crate::Runtime;
use kc_core::checkpoint::{FileCheckpoint, RewindFile, RewindPreview, UndoOutcome};
use kc_core::error::{Error, Result};
use kc_core::paths::Workspace;
use kc_core::store::{RewindPoint, Store};
use kc_core::tool::{capture_file, JournalHandle};
use kc_core::{AgentId, RunId, SessionId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Tools that record a before-image, so a rewind can undo them.
///
/// Keep this in step with the `ctx.record_checkpoint` calls in `kc-tools`: a tool
/// listed here that does not journal makes a revert claim more than it delivers.
const JOURNALING_TOOLS: &[&str] = &["write_file", "edit_file"];

/// How many messages after the rewind point are scanned for tool calls.
///
/// The preview only needs the *names* of the tools that ran, and a session with
/// thousands of messages after the point would otherwise be read in full to
/// answer a dialog. The cap is generous: past it the list is a sample, which is
/// all an "this cannot be undone" warning needs.
const REVERSIBLE_SCAN_LIMIT: usize = 5_000;

/// The [`JournalHandle`] a run hands to its tools.
///
/// One per run (and per subagent, which gets its own `agent_id`), so every row
/// it writes names the run that produced it and the tool result it belongs to.
pub(crate) struct StoreJournal {
    store: Arc<dyn Store>,
    session_id: SessionId,
    run_id: RunId,
    agent_id: Option<AgentId>,
    workspace: Arc<Workspace>,
    /// The id of the tool result currently being executed.
    ///
    /// A tool records while its call is in flight, and the tool result row does
    /// not exist yet - it is appended by the loop *after* `invoke` returns. So
    /// the journal cannot know it at capture time. It is filled in by
    /// [`StoreJournal::bind_message`], which the loop calls once the result is
    /// persisted, for every row this run appended in the meantime.
    ///
    /// A value of `0` means "not placed yet"; such a row is treated as part of
    /// the region up to the next real one, which is what a rewind wants (an
    /// unplaced change still happened after the prompt).
    pending: std::sync::Mutex<PendingRows>,
}

#[derive(Default)]
struct PendingRows {
    /// Ids of rows written since the last `bind_message`.
    unbound: Vec<i64>,
}

impl StoreJournal {
    pub(crate) fn new(
        store: Arc<dyn Store>,
        session_id: SessionId,
        run_id: RunId,
        agent_id: Option<AgentId>,
        workspace: Arc<Workspace>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            session_id,
            run_id,
            agent_id,
            workspace,
            pending: std::sync::Mutex::new(PendingRows::default()),
        })
    }

    /// Attach the tool result's message id to every row written since the last
    /// call.
    ///
    /// Called by the loop after it persists a tool result. Two writes to the same
    /// file in one turn produce two rows, and both belong to the same result;
    /// that is correct, because undoing the result undoes both.
    pub(crate) async fn bind_message(&self, message_id: i64) {
        let ids = {
            let mut pending = self.pending.lock().expect("journal lock");
            std::mem::take(&mut pending.unbound)
        };
        if ids.is_empty() {
            return;
        }
        // The runtime's `Store` trait has no "update a checkpoint" verb: a journal
        // row is written once. So the row is rewritten rather than patched, which
        // is the same cost as the insert and keeps the trait surface small.
        for id in ids {
            if let Err(e) = self.store.set_checkpoint_message_id(id, message_id).await {
                tracing::warn!(
                    checkpoint = id,
                    message_id,
                    error = %e,
                    "could not place a journal row on the transcript timeline; \
                     it will be treated as part of the rewound region"
                );
            }
        }
    }
}

#[async_trait::async_trait]
impl JournalHandle for StoreJournal {
    async fn record(&self, rel: &str, tool: &str) {
        let captured = match self.workspace.resolve_existing(rel) {
            Ok(abs) => capture_file(&abs),
            // A file the tool is about to create does not resolve as "existing",
            // so the capture is of a path that is not there - which is exactly
            // what undoing the create needs.
            Err(_) => capture_file(&self.workspace.root().join(rel)),
        };
        let row = FileCheckpoint {
            id: 0,
            session_id: self.session_id.0.clone(),
            run_id: self.run_id.0.clone(),
            agent_id: self.agent_id.as_ref().map(|a| a.0.clone()),
            // The tool-call id is filled in by `capture_path`'s caller when it
            // matters; a rewind addresses a message, not a call.
            call_id: String::new(),
            tool: tool.to_string(),
            path: rel.to_string(),
            existed: captured.existed,
            content: captured.content,
            capture_failed: captured.failed,
            message_id: 0,
            created_at: 0,
        };
        match self.store.record_checkpoint(&row).await {
            Ok(id) => self.pending.lock().expect("journal lock").unbound.push(id),
            Err(e) => tracing::warn!(
                path = rel,
                tool,
                error = %e,
                "could not journal a file change; a rewind will report it as unrestorable"
            ),
        }
    }

    async fn capture_path(&self, rel: &str, tool: &str) -> bool {
        self.record(rel, tool).await;
        true
    }
}

/// Resolve, preview and revert: everything a rewind needs that does not require
/// starting a run.
pub(crate) struct Rewinder {
    store: Arc<dyn Store>,
    workspace: Arc<Workspace>,
}

impl Rewinder {
    pub(crate) fn new(store: Arc<dyn Store>, workspace: Arc<Workspace>) -> Self {
        Self { store, workspace }
    }

    /// What a rewind to `message_id` would do.
    pub(crate) async fn preview(
        &self,
        session_id: &str,
        message_id: i64,
    ) -> Result<(RewindPoint, RewindPreview)> {
        let point = self.store.rewind_point(session_id, message_id).await?;
        let rows = self
            .store
            .list_checkpoints(session_id, point.keep_through)
            .await?;
        let opaque = self.opaque_tools(session_id, point.keep_through).await?;
        let preview = summarize(session_id, &point, &rows, opaque);
        Ok((point, preview))
    }

    /// Tools the rewound region called that record no journal row.
    ///
    /// Only the transcript knows what ran, and only a tool that records is
    /// reversible. `write_file` and `edit_file` journal themselves; a shell
    /// command, a plugin call or an MCP call can change anything and says so.
    async fn opaque_tools(&self, session_id: &str, keep_through: i64) -> Result<Vec<String>> {
        let stored = self
            .store
            .list_messages_after(session_id, keep_through, REVERSIBLE_SCAN_LIMIT)
            .await?;
        let mut seen: Vec<String> = Vec::new();
        for message in stored {
            for call in &message.tool_calls {
                if JOURNALING_TOOLS.contains(&call.name.as_str()) {
                    continue;
                }
                if !seen.contains(&call.name) {
                    seen.push(call.name.clone());
                }
            }
        }
        Ok(seen)
    }

    /// The journal rows a rewind to `keep_through` would reverse.
    pub(crate) async fn rows(
        &self,
        session_id: &str,
        keep_through: i64,
    ) -> Result<Vec<FileCheckpoint>> {
        self.store.list_checkpoints(session_id, keep_through).await
    }

    /// Undo every row, newest first.
    ///
    /// Newest first is the only order that restores the right state: a file
    /// written four times has four before-images, and replaying them oldest-first
    /// would end on the second write's bytes instead of the original.
    ///
    /// A row is deleted only when its undo actually happened. Rows that could not
    /// be undone stay, so the failure is visible on the next preview instead of
    /// being silently forgotten.
    pub(crate) async fn revert(&self, rows: &[FileCheckpoint]) -> Vec<UndoOutcome> {
        let mut outcomes = Vec::with_capacity(rows.len());
        for row in rows.iter().rev() {
            let outcome = self.undo_one(row).await;
            match outcome {
                UndoOutcome::Restored | UndoOutcome::Removed => {
                    if let Err(e) = self.store.delete_checkpoint(row.id).await {
                        tracing::warn!(
                            checkpoint = row.id,
                            path = %row.path,
                            error = %e,
                            "restored the file but could not retire its journal row"
                        );
                    }
                }
                UndoOutcome::Failed => tracing::warn!(
                    path = %row.path,
                    tool = %row.tool,
                    "could not restore this file; its journal row is kept so the next \
                     rewind reports it again"
                ),
                UndoOutcome::NotCaptured => {}
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    async fn undo_one(&self, row: &FileCheckpoint) -> UndoOutcome {
        if !row.undoable() {
            return UndoOutcome::NotCaptured;
        }
        if !row.existed {
            // The tool created the file, so undoing it means removing it.
            let abs = match self.workspace.resolve_for_write(&row.path) {
                Ok(abs) => abs,
                Err(e) => {
                    tracing::warn!(path = %row.path, error = %e, "created file is no longer reachable");
                    return UndoOutcome::Failed;
                }
            };
            if !abs.exists() {
                // Already gone: a later tool call in the same region may have
                // removed it, or the user did. Either way the intended state
                // holds.
                return UndoOutcome::Removed;
            }
            return match std::fs::remove_file(&abs) {
                Ok(()) => UndoOutcome::Removed,
                Err(e) => {
                    tracing::warn!(path = %row.path, error = %e, "could not remove a created file");
                    UndoOutcome::Failed
                }
            };
        }

        let abs = match self.workspace.resolve_for_write(&row.path) {
            Ok(abs) => abs,
            Err(e) => {
                tracing::warn!(path = %row.path, error = %e, "cannot restore an unreachable path");
                return UndoOutcome::Failed;
            }
        };
        match write_restored(&abs, row.content.as_bytes()) {
            Ok(()) => UndoOutcome::Restored,
            Err(e) => {
                tracing::warn!(path = %row.path, error = %e, "could not restore a file");
                UndoOutcome::Failed
            }
        }
    }
}

/// Write restored bytes through the workspace's atomic writer.
///
/// `kc-tools` owns `write_atomic`, but the runtime cannot depend on it (the
/// dependency direction is runtime -> core only), so this mirrors it: a sibling
/// temp file and a rename, which is what makes a restore all-or-nothing.
fn write_restored(abs: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = match abs.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent)?;
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = parent.join(format!(".{name}.rewind-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, abs)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Group the journal rows into one entry per path, describing the state the
/// rewind leaves behind.
///
/// The *earliest* before-image wins: undoing newest-first ends on the oldest
/// record for the path, so that is the state the preview must describe.
///
/// `opaque_tools` is every tool the rewound transcript asked for that records no
/// journal row. It is passed in rather than guessed here, because only the
/// transcript knows what ran.
pub(crate) fn summarize(
    session_id: &str,
    point: &RewindPoint,
    rows: &[FileCheckpoint],
    opaque_tools: Vec<String>,
) -> RewindPreview {
    // Insertion order is journal order (oldest first); `BTreeMap` sorts by path,
    // which is what the dialog wants to list.
    let mut by_path: BTreeMap<&str, Vec<&FileCheckpoint>> = BTreeMap::new();
    for row in rows {
        by_path.entry(row.path.as_str()).or_default().push(row);
    }

    let mut files = Vec::with_capacity(by_path.len());
    let mut incomplete = false;
    for (path, mut history) in by_path {
        // Journal order is ascending id; restore from the oldest.
        history.sort_by_key(|row| row.id);
        let first = history[0];
        if history.iter().any(|row| !row.undoable()) {
            incomplete = true;
        }
        let mut tools: Vec<String> = Vec::new();
        for row in &history {
            if !tools.contains(&row.tool) {
                tools.push(row.tool.clone());
            }
        }
        files.push(RewindFile {
            path: path.to_string(),
            // A create has no before-image; the undo removes the file instead.
            restore: (first.existed && first.undoable() && !first.content.is_empty())
                .then(|| first.content.clone()),
            removes_file: !first.existed,
            changes: history.len(),
            tools,
            incomplete: history.iter().any(|row| !row.undoable()),
        });
    }

    // Journal rows only exist for the tools that record. Everything else a run can
    // do - a shell command, a plugin, an MCP call - leaves effects this rewind
    // cannot reach, and saying so is part of an honest revert.
    RewindPreview {
        session_id: session_id.to_string(),
        message_id: point.user_message_id,
        messages_removed: point.messages_removed,
        files,
        incomplete,
        opaque_tools,
    }
}

/// `Runtime::rewind`'s implementation, kept out of `lib.rs` so the orchestration
/// reads as one procedure.
pub(crate) async fn rewind(
    runtime: &Arc<Runtime>,
    session_id: &str,
    message_id: i64,
    revert: bool,
    keep_prompt: bool,
) -> Result<(RewindPoint, RewindPreview, Vec<UndoOutcome>)> {
    let inner = runtime.shared();
    if runtime.is_running(session_id) {
        return Err(Error::BadRequest(format!(
            "session {session_id} has a run in flight; cancel it before rewinding"
        )));
    }

    let session = inner
        .deps
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
    // Resolve the workspace before touching anything: a project whose directory
    // disappeared must refuse the rewind, not hide the transcript and then fail
    // to restore the files.
    let workspace = inner
        .deps
        .workspaces
        .workspace_for(session.project_id.as_deref())?;

    let rewinder = Rewinder::new(inner.deps.store.clone(), workspace);
    let (point, preview) = rewinder.preview(session_id, message_id).await?;
    let cut = point.cut(keep_prompt);

    let outcomes = if revert {
        // The journal is read up to the *prompt-was-kept* boundary and then
        // filtered: a retry must undo the work of the turn it replaces, even
        // though the prompt row itself is about to be hidden.
        let rows: Vec<FileCheckpoint> = rewinder
            .rows(session_id, point.before_prompt)
            .await?
            .into_iter()
            .filter(|row| row.message_id == 0 || row.message_id > cut)
            .collect();
        rewinder.revert(&rows).await
    } else {
        Vec::new()
    };

    // The transcript is hidden *after* the files are restored. A revert that
    // fails halfway must not leave a session whose transcript claims the work is
    // gone while the workspace still has it.
    inner
        .deps
        .store
        .mark_messages_rewound(session_id, cut)
        .await?;
    inner.deps.store.touch_session(session_id).await?;

    Ok((point, preview, outcomes))
}
