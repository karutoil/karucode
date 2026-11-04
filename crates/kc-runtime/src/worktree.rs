//! Git worktree management for subagents.
//!
//! When a subagent is spawned, a git worktree is created (if the repository
//! supports it) to isolate its work from the parent and sibling agents. This
//! prevents concurrent file edits from conflicting and allows independent
//! checkouts of different branches or commits.
//!
//! A worktree is created with a unique name derived from the agent id, and is
//! cleaned up when the subagent's loop completes (success or failure). The
//! cleanup happens in the drop handler.

use kc_core::ids::AgentId;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A git worktree tied to a subagent's lifetime.
pub struct WorktreeGuard {
    /// The worktree directory itself (where work happens).
    pub worktree_path: PathBuf,
    /// The git repository root (needed for cleanup).
    repo_root: PathBuf,
    /// Whether we actually created a worktree or fell back to the main workspace.
    is_active: bool,
}

impl WorktreeGuard {
    /// Try to create a git worktree for a subagent.
    ///
    /// If the workspace is a git repository and worktree creation succeeds, this
    /// returns an active guard pointing to a new worktree directory. If the
    /// workspace is not a repo, not a valid git repo, or git is unavailable, this
    /// returns an inactive guard that points to the original workspace path
    /// (transparent fallback).
    pub async fn try_create(workspace_root: &Path, agent_id: &AgentId) -> Result<Self, String> {
        // First, check if this is a git repository and if git is available.
        let is_git_repo = is_git_repository(workspace_root).await;
        if !is_git_repo {
            // Not a git repo; fall back to the main workspace.
            return Ok(Self {
                worktree_path: workspace_root.to_path_buf(),
                repo_root: workspace_root.to_path_buf(),
                is_active: false,
            });
        }

        let repo_root = workspace_root.to_path_buf();
        let agent_id_short: String = agent_id
            .0
            .chars()
            .rev()
            .take(12)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let worktree_name = format!("karucode-wt-{}", agent_id_short);

        // Place the worktree in a temporary location adjacent to the repository.
        // This avoids polluting .git/worktrees with agent ids that mean nothing
        // outside the runtime, and makes cleanup easier if something goes wrong.
        let worktree_parent = repo_root.parent().unwrap_or_else(|| Path::new("/tmp"));
        let worktree_path = worktree_parent.join(&worktree_name);

        match create_worktree(&repo_root, &worktree_path, &worktree_name).await {
            Ok(_) => Ok(Self {
                worktree_path,
                repo_root,
                is_active: true,
            }),
            Err(e) => {
                // Worktree creation failed; fall back to the main workspace.
                tracing::warn!(
                    agent_id = %agent_id.0,
                    worktree_name = %worktree_name,
                    error = %e,
                    "failed to create git worktree, falling back to main workspace"
                );
                Ok(Self {
                    worktree_path: repo_root.clone(),
                    repo_root,
                    is_active: false,
                })
            }
        }
    }

    /// Create a fallback worktree guard (inactive) that uses the main workspace.
    /// Used when all creation attempts have failed.
    pub fn fallback(workspace_root: &Path) -> Self {
        Self {
            worktree_path: workspace_root.to_path_buf(),
            repo_root: workspace_root.to_path_buf(),
            is_active: false,
        }
    }

    /// The path where this subagent should work.
    pub fn path(&self) -> &Path {
        &self.worktree_path
    }

    /// Whether this is an active worktree (true) or a fallback to main workspace (false).
    pub fn is_active(&self) -> bool {
        self.is_active
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if !self.is_active {
            return;
        }
        // Clean up the worktree synchronously in the drop handler.
        // We cannot use async here, so we use blocking operations.
        let worktree_name = self
            .worktree_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        match std::process::Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(worktree_name)
            .current_dir(&self.repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => {
                tracing::debug!(
                    worktree = %worktree_name,
                    "cleaned up git worktree"
                );
            }
            Ok(status) => {
                tracing::warn!(
                    worktree = %worktree_name,
                    status = ?status,
                    "failed to clean up git worktree (non-zero exit)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    worktree = %worktree_name,
                    error = %e,
                    "failed to clean up git worktree"
                );
            }
        }
    }
}

