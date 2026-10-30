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
/// * `core.pager=cat` defeats a repo-configured pager;
/// * `core.attributesFile=/dev/null` drops the *user's* global attributes file,
///   so only the repository's own `.gitattributes` can select a driver (which
///   [`filter_env`] then neutralises);
/// * `core.safecrlf=false` keeps a repo-configured `safecrlf` from turning a
///   read-only diff into a hard failure.
///
/// Clean/smudge/process filters are *not* covered here: they are named by the
/// repository, so they have to be enumerated per repository by [`filter_env`].
pub const SAFE_CONFIG: [&str; 10] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
    "-c",
    "core.attributesFile=/dev/null",
    "-c",
    "core.safecrlf=false",
];

/// Environment every read-only git invocation gets. Shared with `kc-server` so
/// the HTTP git endpoints cannot drift from the tools.
///
/// `GIT_CONFIG_NOSYSTEM`/`GIT_ATTR_NOSYSTEM` drop `/etc/gitconfig` and the
/// system attributes file: both are outside the workspace but could still name
/// a filter or diff driver that the repository then selects.
pub fn safe_env() -> [(&'static str, &'static str); 7] {
    [
        ("LC_ALL", "C"),
        ("GIT_PAGER", "cat"),
        // An empty value explicitly disables an external diff driver.
        ("GIT_EXTERNAL_DIFF", ""),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_ATTR_NOSYSTEM", "1"),
    ]
}

/// Suffixes that make up a `filter.<driver>.*` configuration key.
const FILTER_SUFFIXES: [&str; 4] = [".clean", ".smudge", ".process", ".required"];

/// How long the filter enumeration may take. It only reads config, so anything
/// slower than this is a pathological repository.
const FILTER_ENUM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn git(dir: &Path, args: &[String]) -> Result<GitRun> {
    git_with_env(dir, args, &[]).await
}

