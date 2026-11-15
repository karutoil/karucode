//! Per-subagent git worktrees, opt-in.
//!
//! The default is the parent's live workspace: a child sees the parent's dirty
//! and untracked files, its edits are visible immediately and survive
//! completion, and nothing has to be merged. That is what
//! [`WorktreeGuard::shared`] returns and it stays the default because it is the
//! safer contract - a child that silently lands somewhere else is a surprise, and
//! the previous implementation was removed for exactly that reason.
//!
//! `settings.subagents.worktrees = true` opts into the other contract: each child
//! gets its own detached checkout so two children editing the same file cannot
//! race each other or the parent.
//!
//! **What the child's starting point is.** A worktree starts at a commit, and the
//! parent's tree is usually not committed. So the child's base is a synthetic
//! commit built from the parent's *index* (which already holds staged and
//! unstaged changes to tracked files) with parent `HEAD`, plus the parent's
//! untracked, non-ignored files copied in. Without that, a child sent to "fix the
//! test I just broke" would open a file at its last committed state and edit the
//! wrong thing - the reason the feature was dropped the first time.
//!
//! **Where it lives.** `<workspace>/.karucode/worktrees/<agent-id>`, i.e. *inside*
//! the parent workspace. That is deliberate: every other part of the harness
//! confines tools to the project root, and a checkout outside it would have to
//! be added as an extra root on every read/write/command - one missed place and
//! the child cannot write its own tree. The directory is excluded from git status
//! in the parent (see [`EXCLUDE_LINE`]) so it never shows up as untracked.
//!
//! **What is not mirrored.** Ignored files (build output, `node_modules`) and
//! nested repositories/submodules are skipped: copying them is expensive, and a
//! child that needs a rebuild can run it inside its own tree. A child is told its
//! checkout is fresh so it does not assume a stale artifact is there.

use std::path::{Path, PathBuf};
use std::process::Command;

use kc_core::error::{Error, Result};

/// Directory under the workspace that holds every child checkout.
pub const WORKTREE_DIR: &str = ".karucode/worktrees";
/// Line written to `.git/info/exclude` so a checkout is never part of `git status`.
pub const EXCLUDE_LINE: &str = "/.karucode/worktrees/";
/// Prefix of the branch a child's finished work is committed onto.
pub const BRANCH_PREFIX: &str = "karucode/subagent";

/// A workspace a subagent loop may use, and what has to be undone afterwards.
pub struct WorktreeGuard {
    /// The directory the child's tools are confined to.
    workspace_path: PathBuf,
    /// What to undo on drop. `None` for the shared-workspace default.
    cleanup: Option<Cleanup>,
    /// Non-fatal problems building the checkout (a skipped nested repository, an
    /// exclude line that could not be written). Reported with the spawn so the
    /// parent is not told the tree is complete when it is not.
    warnings: Vec<String>,
}

/// Everything one isolated child needs to be tidied away.
struct Cleanup {
    /// The parent repository (worktree commands run from here).
    repo: PathBuf,
    /// `.git/info/exclude` we appended [`EXCLUDE_LINE`] to, if we created it.
    exclude_file: PathBuf,
    /// Whether this guard appended the line (and so must remove it).
    wrote_exclude: bool,
    /// The child's checkout.
    worktree: PathBuf,
    /// Agent id, for the branch name and messages.
    agent_id: String,
}

/// What a finished child left behind, reported to its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// Branch the child's changes were committed to. `None` means the changes
    /// exist only in the checkout named by [`Handoff::checkout`] - the branch
    /// could not be written, so that directory must not be deleted.
    pub branch: Option<String>,
    /// Files that differ from the child's starting commit.
    pub files: usize,
    /// The base commit the child started from, for `git diff`.
    pub base: String,
    /// Where the child's checkout was. Gone after a successful hand-off; still
    /// there, holding the only copy of the work, when `branch` is `None`.
    pub checkout: PathBuf,
}

impl WorktreeGuard {
    /// Use the parent's live workspace, including dirty and untracked state.
    ///
    /// This is the default for every subagent and the fallback whenever the
    /// caller did not ask for isolation. The legacy type name keeps the runtime
    /// module boundary small.
    pub fn shared(workspace_root: &Path) -> Self {
        Self {
            workspace_path: workspace_root.to_path_buf(),
            cleanup: None,
            warnings: Vec::new(),
        }
    }

