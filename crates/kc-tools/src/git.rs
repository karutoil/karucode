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

async fn git(dir: &Path, args: &[String]) -> Result<GitRun> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
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
    let mut args: Vec<String> = vec!["diff".to_string()];
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
    use super::parse_branch;

    #[test]
    fn branch_header_is_parsed() {
        assert_eq!(parse_branch("main"), "main");
        assert_eq!(parse_branch("main...origin/main [ahead 1]"), "main");
        assert_eq!(parse_branch("No commits yet on master"), "master");
        assert_eq!(parse_branch("HEAD (no branch)"), "(detached HEAD)");
    }
}
