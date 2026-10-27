//! Read-only git inspection. Both tools invoke `git` with an explicit argv (no
//! shell) and never mutate the repository.

use kc_core::paths::Workspace;
use kc_core::{Error, Result};
use std::path::{Path, PathBuf};

struct GitRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub text: String,
    pub is_repo: bool,
    pub branch: Option<String>,
    pub entries: usize,
}

#[derive(Debug, Clone)]
pub struct DiffReport {
    pub text: String,
    pub empty: bool,
}

/// Config overrides passed before the subcommand so a cloned repository cannot
/// make a read-only inspection run code from its own config:
///
/// * `core.fsmonitor=false` disables the fsmonitor hook (`git status` runs it);
/// * `diff.external=` disables `GIT_EXTERNAL_DIFF`/`diff.external` (`git diff`
///   would otherwise exec it);
/// * `core.pager=cat` defeats a repo-configured pager.
const SAFE_CONFIG: [&str; 6] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
];

async fn git(dir: &Path, args: &[String]) -> Result<GitRun> {
    let output = tokio::process::Command::new("git")
        .args(SAFE_CONFIG)
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        // An empty value explicitly disables an external diff driver.
        .env("GIT_EXTERNAL_DIFF", "")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        // Inherited git variables would point the command at another repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Tool("git executable not found on PATH".into())
            } else {
                Error::Tool(format!("failed to run `git {}`: {e}", args.join(" ")))
            }
        })?;
    Ok(GitRun {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn dir_for(ws: &Workspace, path: Option<&str>) -> Result<PathBuf> {
    let abs = ws.resolve_existing(path.unwrap_or("."))?;
    if abs.is_dir() {
        Ok(abs)
    } else {
        Ok(abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| ws.root().to_path_buf()))
    }
}

/// `git status --porcelain=v1 -b`, summarised with the branch name.
pub async fn status(ws: &Workspace, path: Option<&str>) -> Result<StatusReport> {
    let dir = dir_for(ws, path)?;
    let args: Vec<String> = ["status", "--porcelain=v1", "-b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let run = git(&dir, &args).await?;

    if is_not_a_repo(&run) {
        let rel = ws.relative(&dir);
        return Ok(StatusReport {
            is_repo: false,
            branch: None,
            entries: 0,
            text: format!(
                "is_repo: false — `{rel}` is not inside a git repository (or git is not initialised). \
                 No branch or changes are available."
            ),
        });
    }
    if run.code != Some(0) {
        return Err(Error::Tool(format!(
            "`git status` failed in `{}` (exit {}): {}",
            ws.relative(&dir),
            run.code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
            if run.stderr.is_empty() {
                "no output".to_string()
            } else {
                run.stderr
            }
        )));
    }

    let mut branch: Option<String> = None;
    let mut entries: Vec<String> = Vec::new();
    for line in run.stdout.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            branch = Some(parse_branch(rest));
            continue;
        }
        if !line.trim().is_empty() {
            entries.push(line.to_string());
        }
    }

    let mut text = format!(
        "is_repo: true\nbranch: {}",
        branch.as_deref().unwrap_or("(unknown)")
    );
    if entries.is_empty() {
        text.push_str("\nworking tree clean");
    } else {
        text.push_str(&format!("\nchanges ({}):", entries.len()));
        for entry in &entries {
            text.push_str("\n  ");
            text.push_str(entry);
        }
    }

    Ok(StatusReport {
        text,
        is_repo: true,
        branch,
        entries: entries.len(),
    })
}

fn is_not_a_repo(run: &GitRun) -> bool {
    run.code != Some(0) && run.stderr.contains("not a git repository")
}

/// `## main...origin/main [ahead 1]` -> `main`.
fn parse_branch(header: &str) -> String {
    let rest = header.trim();
    let rest = rest.strip_prefix("No commits yet on ").unwrap_or(rest);
    if rest.starts_with("HEAD (no branch)") {
        return "(detached HEAD)".to_string();
    }
    let name = rest.split("...").next().unwrap_or(rest);
    let name = name.split(' ').next().unwrap_or(name);
    name.trim().to_string()
}