    /// Give one subagent its own detached checkout of `workspace_root`.
    ///
    /// Refused, with an explanation the model can act on, when the workspace is
    /// not a git repository or has no commits: there is no tree to branch from,
    /// and quietly falling back to the shared workspace would answer a different
    /// question than the one the operator asked.
    pub fn isolated(workspace_root: &Path, agent_id: &str) -> Result<Self> {
        let repo = git_toplevel(workspace_root)?;
        // Cheap pre-check with a message that names the setting; `build_base_commit`
        // re-reads HEAD anyway.
        git(&repo, &["rev-parse", "--verify", "HEAD"]).map_err(|e| {
            Error::Tool(format!(
                "subagents.worktrees is on, but {} has no commit to start a worktree from \
                 ({e}); make one commit, or turn the setting off to let subagents share \
                 the parent workspace",
                repo.display()
            ))
        })?;

        let worktree = workspace_root.join(WORKTREE_DIR).join(safe_name(agent_id));
        if worktree.exists() {
            // A previous run with the same id left it behind (a crash, a kill).
            // Removing it is safe: it is a detached checkout this module created.
            let _ = remove_worktree(&repo, &worktree);
        }
        if let Some(parent) = worktree.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
        }

        // Build the child's base commit from the parent's working state, so
        // uncommitted edits to tracked files are visible to it.
        let base = build_base_commit(&repo, agent_id)?;

        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.to_string_lossy(),
                &base,
            ],
        )
        .map_err(|e| {
            Error::Tool(format!(
                "could not create a worktree for subagent {agent_id} at {}: {e}",
                worktree.display()
            ))
        })?;

        let wrote_exclude = ensure_excluded(&repo);
        let mut guard = Self {
            workspace_path: worktree.clone(),
            cleanup: Some(Cleanup {
                repo: repo.clone(),
                exclude_file: exclude_path(&repo),
                wrote_exclude,
                worktree,
                agent_id: agent_id.to_string(),
            }),
            warnings: Vec::new(),
        };

        if !wrote_exclude {
            // Not fatal: the checkout works, but the parent's `git status` will
            // list it as untracked until someone adds the ignore entry. Also means
            // the untracked-file mirror below sees this child's own checkout, which
            // it skips by path.
            guard.set_warning(format!(
                "could not add {EXCLUDE_LINE} to {}; the child's checkout will show up as \
                 untracked in the parent repository",
                exclude_path(&repo).display()
            ));
        }

        // Untracked files the parent has and the child should see. Ignored files
        // and nested repositories are deliberately skipped - see the module doc.
        if let Err(e) = mirror_untracked(workspace_root, guard.path()) {
            // The checkout exists and is usable; a file that failed to copy is
            // worth reporting, not worth failing the spawn over.
            guard.set_warning(e.to_string());
        }
        Ok(guard)
    }

    /// The exact tree the child's tools use.
    pub fn path(&self) -> &Path {
        &self.workspace_path
    }

    /// Warnings collected while building the checkout (a file that could not be
    /// copied, an exclude line that could not be written).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn set_warning(&mut self, message: String) {
        self.warnings.push(message);
    }

    /// Commit whatever the child changed onto its own branch, then remove the
    /// checkout.
    ///
    /// A child that changed nothing leaves nothing - no branch, no directory. A
    /// child that did change files gets a local branch and a [`Handoff`] the
    /// parent can report, because otherwise removing the checkout would destroy
    /// work with no trace, which is the failure this feature has to avoid.
    pub fn finish(mut self) -> (Option<Handoff>, Vec<String>) {
        let Some(cleanup) = self.cleanup.take() else {
            return (None, Vec::new());
        };
        let mut warnings = Vec::new();
        let worktree = cleanup.worktree.clone();
        let repo = cleanup.repo.clone();

        let base = git(&worktree, &["rev-parse", "HEAD"]).unwrap_or_default();
        let changed = git(&worktree, &["status", "--porcelain"]).unwrap_or_default();
        let files = changed.lines().filter(|l| !l.trim().is_empty()).count();

        let mut handoff = None;
        if files > 0 {
            // The child's edits are uncommitted; stage and commit them onto a
            // branch named for the agent. A commit fails if the child left no
            // usable identity behind, so one is supplied explicitly rather than
            // depending on the operator's global git config.
            let branch = format!("{BRANCH_PREFIX}/{}", cleanup.agent_id);
            let staged = git(&worktree, &["add", "-A"]).and_then(|_| {
                git(
                    &worktree,
                    &[
                        "-c",
                        "user.name=karucode subagent",
                        "-c",
                        "user.email=subagent@karucode.invalid",
                        "commit",
                        "-q",
                        "-m",
                        &format!(
                            "karucode subagent {}: {} file{}",
                            cleanup.agent_id,
                            files,
                            if files == 1 { "" } else { "s" }
                        ),
                    ],
                )
            });
            match staged {
                Ok(_) => {
                    let branch_ref = format!("refs/heads/{branch}");
                    if let Err(e) = git(&worktree, &["update-ref", &branch_ref, "HEAD"]) {
                        warnings.push(format!(
                            "subagent {0} changed {1} file(s) but its branch could not be \
                             written ({e}); its checkout is left at {2}",
                            cleanup.agent_id,
                            files,
                            worktree.display()
                        ));
                        // Leave the checkout in place: it is the only copy now, and
                        // the handoff says so instead of reporting "nothing changed".
                        return (
                            Some(Handoff {
                                branch: None,
                                files,
                                base,
                                checkout: worktree,
                            }),
                            warnings,
                        );
                    }
                    handoff = Some(Handoff {
                        branch: Some(branch),
                        files,
                        base: base.clone(),
                        checkout: worktree.clone(),
                    });
                }
                Err(e) => {
                    warnings.push(format!(
                        "subagent {0} changed {1} file(s) and they could not be committed \
                         ({e}); its checkout is left at {2}",
                        cleanup.agent_id,
                        files,
                        worktree.display()
                    ));
                    // Keep the files: removing the checkout would lose the work.
                    return (
                        Some(Handoff {
                            branch: None,
                            files,
                            base,
                            checkout: worktree,
                        }),
                        warnings,
                    );
                }
            }
        }

        if let Err(e) = remove_worktree(&repo, &worktree) {
            warnings.push(format!(
                "could not remove the worktree at {}: {e}",
                worktree.display()
            ));
        }
        if cleanup.wrote_exclude {
            remove_exclude_line(&cleanup.exclude_file);
        }
        (handoff, warnings)
    }
}

