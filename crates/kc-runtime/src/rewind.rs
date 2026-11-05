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
// A preview is also returned by the rewind action; fail before any mutation if
// the complete answer cannot fit, rather than silently misrepresenting it.
const PREVIEW_CONTENT_LIMIT: usize = 8 * 1024 * 1024;
const TOOL_SCAN_BYTES: usize = 8 * 1024 * 1024;

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
    unbound: Vec<PendingBinding>,
}

struct PendingBinding {
    id: i64,
    /// Set by the first bind attempt and preserved across retries. Reassigning
    /// a failed row to a later tool result would move the write forward on the
    /// transcript timeline and could exclude it from an intermediate rewind.
    message_id: Option<i64>,
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
        let batches = {
            let mut pending = self.pending.lock().expect("journal lock");
            for row in &mut pending.unbound {
                if row.message_id.is_none() {
                    row.message_id = Some(message_id);
                }
            }
            let mut batches: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
            for row in &pending.unbound {
                if let Some(target) = row.message_id {
                    batches.entry(target).or_default().push(row.id);
                }
            }
            batches
        };

        // Do not remove an id until the store confirms its whole batch. A
        // failure leaves it pending with its original target so the next result
        // retries it rather than silently losing or moving the checkpoint.
        for (target, ids) in batches {
            let failure = match self.store.bind_checkpoints(&ids, target).await {
                Ok(bound) if bound == ids.len() => {
                    self.pending
                        .lock()
                        .expect("journal lock")
                        .unbound
                        .retain(|row| !ids.contains(&row.id));
                    continue;
                }
                Ok(bound) => format!("store bound {bound} of {} checkpoints", ids.len()),
                Err(e) => e.to_string(),
            };
            tracing::warn!(
                checkpoints = ?ids,
                message_id = target,
                error = %failure,
                "could not place journal rows on the transcript timeline; they remain pending and unbound rows are included in the rewound region"
            );
            return;
        }
    }

    /// Capture and persist one before-image, returning whether a rewind can
    /// actually restore it. Persisting a `capture_failed` marker is valuable for
    /// visibility, but is not a successful restorable capture.
    async fn capture(&self, rel: &str, tool: &str) -> bool {
        let captured = match self.workspace.resolve_existing(rel) {
            Ok(abs) => capture_file(&abs),
            // A file the tool is about to create does not resolve as "existing",
            // so the capture is of a path that is not there - which is exactly
            // what undoing the create needs.
            Err(_) => capture_file(&self.workspace.root().join(rel)),
        };
        let restorable = !captured.failed;
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
            Ok(id) => {
                self.pending
                    .lock()
                    .expect("journal lock")
                    .unbound
                    .push(PendingBinding {
                        id,
                        message_id: None,
                    });
                if !restorable {
                    tracing::warn!(
                        path = rel,
                        tool,
                        checkpoint = id,
                        "before-image is not capturable; recorded that this path is unrestorable"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    path = rel,
                    tool,
                    error = %e,
                    "could not persist a file before-image; this change cannot be restored by rewind"
                );
                return false;
            }
        }
        restorable
    }
}

#[async_trait::async_trait]
impl JournalHandle for StoreJournal {
    async fn record(&self, rel: &str, tool: &str) {
        // Recording is deliberately best-effort for ordinary file tools. The
        // error log is honest about the missing row; callers that must refuse an
        // unrestorable write use `capture_path` and observe the same false result.
        let _ = self.capture(rel, tool).await;
    }

