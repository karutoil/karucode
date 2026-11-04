//! Read-only git inspection. Both tools invoke `git` with an explicit argv (no
//! shell) and never mutate the repository.

use kc_core::paths::Workspace;
use kc_core::{Error, Result};
use std::path::Path;

const GIT_STDOUT_LIMIT: usize = 1024 * 1024;
const GIT_STDERR_LIMIT: usize = 64 * 1024;

struct GitRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub text: String,
    pub is_repo: bool,
    pub branch: Option<String>,
    pub entries: usize,
    /// True when the workspace is a *subdirectory* of the repository, so the
    /// report is confined to it and the enclosing repository is bigger.
    pub confined: bool,
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
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Tool("git executable not found on PATH".into())
            } else {
                Error::Tool(format!("failed to run `git {}`: {e}", args.join(" ")))
            }
        })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (stdout, stderr, status) = tokio::join!(
        crate::bounded_pipe::read(stdout, GIT_STDOUT_LIMIT),
        crate::bounded_pipe::read(stderr, GIT_STDERR_LIMIT),
        child.wait()
    );
    let (stdout, stdout_truncated) =
        stdout.map_err(|e| Error::Tool(format!("failed to read git stdout: {e}")))?;
    let (stderr, stderr_truncated) =
        stderr.map_err(|e| Error::Tool(format!("failed to read git stderr: {e}")))?;
    let status = status.map_err(|e| Error::Tool(format!("failed to wait for git: {e}")))?;
    let mut stderr = String::from_utf8_lossy(&stderr).trim().to_string();
    if stderr_truncated {
        stderr.push_str("\n[git stderr truncated]");
    }
    Ok(GitRun {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr,
        stdout_truncated,
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
    // An incomplete list would leave unenumerated executable filters enabled.
    if run.stdout_truncated {
        return Err(Error::Tool(
            "git filter enumeration exceeded its output limit".into(),
        ));
    }
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

/// Upper bound on any ref the HTTP layer or a tool hands to git. Long enough for
/// a real branch name, short enough that a ref cannot be used as a payload.
pub const MAX_REF_CHARS: usize = 120;

/// Validate a *revision* argument (a `base=`, a start point, a commit range):
/// the conservative `^[A-Za-z0-9._/~^-]{1,120}$` grammar the read-only diff
/// endpoint has always used, because a ref is the one string that ends up in a
/// flag position (`git diff <base>`, `git switch -c <new> <start>`).
///
/// A leading `-` is refused separately from the character class: `-c` and
/// `--upload-pack=…` are valid characters inside the class but are options, not
/// refs, and git parses them as such.
///
/// Returned as a plain `String` so both callers can wrap it in their own error
/// type (`kc_core::Error::BadRequest`, `ApiError::bad_request`) without this
/// module depending on either.
pub fn validate_revision_arg(value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > MAX_REF_CHARS {
        return Err(format!("a git ref must be 1-{MAX_REF_CHARS} characters"));
    }
    if value.starts_with('-') {
        return Err("a git ref may not start with `-`".to_string());
    }
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '~' | '^' | '-');
    if !value.chars().all(allowed) {
        return Err("a git ref may only contain letters, digits and . _ / ~ ^ -".to_string());
    }
    Ok(())
}

/// Validate a name that will be *created* as a branch (`git branch <name>`,
/// `git switch -c <name>`).
///
/// Stricter than [`validate_revision_arg`], because a revision may legitimately
/// carry `~`/`^`/`..` (`HEAD~3`, `main..feat`) while a branch name may not. The
/// rules are the subset of `git check-ref-format --branch` that a UI can explain
/// without a subprocess: printable ASCII without whitespace, no `~^:?*[\`, no
/// `..`, no `@{`, no `//`, no leading/trailing `/`, no trailing `.` or `.lock`,
/// and no `-` first (it would parse as an option and also reads as a flag).
pub fn validate_branch_name(value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > MAX_REF_CHARS {
        return Err(format!(
            "a branch name must be 1-{MAX_REF_CHARS} characters"
        ));
    }
    if value.starts_with('-') {
        return Err("a branch name may not start with `-`".to_string());
    }
    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return Err("a branch name may not begin, end or double a `/`".to_string());
    }
    if value.ends_with('.') || value.ends_with(".lock") {
        return Err("a branch name may not end with `.` or `.lock`".to_string());
    }
    if value.contains("..") || value.contains("@{") || value == "@" {
        return Err("a branch name may not contain `..`, `@{` or be `@`".to_string());
    }
    for c in value.chars() {
        if c.is_ascii_control()
            || c.is_ascii_whitespace()
            || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | ']' | '\\')
        {
            return Err(format!("a branch name may not contain {c:?}"));
        }
    }
    Ok(())
}