impl Drop for WorktreeGuard {
    /// A guard that was never finished (a panic, a cancelled task) still cleans
    /// up, but it cannot commit: the edits stay in the checkout and the runtime
    /// reports its path instead of deleting work it never recorded.
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        let dirty = git(&cleanup.worktree, &["status", "--porcelain"])
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if dirty {
            // Leave it for the operator; the process may be going away.
            return;
        }
        let _ = remove_worktree(&cleanup.repo, &cleanup.worktree);
        if cleanup.wrote_exclude {
            remove_exclude_line(&cleanup.exclude_file);
        }
    }
}

/// The repository's top level for `dir`, or a refusal naming the setting.
fn git_toplevel(dir: &Path) -> Result<PathBuf> {
    let out = git(dir, &["rev-parse", "--show-toplevel"]).map_err(|e| {
        Error::Tool(format!(
            "subagents.worktrees is on, but {} is not inside a git repository ({e}); turn the \
             setting off to let subagents share the parent workspace",
            dir.display()
        ))
    })?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err(Error::Tool(format!(
            "subagents.worktrees is on, but {} is not inside a git repository",
            dir.display()
        )));
    }
    Ok(PathBuf::from(trimmed))
}

/// A commit made from the parent's *working state* with `HEAD` as its parent, so
/// the child starts from "what the parent has on disk" rather than "what it last
/// committed".
///
/// `git write-tree` alone is not enough: it reads the index, so an unstaged edit
/// to a tracked file (the common case - a parent mid-change asking a child to
/// finish or review it) would be missing. The fix is to apply the worktree-versus-
/// index diff to a *copy* of the index and write the tree from that copy, which
/// needs no changes to the real index and no stash.
fn build_base_commit(repo: &Path, agent_id: &str) -> Result<String> {
    let head = git(repo, &["rev-parse", "--verify", "HEAD"])
        .map_err(|e| Error::Tool(format!("no HEAD in {}: {e}", repo.display())))?;

    // A copy of the index, so the parent's staging area is never touched.
    let temp_index = temp_index_path(repo);
    if let Some(parent) = temp_index.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
    }
    let real_index = git(repo, &["rev-parse", "--git-path", "index"])
        .map_err(|e| Error::Tool(format!("could not locate the index: {e}")))?;
    let real_index = {
        let p = PathBuf::from(real_index.trim());
        if p.is_absolute() {
            p
        } else {
            repo.join(p)
        }
    };
    if real_index.exists() {
        std::fs::copy(&real_index, &temp_index)
            .map_err(|e| Error::Tool(format!("copy the index for the child's base: {e}")))?;
    }

    let diff = Command::new("git")
        .args(["diff", "--binary"])
        .current_dir(repo)
        .output()
        .map_err(|e| Error::Tool(format!("git not runnable: {e}")))?;
    if !diff.status.success() {
        // A clean tree (no diff at all) is a normal result, not a failure.
        let stderr = String::from_utf8_lossy(&diff.stderr);
        if !diff.stdout.is_empty() || !stderr.trim().is_empty() {
            let _ = std::fs::remove_file(&temp_index);
            return Err(Error::Tool(format!(
                "could not read the parent's uncommitted changes: {}",
                stderr.trim()
            )));
        }
    }

    // Apply that diff to the copied index. `--allow-empty` covers a tree with no
    // uncommitted changes at all.
    let mut apply = Command::new("git")
        .args(["apply", "--cached", "--allow-empty"])
        .current_dir(repo)
        .env("GIT_INDEX_FILE", &temp_index)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Tool(format!("git not runnable: {e}")))?;
    if let Some(stdin) = apply.stdin.as_mut() {
        use std::io::Write as _;
        stdin
            .write_all(&diff.stdout)
            .map_err(|e| Error::Tool(format!("could not pass the parent's diff: {e}")))?;
    }
    let applied = apply
        .wait_with_output()
        .map_err(|e| Error::Tool(format!("git apply: {e}")))?;
    if !applied.status.success() {
        let _ = std::fs::remove_file(&temp_index);
        return Err(Error::Tool(format!(
            "could not snapshot the parent's working tree: {}",
            String::from_utf8_lossy(&applied.stderr).trim()
        )));
    }

    let tree = Command::new("git")
        .arg("write-tree")
        .current_dir(repo)
        .env("GIT_INDEX_FILE", &temp_index)
        .output()
        .map_err(|e| Error::Tool(format!("git not runnable: {e}")))?;
    let _ = std::fs::remove_file(&temp_index);
    if !tree.status.success() {
        return Err(Error::Tool(format!(
            "could not snapshot the parent's working tree: {}",
            String::from_utf8_lossy(&tree.stderr).trim()
        )));
    }
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

    let message = format!("karucode subagent {agent_id} base");
    let commit = git(
        repo,
        &[
            "-c",
            "user.name=karucode subagent",
            "-c",
            "user.email=subagent@karucode.invalid",
            "commit-tree",
            &tree,
            "-p",
            head.trim(),
            "-m",
            &message,
        ],
    )
    .map_err(|e| Error::Tool(format!("could not create the child's base commit: {e}")))?;
    Ok(commit.trim().to_string())
}