/// Check if a path is inside a git repository by looking for .git directory.
async fn is_git_repository(path: &Path) -> bool {
    // First, quickly check if .git exists as a directory or file (submodule).
    // This is much faster than spawning a git command.
    let git_path = path.join(".git");
    if git_path.exists() {
        return true;
    }

    // If .git doesn't exist at this level, we can return false immediately.
    // In a proper git workflow, .git will always be present at the root.
    // Falling back to `git rev-parse` would be expensive and unnecessary.
    false
}

/// Create a new git worktree for a subagent.
async fn create_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    worktree_name: &str,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("worktree")
        .arg("add")
        .arg("--quiet")
        // Detached on purpose. Without it `git worktree add <path>` also creates a
        // branch named after the directory, and `git worktree remove` deletes only
        // the working tree: the branch would outlive every subagent in the user's
        // repository. A scratch tree has no use for a branch.
        .arg("--detach")
        .arg(worktree_path)
        .current_dir(repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("failed to spawn git worktree command: {}", e))?;

    if output.status.success() {
        tracing::debug!(
            worktree = %worktree_name,
            path = %worktree_path.display(),
            "created git worktree for subagent"
        );
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("git worktree add failed: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Stdio;

    /// A throwaway directory that removes itself. The crate has no `tempfile`
    /// dependency and these tests need a real repository on disk.
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
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Whether git is installed. The worktree feature degrades to a no-op fallback
    /// without it, so a machine without git has nothing to assert here.
    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// A repository with one commit: `git worktree add` needs a resolvable HEAD.
    fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "hi\n").expect("write file");
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        repo
    }

    fn branch_names(repo: &Path) -> Vec<String> {
        git(repo, &["branch", "--list", "--format=%(refname:short)"])
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn a_git_workspace_gets_a_worktree_that_is_removed_on_drop() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let guard = WorktreeGuard::try_create(&repo, &AgentId::new())
            .await
            .expect("try_create is infallible and falls back internally");
        assert!(guard.is_active(), "a git workspace must get a worktree");
        let worktree = guard.path().to_path_buf();
        assert_ne!(worktree, repo, "the worktree must not be the main tree");
        assert!(worktree.is_dir(), "the worktree directory must exist");

        drop(guard);

        assert!(
            !worktree.exists(),
            "drop must remove the worktree directory {}",
            worktree.display()
        );
        let listed = git(&repo, &["worktree", "list", "--porcelain"]);
        assert!(
            !listed.contains("karucode-wt-"),
            "drop must unregister the worktree; git still lists:\n{listed}"
        );
    }

    /// Regression: `git worktree add <path>` also creates a branch, and
    /// `git worktree remove` deletes only the working tree. Every subagent then
    /// left a `karucode-wt-<id>` branch in the user's repository forever, while
    /// `git worktree list` made the cleanup look complete.
    #[tokio::test]
    async fn creating_and_removing_a_worktree_leaves_no_branch_behind() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let before = branch_names(&repo);

        let guard = WorktreeGuard::try_create(&repo, &AgentId::new())
            .await
            .expect("try_create is infallible and falls back internally");

        let during = branch_names(&repo);
        assert_eq!(
            before, during,
            "creating a worktree must not add a branch (a leaked karucode-wt-* is the bug)"
        );

        drop(guard);

        let after = branch_names(&repo);
        assert_eq!(
            before, after,
            "removing a worktree must leave the branch list exactly as it was"
        );
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_git_repository_uses_the_main_tree() {
        let tmp = TempDir::new();
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).expect("create plain dir");

        let guard = WorktreeGuard::try_create(&plain, &AgentId::new())
            .await
            .expect("try_create is infallible and falls back internally");

        assert!(!guard.is_active(), "no repository means no worktree");
        assert_eq!(
            guard.path(),
            plain.as_path(),
            "the workspace is the fallback"
        );
    }
}