/// How a read-only git command is confined to the workspace.
///
/// # The policy: confine to the workspace, never report the enclosing repository
///
/// git discovers a repository by walking *up* from its working directory, so a
/// workspace that is a subdirectory of a larger repository (a checkout under a
/// home-directory repo, a package inside a monorepo, `~/dotfiles/editors`) used
/// to report that whole repository: `git_status` listed sibling paths outside the
/// workspace and `git_diff` printed their contents. Both tools are read-only and
/// auto-approved in every mode, so the disclosure happened with no approval
/// prompt, and it contradicted the file tools, which refuse the same paths.
///
/// Two policies fix that; this module takes the second.
///
/// (a) *Refuse* when `git rev-parse --show-toplevel` is not the workspace root.
///     Simple, but it breaks an ordinary layout - a project checked out inside a
///     bigger repository is not an attack - and it removes `git_status` outright
///     for it while `run_command` still has the whole filesystem.
/// (b) *Confine*: run git from the workspace root with an explicit pathspec.
///     Keeps the tool useful and makes it agree with the file tools.
///
/// Confinement is built from three pieces, because no single git flag covers both
/// tools:
///
/// * an explicit pathspec (`.` for the whole workspace, or the workspace-relative
///   `path` argument) after `--`, which is what actually stops git at the
///   workspace boundary;
/// * `GIT_LITERAL_PATHSPECS=1` ([`literal_pathspec_env`]), so a workspace file
///   named `a[1].txt` or `*.rs` cannot turn the pathspec into a glob that matches
///   something else;
/// * path re-basing. `git diff` accepts `--relative`, which rewrites the `a/…`
///   `b/…` headers against the working directory (the workspace root). `git
///   status` has **no** `--relative` - porcelain paths are always relative to the
///   repository root - so its paths are re-based by stripping the
///   `git rev-parse --show-prefix` of the workspace root.
struct Scope {
    /// The resolved `path` argument, relative to the workspace root, for messages.
    target_rel: String,
    /// The pathspec handed to git: workspace-relative, `.` for the whole workspace.
    pathspec: String,
    /// `git rev-parse --show-prefix` for the workspace root: empty at the top of
    /// the repository, `inner/` when the workspace is a subdirectory of it.
    prefix: String,
    /// `git rev-parse --show-toplevel`, only used to explain the confinement.
    toplevel: String,
}

impl Scope {
    /// Whether the workspace is a strict subdirectory of its repository, i.e.
    /// whether anything had to be confined away.
    fn confined(&self) -> bool {
        !self.prefix.is_empty()
    }

    /// Re-base a repository-relative path on the workspace root.
    ///
    /// `None` means the path is outside the workspace. The pathspec makes that
    /// impossible for the paths git reports, with one exception: the *source* side
    /// of a rename that moved a file into the workspace from elsewhere in the
    /// enclosing repository.
    fn rebase(&self, repo_path: &str) -> Option<String> {
        if self.prefix.is_empty() {
            return Some(repo_path.to_string());
        }
        let rel = repo_path.strip_prefix(self.prefix.as_str())?;
        if rel.is_empty() {
            Some(".".to_string())
        } else {
            Some(rel.to_string())
        }
    }
}