/// A temporary index path that does not collide with a concurrent child.
fn temp_index_path(repo: &Path) -> PathBuf {
    repo.join(format!(
        ".karucode/kc-index-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

/// Append [`EXCLUDE_LINE`] to `.git/info/exclude` unless it is already there.
/// Returns whether this call wrote it (and so must remove it later).
fn ensure_excluded(repo: &Path) -> bool {
    let path = exclude_path(repo);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == EXCLUDE_LINE) {
        return false;
    }
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(EXCLUDE_LINE);
    text.push('\n');
    std::fs::write(&path, text).is_ok()
}

fn remove_exclude_line(path: &Path) {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return;
    };
    let kept: Vec<&str> = existing
        .lines()
        .filter(|l| l.trim() != EXCLUDE_LINE)
        .collect();
    let mut text = kept.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let _ = std::fs::write(path, text);
}

/// `git rev-parse --git-path info/exclude` handles `core.excludesFile` and the
/// worktree/admin layout; fall back to the conventional path if it fails.
fn exclude_path(repo: &Path) -> PathBuf {
    if let Ok(out) = git(repo, &["rev-parse", "--git-path", "info/exclude"]) {
        let p = PathBuf::from(out.trim());
        if p.is_absolute() {
            return p;
        }
        return repo.join(p);
    }
    repo.join(".git/info/exclude")
}

fn remove_worktree(repo: &Path, worktree: &Path) -> Result<()> {
    let path = worktree.to_string_lossy().to_string();
    git(repo, &["worktree", "remove", "--force", &path])?;
    // Prune the administrative directory even when the checkout was already gone.
    let _ = git(repo, &["worktree", "prune"]);
    Ok(())
}

/// Copy the parent's untracked, non-ignored files into the child's checkout.
///
/// `git ls-files --others --exclude-standard` is exactly that set: it already
/// applies `.gitignore`, `core.excludesFile` and `.git/info/exclude`, so the
/// child's own checkout directory is in the list and must be skipped by hand.
fn mirror_untracked(source: &Path, target: &Path) -> Result<()> {
    let out = git(
        source,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .map_err(|e| Error::Tool(format!("could not list untracked files: {e}")))?;
    let mut skipped = Vec::new();
    for rel in out.split('\0').filter(|s| !s.is_empty()) {
        // Never copy a worktree directory (the child's own, or a sibling's) and
        // never follow a path out of the tree.
        let rel_path = Path::new(rel);
        if rel_path.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        if rel.starts_with(".karucode/worktrees/") || rel == ".karucode/worktrees" {
            continue;
        }
        // A nested repository (or submodule) is its own tree; copying the outer
        // directory would produce a checkout whose git commands talk to the
        // parent's repository. Tracked contents are already in the base commit.
        let source_path = source.join(rel_path);
        if source_path.is_dir() && source_path.join(".git").exists() {
            skipped.push(rel.to_string());
            continue;
        }
        let dest = target.join(rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Tool(format!("create {}: {e}", parent.display())))?;
        }
        if source_path.is_dir() {
            copy_dir(&source_path, &dest)?;
        } else if source_path.exists() {
            if let Err(e) = std::fs::copy(&source_path, &dest) {
                return Err(Error::Tool(format!("copy {}: {e}", source_path.display())));
            }
        }
    }
    if !skipped.is_empty() {
        return Err(Error::Tool(format!(
            "skipped nested repositories (not mirrored into the child's worktree): {}",
            skipped.join(", ")
        )));
    }
    // The mirror put these files in the checkout, but they are not in its index,
    // so they would read as the child's own new files and end up in its branch.
    // Adding them marks them as inherited state; git carries an explicit empty
    // list as an empty value (not "all files"), so this stays a no-op for a clean
    // parent with nothing untracked.
    let mut add = Command::new("git")
        .args(["add", "--pathspec-from-file=-", "--pathspec-file-nul", "-f"])
        .current_dir(target)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Tool(format!("git not runnable: {e}")))?;
    {
        use std::io::Write as _;
        let stdin = add
            .stdin
            .as_mut()
            .ok_or_else(|| Error::Tool("git add: no stdin".into()))?;
        for rel in out.split('\0').filter(|s| !s.is_empty()) {
            let rel_path = Path::new(rel);
            if rel_path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            if rel.starts_with(".karucode/worktrees/") || rel == ".karucode/worktrees" {
                continue;
            }
            let source_path = source.join(rel_path);
            if source_path.is_dir() && source_path.join(".git").exists() {
                continue;
            }
            stdin
                .write_all(rel.as_bytes())
                .and_then(|_| stdin.write_all(&[0]))
                .map_err(|e| Error::Tool(format!("could not pass mirrored paths: {e}")))?;
        }
    }
    let added = add
        .wait_with_output()
        .map_err(|e| Error::Tool(format!("git add: {e}")))?;
    if !added.status.success() {
        let stderr = String::from_utf8_lossy(&added.stderr).trim().to_string();
        // A file that vanished between listing and adding is not worth failing
        // the spawn over; anything else is.
        if !stderr.is_empty() && !stderr.contains("did not match any files") {
            return Err(Error::Tool(format!(
                "could not mark the mirrored files as the child's starting state: {stderr}"
            )));
        }
    }
    // Fold the mirrored files into the child's base commit, so its checkout is
    // clean *and* its eventual diff is exactly what it changed. Without this the
    // inherited files look like the child's own work and would be committed onto
    // its branch as if it had written them.
    if !out.trim_matches('\0').is_empty() {
        let amended = git(
            target,
            &[
                "-c",
                "user.name=karucode subagent",
                "-c",
                "user.email=subagent@karucode.invalid",
                "commit",
                "--amend",
                "-q",
                "--no-edit",
                "--allow-empty",
            ],
        );
        if let Err(e) = amended {
            return Err(Error::Tool(format!(
                "could not record the parent's untracked files in the child's base commit: {e}"
            )));
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)
        .map_err(|e| Error::Tool(format!("create {}: {e}", to.display())))?;
    for entry in
        std::fs::read_dir(from).map_err(|e| Error::Tool(format!("read {}: {e}", from.display())))?
    {
        let entry = entry.map_err(|e| Error::Tool(format!("read entry: {e}")))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)
                .map_err(|e| Error::Tool(format!("copy {}: {e}", src.display())))?;
        }
    }
    Ok(())
}