    async fn capture_path(&self, rel: &str, tool: &str) -> bool {
        self.capture(rel, tool).await
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
        let mut rows = self
            .store
            .list_checkpoint_metadata(session_id, point.keep_through)
            .await?;
        let mut seen = std::collections::HashSet::new();
        let mut remaining = PREVIEW_CONTENT_LIMIT;
        for row in &mut rows {
            if seen.insert(row.path.clone()) && row.existed && row.undoable() {
                match self
                    .store
                    .checkpoint_content_bounded(row.id, remaining)
                    .await?
                {
                    Some(content) => {
                        remaining -= content.len();
                        row.content = content;
                    }
                    None => row.capture_failed = true,
                }
            }
        }
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
        let names = self
            .store
            .rewind_tool_names(
                session_id,
                keep_through,
                REVERSIBLE_SCAN_LIMIT,
                TOOL_SCAN_BYTES,
            )
            .await?;
        let mut seen: Vec<String> = Vec::new();
        for name in names {
            if !JOURNALING_TOOLS.contains(&name.as_str()) && !seen.contains(&name) {
                seen.push(name);
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
        self.store
            .list_checkpoint_metadata(session_id, keep_through)
            .await
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
        let content = match self.store.checkpoint_content(row.id).await {
            Ok(Some(content)) => content,
            Ok(None) => {
                tracing::warn!(checkpoint = row.id, "checkpoint before-image is missing");
                return UndoOutcome::Failed;
            }
            Err(e) => {
                tracing::warn!(checkpoint = row.id, error = %e, "could not read checkpoint before-image");
                return UndoOutcome::Failed;
            }
        };
        match write_restored(&abs, content.as_bytes()) {
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
    let _lifecycle = runtime.shared().lock_session_lifecycle(session_id).await;
    rewind_quiescent(runtime, session_id, message_id, revert, keep_prompt).await
}

/// Rewind while the caller owns this session's lifecycle guard. The guard makes
/// this registered-run check and all mutations indivisible with respect to a
/// concurrent submit. Cancelled loops count until they have deregistered: their
/// last tool results and terminal status can still be in flight.
pub(crate) async fn rewind_quiescent(
    runtime: &Arc<Runtime>,
    session_id: &str,
    message_id: i64,
    revert: bool,
    keep_prompt: bool,
) -> Result<(RewindPoint, RewindPreview, Vec<UndoOutcome>)> {
    let inner = runtime.shared();
    if runtime.has_registered_runs(session_id) {
        return Err(Error::BadRequest(format!(
            "session {session_id} has a run in flight or still unwinding; wait for it to finish before rewinding"
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

#[cfg(test)]
mod journal_tests {
    use super::*;
    use crate::tests_support::FakeStore;
    use std::path::{Path, PathBuf};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("kc-journal-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create journal temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn journal(root: &Path) -> (Arc<StoreJournal>, Arc<FakeStore>) {
        let store = Arc::new(FakeStore::new());
        let workspace = Arc::new(Workspace::new(root).expect("journal workspace"));
        let journal = StoreJournal::new(
            store.clone(),
            SessionId::from_string("s_journal"),
            RunId::from_string("r_journal"),
            None,
            workspace,
        );
        (journal, store)
    }

    #[tokio::test]
    async fn capture_path_reports_a_failed_checkpoint_insert() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("file.txt"), "before\n").unwrap();
        let (journal, store) = journal(&tmp.path);
        store.fail_checkpoints(true);

        assert!(
            !journal.capture_path("file.txt", "write_file").await,
            "a failed DB insert is not a landed capture"
        );
        assert!(store
            .list_checkpoints("s_journal", 0)
            .await
            .unwrap()
            .is_empty());
        assert!(
            journal.pending.lock().unwrap().unbound.is_empty(),
            "there is no row id to bind after a failed insert"
        );
    }

    #[tokio::test]
    async fn capture_path_refuses_an_uncapturable_write_but_leaves_visible_evidence() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("binary.bin"), [0xff, 0xfe]).unwrap();
        let (journal, store) = journal(&tmp.path);

        assert!(
            !journal.capture_path("binary.bin", "write_file").await,
            "persisting an unrestorable marker is not a restorable capture"
        );
        let rows = store.list_checkpoints("s_journal", 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].capture_failed);
        let point = RewindPoint {
            session_id: "s_journal".into(),
            user_message_id: 1,
            prompt: "write binary".into(),
            keep_through: 0,
            before_prompt: 0,
            messages_removed: 1,
        };
        let preview = summarize("s_journal", &point, &rows, Vec::new());
        assert!(preview.incomplete);
        assert!(preview.files[0].incomplete);

        let rewinder = Rewinder::new(
            store.clone(),
            Arc::new(Workspace::new(&tmp.path).expect("workspace")),
        );
        assert_eq!(rewinder.revert(&rows).await, vec![UndoOutcome::NotCaptured]);
        assert_eq!(
            store.list_checkpoints("s_journal", 0).await.unwrap().len(),
            1,
            "an unrestorable row is retained and remains visible to the next preview"
        );
    }

    #[tokio::test]
    async fn failed_checkpoint_binding_retains_ids_and_their_original_message() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("first.txt"), "before\n").unwrap();
        std::fs::write(tmp.path.join("second.txt"), "before\n").unwrap();
        let (journal, store) = journal(&tmp.path);
        assert!(journal.capture_path("first.txt", "write_file").await);
        store.fail_checkpoint_binds(true);

        journal.bind_message(10).await;
        assert_eq!(
            store.list_checkpoints("s_journal", 0).await.unwrap()[0].message_id,
            0
        );
        assert_eq!(journal.pending.lock().unwrap().unbound.len(), 1);

        store.fail_checkpoint_binds(false);
        assert!(journal.capture_path("second.txt", "write_file").await);
        journal.bind_message(11).await;
        let mut rows = store.list_checkpoints("s_journal", 0).await.unwrap();
        rows.sort_by_key(|row| row.id);
        assert_eq!(
            rows.iter().map(|r| r.message_id).collect::<Vec<_>>(),
            [10, 11]
        );
        assert!(journal.pending.lock().unwrap().unbound.is_empty());
    }
}