/// Environment that stops git from reading a pathspec as a glob.
///
/// Deliberately *not* part of [`safe_env`], which `kc-server` shares: this is the
/// tools' confinement policy, and keeping it here means the HTTP routes keep the
/// argv they had. The *mutating* git routes opt in explicitly (see
/// `kc_server::routes::git`), because `git add -- a[1].txt` would otherwise stage
/// a glob match and `git restore -- *.rs` would discard files the user never
/// selected.
pub fn literal_pathspec_env() -> [(String, String); 1] {
    [("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string())]
}

/// Where the workspace sits inside its repository.
///
/// `Ok(None)` is "not a git repository" - the caller reports `is_repo: false`
/// exactly as before, so the answer for a non-repository workspace is unchanged.
async fn repo_scope(ws: &Workspace, path: Option<&str>) -> Result<Option<Scope>> {
    let target = ws.resolve_existing(path.unwrap_or("."))?;
    // A file argument selects the file; a directory (or the default `.`) selects
    // the subtree, so the pathspec is the argument itself either way.
    let target_rel = ws.relative(&target);
    let root = ws.root().to_path_buf();
    let args: Vec<String> = ["rev-parse", "--show-toplevel", "--show-prefix"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let run = git(&root, &args).await?;
    if is_not_a_repo(&run) {
        return Ok(None);
    }
    if run.code != Some(0) {
        // A bare repository, for instance: `status`/`diff` cannot work in a work
        // tree that does not exist, and this is the process that finds out.
        return Err(Error::Tool(format!(
            "`git rev-parse` failed in `{}` (exit {}): {}",
            ws.relative(&root),
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
    if run.stdout_truncated {
        return Err(Error::Tool(
            "git repository scope exceeded its output limit".into(),
        ));
    }
    let mut lines = run.stdout.lines();
    let toplevel = lines.next().unwrap_or_default().to_string();
    let prefix = lines.next().unwrap_or_default().to_string();
    Ok(Some(Scope {
        target_rel: target_rel.clone(),
        pathspec: target_rel,
        prefix,
        toplevel,
    }))
}

/// `git status --porcelain=v1 -b`, summarised with the branch name and confined
/// to the workspace (see [`Scope`]).
pub async fn status(ws: &Workspace, path: Option<&str>) -> Result<StatusReport> {
    let Some(scope) = repo_scope(ws, path).await? else {
        return Ok(StatusReport {
            is_repo: false,
            branch: None,
            entries: 0,
            confined: false,
            text: format!(
                "is_repo: false — `{}` is not inside a git repository (or git is not initialised). \
                 No branch or changes are available.",
                ws.relative(&ws.resolve_existing(path.unwrap_or("."))?)
            ),
        });
    };
    // `-z` for machine parsing: paths arrive unquoted and NUL-terminated, so a
    // name containing `"`, `\` or a newline cannot be mis-parsed, and the
    // workspace prefix can be stripped from the raw name.
    let mut args: Vec<String> = ["status", "--porcelain=v1", "-b", "-z"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push("--".to_string());
    args.push(scope.pathspec.clone());
    let run = git_with_env(ws.root(), &args, &literal_pathspec_env()).await?;

    if is_not_a_repo(&run) {
        return Ok(StatusReport {
            is_repo: false,
            branch: None,
            entries: 0,
            confined: false,
            text: format!(
                "is_repo: false — `{}` is not inside a git repository (or git is not initialised). \
                 No branch or changes are available.",
                scope.target_rel
            ),
        });
    }
    if run.code != Some(0) {
        return Err(Error::Tool(format!(
            "`git status` failed in `{}` (exit {}): {}",
            scope.target_rel,
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

    if run.stdout_truncated {
        return Err(Error::Tool(
            "git status exceeded its output limit; narrow the path".into(),
        ));
    }
    let (branch, entries) = parse_status_records(&run.stdout, &scope);

    let mut text = format!(
        "is_repo: true\nbranch: {}",
        branch.as_deref().unwrap_or("(unknown)")
    );
    if scope.confined() {
        // Say it explicitly. The workspace root is an ancestor of nothing here:
        // the enclosing repository is the workspace's *parent*, and a model that
        // is told only "working tree clean" would wrongly conclude that the
        // repository has no other files.
        text.push_str(&format!(
            "\n[confined to this workspace, which is a subdirectory of the git repository at {}: \
             changes outside it are not listed and their paths are not revealed]",
            scope.toplevel
        ));
    }
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
        confined: scope.confined(),
    })
}

/// Parse `git status --porcelain=v1 -b -z` into the branch and one display line
/// per change, with every path re-based on the workspace root.
///
/// Each record is `<XY> <path>`; a rename or copy (`R`/`C`) is followed by the
/// original path as its own record. The rendered line keeps the shape the
/// non-`-z` output had (`R  old -> new`), so callers and the model see the same
/// thing, only workspace-relative.
fn parse_status_records(stdout: &str, scope: &Scope) -> (Option<String>, Vec<String>) {
    let mut branch: Option<String> = None;
    let mut entries: Vec<String> = Vec::new();
    let mut records = stdout.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if let Some(rest) = record.strip_prefix("## ") {
            branch = Some(parse_branch(rest));
            continue;
        }
        let Some(code) = record.get(..2) else {
            continue;
        };
        let path = record.get(3..).unwrap_or_default();
        let rename = code.contains('R') || code.contains('C');
        // Consume the origin record even when the new path is outside the
        // workspace, so it cannot be mistaken for the next entry.
        let origin = if rename { records.next() } else { None };
        let Some(path) = scope.rebase(path) else {
            continue;
        };
        match origin {
            // A rename whose source is outside the workspace cannot be described
            // without naming an outside path, so it is reported as the addition
            // the confined diff sees as well.
            Some(origin) => match scope.rebase(origin) {
                Some(origin) => entries.push(format!("{code} {origin} -> {path}")),
                None => entries.push(format!("{code} {path}")),
            },
            None => entries.push(format!("{code} {path}")),
        }
    }
    (branch, entries)
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

/// `git diff [--staged] [base] --relative -- <path>`, confined to the workspace
/// (see [`Scope`]).
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

    let Some(scope) = repo_scope(ws, path).await? else {
        let rel = ws.relative(&ws.resolve_existing(path.unwrap_or("."))?);
        return Ok(DiffReport {
            text: format!(
                "is_repo: false — `{rel}` is not inside a git repository (or git is not initialised)."
            ),
            empty: true,
        });
    };
    // Running from the workspace root (never from the `path` argument's own
    // directory) is what keeps `--relative` honest: the patch headers are
    // re-based on the workspace root, and the pathspec - not the working
    // directory - decides what is inside.
    let dir = ws.root().to_path_buf();
    // `--relative` for the diff: unlike `status`, `diff` accepts it, and it
    // rewrites the `a/…`/`b/…` headers against the workspace root so a patch never
    // names a path above it.
    let mut args = diff_args(staged, base, Some(&scope.pathspec));
    args.insert(1, "--relative".to_string());

    // `git diff` runs `filter.<driver>.clean`/`.process` to build the
    // working-tree side of the patch, so the repository's own filter drivers
    // have to be neutralised before the diff runs.
    let mut overrides = filter_env(&dir).await?;
    overrides.extend(literal_pathspec_env());

    let run = git_with_env(&dir, &args, &overrides).await?;
    if is_not_a_repo(&run) {
        return Ok(DiffReport {
            text: format!(
                "is_repo: false — `{}` is not inside a git repository (or git is not initialised).",
                scope.target_rel
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
                scope.target_rel,
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
        // `path: null` means "the whole workspace"; `.` reads better in a message
        // than the pathspec does, and it says what was searched.
        let where_ = (scope.target_rel != ".").then_some(scope.target_rel.as_str());
        let text = match (staged, base, where_) {
            (true, _, Some(p)) => format!("No staged changes for `{p}`"),
            (true, _, None) => "No staged changes".to_string(),
            (false, Some(b), Some(p)) => format!("No changes for `{p}` against `{b}`"),
            (false, Some(b), None) => format!("No changes against `{b}`"),
            (false, None, Some(p)) => format!("No unstaged changes for `{p}`"),
            (false, None, None) => {
                "No unstaged changes (working tree matches the index)".to_string()
            }
        };
        return Ok(DiffReport { text, empty: true });
    }

    let mut text = run.stdout;
    if run.stdout_truncated {
        text.push_str("\n[git diff output truncated]\n");
    }
    if scope.confined() && !text.ends_with('\n') {
        text.push('\n');
    }
    if scope.confined() {
        // The patch is already re-based; the note stops a reader concluding that
        // this is the repository's whole diff.
        text.push_str(&format!(
            "[confined to this workspace, a subdirectory of the git repository at {}; the diff is \
             relative to the workspace root and changes outside it are not shown]\n",
            scope.toplevel
        ));
    }

    Ok(DiffReport { text, empty: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
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

    #[test]
    fn ref_and_branch_names_are_validated_before_they_reach_git() {
        // Revisions allow `~`/`^`/`..` (HEAD~3, main..feat) and dots in a path.
        for ok in [
            "main",
            "feat/x-1",
            "HEAD~3",
            "v1.2.3",
            "origin/main",
            "main..feat",
        ] {
            assert!(validate_revision_arg(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-c", "--upload-pack=evil", "a b", "x*y", "a\nb"] {
            assert!(validate_revision_arg(bad).is_err(), "{bad:?}");
        }
        assert!(validate_revision_arg(&"a".repeat(MAX_REF_CHARS + 1)).is_err());

        for ok in ["main", "feat/x-1", "release_2.0", "a.b.c"] {
            assert!(validate_branch_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-dash",
            "/lead",
            "trail/",
            "double//slash",
            "ends.",
            "x.lock",
            "a..b",
            "at@{1}",
            "@",
            "sp ace",
            "tab\there",
            "caret^",
            "tilde~",
            "colon:",
            "question?",
            "star*",
            "brack[et]",
            "back\\slash",
            "nul\u{0}",
        ] {
            assert!(validate_branch_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_branch_name(&"a".repeat(MAX_REF_CHARS + 1)).is_err());
    }

    #[tokio::test]
    async fn oversized_filter_listing_fails_closed() {
        let dir = temp_dir("filter-limit");
        git_in(&dir, &["init", "-q", "."]);
        let mut config = std::fs::read_to_string(dir.join(".git/config")).unwrap();
        for i in 0..40000 {
            config.push_str(&format!(
                "\n[filter \"driver{i:08}xxxxxxxxxxxxxxxx\"]\n clean = cat\n"
            ));
        }
        std::fs::write(dir.join(".git/config"), config).unwrap();
        let err = filter_env(&dir).await.unwrap_err().to_string();
        assert!(err.contains("enumeration exceeded"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn large_git_diff_is_bounded_and_marked() {
        let dir = temp_dir("diff-limit");
        git_in(&dir, &["init", "-q", "."]);
        std::fs::write(dir.join("a"), "old\n").unwrap();
        git_in(&dir, &["add", "a"]);
        std::fs::write(dir.join("a"), "new line\n".repeat(200000)).unwrap();
        let ws = Workspace::new(&dir).unwrap();
        let report = diff(&ws, None, false, None).await.unwrap();
        assert!(report.text.contains("[git diff output truncated]"));
        assert!(report.text.len() < GIT_STDOUT_LIMIT + 100);
        std::fs::remove_dir_all(dir).unwrap();
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

    /// Porcelain `-z` records are re-based on the workspace root, and a rename is
    /// rendered in the same `R  old -> new` shape the line-based output had.
    #[test]
    fn status_records_are_rebased_and_renames_are_rendered() {
        let at_root = Scope {
            target_rel: ".".into(),
            pathspec: ".".into(),
            prefix: String::new(),
            toplevel: "/repo".into(),
        };
        let (branch, entries) =
            parse_status_records("## main\0 M a.txt\0R  b.txt\0a.txt\0?? dir/\0", &at_root);
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(
            entries,
            vec![" M a.txt", "R  a.txt -> b.txt", "?? dir/"],
            "at the top of the repository the paths are already workspace-relative"
        );
        assert!(!at_root.confined());

        // The workspace is `inner/` inside the repository: everything is stripped.
        let nested = Scope {
            target_rel: ".".into(),
            pathspec: ".".into(),
            prefix: "inner/".into(),
            toplevel: "/repo".into(),
        };
        let (_, entries) = parse_status_records(
            "## main\0 M inner/a.txt\0R  inner/b.txt\0inner/a.txt\0",
            &nested,
        );
        assert_eq!(entries, vec![" M a.txt", "R  a.txt -> b.txt"]);
        assert!(nested.confined());

        // A rename whose source is outside the workspace cannot be described without
        // naming an outside path, so only the visible side is reported - and the
        // origin record is consumed, not mistaken for the next entry.
        let (_, entries) = parse_status_records(
            "## main\0R  inner/moved.txt\0../outside.txt\0 M inner/b.txt\0",
            &nested,
        );
        assert_eq!(entries, vec!["R  moved.txt", " M b.txt"]);
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
