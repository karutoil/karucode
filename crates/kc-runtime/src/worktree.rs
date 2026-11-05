//! Shared-workspace selection for subagents.
//!
//! Automatic git worktrees are intentionally not used. They start at `HEAD`, so
//! a child cannot see its parent's dirty and untracked files, and there is no
//! durable patch-handoff protocol that could make force-removing the checkout
//! safe. Until such a protocol exists, every child works in the parent workspace
//! and its completed edits stay there. `owned_paths` provides conflict warnings,
//! not filesystem isolation or locking.

use std::path::{Path, PathBuf};

/// A subagent's borrowed view of the shared parent workspace.
///
/// The legacy type name keeps the runtime module boundary small; this guard
/// never creates, deletes, or cleans a git worktree.
pub struct WorktreeGuard {
    workspace_path: PathBuf,
}

impl WorktreeGuard {
    /// Use the parent's live workspace, including dirty and untracked state.
    pub fn fallback(workspace_root: &Path) -> Self {
        Self {
            workspace_path: workspace_root.to_path_buf(),
        }
    }

    /// The exact tree used by the parent's tools and checkpoint journal.
    pub fn path(&self) -> &Path {
        &self.workspace_path
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

    fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("tracked.txt"), "committed\n").expect("write tracked file");
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

    #[test]
    fn a_git_subagent_sees_the_parents_dirty_and_untracked_files() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        std::fs::write(repo.join("tracked.txt"), "parent dirty\n").expect("dirty tracked file");
        std::fs::write(repo.join("parent-only.txt"), "untracked parent state\n")
            .expect("write untracked file");
        let worktrees_before = git(&repo, &["worktree", "list", "--porcelain"]);

        let guard = WorktreeGuard::fallback(&repo);

        assert_eq!(guard.path(), repo.as_path());
        assert_eq!(
            std::fs::read_to_string(guard.path().join("tracked.txt")).unwrap(),
            "parent dirty\n",
            "a clean HEAD checkout would hide this edit"
        );
        assert_eq!(
            std::fs::read_to_string(guard.path().join("parent-only.txt")).unwrap(),
            "untracked parent state\n"
        );
        assert_eq!(
            git(&repo, &["worktree", "list", "--porcelain"]),
            worktrees_before,
            "selecting a child workspace must not create a git worktree"
        );
    }

    #[test]
    fn completed_edits_survive_the_shared_workspace_guard() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new();
        let repo = init_repo(tmp.path());
        let guard = WorktreeGuard::fallback(&repo);
        std::fs::write(guard.path().join("tracked.txt"), "child edit\n").expect("child edit");
        std::fs::write(guard.path().join("child-created.txt"), "new child file\n")
            .expect("child-created file");

        drop(guard);

        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
            "child edit\n",
            "completion must not roll back a successful child edit"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("child-created.txt")).unwrap(),
            "new child file\n",
            "completion must not force-delete an untracked child file"
        );
        assert!(repo.is_dir(), "the parent workspace itself must survive");
    }

    #[test]
    fn a_plain_workspace_also_uses_the_parent_tree() {
        let tmp = TempDir::new();
        let guard = WorktreeGuard::fallback(tmp.path());
        assert_eq!(guard.path(), tmp.path());
    }
}