/// Run one git command in `dir`, returning stdout, or the stderr on failure.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| Error::Tool(format!("git not runnable: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::Tool(if stderr.is_empty() {
            format!("git {} failed ({})", args.join(" "), output.status)
        } else {
            stderr
        }))
    }
}

/// A filesystem-safe form of an agent id (`ag_abc` → `ag_abc`).
fn safe_name(agent_id: &str) -> String {
    let cleaned: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// A throwaway directory that removes only its own test fixture.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("kc-worktree-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // `worktree remove` clears `.git/worktrees`; a plain recursive delete
            // is enough for a fixture that may hold a partially built checkout.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn ok(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()))
    }

    fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        ok(&repo, &["init", "-q"]);
        ok(&repo, &["config", "user.email", "test@example.invalid"]);
        ok(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("tracked.txt"), "committed\n").expect("write tracked file");
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
        ok(&repo, &["add", "."]);
        ok(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    #[test]
    fn the_default_workspace_is_the_parents_own_tree() {
        let tmp = TempDir::new();
        let guard = WorktreeGuard::shared(tmp.path());
        assert_eq!(guard.path(), tmp.path());
        let (handoff, warnings) = guard.finish();
        assert!(handoff.is_none(), "a shared guard has nothing to hand off");
        assert!(warnings.is_empty());
        assert!(
            tmp.path().is_dir(),
            "the parent workspace itself must survive"
        );
    }

    /// The whole point of the option: two children on the same file, neither
    /// seeing the other and neither touching the parent's working tree.
    #[test]
    fn two_children_edit_the_same_file_without_seeing_each_other() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());

        let a = WorktreeGuard::isolated(&repo, "ag_a").expect("worktree for a");
        let b = WorktreeGuard::isolated(&repo, "ag_b").expect("worktree for b");
        assert_ne!(a.path(), repo.as_path());
        assert_ne!(a.path(), b.path());

        std::fs::write(a.path().join("tracked.txt"), "from a\n").unwrap();
        std::fs::write(b.path().join("tracked.txt"), "from b\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(a.path().join("tracked.txt")).unwrap(),
            "from a\n",
            "a child must not see another child's edit"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
            "committed\n",
            "the parent's working tree is untouched while children work"
        );

        let (handoff_a, _) = a.finish();
        let (handoff_b, _) = b.finish();
        let a = handoff_a.expect("a changed a file, so it has a handoff");
        let b = handoff_b.expect("b changed a file, so it has a handoff");
        assert_eq!(a.files, 1);
        assert_eq!(b.files, 1);
        assert_ne!(a.branch, b.branch, "each child gets its own branch");

        // Each branch really carries that child's version.
        let branch_a = a.branch.clone().unwrap();
        let branch_b = b.branch.clone().unwrap();
        assert_eq!(
            ok(&repo, &["show", &format!("{branch_a}:tracked.txt")]),
            "from a\n"
        );
        assert_eq!(
            ok(&repo, &["show", &format!("{branch_b}:tracked.txt")]),
            "from b\n"
        );
        assert!(
            !repo.join(WORKTREE_DIR).join("ag_a").exists(),
            "a finished child's checkout is cleaned up"
        );
        assert!(
            !repo.join(WORKTREE_DIR).join("ag_b").exists(),
            "a finished child's checkout is cleaned up"
        );
    }

    /// A child sent to "fix what I just changed" must see the parent's
    /// uncommitted and untracked work, not last week's commit. This is the exact
    /// failure that got automatic worktrees removed the first time.
    #[test]
    fn a_child_starts_from_the_parents_uncommitted_and_untracked_state() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        // Unstaged edit to a tracked file, a staged one, and an untracked file.
        std::fs::write(repo.join("tracked.txt"), "parent dirty\n").unwrap();
        std::fs::write(repo.join("staged.txt"), "staged content\n").unwrap();
        ok(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("untracked.txt"), "untracked content\n").unwrap();
        // Ignored files are deliberately not copied.
        std::fs::write(repo.join("ignored.txt"), "build output\n").unwrap();

        let guard = WorktreeGuard::isolated(&repo, "ag_see").expect("worktree");

        assert_eq!(
            std::fs::read_to_string(guard.path().join("tracked.txt")).unwrap(),
            "parent dirty\n",
            "an unstaged edit must be visible to the child"
        );
        assert_eq!(
            std::fs::read_to_string(guard.path().join("staged.txt")).unwrap(),
            "staged content\n",
            "a staged new file must be visible to the child"
        );
        assert_eq!(
            std::fs::read_to_string(guard.path().join("untracked.txt")).unwrap(),
            "untracked content\n",
            "an untracked file must be visible to the child"
        );
        assert!(
            !guard.path().join("ignored.txt").exists(),
            "ignored build output is not mirrored"
        );
        // The mirror is not a dirty tree from git's point of view: the child's
        // checkout is clean, so its own diff is exactly what it changed.
        assert_eq!(
            ok(guard.path(), &["status", "--porcelain"]).trim(),
            "",
            "the child starts from a clean checkout of the parent's state"
        );
        drop(guard);
    }

    #[test]
    fn a_clean_child_leaves_no_branch_and_no_checkout() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let before = ok(&repo, &["branch", "--list"]);

        let guard = WorktreeGuard::isolated(&repo, "ag_clean").expect("worktree");
        // Read only: nothing to commit.
        let _ = std::fs::read_to_string(guard.path().join("tracked.txt")).unwrap();
        let (handoff, warnings) = guard.finish();

        assert!(
            handoff.is_none(),
            "a child that changed nothing has no handoff"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            ok(&repo, &["branch", "--list"]),
            before,
            "no branch is created for an empty child"
        );
        assert!(!tmp
            .path()
            .join("repo")
            .join(WORKTREE_DIR)
            .join("ag_clean")
            .exists());
    }

    /// The checkout lives inside the parent workspace, which is what every path
    /// tool's confinement check expects; it must not appear as parent work.
    #[test]
    fn the_checkout_is_inside_the_workspace_and_ignored_by_the_parent() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let guard = WorktreeGuard::isolated(&repo, "ag_inside").expect("worktree");
        assert!(
            guard.path().starts_with(&repo),
            "the child's tree must stay inside the parent workspace: {}",
            guard.path().display()
        );
        assert!(
            kc_core::paths::Workspace::new(guard.path()).is_ok(),
            "the child's tree must be usable as a workspace"
        );
        let status = ok(&repo, &["status", "--porcelain"]);
        assert!(
            !status.contains("worktrees"),
            "the child's checkout must not show up as untracked parent work: {status:?}"
        );
        // A parent edit made while the child works is still visible to the parent
        // and does not leak into the child.
        std::fs::write(repo.join("parent-later.txt"), "later\n").unwrap();
        assert!(!guard.path().join("parent-later.txt").exists());
        drop(guard);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_refused_by_name() {
        let tmp = TempDir::new();
        let err = WorktreeGuard::isolated(tmp.path(), "ag_plain")
            .err()
            .expect("a plain directory has no tree to branch from")
            .to_string();
        assert!(err.contains("subagents.worktrees"), "{err}");
        assert!(err.contains("git repository"), "{err}");
    }

    #[test]
    fn a_repository_without_commits_is_refused_with_instruction() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = tmp.path().join("empty");
        std::fs::create_dir_all(&repo).unwrap();
        ok(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        let err = WorktreeGuard::isolated(&repo, "ag_nocommit")
            .err()
            .expect("no HEAD to branch from")
            .to_string();
        assert!(err.contains("no commit"), "{err}");
        assert!(err.contains("subagents.worktrees"), "{err}");
    }

    /// A panic or a cancellation never calls `finish`, so the guard's own drop
    /// must tidy a clean checkout - and must not delete one with edits in it.
    #[test]
    fn dropping_a_guard_cleans_a_clean_checkout_but_keeps_edits() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());

        let clean_path = {
            let guard = WorktreeGuard::isolated(&repo, "ag_drop_clean").expect("worktree");
            guard.path().to_path_buf()
        };
        assert!(
            !clean_path.exists(),
            "a clean checkout is removed when the guard is dropped"
        );

        let dirty_path = {
            let guard = WorktreeGuard::isolated(&repo, "ag_drop_dirty").expect("worktree");
            std::fs::write(guard.path().join("tracked.txt"), "unrecorded edit\n").unwrap();
            guard.path().to_path_buf()
        };
        assert!(
            dirty_path.join("tracked.txt").exists(),
            "a dropped guard must not destroy edits it never recorded"
        );
        assert_eq!(
            std::fs::read_to_string(dirty_path.join("tracked.txt")).unwrap(),
            "unrecorded edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
            "committed\n",
            "the parent tree is still untouched"
        );
    }

    /// When a child's changes exist but cannot be committed to a branch, the
    /// handoff must still name them. Reporting "nothing changed" while deleting
    /// the only copy of the work is the exact failure this feature must avoid.
    #[test]
    fn an_uncommittable_child_still_hands_back_its_checkout() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let guard = WorktreeGuard::isolated(&repo, "ag_stuck").expect("worktree");
        let checkout = guard.path().to_path_buf();
        std::fs::write(checkout.join("tracked.txt"), "precious work\n").unwrap();

        // Make the branch unwritable so `finish` cannot record the work. The commit
        // itself still succeeds, which is the interesting case: the changes exist
        // and only the hand-off failed. A read-only `refs/heads` is confined to this
        // test's own repository; it does nothing when running as root, where
        // permissions do not bite, so that case is skipped rather than failed.
        let heads = repo.join(".git/refs/heads");
        let mut perms = std::fs::metadata(&heads).expect("refs/heads").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o500);
        }
        if std::fs::set_permissions(&heads, perms.clone()).is_err() {
            return;
        }
        let still_writable = std::fs::write(heads.join("probe"), "x").is_ok();
        if still_writable {
            let _ = std::fs::remove_file(heads.join("probe"));
        }

        let (handoff, warnings) = guard.finish();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o700);
        }
        let _ = std::fs::set_permissions(&heads, perms);
        if still_writable {
            // Running as root: the permission trick cannot fail the write, so
            // there is nothing to assert about an unreachable branch.
            return;
        }

        let handoff = handoff.expect("a child that changed files must always hand something back");
        assert!(
            handoff.files >= 1,
            "the change is counted at least once, got {}",
            handoff.files
        );
        assert!(handoff.branch.is_none(), "no branch could be written");
        assert_eq!(handoff.checkout, checkout);
        assert!(
            warnings.iter().any(|w| w.contains("left at")),
            "the warning must say where the work is: {warnings:?}"
        );
        assert!(
            checkout.join("tracked.txt").exists(),
            "the only copy of the work must survive a failed hand-off"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked.txt")).unwrap(),
            "precious work\n"
        );
    }

    #[test]
    fn agent_ids_become_safe_directory_names() {
        assert_eq!(safe_name("ag_abc123"), "ag_abc123");
        assert_eq!(safe_name("ag/with/slashes"), "ag-with-slashes");
        assert_eq!(safe_name(""), "agent");
        assert_eq!(safe_name(".."), "--");
    }
}