async fn git_with_env(
    dir: &Path,
    args: &[String],
    extra_env: &[(String, String)],
) -> Result<GitRun> {
    let mut command = tokio::process::Command::new("git");
    command.args(SAFE_CONFIG).args(args).current_dir(dir);
    for (key, value) in safe_env() {
        command.env(key, value);
    }
    // Inherited git variables would point the command at another repository, or
    // inject configuration the caller did not ask for.
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CONFIG_COUNT");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
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

/// Driver names configured under `filter.<name>.*` in this repository.
///
/// `git config` only *reads* configuration, so this cannot execute anything; the
/// pager is pinned by [`SAFE_CONFIG`] and `GIT_PAGER` in case a repository tries
/// to page the output. `include.path` is followed by git, so a driver pulled in
/// from an included file is enumerated too.
async fn filter_driver_names(dir: &Path) -> Result<Vec<String>> {
    let args: Vec<String> = ["config", "-z", "--name-only", "--get-regexp", r"^filter\."]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let run = match tokio::time::timeout(FILTER_ENUM_TIMEOUT, git(dir, &args)).await {
        Ok(run) => run?,
        Err(_) => {
            return Err(Error::Tool(
                "timed out enumerating git filter drivers".into(),
            ))
        }
    };
    // Exit code 1 simply means "no filter configured".
    if run.code != Some(0) && run.code != Some(1) {
        return Err(Error::Tool(format!(
            "`git config --get-regexp '^filter\\.'` failed (exit {}): {}",
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

    let mut names: Vec<String> = Vec::new();
    for key in run.stdout.split('\0').filter(|key| !key.is_empty()) {
        let Some(rest) = key.strip_prefix("filter.") else {
            continue;
        };
        // Driver names may themselves contain dots (`filter.my.driver.clean`),
        // so only the *last* known suffix is stripped.
        let Some(name) = FILTER_SUFFIXES
            .iter()
            .find_map(|suffix| rest.strip_suffix(suffix))
        else {
            continue;
        };
        if name.is_empty() || names.iter().any(|seen| seen == name) {
            continue;
        }
        names.push(name.to_string());
    }
    Ok(names)
}

/// Environment that neutralises every clean/smudge/process filter the repository
/// configures, as `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`.
///
/// `git diff` runs `filter.<driver>.clean` (and `filter.<driver>.process`) to
/// compute the working-tree side of a patch, so a repository with
/// `filter.evil.clean = /path/to/script` plus `*.txt filter=evil` in
/// `.gitattributes` executes that script on a plain `git diff`. `--no-ext-diff`
/// and `--no-textconv` do not cover this.
///
/// The overrides go through the environment rather than `-c` because a driver
/// name may contain `=` (`filter.a=b.clean`): `-c` splits on the first `=`, so
/// `-c filter.a=b.clean=` would set `filter.a` to `b.clean=` and leave the real
/// driver armed. `GIT_CONFIG_KEY_n` takes the key verbatim.
///
/// `required=false` is set as well so a repository that marks its filter
/// `required` cannot turn the neutralised diff into a hard failure.
pub async fn filter_env(dir: &Path) -> Result<Vec<(String, String)>> {
    let names = filter_driver_names(dir).await?;
    let mut env: Vec<(String, String)> = Vec::with_capacity(1 + names.len() * 8);
    env.push((
        "GIT_CONFIG_COUNT".to_string(),
        (names.len() * 4).to_string(),
    ));
    let mut index = 0usize;
    for name in &names {
        for (suffix, value) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            env.push((
                format!("GIT_CONFIG_KEY_{index}"),
                format!("filter.{name}.{suffix}"),
            ));
            env.push((format!("GIT_CONFIG_VALUE_{index}"), value.to_string()));
            index += 1;
        }
    }
    Ok(env)
}

/// The exact argv (after [`SAFE_CONFIG`]) for a read-only diff.
///
/// `--no-ext-diff` covers external diff *drivers* selected through
/// `.gitattributes` (`diff.<driver>.command`), which `diff.external=` alone does
/// not disable. `--no-textconv` does the same for `diff.<driver>.textconv`. Both
/// are repository-controlled commands that `git diff` would otherwise execute.
/// Clean/smudge/process filters are handled by [`filter_env`].
///
/// Shared with `kc-server` so the HTTP git endpoints cannot drift from the tool.
pub fn diff_args(staged: bool, base: Option<&str>, pathspec: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["diff", "--no-ext-diff", "--no-textconv"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if staged {
        args.push("--staged".to_string());
    }
    if let Some(base) = base {
        args.push(base.to_string());
    }
    args.push("--".to_string());
    if let Some(spec) = pathspec {
        args.push(spec.to_string());
    }
    args
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
    let base = match base {
        Some(base) => {
            let base = base.trim();
            if base.is_empty() {
                return Err(Error::BadRequest("base may not be empty".into()));
            }
            if base.starts_with('-') {
                return Err(Error::BadRequest(format!(
                    "invalid git ref {base:?}: refs may not start with `-`"
                )));
            }
            Some(base)
        }
        None => None,
    };

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

    // `git diff` runs `filter.<driver>.clean`/`.process` to build the
    // working-tree side of the patch, so the repository's own filter drivers
    // have to be neutralised before the diff runs.
    let filter_overrides = filter_env(&dir).await?;
    let args = diff_args(staged, base, pathspec.as_deref());

    let run = git_with_env(&dir, &args, &filter_overrides).await?;
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

    /// A repository whose `.gitattributes` selects a `filter.<driver>` that runs
    /// a marker script.
    ///
    /// The filter is configured *after* `git add`, so the setup itself never
    /// executes it and the marker starts out absent.
    fn filter_trapped_repo(tag: &str, driver: &str, keys: &[&str]) -> (PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        git_in(&dir, &["init", "-q", "."]);
        let marker = dir.join("FILTER_MARKER");
        let script = marker_script(&dir, "filter.sh", &marker);
        std::fs::write(
            dir.join(".gitattributes"),
            format!("*.txt filter={driver}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "a.txt"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        for key in keys {
            // `required` is a boolean; every other key is a command.
            let value = if *key == "required" {
                "true".to_string()
            } else {
                script.to_str().unwrap().to_string()
            };
            git_in(&dir, &["config", &format!("filter.{driver}.{key}"), &value]);
        }
        assert!(!marker.exists(), "setup must not run the filter");
        (dir, marker)
    }

    /// Hole: `git diff` runs `filter.<driver>.clean` (and `.process`) to build
    /// the working-tree side of the patch. `--no-ext-diff`/`--no-textconv` do
    /// not cover it, so a cloned repository could execute code on a plain
    /// `git_diff` — a `ToolClass::ReadOnly` tool that is auto-approved.
    #[tokio::test]
    async fn diff_never_executes_repository_configured_clean_filters() {
        let (dir, marker) = filter_trapped_repo("filter-clean", "evil", &["clean"]);

        // Control: the unhardened argv really does execute the filter, otherwise
        // the assertion below would prove nothing.
        Command::new("git")
            .args(["diff", "--no-ext-diff", "--no-textconv"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "control: `git diff` should run filter.evil.clean"
        );
        std::fs::remove_file(&marker).ok();

        let ws = Workspace::new(&dir).unwrap();
        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !marker.exists(),
            "`git diff` executed a repository-configured filter.evil.clean"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `filter.<driver>.process` is a second way to run a repository-controlled
    /// command; blanking only `clean`/`smudge` would leave the hole open.
    #[tokio::test]
    async fn diff_never_executes_repository_configured_process_filters() {
        let (dir, marker) = filter_trapped_repo("filter-process", "evil", &["process"]);

        Command::new("git")
            .args(["diff", "--no-ext-diff", "--no-textconv"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "control: `git diff` should run filter.evil.process"
        );
        std::fs::remove_file(&marker).ok();

        let ws = Workspace::new(&dir).unwrap();
        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !marker.exists(),
            "`git diff` executed a repository-configured filter.evil.process"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A driver marked `required` must not turn the neutralised diff into a hard
    /// failure: the override also sets `filter.<driver>.required=false`.
    #[tokio::test]
    async fn diff_survives_a_required_filter_without_running_it() {
        let (dir, marker) = filter_trapped_repo("filter-required", "evil", &["clean", "required"]);

        let ws = Workspace::new(&dir).unwrap();
        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !marker.exists(),
            "`git diff` executed a repository-configured filter.evil.clean"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Driver names may contain `=`, and `-c` splits on the *first* `=`, so
    /// `-c filter.a=b.clean=` would set `filter.a` to `b.clean=` and leave the
    /// real driver armed. The overrides therefore go through
    /// `GIT_CONFIG_KEY_n`, which takes the key verbatim.
    #[tokio::test]
    async fn diff_neutralises_filter_drivers_whose_name_contains_equals() {
        let (dir, marker) = filter_trapped_repo("filter-equals", "a=b", &["clean"]);

        Command::new("git")
            .args(["diff", "--no-ext-diff", "--no-textconv"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            marker.exists(),
            "control: `git diff` should run filter.a=b.clean"
        );
        std::fs::remove_file(&marker).ok();

        let ws = Workspace::new(&dir).unwrap();
        let patch = diff(&ws, None, false, None).await.unwrap();
        assert!(!patch.empty, "{}", patch.text);
        assert!(
            !marker.exists(),
            "`git diff` executed a repository-configured filter.a=b.clean"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The HTTP route in `kc-server` delegates to these helpers, so pinning them
    /// here also pins the route's argv.
    #[test]
    fn diff_args_are_the_hardened_tool_argv() {
        assert_eq!(
            diff_args(false, None, None),
            vec!["diff", "--no-ext-diff", "--no-textconv", "--"]
        );
        assert_eq!(
            diff_args(true, Some("main"), Some("a.txt")),
            vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--staged",
                "main",
                "--",
                "a.txt"
            ]
        );
        // The path always follows `--`, so it can never be read as a flag.
        assert_eq!(
            diff_args(false, None, Some("--upload-pack=evil")),
            vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                "--upload-pack=evil"
            ]
        );
    }

    #[tokio::test]
    async fn filter_env_neutralises_every_configured_driver() {
        let dir = temp_dir("filter-env");
        git_in(&dir, &["init", "-q", "."]);
        git_in(&dir, &["config", "filter.evil.clean", "/bin/false"]);
        git_in(&dir, &["config", "filter.evil.required", "true"]);
        // A dotted driver name must not be truncated to `my`.
        git_in(&dir, &["config", "filter.my.driver.smudge", "/bin/false"]);

        let env = filter_env(&dir).await.unwrap();
        let map: std::collections::HashMap<&str, &str> =
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(map.get("GIT_CONFIG_COUNT"), Some(&"8"));

        let keys: Vec<&str> = env
            .iter()
            .filter(|(k, _)| k.starts_with("GIT_CONFIG_KEY_"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert!(keys.contains(&"filter.evil.clean"), "{keys:?}");
        assert!(keys.contains(&"filter.evil.smudge"), "{keys:?}");
        assert!(keys.contains(&"filter.evil.process"), "{keys:?}");
        assert!(keys.contains(&"filter.evil.required"), "{keys:?}");
        assert!(keys.contains(&"filter.my.driver.clean"), "{keys:?}");
        assert!(keys.contains(&"filter.my.driver.smudge"), "{keys:?}");
        assert!(keys.contains(&"filter.my.driver.process"), "{keys:?}");
        assert!(keys.contains(&"filter.my.driver.required"), "{keys:?}");

        // Every command is blanked; `required` is forced off so the neutralised
        // diff still succeeds.
        for (key, value) in &env {
            if key.starts_with("GIT_CONFIG_KEY_") {
                continue;
            }
            if key.starts_with("GIT_CONFIG_VALUE_") {
                let index = key.trim_start_matches("GIT_CONFIG_VALUE_");
                let name = map
                    .get(format!("GIT_CONFIG_KEY_{index}").as_str())
                    .copied()
                    .unwrap();
                if name.ends_with(".required") {
                    assert_eq!(value, "false", "{name}");
                } else {
                    assert_eq!(value, "", "{name}");
                }
            }
        }

        // A repository with no filters needs no overrides at all.
        let plain = temp_dir("filter-env-plain");
        git_in(&plain, &["init", "-q", "."]);
        let env = filter_env(&plain).await.unwrap();
        assert_eq!(env, vec![("GIT_CONFIG_COUNT".to_string(), "0".to_string())]);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&plain).ok();
    }
}