/// `git diff [--staged] [base] -- <path>`.
pub async fn diff(
    ws: &Workspace,
    path: Option<&str>,
    staged: bool,
    base: Option<&str>,
) -> Result<DiffReport> {
    // `--no-ext-diff` covers external diff *drivers* selected through
    // `.gitattributes` (`diff.<driver>.command`), which `diff.external=` alone
    // does not disable. `--no-textconv` does the same for `diff.<driver>.textconv`:
    // both are repository-controlled commands that `git diff` would execute.
    let mut args: Vec<String> = ["diff", "--no-ext-diff", "--no-textconv"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if staged {
        args.push("--staged".to_string());
    }
    if let Some(base) = base {
        let base = base.trim();
        if base.is_empty() {
            return Err(Error::BadRequest("base may not be empty".into()));
        }
        if base.starts_with('-') {
            return Err(Error::BadRequest(format!(
                "invalid git ref {base:?}: refs may not start with `-`"
            )));
        }
        args.push(base.to_string());
    }

    // Resolve the working directory and optional pathspec together so a file
    // argument works without shell quoting.
    let (dir, pathspec) = match path {
        Some(p) => {
            let abs = ws.resolve_existing(p)?;
            if abs.is_dir() {
                (abs, None)
            } else {
                let dir = ws.root().to_path_buf();
                let rel = ws.relative(&abs);
                (dir, Some(rel))
            }
        }
        None => (ws.root().to_path_buf(), None),
    };
    args.push("--".to_string());
    if let Some(spec) = &pathspec {
        args.push(spec.clone());
    }

    let run = git(&dir, &args).await?;
    if is_not_a_repo(&run) {
        let rel = ws.relative(&dir);
        return Ok(DiffReport {
            text: format!(
                "is_repo: false — `{rel}` is not inside a git repository (or git is not initialised)."
            ),
            empty: true,
        });
    }
    match run.code {
        Some(0) | Some(1) => {}
        other => {
            return Err(Error::Tool(format!(
                "`git {}` failed in `{}` (exit {}): {}",
                args.join(" "),
                ws.relative(&dir),
                other
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                if run.stderr.is_empty() {
                    "no output".to_string()
                } else {
                    run.stderr
                }
            )))
        }
    }

    if run.stdout.trim().is_empty() {
        let scope = match (staged, base, pathspec.as_deref()) {
            (true, _, Some(p)) => format!("No staged changes for `{p}`"),
            (true, _, None) => "No staged changes".to_string(),
            (false, Some(b), Some(p)) => format!("No changes for `{p}` against `{b}`"),
            (false, Some(b), None) => format!("No changes against `{b}`"),
            (false, None, Some(p)) => format!("No unstaged changes for `{p}`"),
            (false, None, None) => {
                "No unstaged changes (working tree matches the index)".to_string()
            }
        };
        return Ok(DiffReport {
            text: scope,
            empty: true,
        });
    }

    Ok(DiffReport {
        text: run.stdout,
        empty: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("kc-tools-git-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("the git-tool tests need `git` on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn marker_script(dir: &Path, name: &str, marker: &Path) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A repository whose own config runs marker scripts on `status`/`diff`.
    fn trapped_repo(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        git_in(&dir, &["init", "-q", "."]);
        let fs_marker = dir.join("FSMONITOR_MARKER");
        let diff_marker = dir.join("DIFF_MARKER");
        let fs_hook = marker_script(&dir, "fsmonitor.sh", &fs_marker);
        let diff_hook = marker_script(&dir, "diff.sh", &diff_marker);
        git_in(
            &dir,
            &["config", "core.fsmonitor", fs_hook.to_str().unwrap()],
        );
        git_in(
            &dir,
            &["config", "diff.external", diff_hook.to_str().unwrap()],
        );
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "a.txt"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        (dir, fs_marker, diff_marker)
    }

    #[tokio::test]
    async fn status_and_diff_never_execute_repository_configured_hooks() {
        let (dir, fs_marker, diff_marker) = trapped_repo("config-hooks");

        // Control: without the hardening the repository really does execute
        // both hooks; otherwise the assertions below would prove nothing.
        Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            fs_marker.exists(),
            "control: `git status` should run core.fsmonitor"
        );
        std::fs::remove_file(&fs_marker).ok();
        Command::new("git")
            .args(["diff"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            diff_marker.exists(),
            "control: `git diff` should run diff.external"
        );
        std::fs::remove_file(&fs_marker).ok();
        std::fs::remove_file(&diff_marker).ok();

        let ws = Workspace::new(&dir).unwrap();
        let report = status(&ws, None).await.unwrap();
        assert!(report.is_repo, "{}", report.text);
        assert!(
            !fs_marker.exists(),
            "`git status` executed a repository-configured core.fsmonitor"
        );
        assert!(!diff_marker.exists());

        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !fs_marker.exists(),
            "`git diff` executed a repository-configured core.fsmonitor"
        );
        assert!(
            !diff_marker.exists(),
            "`git diff` executed a repository-configured diff.external"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn diff_ignores_attributes_selected_external_drivers_and_textconv() {
        let dir = temp_dir("attribute-drivers");
        git_in(&dir, &["init", "-q", "."]);
        let ext_marker = dir.join("EXT_MARKER");
        let conv_marker = dir.join("TEXTCONV_MARKER");
        let ext = marker_script(&dir, "ext.sh", &ext_marker);
        let conv = marker_script(&dir, "conv.sh", &conv_marker);
        std::fs::write(dir.join(".gitattributes"), "*.txt diff=evil\n").unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "a.txt"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();

        // Control: an external driver from .gitattributes + repo config runs.
        git_in(
            &dir,
            &["config", "diff.evil.command", ext.to_str().unwrap()],
        );
        Command::new("git")
            .args(["diff"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            ext_marker.exists(),
            "control: `git diff` should run diff.evil.command"
        );
        git_in(&dir, &["config", "--unset", "diff.evil.command"]);
        std::fs::remove_file(&ext_marker).ok();

        // Control: a textconv driver executes too.
        git_in(
            &dir,
            &["config", "diff.evil.textconv", conv.to_str().unwrap()],
        );
        Command::new("git")
            .args(["diff"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            conv_marker.exists(),
            "control: `git diff` should run diff.evil.textconv"
        );
        git_in(&dir, &["config", "--unset", "diff.evil.textconv"]);
        std::fs::remove_file(&ext_marker).ok();
        std::fs::remove_file(&conv_marker).ok();

        // Hardened: both drivers are still configured, but neither may run.
        git_in(
            &dir,
            &["config", "diff.evil.command", ext.to_str().unwrap()],
        );
        git_in(
            &dir,
            &["config", "diff.evil.textconv", conv.to_str().unwrap()],
        );
        let ws = Workspace::new(&dir).unwrap();
        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !ext_marker.exists(),
            "`git diff` ran an external diff driver"
        );
        assert!(!conv_marker.exists(), "`git diff` ran a textconv driver");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn branch_header_is_parsed() {
        assert_eq!(parse_branch("main"), "main");
        assert_eq!(parse_branch("main...origin/main [ahead 1]"), "main");
        assert_eq!(parse_branch("No commits yet on master"), "master");
        assert_eq!(parse_branch("HEAD (no branch)"), "(detached HEAD)");
    }
}
