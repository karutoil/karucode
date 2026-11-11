//! `karucode plugin install`: install plugins from a GitHub repository (or a
//! local directory) into `<data_dir>/plugins/<name>/`, without the server
//! running and without a local git checkout.
//!
//! The repository format follows what a plugin collection looks like in
//! practice:
//!
//! * one directory per plugin at the repo root, each with a `plugin.toml`;
//! * optionally a root `index.toml` that names them
//!   (`[[plugins]] name = …, path = …, description = …`) for an exact,
//!   human-curated list;
//! * or, in a single-plugin repository, a `plugin.toml` at the repo root
//!   itself.
//!
//! # What "install" means here
//!
//! Download (or copy) the sources, build if the plugin's `[install]` table
//! says so, then place the plugin's files under
//! `<data_dir>/plugins/<name>/` with a `plugin.toml` whose `command` points at
//! files **inside that directory**. The running host picks it up through its
//! normal disk-manifest discovery (`POST /api/plugins/reload` or
//! `plugins_watch`); nothing here starts or restarts anything.
//!
//! # Trust
//!
//! A plugin runs with the server's OS permissions, so the interesting question
//! is what a plugin author can get the installer to do. The rule is: **the
//! repository's own `plugin.toml` is untrusted input** — the same rule the host
//! already applies to a disk manifest (`kc_plugin_host`'s
//! `strip_disk_trust_settings` / `strip_disk_permission_wildcard`). Enforced
//! here, at install time, as a hard error instead of a warning:
//!
//! * `permissions = ["*"]` — the wildcard is operator-only. A remote manifest
//!   asking for it is an error, because "honour everything it declares" is a
//!   decision the person running the command must make explicitly.
//! * `settings.trust = true` — operator-only, same reason.
//! * `settings.read_only_tools = [...]` — an approval exemption; operator-only.
//!
//! The `[install]` table, by contrast, is allowed: building the plugin is the
//! whole point of the command, and the build runs in a staging directory with
//! the operator's own environment, exactly like `cargo build` run by hand.
//!
//! # Why in-process tar extraction
//!
//! Shelling out to `tar` is one `execve` away from an argument-injection bug
//! and needs the tar binary present. The `tar` + `flate2` crates are already
//! in the workspace tree (flate2 transitively), so the extraction happens
//! here, in Rust, with every member checked before a byte is written:
//!
//! * no absolute paths, no `..`, no empty components;
//! * no symlinks and no hardlinks — a plugin has no legitimate reason to ship
//!   one, and a link is the classic extraction escape;
//! * archive capped at 256 MiB (matching the updater's `MAX_ARTIFACT_BYTES`)
//!   and total unpacked bytes at 512 MiB, so a compression-bomb tarball is
//!   refused rather than filling the disk.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Ref used when none is given: whatever GitHub's `HEAD` points at.
pub const DEFAULT_REF: &str = "HEAD";

/// One archive may not exceed this (compressed download). Matches the updater's
/// `MAX_ARTIFACT_BYTES`: a plugin source archive is in the same weight class.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// Total unpacked bytes across all members. A gzip stream can expand ~1000x,
/// so the archive cap alone is not a cap at all.
const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;

/// One file the archive may not claim to be. 8 GiB entries are how a "small"
/// tarball asks for the whole disk without downloading a single byte.
const MAX_MEMBER_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Default `[install] build_timeout_secs`.
const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 900;

/// A repository listing more plugins than this is more likely a misparse than
/// a plugin collection.
const MAX_PLUGINS_PER_REPO: usize = 128;

/// Token env vars, in priority order (mirrors the `gh` CLI's precedence).
const TOKEN_ENV_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// Directory names never copied into an installed plugin: build outputs,
/// caches and VCS data that bloat the install and — for `target/` — can hold
/// stale binaries that shadow the freshly built one.
const EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".DS_Store",
];

// ---------------------------------------------------------------------------
// The command surface
// ---------------------------------------------------------------------------

/// What `karucode plugin install` was asked to do, parsed by
/// `bootstrap::parse_plugin_install`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginInstallOptions {
    /// A GitHub repository reference (`owner/repo`, a full URL, the `git@`
    /// SSH form, optionally with `@ref` or `/tree/<ref>[/<dir>]`) or a local
    /// directory path.
    pub source: String,
    /// Specific plugin names to install. Empty = every plugin the repository
    /// offers (the root `index.toml` when present, else the auto-scan).
    pub names: Vec<String>,
    /// Explicit git ref (branch, tag or commit). Overrides one carried by the
    /// source string. `None` = `HEAD`.
    pub git_ref: Option<String>,
    /// List what the source offers and exit; install nothing.
    pub list: bool,
    /// Replace an existing `<data_dir>/plugins/<name>/` instead of refusing.
    pub force: bool,
    /// Skip `[install] build` even when the plugin declares one.
    pub no_build: bool,
    /// Override `[install] build_timeout_secs`.
    pub build_timeout_secs: Option<u64>,
    /// `--data-dir` override; `None` = config file / default. Filled in by
    /// [`install`] before the per-plugin work, so the installer always has an
    /// absolute target.
    pub data_dir: Option<PathBuf>,
    /// `--config` override; `None` = `KARUCODE_CONFIG` / default path.
    pub config: Option<PathBuf>,
}

/// `karucode plugin install <source> [names…] [flags]`: parse the words after
/// `plugin install`. Positional: the source, then zero or more plugin names.
pub fn parse_plugin_install(words: &[String]) -> anyhow::Result<PluginInstallOptions> {
    let mut options = PluginInstallOptions {
        source: String::new(),
        ..Default::default()
    };
    let mut i = 0usize;
    while i < words.len() {
        let raw = &words[i];
        let (flag, inline) = match raw.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (raw.clone(), None),
        };
        match flag.as_str() {
            "--ref" | "-r" => {
                options.git_ref = Some(take_value(
                    &flag,
                    inline,
                    words,
                    &mut i,
                    "a branch, tag or commit",
                )?)
            }
            "--list" | "-l" => {
                no_value(&flag, inline)?;
                options.list = true;
            }
            "--force" | "-f" => {
                no_value(&flag, inline)?;
                options.force = true;
            }
            "--no-build" => {
                no_value(&flag, inline)?;
                options.no_build = true;
            }
            "--build-timeout-secs" => {
                let value = take_value(&flag, inline, words, &mut i, "seconds the build may run")?;
                options.build_timeout_secs = Some(
                    value
                        .parse::<u64>()
                        .map_err(|e| anyhow!("`{flag}` needs a number of seconds: {e}"))?,
                );
            }
            "--data-dir" => {
                let value = take_value(&flag, inline, words, &mut i, "a directory")?;
                options.data_dir = Some(PathBuf::from(value));
            }
            "--config" => {
                let value = take_value(&flag, inline, words, &mut i, "a config file")?;
                options.config = Some(PathBuf::from(value));
            }
            other if other.starts_with('-') => {
                bail!(
                    "unknown option `{other}`; see `karucode plugin install --help` in the \
                     `plugins` documentation"
                )
            }
            positional => {
                if options.source.is_empty() {
                    options.source = positional.to_string();
                } else {
                    options.names.push(positional.to_string());
                }
            }
        }
        i += 1;
    }
    if options.source.is_empty() {
        bail!(
            "`plugin install` needs a source: a GitHub repository (`owner/repo`, a github.com \
             URL or `owner/repo@ref`) or a local directory, then optional plugin names"
        );
    }
    if options.list && options.force {
        bail!("`--list` installs nothing, so `--force` has nothing to replace");
    }
    if options.list && !options.names.is_empty() {
        // Not an error: listing with names is just narrower than useful, so
        // warn and list everything.
        eprintln!("note: --list ignores plugin names and lists everything the source offers");
        options.names.clear();
    }
    Ok(options)
}

fn take_value(
    flag: &str,
    inline: Option<String>,
    words: &[String],
    i: &mut usize,
    what: &str,
) -> anyhow::Result<String> {
    match inline {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => bail!("`{flag}` requires a non-empty value ({what})"),
        None => {
            *i += 1;
            match words.get(*i) {
                Some(value) if !value.trim().is_empty() => Ok(value.clone()),
                _ => bail!("`{flag}` requires a value ({what})"),
            }
        }
    }
}

fn no_value(flag: &str, inline: Option<String>) -> anyhow::Result<()> {
    if inline.is_some() {
        bail!("`{flag}` does not take a value");
    }
    Ok(())
}

impl PluginInstallOptions {
    /// The user-facing synopsis, shared by the usage text and errors.
    pub fn usage() -> &'static str {
        "karucode plugin install <source> [names…]

  source  owner/repo, owner/repo@ref, https://github.com/owner/repo
          (optionally /tree/<ref>[/one-dir]), git@github.com:owner/repo,
          or a local directory
  names   zero or more plugin names; omitted = every plugin the source offers

  --ref, -r <ref>            git ref (branch, tag or commit; default HEAD)
  --list, -l                 list what the source offers and exit
  --force, -f                replace an existing install (old one -> <name>.old)
  --no-build                 skip the [install] build step
  --build-timeout-secs <n>   override [install] build_timeout_secs (default 900)
  --data-dir <path>          install under <path>/plugins instead of the
                             configured data directory
  --config <path>            config file for the data directory (default
                             $KARUCODE_CONFIG or ~/.config/karucode/config.toml)"
    }
}

/// `karucode plugin install …`: resolve, fetch, build, place. Exit 0 with one
/// line per installed plugin, non-zero with an actionable message on failure.
pub async fn cli(options: PluginInstallOptions) -> anyhow::Result<()> {
    if let Some(path) = &options.config {
        if !path.exists() {
            bail!(
                "config file {} does not exist; create it or omit --config",
                path.display()
            );
        }
        std::env::set_var("KARUCODE_CONFIG", path);
    }
    let mut config = kc_core::config::Config::load()
        .map_err(|e| anyhow!("could not load configuration: {e}"))?;
    if let Some(dir) = &options.data_dir {
        config.data_dir = dir.clone();
    }
    let reporter = PrintReporter;
    if options.list {
        let preview = preview(&options, &config.data_dir).await?;
        print_preview(&preview);
        for note in &preview.notes {
            eprintln!("{note}");
        }
        return Ok(());
    }
    let outcome = install(&options, &config.data_dir, &reporter).await?;
    print_outcome(&outcome);
    Ok(())
}

/// One progress line of an install, in the order it happened.
///
/// The CLI prints these as they occur; the HTTP job returns them as a list the
/// browser can replay, and both call sites share one emitter so the wording
/// cannot drift between surfaces.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallEvent {
    /// `==> downloading <url>`
    Downloading { url: String },
    /// `note:`/`warning:` lines the operator should see but that do not fail
    /// anything (operator-only keys stripped, implied build, --no-build).
    Note { message: String },
    /// `==> building \`name\` (command in dir)`
    Building { plugin: String, command: String },
    /// `installed \`name\` -> path`
    Installed { plugin: String, path: String },
    /// The trailing "what now" text.
    Done { installed: usize },
}

impl InstallEvent {
    /// The line as the CLI prints it. The `note` variants already carry their
    /// `warning:`/`note:` prefix; the others add their decoration here.
    fn cli_line(&self) -> String {
        match self {
            InstallEvent::Downloading { url } => format!("==> downloading {url}"),
            InstallEvent::Note { message } => message.clone(),
            InstallEvent::Building { plugin, command } => {
                format!("==> building `{plugin}` ({command})")
            }
            InstallEvent::Installed { plugin, path } => format!("installed `{plugin}` -> {path}"),
            InstallEvent::Done { installed } => format!(
                "{installed} plugin{} installed.",
                if *installed == 1 { "" } else { "s" }
            ),
        }
    }
}

/// How an install reports progress. The CLI prints immediately; an HTTP job
/// buffers lines for polling. `Send + Sync` so it can travel into a task.
pub trait InstallReporter: Send + Sync {
    fn emit(&self, event: InstallEvent);
}

/// The CLI's reporter: every line goes to stderr (stdout stays clean for
/// scripts; the `--list` output is the only thing on stdout).
struct PrintReporter;

impl InstallReporter for PrintReporter {
    fn emit(&self, event: InstallEvent) {
        eprintln!("{}", event.cli_line());
    }
}

/// A collector that stores every event, for the HTTP job's poll responses.
#[derive(Default)]
pub struct CollectReporter {
    events: std::sync::Mutex<Vec<InstallEvent>>,
}

impl CollectReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot of everything emitted so far.
    pub fn events(&self) -> Vec<InstallEvent> {
        self.events.lock().expect("install events lock").clone()
    }
}

impl InstallReporter for CollectReporter {
    fn emit(&self, event: InstallEvent) {
        self.events.lock().expect("install events lock").push(event);
    }
}

/// What a repository offers, for a preview. This is the data `--list` prints
/// and the web UI shows before choosing plugins.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginPreview {
    /// Where the sources came from: `<owner>/<repo>@<sha-or-ref>` or
    /// `local:<path>`.
    pub source: String,
    /// `remote` or `local`.
    pub kind: &'static str,
    /// Commit sha the ref resolved to, when GitHub reported one.
    pub resolved: Option<String>,
    pub plugins: Vec<PluginPreviewEntry>,
    /// Non-fatal notes gathered while resolving (operator-only keys
    /// stripped, ...): shown before install so the operator is not surprised.
    pub notes: Vec<String>,
}

/// One plugin a preview offers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginPreviewEntry {
    pub name: String,
    pub description: Option<String>,
    /// The manifest's declared permissions — what the plugin asks to touch.
    /// An install review surface must show this before anything runs.
    pub permissions: Vec<String>,
    /// Whether the plugin will be built (declared `[install]`, or implied for
    /// `target/`-pointing commands).
    pub builds: bool,
    /// Whether `<data_dir>/plugins/<name>` already exists (an install needs
    /// `force` to replace it).
    pub already_installed: bool,
}

/// Resolve a source and describe what it offers, without installing anything.
///
/// The CLI's `--list` and the web UI's preview share this. `data_dir` is the
/// effective directory, used only to mark `already_installed`.
pub async fn preview(
    options: &PluginInstallOptions,
    data_dir: &Path,
) -> anyhow::Result<PluginPreview> {
    let repository = fetch_repository(options, &NullReporter).await?;
    let notes = repository
        .plugins
        .iter()
        .flat_map(|p| p.warnings.iter().cloned())
        .collect();
    let plugins = repository
        .plugins
        .iter()
        .map(|plugin| PluginPreviewEntry {
            name: plugin.name.clone(),
            description: plugin.description.clone(),
            permissions: plugin.manifest.permissions.clone(),
            builds: plugin
                .install_spec()
                .is_some_and(|spec| !spec.build.is_empty())
                || plugin.will_imply_build(&repository.plugin_origin_dir(plugin)),
            already_installed: data_dir.join("plugins").join(&plugin.name).exists(),
        })
        .collect();
    let out = PluginPreview {
        source: repository.source_label.clone(),
        kind: if repository.is_remote() {
            "remote"
        } else {
            "local"
        },
        resolved: repository.resolved.clone(),
        plugins,
        notes,
    };
    cleanup(&repository);
    Ok(out)
}

/// Print a preview the way `--list` always did.
fn print_preview(preview: &PluginPreview) {
    println!(
        "{}: {} plugin{}",
        preview.source,
        preview.plugins.len(),
        if preview.plugins.len() == 1 { "" } else { "s" }
    );
    for entry in &preview.plugins {
        match &entry.description {
            Some(description) => println!("  {}    {}", entry.name, description),
            None => println!("  {}", entry.name),
        }
    }
    if preview.plugins.is_empty() {
        println!(
            "  (none found: expected a root index.toml, */plugin.toml, or a root \
             plugin.toml)"
        );
    }
}

/// What an install did, for callers that want the result as data.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallOutcome {
    pub installed: Vec<String>,
    /// The path each plugin landed at, by name.
    pub paths: Vec<(String, String)>,
}

/// Print the tail the CLI has always printed after a successful install.
fn print_outcome(outcome: &InstallOutcome) {
    println!(
        "\n{} plugin{} installed. Apply with the server's normal rescan (no restart):",
        outcome.installed.len(),
        if outcome.installed.len() == 1 {
            ""
        } else {
            "s"
        }
    );
    println!(
        "  curl -s -X POST localhost:8787/api/plugins/reload \
         -H 'Content-Type: application/json' -d '{{}}' | jq .report"
    );
    println!("  (or set plugins_watch = true and the server notices on its own)");
}

/// Everything a plugin in the repository is, after resolution.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    pub name: String,
    /// The plugin's own directory relative to the repository root. Empty when
    /// the repository root *is* the plugin.
    pub rel_dir: PathBuf,
    /// The plugin's `plugin.toml`, verbatim minus any stripped operator-only
    /// lines (the `command` line is rewritten on install; everything else
    /// round-trips unchanged).
    pub manifest_text: String,
    /// Parsed view of the same manifest, for the fields the installer needs.
    pub manifest: PluginFileConfig,
    pub description: Option<String>,
    /// Non-fatal findings: operator-only keys that were stripped, a name that
    /// fell back to the directory. The CLI prints them; the web UI shows them
    /// before an install so nobody is surprised by a manifest that changed
    /// shape on the way in.
    pub warnings: Vec<String>,
}

impl ResolvedPlugin {
    /// `[install]`, when the manifest declares one.
    pub fn install_spec(&self) -> Option<&InstallSpec> {
        self.manifest.install.as_ref()
    }

    /// Whether the install would imply a `cargo build --release` for this
    /// plugin: the command names a `target/release|debug` artifact, a
    /// `Cargo.toml` is present in `plugin_dir`, and no `[install]` table
    /// declared its own build. A preview shows "will be built" using this.
    pub fn will_imply_build(&self, plugin_dir: &Path) -> bool {
        self.manifest
            .install
            .as_ref()
            .is_none_or(|spec| spec.build.is_empty())
            && implied_build(&self.manifest.command, plugin_dir).is_some()
    }
}

/// `[install]` in a plugin's `plugin.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InstallSpec {
    /// Command that produces the plugin binary, run with the plugin directory
    /// as cwd. `["cargo", "build", "--release"]` is the expected value.
    #[serde(default)]
    pub build: Vec<String>,
    /// Seconds the build may take. Default 900.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
    /// Extra directory names (relative to the plugin's own directory) to
    /// exclude from the installed copy, beyond the built-in list.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// The `plugin.toml` fields the installer reads. A separate, narrower struct
/// from `kc_core::config::PluginConfig` on purpose: the repository's manifest
/// is untrusted input, so only what the installer *acts on* is parsed here.
/// Everything else — `panels`, `ui`, and any key the host's own manifest
/// reader accepts — is carried through the verbatim manifest text untouched,
/// which is why this struct does **not** use `deny_unknown_fields`: rejecting
/// a key the host understands would make the installer narrower than the host
/// and break plugins that use host features this struct has never heard of.
///
/// `settings` is kept as raw JSON values and round-trips through the verbatim
/// manifest text, so harmless future keys in that free-form table survive an
/// install — except the operator-only ones, which are stripped with a warning
/// exactly the way the host strips them from a disk manifest.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginFileConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub install: Option<InstallSpec>,
}

/// The root `index.toml`: `[[plugins]] name = …, path = …, description = …`.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RepoIndex {
    #[serde(default, rename = "plugins")]
    entries: Vec<RepoIndexEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoIndexEntry {
    name: String,
    /// The plugin's directory, relative to the repository root. Omitted when
    /// the plugin is the repository root itself.
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// A repository reference, parsed from the source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSource {
    pub owner: String,
    pub repo: String,
    /// Subdirectory when the URL carried `/tree/<ref>/<subdir>`; only the
    /// first component is kept, because a plugin is one directory.
    pub subdir: Option<String>,
    /// Ref carried by the URL (`/tree/<ref>` or `owner/repo@ref`).
    pub url_ref: Option<String>,
    /// Set when the source is a local directory rather than GitHub.
    pub local_dir: Option<PathBuf>,
}

impl RepoSource {
    /// The ref to fetch: `--ref` beats the URL's, then `HEAD`.
    pub fn ref_to_use(&self, explicit: Option<&str>) -> String {
        explicit
            .or(self.url_ref.as_deref())
            .unwrap_or(DEFAULT_REF)
            .to_string()
    }

    /// `owner/repo`, for messages.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Parse a source string into a [`RepoSource`].
///
/// Accepted forms:
///
/// * `owner/repo`, optionally `owner/repo@ref`
/// * `https://github.com/owner/repo` (a trailing `.git` is stripped)
/// * `https://github.com/owner/repo/tree/<ref>` and
///   `…/tree/<ref>/<subdir>`
/// * `git@github.com:owner/repo.git`
/// * a path that exists on disk (relative or absolute) — local mode
///
/// Any other host, a `git://` URL or a non-existent path-shaped string is
/// refused with the supported forms.
pub fn parse_source(input: &str) -> anyhow::Result<RepoSource> {
    let raw = input.trim();
    if raw.is_empty() {
        bail!("plugin install source is empty; give a GitHub repository or a local directory");
    }

    // Local directory first: an existing path must win over any slug-shaped
    // reading of it, and a path-shaped string that does not exist is a path
    // mistake rather than a repository.
    let candidate = Path::new(raw);
    if candidate.is_dir() {
        return Ok(RepoSource {
            owner: String::new(),
            repo: String::new(),
            subdir: None,
            url_ref: None,
            local_dir: Some(canonicalish(candidate)),
        });
    }
    if !looks_like_remote(raw) {
        if raw.contains('/') || raw.contains('\\') || raw.starts_with('.') {
            bail!(
                "`{raw}` is not an existing directory; a local install source must exist on disk"
            );
        }
        bail!(
            "`{raw}` is neither a GitHub repository (`owner/repo`, a github.com URL or a \
             git@github.com form) nor an existing local directory"
        );
    }

    // git@github.com:owner/repo.git
    if let Some(rest) = raw.strip_prefix("git@github.com:") {
        let rest = rest.trim_end_matches(".git");
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next().unwrap_or_default();
        let repo = parts.next().unwrap_or_default();
        return finish_github(owner, repo, None, None);
    }

    // http(s)://github.com/owner/repo[/tree/<ref>[/<dir>]]
    if raw.contains("://") {
        let rest = raw.split_once("://").map(|(_, rest)| rest).unwrap_or(raw);
        let rest = rest.trim_start_matches("//").trim_end_matches('/');
        let rest = rest.trim_end_matches(".git");
        let (host, path) = match rest.split_once('/') {
            Some(split) => split,
            None => bail!(
                "`{raw}` is a URL without a repository path; expected \
                 github.com/<owner>/<repo>"
            ),
        };
        if !host.eq_ignore_ascii_case("github.com") {
            bail!(
                "`{raw}` names host `{host}`; only github.com repositories are supported. For \
                 another git host, clone it and pass the local directory as the source"
            );
        }
        let mut parts = path.split('/').filter(|p| !p.is_empty());
        let owner = parts.next().unwrap_or_default();
        let repo = parts.next().unwrap_or_default();
        let mut url_ref: Option<String> = None;
        let mut subdir: Option<String> = None;
        if let Some(marker) = parts.next() {
            if !marker.eq_ignore_ascii_case("tree") {
                bail!(
                    "`{raw}` is a `{marker}` URL; only repository and /tree/<ref> URLs are \
                     supported install sources"
                );
            }
            url_ref = parts.next().map(str::to_string);
            // One directory: a plugin is one directory, so the rest of the
            // path collapses to its first component.
            subdir = parts.next().map(str::to_string);
            if parts.next().is_some() {
                bail!(
                    "`{raw}` points deeper than one plugin directory; install the repository \
                     root and name the plugin(s) instead"
                );
            }
        }
        return finish_github(owner, repo, subdir, url_ref);
    }

    // owner/repo[@ref]
    let (slug, url_ref) = match raw.split_once('@') {
        Some((slug, r)) => (slug, Some(r.to_string())),
        None => (raw, None),
    };
    let mut parts = slug.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        bail!(
            "`{raw}` has more than one `/`; an install source is `owner/repo` (with `@ref`), a \
             github.com URL, or a local directory"
        );
    }
    finish_github(owner, repo, None, url_ref)
}

/// Whether the string could name a remote repository at all.
fn looks_like_remote(raw: &str) -> bool {
    let lowered = raw.to_ascii_lowercase();
    if lowered.starts_with("https://")
        || lowered.starts_with("http://")
        || lowered.starts_with("git@")
    {
        return true;
    }
    if raw.starts_with('.') || raw.starts_with('/') || raw.starts_with('~') {
        return false;
    }
    // `owner/repo`: exactly one slash.
    raw.matches('/').count() == 1
}

fn finish_github(
    owner: &str,
    repo: &str,
    subdir: Option<String>,
    url_ref: Option<String>,
) -> anyhow::Result<RepoSource> {
    let owner = owner.trim();
    let repo = repo.trim();
    if owner.is_empty() || repo.is_empty() {
        bail!(
            "could not read an `owner/repo` out of the source; expected forms are `owner/repo`, \
             `owner/repo@ref`, `https://github.com/owner/repo[/tree/<ref>[/<dir>]]` and \
             `git@github.com:owner/repo.git`"
        );
    }
    for part in [owner, repo] {
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!(
                "`{part}` is not a legal GitHub owner or repository name (letters, digits, `-`, \
                 `_`)"
            );
        }
    }
    Ok(RepoSource {
        owner: owner.to_string(),
        repo: repo.to_string(),
        subdir,
        url_ref,
        local_dir: None,
    })
}

fn canonicalish(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Repository: fetch, then resolve
// ---------------------------------------------------------------------------

/// A fetched repository, ready to install from.
pub struct Repository {
    /// Where the sources came from, for messages and `install.json`:
    /// `<owner>/<repo>@<sha-or-ref>` or `local:<path>`.
    pub source_label: String,
    /// The working tree: the extracted tarball (remote) or a staging copy of
    /// the local directory. Plugin files are read from here.
    pub root: PathBuf,
    /// For local sources, the directory the operator pointed at — the path
    /// absolute `command` entries in the manifest actually refer to.
    pub origin: Option<PathBuf>,
    /// Commit sha the ref resolved to, when GitHub reported one.
    pub resolved: Option<String>,
    /// sha256 of the downloaded tarball, remote only.
    pub tarball_sha256: Option<String>,
    /// The plugins the repository offers, in index order (when there is an
    /// index) or directory order.
    pub plugins: Vec<ResolvedPlugin>,
}

impl Repository {
    fn is_remote(&self) -> bool {
        self.origin.is_none()
    }

    /// The directory a manifest's absolute paths are written against: the
    /// extracted tree for a remote source, the operator's directory for a
    /// local one.
    fn path_origin(&self) -> &Path {
        self.origin.as_deref().unwrap_or(&self.root)
    }

    /// The source directory of one plugin, in path-origin coordinates.
    fn plugin_origin_dir(&self, plugin: &ResolvedPlugin) -> PathBuf {
        if plugin.rel_dir.as_os_str().is_empty() {
            self.path_origin().to_path_buf()
        } else {
            self.path_origin().join(&plugin.rel_dir)
        }
    }
}

/// Download (or copy) the repository and resolve its plugins.
///
/// A preview passes [`NullReporter`] (its job is the result, not the
/// progress); an install passes the real reporter so the download line shows
/// up wherever the operator is looking.
async fn fetch_repository(
    options: &PluginInstallOptions,
    reporter: &dyn InstallReporter,
) -> anyhow::Result<Repository> {
    let source = parse_source(&options.source)?;
    if let Some(dir) = &source.local_dir {
        return local_repository(dir).await;
    }
    let reference = source.ref_to_use(options.git_ref.as_deref());
    let (root, resolved, tarball_sha256) =
        download_and_extract(&source, &reference, reporter).await?;
    let source_label = match &resolved {
        Some(sha) => format!("{}@{}", source.slug(), sha),
        None => format!("{}@{}", source.slug(), reference),
    };
    let subdir = source.subdir.as_deref();
    let plugins = match resolve_plugins(&root, subdir).await {
        Ok(plugins) => plugins,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&root);
            return Err(error);
        }
    };
    Ok(Repository {
        source_label,
        root,
        origin: None,
        resolved,
        tarball_sha256,
        plugins,
    })
}

/// A local directory as the repository. Files are copied to a staging tree
/// first so the install path (exclusions, build, command rewrite) is exactly
/// the same as the remote one, and the operator's checkout is never written
/// to.
async fn local_repository(dir: &Path) -> anyhow::Result<Repository> {
    let staging = tempfile_dir("karucode-plugin-local")?;
    copy_tree(dir, &staging, &[])?;
    let plugins = match resolve_plugins(&staging, None).await {
        Ok(plugins) => plugins,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    Ok(Repository {
        source_label: format!("local:{}", dir.display()),
        root: staging,
        origin: Some(dir.to_path_buf()),
        resolved: None,
        tarball_sha256: None,
        plugins,
    })
}

/// Read the repository's plugin list out of a fetched tree.
///
/// Order of preference, matching the documented format:
///
/// 1. root `index.toml` — the curated list; every entry must resolve;
/// 2. every `*/plugin.toml` at the root — the auto-scan;
/// 3. a `plugin.toml` at the root itself — the single-plugin repository.
///
/// With a `/tree/<ref>/<subdir>` source, the same three probes run against
/// that subdirectory instead of the repository root.
async fn resolve_plugins(root: &Path, subdir: Option<&str>) -> anyhow::Result<Vec<ResolvedPlugin>> {
    let base: PathBuf = match subdir {
        Some(sub) => {
            let path = root.join(safe_rel(sub)?);
            if !path.is_dir() {
                bail!(
                    "`{sub}` (from the source URL) is not a directory in the repository; the \
                     supported form is /tree/<ref>/<one-plugin-dir>"
                );
            }
            path
        }
        None => root.to_path_buf(),
    };

    // 1. index.toml
    let index_path = base.join("index.toml");
    if index_path.is_file() {
        let text = std::fs::read_to_string(&index_path)
            .with_context(|| format!("could not read {}", index_path.display()))?;
        let index: RepoIndex = toml::from_str(&text).map_err(|e| {
            anyhow!(
                "{} is not a valid plugin repository index (expected [[plugins]] with \
                 name/path/description): {e}",
                index_path.display()
            )
        })?;
        if index.entries.len() > MAX_PLUGINS_PER_REPO {
            bail!(
                "the index at {} lists {} plugins, more than the {} allowed",
                index_path.display(),
                index.entries.len(),
                MAX_PLUGINS_PER_REPO
            );
        }
        let mut out = Vec::new();
        for entry in &index.entries {
            let rel_dir = match &entry.path {
                Some(path) => safe_rel(path)?,
                None => PathBuf::new(),
            };
            let manifest_path = if rel_dir.as_os_str().is_empty() {
                base.join("plugin.toml")
            } else {
                base.join(&rel_dir).join("plugin.toml")
            };
            out.push(read_manifest(
                &manifest_path,
                rel_dir,
                Some(&entry.name),
                entry.description.clone(),
            )?);
        }
        return Ok(out);
    }

    // 2. auto-scan of */plugin.toml
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .with_context(|| format!("could not read {}", base.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut found: Vec<ResolvedPlugin> = Vec::new();
    for dir in dirs {
        let manifest_path = dir.join("plugin.toml");
        if manifest_path.is_file() {
            let rel_dir = dir
                .strip_prefix(&base)
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            found.push(read_manifest(&manifest_path, rel_dir, None, None)?);
        }
    }
    if !found.is_empty() {
        if found.len() > MAX_PLUGINS_PER_REPO {
            bail!(
                "{} offers {} plugin directories, more than the {} allowed",
                base.display(),
                found.len(),
                MAX_PLUGINS_PER_REPO
            );
        }
        return Ok(found);
    }

    // 3. the repository root as a single plugin
    let root_manifest = base.join("plugin.toml");
    if root_manifest.is_file() {
        return Ok(vec![read_manifest(
            &root_manifest,
            PathBuf::new(),
            None,
            None,
        )?]);
    }

    bail!(
        "no plugins in {}: expected a root index.toml, a `plugin.toml` in each plugin \
         directory, or a plugin.toml at the root itself",
        base.display()
    );
}

/// Read and security-check one `plugin.toml`.
///
/// `expected_name` comes from the repository index; when present, a manifest
/// `name` that disagrees with it is an error, because the index name is what
/// the operator selects by.
fn read_manifest(
    path: &Path,
    rel_dir: PathBuf,
    expected_name: Option<&str>,
    description: Option<String>,
) -> anyhow::Result<ResolvedPlugin> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let file: PluginFileConfig = toml::from_str(&text).map_err(|e| {
        anyhow!(
            "{} is not a valid plugin manifest (expected \
             name/description/enabled/command/permissions/env/settings/[install]): {e}",
            path.display()
        )
    })?;
    if file.command.is_empty() {
        bail!(
            "{} has no `command = [\"program\", \"args\"]`; a plugin needs one",
            path.display()
        );
    }
    if file
        .command
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        bail!(
            "{} has an empty `command` program; a plugin needs `command = [\"program\", …]`",
            path.display()
        );
    }

    // Operator-only keys are stripped, with a warning, exactly as the host
    // strips them from a disk manifest (`kc_plugin_host`'s
    // `strip_disk_trust_settings` / `strip_disk_permission_wildcard`): a
    // repository manifest is untrusted input, so it never gets the last word
    // on permissions or approval exemptions — but refusing to install would
    // block a `--list` and leave the operator without the plugin, while
    // stripping keeps the install fail-closed *and* usable. The warning tells
    // the operator what was dropped and where to put it if they meant it.
    let mut dropped: Vec<&str> = Vec::new();
    let has_wildcard = file.permissions.iter().any(|p| p.trim() == "*");
    let has_trust = file.settings.contains_key("trust");
    let has_read_only_tools = file.settings.contains_key("read_only_tools");
    if has_wildcard {
        dropped.push("`permissions = [\"*\"]`");
    }
    if has_trust {
        dropped.push("`settings.trust`");
    }
    if has_read_only_tools {
        dropped.push("`settings.read_only_tools`");
    }
    let warning = (!dropped.is_empty()).then(|| {
        format!(
            "warning: {} declares {}, which are operator-only approval/permission grants; they \
             were stripped from the installed manifest (the host would ignore them anyway). Add \
             the key yourself in the [[plugins]] entry in config.toml if you really mean it",
            path.display(),
            dropped.join(" and ")
        )
    });

    // The written manifest carries the stripped text, not the original: the
    // installed plugin.toml is a disk manifest the host reads, and it should
    // not even carry keys the host would warn about.
    let manifest_text = if has_wildcard || has_trust || has_read_only_tools {
        strip_operator_only_lines(&text)
    } else {
        text.clone()
    };

    let name = match (&file.name, expected_name) {
        (Some(name), Some(expected)) => {
            if name.trim() != expected.trim() {
                bail!(
                    "{} says its name is `{name}` but the repository index calls it `{expected}`; \
                     the index name is what `plugin install` selects, so they must agree",
                    path.display()
                );
            }
            name.trim().to_string()
        }
        (Some(name), None) => name.trim().to_string(),
        (None, Some(expected)) => expected.trim().to_string(),
        (None, None) => rel_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    };
    if let Err(reason) = validate_plugin_name(&name) {
        bail!(
            "plugin name `{name}` from {} is not usable: {reason}. The name becomes the \
             directory under <data_dir>/plugins/ and the manifest key, so it must be lowercase \
             ASCII letters, digits, `-` or `_`, starting with a letter",
            path.display()
        );
    }

    let fallback_description = file_description(&text);
    Ok(ResolvedPlugin {
        name,
        rel_dir,
        manifest_text,
        manifest: file,
        description: description.or(fallback_description),
        warnings: warning.into_iter().collect(),
    })
}

/// Remove the operator-only assignments from a manifest's text:
/// `permissions` containing the `*` wildcard (top level), and `trust` /
/// `read_only_tools` inside `[settings]`. Everything else round-trips byte
/// for byte. A dropped assignment's multi-line array is consumed with it.
///
/// This mirrors the parsed-side checks in [`read_manifest`]: what the host
/// would strip from a disk manifest with a warning is not even written into
/// the installed one.
fn strip_operator_only_lines(text: &str) -> String {
    // Which `[table]` the line is in: "" = before any table (the top level).
    let mut table: String = String::new();
    let mut out = String::with_capacity(text.len());
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            table = trimmed
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let (key, rest) = match trimmed.split_once('=') {
            Some(split) => split,
            None => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
        };
        let key = key.trim();
        let rest = rest.trim();
        let in_settings = table == "settings";
        let at_top = table.is_empty();
        let drop = (at_top && key == "permissions" && rest.contains('*'))
            || (in_settings && (key == "trust" || key == "read_only_tools"));
        if !drop {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Consume a multi-line value with the dropped line: an unclosed `[`
        // after this line means the array continues below.
        let mut depth = trimmed.matches('[').count() as i64 - trimmed.matches(']').count() as i64;
        while depth > 0 {
            match lines.next() {
                Some(next) => {
                    depth += next.matches('[').count() as i64;
                    depth -= next.matches(']').count() as i64;
                }
                None => break,
            }
        }
    }
    out
}

/// The description line of a manifest, pulled from the raw text so it works
/// without widening the parsed struct (it is informational only).
fn file_description(text: &str) -> Option<String> {
    let mut in_manifest = true;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_manifest = false;
        }
        if !in_manifest {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("description") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// The same rule as `kc_plugin_sdk::embed::validate_name`, stated locally: an
/// installed name is a directory, a manifest key and (for Rust) a binary name.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("it is empty".into());
    }
    let first = name.chars().next().expect("checked non-empty");
    if !first.is_ascii_lowercase() {
        return Err("it must start with a lowercase ASCII letter".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err("it may contain only lowercase ASCII letters, digits, `-` and `_`".into());
    }
    Ok(())
}

/// Reject `..`, absolute paths and empty components in a repository-relative
/// path.
fn safe_rel(path: &str) -> anyhow::Result<PathBuf> {
    let trimmed = path.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        bail!("a repository-relative path cannot be empty");
    }
    let mut out = PathBuf::new();
    for component in trimmed.split(['/', '\\']) {
        match component {
            "" | "." => continue,
            ".." => bail!("`{path}` climbs out of the repository with `..`"),
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("`{path}` is not a usable repository-relative path");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Download and extraction
// ---------------------------------------------------------------------------

/// A reporter that drops everything, for call sites that only want the result
/// (tests, or a preview's internal fetch).
pub struct NullReporter;

impl InstallReporter for NullReporter {
    fn emit(&self, _event: InstallEvent) {}
}

/// Download `owner/repo` at `reference` and extract it to a temporary
/// directory. Returns the directory, the commit sha GitHub resolved the ref to
/// (from `Content-Disposition`, when present) and the tarball's sha256.
async fn download_and_extract(
    source: &RepoSource,
    reference: &str,
    reporter: &dyn InstallReporter,
) -> anyhow::Result<(PathBuf, Option<String>, Option<String>)> {
    let url = codeload_url(&source.owner, &source.repo, reference);
    let (bytes, sha) = download_tarball(&url, reporter).await?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        bail!(
            "{url} is {} bytes, over the {}-byte archive limit",
            bytes.len(),
            MAX_ARCHIVE_BYTES
        );
    }
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let tarball_sha256 = hex(&hasher.finalize());

    let dir = tempfile_dir("karucode-plugin-src")?;
    extract_tarball(&bytes, &dir).with_context(|| {
        format!(
            "could not extract the tarball from {url} (sha256 {tarball_sha256}, {} bytes)",
            bytes.len()
        )
    })?;
    Ok((dir, sha, Some(tarball_sha256)))
}

/// `https://codeload.github.com/<owner>/<repo>/tar.gz/<ref>`: the anonymous,
/// git-free download endpoint GitHub serves for every repository (public with
/// no token, private with one). Separate from [`download_tarball`] so a test
/// can drive the download+sha+extract pipeline against its own URL.
fn codeload_url(owner: &str, repo: &str, reference: &str) -> String {
    format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{reference}")
}

/// Fetch one tarball. Returns the bytes and, when the server's
/// `Content-Disposition` names one (codeload always does:
/// `attachment; filename=repo-<sha>.tar.gz`), the commit sha the ref resolved
/// to. Public for the integration tests, which stand in for codeload with a
/// local server; never used against anything but the [`codeload_url`] shape.
///
/// `reporter` sees the download event so the CLI prints it and an HTTP job
/// records it; pass a [`NullReporter`] when the caller does not care.
pub async fn download_tarball(
    url: &str,
    reporter: &dyn InstallReporter,
) -> anyhow::Result<(Vec<u8>, Option<String>)> {
    reporter.emit(InstallEvent::Downloading {
        url: url.to_string(),
    });
    let client = reqwest::Client::builder()
        .user_agent(format!("karucode/{}", crate::version::VERSION))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| anyhow!("could not build an HTTP client: {e}"))?;
    let mut request = client.get(url);
    // Both accepted spellings, same priority order as the `gh` CLI.
    for var in TOKEN_ENV_VARS {
        if let Ok(token) = std::env::var(var) {
            if !token.trim().is_empty() {
                request = request.bearer_auth(token.trim());
            }
            break;
        }
    }
    let response = request
        .send()
        .await
        .map_err(|e| anyhow!("could not download {url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        if status.as_u16() == 404 {
            bail!(
                "{url} answered 404: no such repository or ref (a private repository needs \
                 GH_TOKEN or GITHUB_TOKEN set)"
            );
        }
        bail!("could not download {url}: HTTP {status}");
    }
    let sha = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1))
        .and_then(|middle| middle.split('.').next())
        .map(str::to_string)
        .filter(|sha| sha.len() >= 7);
    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow!("download of {url} failed: {e}"))?;
    Ok((bytes.to_vec(), sha))
}

fn tempfile_dir(prefix: &str) -> anyhow::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("{prefix}-{}-{}-{n}", std::process::id(), now_ms()));
    if dir.exists() {
        // Paranoia: a same-name directory must never be silently reused, and
        // the counter should already make that impossible.
        anyhow::bail!(
            "staging directory {} already exists; refusing to reuse it",
            dir.display()
        );
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("could not create staging directory {}", dir.display()))?;
    Ok(dir)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Extract an in-memory `.tar.gz` into `dest`, rejecting everything unsafe
/// before it is written:
///
/// * absolute paths and any `..` component;
/// * symlinks and hardlinks — a plugin source tree has no legitimate use for
///   either, and a link is the classic extraction escape;
/// * member or total sizes over their caps.
///
/// The single leading directory GitHub puts in every tarball (`repo-<sha>/`)
/// is stripped: every member must share exactly one, and it must be stripped,
/// or a `plugin.toml` at "the repository root" would not be found.
pub fn extract_tarball(bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(true);
    let mut top: Option<String> = None;
    let mut total: u64 = 0;
    let mut members = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry.map_err(|e| {
            anyhow!("could not read a tar member from the archive (it may be corrupt): {e}")
        })?;
        members += 1;
        let header = entry.header();
        let entry_type = header.entry_type();
        let mode = header.mode().unwrap_or(0);
        if entry_type.is_hard_link() || entry_type.is_symlink() {
            bail!(
                "the archive contains a link (`{}`), which plugin sources never legitimately \
                 need; refusing the archive",
                entry.path().unwrap_or_default().display()
            );
        }
        // A `pax_global_header` is metadata GitHub puts at the front of every
        // tarball it serves (repo name, ref, sha) — not a file. GNU tar skips
        // it; so does the `tar` crate when asked politely. Refusing it would
        // reject every real codeload archive.
        if entry_type.is_pax_global_extensions() {
            continue;
        }
        if !entry_type.is_dir() && !entry_type.is_file() {
            bail!(
                "the archive contains a non-regular member `{}`; refusing the archive",
                entry.path().unwrap_or_default().display()
            );
        }
        let path = entry
            .path()
            .map_err(|e| anyhow!("could not read a member path: {e}"))?
            .to_path_buf();
        if path.is_absolute() {
            bail!(
                "the archive contains the absolute member path `{}`, which is never legitimate; \
                 refusing the archive",
                path.display()
            );
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!(
                "the archive member `{}` climbs out with `..`; refusing the archive",
                path.display()
            );
        }
        let size = header.size().unwrap_or(0);
        if size > MAX_MEMBER_BYTES {
            bail!(
                "archive member `{}` claims {} bytes, over the {}-byte per-file limit",
                path.display(),
                size,
                MAX_MEMBER_BYTES
            );
        }
        total += size;
        if total > MAX_UNPACKED_BYTES {
            bail!(
                "the archive unpacks to over the {}-byte total limit; refusing",
                MAX_UNPACKED_BYTES
            );
        }

        // Strip the single common leading directory.
        let mut components = path.components();
        let first = components
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        let rest: PathBuf = components.collect();
        match (top.as_deref(), first.as_deref()) {
            (None, Some(f)) => {
                top = Some(f.to_string());
            }
            (Some(expected), Some(f)) => {
                if *f != *expected {
                    bail!(
                        "the archive has two top-level directories (`{expected}` and `{f}`); a \
                         repository tarball must have exactly one",
                    );
                }
            }
            (_, None) => {
                bail!(
                    "the archive has a member at the root (`{}`); a GitHub tarball always has \
                     a `repo-<sha>/` prefix",
                    path.display()
                );
            }
        }
        if rest.as_os_str().is_empty() {
            // The top-level directory itself; its children create it.
            continue;
        }

        let target = dest.join(&rest);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("could not create {}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let mut file = std::fs::File::create(&target)
            .with_context(|| format!("could not create {}", target.display()))?;
        let written = std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("could not write {}", target.display()))?;
        // Close before the chmod: a flushed-but-open handle can race on some
        // platforms, and the mode application is the last step anyway.
        drop(file);
        if written != size {
            bail!(
                "archive member `{}` says {} bytes but wrote {written}",
                path.display(),
                size
            );
        }
        apply_mode(&target, mode)
            .with_context(|| format!("could not set the mode of {}", target.display()))?;
    }
    if members == 0 {
        bail!("the archive is empty");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Apply the tar member's unix mode bits to the extracted file. The manual
/// copy above bypasses the tar crate's `set_preserve_permissions`, so the
/// executable bit is carried over explicitly — `command = ["./run.sh"]` must
/// survive an install without a chmod.
#[cfg(unix)]
fn apply_mode(target: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(target)?.permissions();
    perms.set_mode(mode & 0o777);
    std::fs::set_permissions(target, perms)
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn apply_mode(_target: &Path, _mode: u32) -> std::io::Result<()> {
    // Unix mode bits have nothing to map onto on this platform; a plugin is
    // expected to name an interpreter (python3, uvx, …) or a binary the
    // build produced.
    Ok(())
}

// ---------------------------------------------------------------------------
// Tree copying with exclusions
// ---------------------------------------------------------------------------

/// Copy `src` into `dest`, skipping the built-in excluded names plus any
/// caller-provided extras (the plugin's `[install].exclude`).
///
/// Symlinks are followed: a link that resolves to a regular file or directory
/// inside or outside the tree is copied as its content, never as a link — the
/// installed plugin must be a self-contained tree. A dangling link is skipped.
pub fn copy_tree(src: &Path, dest: &Path, extra_excludes: &[String]) -> anyhow::Result<()> {
    if let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) {
        if is_excluded(&name, extra_excludes) {
            return Ok(());
        }
    }
    // `symlink_metadata` would stop at the link; the copy must see what the
    // link points at.
    let meta = match std::fs::metadata(src) {
        Ok(meta) => meta,
        Err(_) => return Ok(()), // dangling symlink or raced away: skip
    };
    if meta.is_dir() {
        std::fs::create_dir_all(dest)
            .with_context(|| format!("could not create {}", dest.display()))?;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(src)
            .with_context(|| format!("could not read {}", src.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for entry in entries {
            copy_tree(
                &entry,
                &dest.join(entry.file_name().unwrap_or_default()),
                extra_excludes,
            )?;
        }
        Ok(())
    } else if meta.is_file() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::copy(src, dest)
            .with_context(|| format!("could not copy {} to {}", src.display(), dest.display()))?;
        preserve_exec_bit(&meta, dest)?;
        Ok(())
    } else {
        Ok(()) // fifos, sockets and the like have no place in a plugin tree
    }
}

/// Copy the executable bit from source metadata, so `command = ["./run.sh"]`
/// works without a chmod.
#[cfg(unix)]
fn preserve_exec_bit(meta: &std::fs::Metadata, dest: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    if mode & 0o111 != 0 {
        let mut perms = std::fs::metadata(dest)?.permissions();
        perms.set_mode(mode & 0o777);
        std::fs::set_permissions(dest, perms)
            .with_context(|| format!("could not chmod {}", dest.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn preserve_exec_bit(_meta: &std::fs::Metadata, _dest: &Path) -> anyhow::Result<()> {
    // The archive's unix mode bits have nothing to map onto; a plugin is
    // expected to name an interpreter (python3, uvx, …) or a binary the build
    // produced.
    Ok(())
}

fn is_excluded(name: &str, extra: &[String]) -> bool {
    EXCLUDED_DIR_NAMES.contains(&name) || extra.iter().any(|e| e == name)
}

/// After a build, keep only the `target/` entries the rewritten `command`
/// names (in destination coordinates: the staging tree is renamed to `dest`
/// before anything runs) and delete the rest of `target/`. A cargo build
/// leaves fingerprints, dependency artifacts and build-script output that can
/// dwarf the plugin binary itself, and none of it is needed once the binary
/// exists. Anything the command does not name is dropped.
fn prune_build_tree(staging: &Path, dest: &Path, command: &[String]) {
    let target_dir = staging.join("target");
    if !target_dir.is_dir() {
        return;
    }
    // What survives: every command argument under the installed `target/`,
    // mapped from destination coordinates (where the command points) back to
    // the staging tree (where the files are before the rename).
    let keep: Vec<PathBuf> = command
        .iter()
        .filter_map(|arg| {
            let path = Path::new(arg.trim());
            let relative = path.strip_prefix(dest).ok()?;
            let candidate = staging.join(relative);
            (candidate.is_file()).then_some(candidate)
        })
        .collect();
    if keep.is_empty() {
        // The command names nothing under target/: a pure build (e.g. `cargo
        // build` producing a shared object the manifest references by another
        // path) keeps target/ rather than deleting what was just produced.
        return;
    }
    // Delete everything in target/ except the keep-set and the directories on
    // each keep-path.
    let mut protected: Vec<PathBuf> = keep.clone();
    for file in &keep {
        let mut dir = file.as_path();
        while let Some(parent) = dir.parent() {
            if parent == staging {
                break;
            }
            protected.push(parent.to_path_buf());
            dir = parent;
        }
    }
    let mut stack = vec![target_dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
                // A directory that holds nothing protected goes now; one that
                // might is walked and its children judged individually.
                let holds_protected = protected
                    .iter()
                    .any(|p| p.starts_with(&path) || path.starts_with(p));
                if !holds_protected {
                    let _ = std::fs::remove_dir_all(&path);
                }
            } else if !protected.contains(&path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// The provenance record written to `install.json` beside `plugin.toml`, so a
/// later upgrade (or a human) can see where an installed plugin came from.
#[derive(Debug, serde::Serialize)]
struct InstallRecord {
    source: String,
    /// Commit sha when GitHub reported one, else the ref that was requested.
    resolved: Option<String>,
    /// `remote` or `local`.
    kind: &'static str,
    tarball_sha256: Option<String>,
    installed_at_ms: u64,
    command: Vec<String>,
    version: u64,
}

/// The whole operation minus the CLI wrapper. `data_dir` is the effective
/// directory (CLI override already applied by [`cli`]).
///
/// Progress goes to `reporter` — the CLI prints each line as it happens; an
/// HTTP job collects them for polling. The result names what was installed
/// and where, so a caller that is not a terminal can show it.
pub async fn install(
    options: &PluginInstallOptions,
    data_dir: &Path,
    reporter: &dyn InstallReporter,
) -> anyhow::Result<InstallOutcome> {
    let repository = fetch_repository(options, reporter).await?;

    let mut options = options.clone();
    options.data_dir = Some(data_dir.to_path_buf());

    // Select the plugins: named subset, or everything the repository offers.
    let selected: Vec<ResolvedPlugin> = if options.names.is_empty() {
        repository.plugins.clone()
    } else {
        let offered = repository
            .plugins
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = Vec::new();
        for wanted in &options.names {
            let found = repository
                .plugins
                .iter()
                .find(|p| &p.name == wanted)
                .with_context(|| {
                    format!(
                        "the repository offers {offered} but not `{wanted}`; run with --list to \
                         see the names"
                    )
                })?;
            out.push(found.clone());
        }
        out
    };
    if selected.is_empty() {
        bail!(
            "the repository offers no plugins; expected a root index.toml, */plugin.toml or a \
             root plugin.toml"
        );
    }
    // Warnings gathered while resolving (operator-only keys stripped) are
    // part of the run's progress: every surface shows them.
    for plugin in &selected {
        for warning in &plugin.warnings {
            reporter.emit(InstallEvent::Note {
                message: warning.clone(),
            });
        }
    }

    let plugins_root = data_dir.join("plugins");
    std::fs::create_dir_all(&plugins_root)
        .with_context(|| format!("could not create {}", plugins_root.display()))?;

    let installer = PluginInstaller {
        options: &options,
        repository: &repository,
        reporter,
    };
    let mut installed: Vec<String> = Vec::new();
    let mut paths: Vec<(String, String)> = Vec::new();
    let mut failure: Option<anyhow::Error> = None;
    for plugin in &selected {
        match installer.install(plugin, &plugins_root).await {
            Ok(path) => {
                reporter.emit(InstallEvent::Installed {
                    plugin: plugin.name.clone(),
                    path: path.clone(),
                });
                paths.push((plugin.name.clone(), path));
                installed.push(plugin.name.clone());
            }
            Err(error) => {
                failure = Some(error.context(format!("could not install `{}`", plugin.name)));
                break;
            }
        }
    }
    cleanup(&repository);
    if let Some(error) = failure {
        return Err(error);
    }
    reporter.emit(InstallEvent::Done {
        installed: installed.len(),
    });
    Ok(InstallOutcome { installed, paths })
}

/// Remove the staging tree when it is a temporary one. A local repository's
/// staging copy is disposable; the operator's own directory is never touched.
fn cleanup(repository: &Repository) {
    let _ = std::fs::remove_dir_all(&repository.root);
}

struct PluginInstaller<'a> {
    options: &'a PluginInstallOptions,
    repository: &'a Repository,
    reporter: &'a dyn InstallReporter,
}

impl PluginInstaller<'_> {
    /// Install one plugin: stage, build, rewrite, swap into place.
    async fn install(
        &self,
        plugin: &ResolvedPlugin,
        plugins_root: &Path,
    ) -> anyhow::Result<String> {
        let name = &plugin.name;
        let dest = plugins_root.join(name);

        // Refuse an existing install unless --force.
        if dest.exists() && !self.options.force {
            bail!(
                "{} already exists; pass --force to replace it (the current directory is moved \
                 to `{name}.old` first, so a rollback is one rename)",
                dest.display()
            );
        }

        // Stage in a temporary directory so a failed build or rewrite leaves
        // nothing behind in the data directory.
        let staging = tempfile_dir(&format!("karucode-plugin-install-{name}"))?;
        let origin_dir = self.repository.plugin_origin_dir(plugin);
        if !origin_dir.is_dir() {
            bail!(
                "plugin `{name}`: {} does not exist in the fetched repository",
                origin_dir.display()
            );
        }
        let extra_excludes: Vec<String> = plugin
            .install_spec()
            .map(|spec| spec.exclude.clone())
            .unwrap_or_default();
        copy_tree(&origin_dir, &staging, &extra_excludes)
            .with_context(|| format!("could not stage `{name}`'s files"))?;

        // Build, unless --no-build. Two routes, in priority order:
        //
        // 1. the `[install]` table the manifest declares — the explicit one,
        //    whatever command it carries;
        // 2. an **implied** build when the manifest's `command` names a file
        //    under `target/` that is not in the staged copy (it never is:
        //    `target/` is excluded) and a `Cargo.toml` sits in the plugin
        //    directory. This is the overwhelmingly common shape — a Rust
        //    plugin whose manifest points at `target/release/<name>` without
        //    declaring how it gets built. Implying `cargo build --release`
        //    there keeps `plugin install` one step instead of an error that
        //    tells the operator to write `[install]` by hand.
        let spec = match plugin.install_spec() {
            Some(spec) if !spec.build.is_empty() => Some(spec.clone()),
            _ => {
                let implied = implied_build(&plugin.manifest.command, &staging);
                if implied.is_some() {
                    self.reporter.emit(InstallEvent::Note {
                        message: format!(
                            "note: `{name}` does not declare [install], but its command points \
                             at a build artifact; running `cargo build --release` for it"
                        ),
                    });
                }
                implied
            }
        };
        if let Some(spec) = spec {
            if !self.options.no_build {
                self.build(name, &staging, &spec).await?;
            } else {
                self.reporter.emit(InstallEvent::Note {
                    message: format!("--no-build given; skipping the build for `{name}`"),
                });
            }
        }

        // Rewrite the manifest's command for the **installed** location: the
        // staging directory is renamed into place, so a command pointing at
        // it would name a path that no longer exists once the rename lands.
        // File existence is checked against the staging copy (that is where
        // the files are right now); the written path is the same position
        // under `dest`.
        let command = self
            .rewrite_command(&plugin.manifest.command, &staging, &origin_dir, &dest)
            .with_context(|| format!("plugin `{name}`: unusable `command`"))?;
        let manifest_text = rewrite_manifest(&plugin.manifest_text, &command)?;

        // A build leaves `target/` full of intermediates that have no business
        // living in the data directory (dependency objects, fingerprints,
        // sometimes hundreds of MiB). Keep the artifacts the rewritten
        // `command` actually names — now in destination coordinates — and
        // drop the rest of `target/`.
        prune_build_tree(&staging, &dest, &command);

        let record = InstallRecord {
            source: self.repository.source_label.clone(),
            resolved: self.repository.resolved.clone(),
            kind: if self.repository.is_remote() {
                "remote"
            } else {
                "local"
            },
            tarball_sha256: self.repository.tarball_sha256.clone(),
            installed_at_ms: now_ms() as u64,
            command: command.clone(),
            version: 1,
        };
        let record_text = serde_json::to_string_pretty(&record)
            .map_err(|e| anyhow!("could not serialize the install record: {e}"))?;

        // Everything is staged; swap it in.
        let mut moved_aside = false;
        if dest.exists() {
            let old = plugins_root.join(format!("{name}.old"));
            if old.exists() {
                let _ = std::fs::remove_dir_all(&old);
            }
            std::fs::rename(&dest, &old)
                .with_context(|| format!("could not move {} aside", dest.display()))?;
            moved_aside = true;
        }
        std::fs::write(staging.join("plugin.toml"), manifest_text)
            .with_context(|| format!("could not write {}/plugin.toml", staging.display()))?;
        std::fs::write(staging.join("install.json"), record_text)
            .with_context(|| format!("could not write {}/install.json", staging.display()))?;
        if let Err(error) = std::fs::rename(&staging, &dest) {
            // Put the previous install back: a failed swap must not leave the
            // data directory without the plugin it had.
            if moved_aside {
                let old = plugins_root.join(format!("{name}.old"));
                if let Err(restore) = std::fs::rename(&old, &dest) {
                    tracing::warn!(
                        plugin = %name,
                        error = %restore,
                        "could not restore the previous install after a failed swap"
                    );
                }
            }
            return Err(anyhow!(error)).with_context(|| {
                format!(
                    "could not move the staged install into {}; the staged copy is at {}",
                    dest.display(),
                    staging.display()
                )
            });
        }
        // The swap landed: the moved-aside copy is now dead weight (its
        // `target/` alone can be hundreds of MiB), so it is dropped rather
        // than left to accumulate. `--force` already implies the operator
        // accepted losing the previous version.
        if moved_aside {
            let old = plugins_root.join(format!("{name}.old"));
            let _ = std::fs::remove_dir_all(&old);
        }
        Ok(dest.display().to_string())
    }

    /// Run the plugin's `[install].build` in its staging directory. The build
    /// inherits the operator's environment, and its stdio is captured so the
    /// HTTP job can report the tail when a build fails (the CLI used to let
    /// it stream straight through, which a browser cannot do).
    async fn build(&self, name: &str, staging: &Path, spec: &InstallSpec) -> anyhow::Result<()> {
        self.reporter.emit(InstallEvent::Building {
            plugin: name.to_string(),
            command: format!("{} in {}", spec.build.join(" "), staging.display()),
        });
        let mut command = build_command(&spec.build);
        command.current_dir(staging).kill_on_drop(true);
        // Captured, not inherited: the install may run as an HTTP job with no
        // terminal attached, and a failing build's actual error lines are the
        // single most useful thing an install failure can carry.
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let child = command
            .spawn()
            .with_context(|| format!("could not start `{}`", spec.build.join(" ")))?;
        let pid = child.id();
        let timeout = Duration::from_secs(
            self.options
                .build_timeout_secs
                .or(spec.build_timeout_secs)
                .unwrap_or(DEFAULT_BUILD_TIMEOUT_SECS),
        );
        let outcome = tokio::time::timeout(timeout, child.wait_with_output()).await;
        match outcome {
            Err(_) => {
                kill_build_tree(pid);
                bail!(
                    "build of `{name}` timed out after {}s (command: `{}`); raise \
                     build_timeout_secs in the plugin's [install] table or pass \
                     --build-timeout-secs",
                    timeout.as_secs(),
                    spec.build.join(" ")
                );
            }
            Ok(Err(e)) => bail!("could not wait for the build of `{name}`: {e}"),
            Ok(Ok(output)) => {
                if !output.status.success() {
                    // The captured tail is why stdio is piped: "exit code 101"
                    // alone would send the operator hunting for a log that
                    // only exists on the terminal of whoever ran the CLI.
                    let tail = output_tail(&output.stdout, &output.stderr, 4000);
                    bail!(
                        "build of `{name}` failed with status {} (command: `{}`){}",
                        output.status,
                        spec.build.join(" "),
                        tail.map(|text| format!(":\n{text}")).unwrap_or_else(|| {
                            format!(". Run it by hand in {} to see the error", staging.display())
                        })
                    );
                }
            }
        }
        Ok(())
    }

    /// Rewrite the manifest's `command` for the **installed** location:
    ///
    /// * an absolute path inside the plugin's source directory maps to the
    ///   same relative position under the install directory;
    /// * any other absolute path is refused — a manifest the installer cannot
    ///   relocate would install a plugin that cannot start;
    /// * a relative argument that names a file in the plugin's directory maps
    ///   to the installed file (the host resolves relative paths against the
    ///   *server's* cwd, so leaving one would work only by accident);
    /// * a bare program name (`python3`) is left alone: argv[0] resolves on
    ///   PATH;
    /// * a bare argument that is not a file (`--release`) is left alone.
    ///
    /// `staging` is where the files physically are during the rewrite (the
    /// build just ran there); `dest` is where that directory will be renamed.
    /// Existence checks run against `staging`, the emitted paths against
    /// `dest`, so the written manifest names files at their final location.
    fn rewrite_command(
        &self,
        command: &[String],
        staging: &Path,
        origin_dir: &Path,
        dest: &Path,
    ) -> anyhow::Result<Vec<String>> {
        let origin_canon = canonicalish(origin_dir);
        let mut out = Vec::with_capacity(command.len());
        for (index, arg) in command.iter().enumerate() {
            let arg = arg.trim();
            if arg.is_empty() {
                bail!("`command` has an empty argument");
            }
            if arg.starts_with('~') {
                bail!(
                    "`command` argument `{arg}` starts with `~`; use a path relative to the \
                     plugin's own directory or an absolute path inside it"
                );
            }
            let path = Path::new(arg);
            if path.is_absolute() {
                let arg_canon = canonicalish(path);
                if let Ok(rest) = arg_canon.strip_prefix(&origin_canon) {
                    if !staging.join(rest).exists() {
                        bail!(
                            "`command` argument `{arg}` names {} inside the plugin's source, but \
                             the file is missing from the staged copy; the build step was \
                             supposed to produce it",
                            rest.display()
                        );
                    }
                    out.push(dest.join(rest).display().to_string());
                    continue;
                }
                bail!(
                    "`command` argument `{arg}` is absolute and outside the plugin's source \
                     directory {}; such a plugin is not relocatable and cannot be installed",
                    origin_dir.display()
                );
            }
            // A relative path with separators, or any bare argument that
            // happens to name a file in the plugin directory (`atlas.py`):
            // map it into the install. Components are normalised so the
            // written manifest carries a clean path, not `…/./wordy.sh`.
            if staging.join(normalise_rel(path)).is_file() {
                let mapped = dest.join(normalise_rel(path));
                out.push(mapped.display().to_string());
                continue;
            }
            if origin_dir.join(path).is_file() {
                // Present in the origin but excluded from the staged copy
                // (`[install].exclude`): refuse rather than install a command
                // whose file is not there.
                bail!(
                    "`command` argument `{arg}` names a file that the install excludes; remove \
                     it from `[install].exclude` or change the command"
                );
            }
            if index == 0 && (arg.contains('/') || arg.contains('\\')) {
                bail!(
                    "`command[0]` is `{arg}`: a relative program path resolves against the \
                     server's working directory, not the plugin's. Name a program on PATH or a \
                     file inside the plugin directory"
                );
            }
            if index == 0 {
                // A bare program name: resolved on PATH by the host, kept.
                out.push(arg.to_string());
                continue;
            }
            // An argument that is not a file: a flag or a value; kept.
            out.push(arg.to_string());
        }
        Ok(out)
    }
}

/// The implied `[install]` spec for a Rust plugin whose manifest points at
/// `target/<profile>/<name>` without declaring one: `cargo build --release`
/// when the named artifact is missing from the staged copy (always, because
/// `target/` is never staged) and a `Cargo.toml` is present in `plugin_dir`.
///
/// Returns `None` when nothing should be implied — the command names no
/// missing build artifact, or there is no Cargo.toml to build from. Free so a
/// preview and the install itself agree on "will this plugin build".
fn implied_build(command: &[String], plugin_dir: &Path) -> Option<InstallSpec> {
    const DEFAULT_BUILD: [&str; 3] = ["cargo", "build", "--release"];
    if !plugin_dir.join("Cargo.toml").is_file() {
        return None;
    }
    let names_missing_target = command.iter().any(|arg| {
        let arg = arg.trim();
        if !arg.contains('/') {
            return false;
        }
        // `<...>/target/release/<bin>` or `<...>/target/debug/<bin>`:
        // an artifact cargo would produce, by any prefix (absolute into
        // the source checkout, or relative to the plugin directory). It
        // is never in the staged copy (target/ is excluded), so implying
        // the build is what produces it.
        let components: Vec<_> = Path::new(arg).components().collect();
        components.windows(2).any(|window| {
            let first = window[0].as_os_str().to_string_lossy();
            let second = window[1].as_os_str().to_string_lossy();
            first == "target" && (second == "release" || second == "debug")
        })
    });
    if !names_missing_target {
        return None;
    }
    Some(InstallSpec {
        build: DEFAULT_BUILD.iter().map(|s| s.to_string()).collect(),
        build_timeout_secs: None,
        exclude: Vec::new(),
    })
}

/// Build the `tokio::process::Command` for an `[install].build` entry, with
/// the build in its own process group so a timeout can tear down the whole
/// tree (`cargo` spawns `rustc` spawns linkers — killing only the parent
/// orphans the rest, which then hold locks in `target/`).
#[cfg(unix)]
fn build_command(build: &[String]) -> tokio::process::Command {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(&build[0]);
    command.args(&build[1..]);
    // Group id 0 = "a new group with the child as leader", the same approach
    // the plugin host uses for plugin processes (kc_plugin_host::procgroup).
    command.process_group(0);
    tokio::process::Command::from(command)
}

#[cfg(not(unix))]
fn build_command(build: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&build[0]);
    command.args(&build[1..]);
    command
}

/// Kill a build's whole process group. Best-effort: a dead pid is fine.
#[cfg(unix)]
fn kill_build_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    // Negative pid = the process group the child leads.
    let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn kill_build_tree(_pid: Option<u32>) {}

/// The last `max_chars` of a build's combined stdout+stderr, for error
/// messages. `None` when both were empty.
fn output_tail(stdout: &[u8], stderr: &[u8], max_chars: usize) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let err_text = String::from_utf8_lossy(stderr);
    let mut combined = String::new();
    if !text.trim().is_empty() {
        combined.push_str(text.trim_end());
    }
    if !err_text.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(err_text.trim_end());
    }
    if combined.is_empty() {
        return None;
    }
    if combined.chars().count() <= max_chars {
        return Some(combined);
    }
    // Keep the *end*: compiler errors are at the tail.
    let skipped = combined.chars().count() - max_chars;
    let tail: String = combined.chars().skip(skipped).collect();
    Some(format!("[… {skipped} characters omitted]\n{tail}"))
}

/// Replace the `command = [...]` assignment in a manifest's text with the
/// rewritten one, leaving every other byte alone (comments, ordering and
/// unknown-but-harmless keys round-trip). Handles both a one-line array and a
/// multi-line one.
fn rewrite_manifest(original: &str, command: &[String]) -> anyhow::Result<String> {
    let replacement = format!("command = {}", toml_array(command));
    let mut out = String::with_capacity(original.len() + command.len() * 8);
    let mut replaced = false;
    let mut lines = original.lines().peekable();
    // Track the current TOML table so a `command` key inside `[install]` is
    // not mistaken for the top-level one.
    let mut in_table = false;
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_table = true;
        }
        if !replaced
            && !in_table
            && trimmed.starts_with("command")
            && trimmed
                .trim_start_matches("command")
                .trim_start()
                .starts_with('=')
        {
            out.push_str(&replacement);
            out.push('\n');
            replaced = true;
            // Consume the rest of a multi-line array.
            let opens = trimmed.matches('[').count() as i64;
            let closes = trimmed.matches(']').count() as i64;
            let mut depth = opens - closes;
            while depth > 0 {
                match lines.next() {
                    Some(next) => {
                        depth += next.matches('[').count() as i64;
                        depth -= next.matches(']').count() as i64;
                    }
                    None => {
                        bail!("the manifest's `command` array is not closed; cannot rewrite it")
                    }
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        bail!(
            "the manifest has no top-level `command = [...]` assignment to rewrite; this \
             installer only understands simple commands"
        );
    }
    Ok(out)
}

/// Drop `.` components from a relative path so a joined path reads clean.
/// `..` is never valid here (the caller already knows the file exists inside
/// the plugin's directory, so a climb cannot be real).
fn normalise_rel(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// `["a", "b"]` as a TOML array literal, with basic-string escaping.
fn toml_array(items: &[String]) -> String {
    let rendered: Vec<String> = items
        .iter()
        .map(|item| {
            let escaped = item
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\t', "\\t");
            format!("\"{escaped}\"")
        })
        .collect();
    format!("[{}]", rendered.join(", "))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "kc-plugin-install-test-{}-{}-{n}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_source_accepts_owner_repo_forms() {
        let s = parse_source("karucode/kc-wordy").unwrap();
        assert_eq!(
            (s.owner.as_str(), s.repo.as_str()),
            ("karucode", "kc-wordy")
        );
        assert!(s.subdir.is_none());
        assert!(s.url_ref.is_none());
        assert!(s.local_dir.is_none());

        let s = parse_source("karucode/kc-wordy@v0.2.0").unwrap();
        assert_eq!(s.url_ref.as_deref(), Some("v0.2.0"));

        let s = parse_source("https://github.com/karucode/kc-wordy").unwrap();
        assert_eq!(s.slug(), "karucode/kc-wordy");

        let s = parse_source("https://github.com/karucode/kc-wordy.git").unwrap();
        assert_eq!(s.slug(), "karucode/kc-wordy");
        assert!(s.url_ref.is_none());

        let s = parse_source("https://github.com/karucode/kc-wordy/").unwrap();
        assert_eq!(s.slug(), "karucode/kc-wordy");

        let s = parse_source("git@github.com:karucode/kc-wordy.git").unwrap();
        assert_eq!(s.slug(), "karucode/kc-wordy");

        let s = parse_source("https://github.com/karucode/plugins/tree/main/kc-wordy").unwrap();
        assert_eq!(s.url_ref.as_deref(), Some("main"));
        assert_eq!(s.subdir.as_deref(), Some("kc-wordy"));

        let s = parse_source("https://github.com/karucode/plugins/tree/main").unwrap();
        assert_eq!(s.url_ref.as_deref(), Some("main"));
        assert!(s.subdir.is_none());
    }

    #[test]
    fn parse_source_refuses_other_hosts_and_junk() {
        let err = parse_source("https://gitlab.com/foo/bar").unwrap_err();
        assert!(err.to_string().contains("only github.com"), "{err}");
        assert!(parse_source("ssh://git@github.com/foo/bar").is_err());
        assert!(parse_source("wordy").is_err());
        assert!(parse_source("").is_err());
        assert!(parse_source("./not/here").is_err());
        assert!(parse_source("/not/here").is_err());
        assert!(parse_source("a/b/c").is_err());
        let err = parse_source("https://github.com/karucode/plugins/tree/main/a/b").unwrap_err();
        assert!(err.to_string().contains("deeper than one"), "{err}");
    }

    #[test]
    fn parse_source_local_directory_wins() {
        let dir = tmp();
        let source = parse_source(dir.to_str().unwrap()).unwrap();
        assert!(source.local_dir.is_some());
        assert!(source.owner.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ref_to_use_prefers_the_flag() {
        let source = parse_source("karucode/plugins").unwrap();
        assert_eq!(source.ref_to_use(None), "HEAD");
        assert_eq!(source.ref_to_use(Some("v3")), "v3");
        let source = parse_source("karucode/plugins@v2").unwrap();
        assert_eq!(source.ref_to_use(None), "v2");
        assert_eq!(source.ref_to_use(Some("v3")), "v3");
    }

    #[test]
    fn safe_rel_rejects_climbing_and_absolute() {
        assert_eq!(safe_rel("kc-wordy").unwrap(), PathBuf::from("kc-wordy"));
        assert_eq!(safe_rel("/kc-wordy").unwrap(), PathBuf::from("kc-wordy"));
        assert_eq!(safe_rel("./kc-wordy").unwrap(), PathBuf::from("kc-wordy"));
        assert!(safe_rel("../outside").is_err());
        assert!(safe_rel("a/../../b").is_err());
        assert!(safe_rel("").is_err());
    }

    #[test]
    fn validate_plugin_name_matches_the_sdk_rule() {
        assert!(validate_plugin_name("wordy").is_ok());
        assert!(validate_plugin_name("kc-2-fast").is_ok());
        assert!(validate_plugin_name("wordy_2").is_ok());
        assert!(validate_plugin_name("Wordy").is_err());
        assert!(validate_plugin_name("2wordy").is_err());
        assert!(validate_plugin_name("wordy extra").is_err());
        assert!(validate_plugin_name("wordy/extra").is_err());
        assert!(validate_plugin_name("").is_err());
    }

    #[test]
    fn toml_array_escapes_paths() {
        let out = toml_array(&["/a b/c".into(), "quote\"x".into()]);
        assert_eq!(out, "[\"/a b/c\", \"quote\\\"x\"]");
    }

    #[test]
    fn codeload_url_is_the_anonymous_tarball_endpoint() {
        assert_eq!(
            codeload_url("karucode", "kc-wordy", "HEAD"),
            "https://codeload.github.com/karucode/kc-wordy/tar.gz/HEAD"
        );
        assert_eq!(
            codeload_url("karucode", "kc-wordy", "ab0814f"),
            "https://codeload.github.com/karucode/kc-wordy/tar.gz/ab0814f"
        );
    }

    #[test]
    fn content_disposition_sha_is_parsed_from_codeloads_filename() {
        // codeload answers:
        //   Content-Disposition: attachment; filename=repo-<sha>.tar.gz
        // The commit sha is the only provenance a tarball download gets.
        let value = "attachment; filename=repo-deadbeef1234567890.tar.gz";
        let sha = value
            .split('-')
            .nth(1)
            .and_then(|middle| middle.split('.').next())
            .map(str::to_string)
            .filter(|sha| sha.len() >= 7);
        assert_eq!(sha.as_deref(), Some("deadbeef1234567890"));

        // A header without the filename keeps `resolved` empty (the label
        // falls back to the requested ref).
        let value = "attachment";
        assert!(value
            .split('-')
            .nth(1)
            .and_then(|middle| middle.split('.').next())
            .is_none());
    }

    // -- manifest rewriting --------------------------------------------------

    #[test]
    fn rewrite_manifest_replaces_only_the_command_line() {
        let original =
            "# a comment\nname = \"wordy\"\ncommand = [\"/old/path\"]\n\n[settings]\nx = 1\n";
        let rewritten = rewrite_manifest(original, &["/new/path".into()]).unwrap();
        assert!(rewritten.starts_with("# a comment\n"));
        assert!(rewritten.contains("command = [\"/new/path\"]"));
        assert!(!rewritten.contains("/old/path"));
        assert!(rewritten.contains("[settings]"));
        assert!(rewritten.contains("name = \"wordy\""));
    }

    #[test]
    fn rewrite_manifest_consumes_a_multiline_command() {
        let original = "name = \"wordy\"\ncommand = [\n  \"/old/path\",\n  \"arg\",\n]\n";
        let rewritten = rewrite_manifest(original, &["/new/path".into()]).unwrap();
        assert!(rewritten.contains("command = [\"/new/path\"]"));
        assert!(!rewritten.contains("/old/path"));
        assert!(!rewritten.contains("\"arg\""));
        // The manifest must still parse.
        let file: PluginFileConfig = toml::from_str(&rewritten).unwrap();
        assert_eq!(file.command, vec!["/new/path".to_string()]);
    }

    #[test]
    fn rewrite_manifest_requires_a_command() {
        assert!(rewrite_manifest("name = \"wordy\"\n", &["x".into()]).is_err());
        // A command inside a table does not count.
        let original = "[install]\ncommand = [\"cargo\"]\n";
        assert!(rewrite_manifest(original, &["x".into()]).is_err());
    }

    // -- tarball extraction --------------------------------------------------

    fn make_tar_gz(entries: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            let path = format!("repo-abc/{path}");
            builder
                .append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        gz(builder.into_inner().unwrap())
    }

    fn make_tar_gz_raw(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            // The tar crate refuses to *write* members with `..` or absolute
            // paths (its own safety check), so the header's path field is
            // written by hand: a hostile archive is exactly what these tests
            // must construct.
            write_header_path(&mut header, path);
            header.set_cksum();
            builder
                .append(&header, std::io::Cursor::new(contents.as_bytes()))
                .unwrap();
        }
        gz(builder.into_inner().unwrap())
    }

    /// Write the raw path bytes into a GNU header, bypassing the tar crate's
    /// path validation. `append_data` and `append_path` both refuse to build a
    /// hostile member; `append(&header, data)` trusts a header that already
    /// has its path set, which is the one door that needs opening for a test
    /// fixture (never for real input).
    fn write_header_path(header: &mut tar::Header, path: &str) {
        let bytes = path.as_bytes();
        assert!(bytes.len() <= 99, "test paths must fit the name field");
        let field: &mut [u8] = &mut header.as_gnu_mut().unwrap().name;
        field[..bytes.len()].copy_from_slice(bytes);
        field[bytes.len()..].fill(0);
    }

    fn gz(bytes: Vec<u8>) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extract_tarball_strips_top_dir_and_keeps_exec_bits() {
        let tarball = make_tar_gz(&[
            ("run.sh", "#!/bin/sh\n", 0o755),
            ("plugin.toml", "name = \"x\"\n", 0o644),
            ("sub/dir.txt", "deep\n", 0o644),
        ]);
        let dest = tmp();
        extract_tarball(&tarball, &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("run.sh")).unwrap(),
            "#!/bin/sh\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("plugin.toml")).unwrap(),
            "name = \"x\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("sub/dir.txt")).unwrap(),
            "deep\n"
        );
        assert!(!dest.join("repo-abc").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dest.join("run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the exec bit survived the extract");
        }
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_links() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_data(&mut header, "repo-abc/link", std::io::empty())
            .unwrap();
        let tarball = gz(builder.into_inner().unwrap());
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("link"), "{err}");
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_hard_links() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Link);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_data(&mut header, "repo-abc/link", std::io::empty())
            .unwrap();
        let tarball = gz(builder.into_inner().unwrap());
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("link"), "{err}");
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_path_traversal() {
        let tarball = make_tar_gz_raw(&[("../escape.txt", "pwned")]);
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("climbs out"), "{err}");
        // Nothing was written anywhere near the escape target.
        assert!(!dest.parent().unwrap().join("escape.txt").exists());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_absolute_member() {
        let tarball = make_tar_gz_raw(&[("/etc/pwned", "x")]);
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("absolute"), "{err}");
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_empty_archive() {
        let tarball = gz(Vec::new());
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn extract_tarball_rejects_two_top_dirs() {
        let tarball = make_tar_gz_raw(&[("a-one/file.txt", "x"), ("b-two/other.txt", "y")]);
        let dest = tmp();
        let err = extract_tarball(&tarball, &dest).unwrap_err();
        assert!(err.to_string().contains("two top-level"), "{err}");
        std::fs::remove_dir_all(&dest).ok();
    }

    // -- repository resolution ----------------------------------------------

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().unwrap()
    }

    #[test]
    fn resolve_plugins_auto_scan_skips_dirs_without_manifests() {
        let root = tmp();
        for name in ["kc-alpha", "kc-beta"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("plugin.toml"),
                format!("name = \"{name}\"\ncommand = [\"/{name}\"]\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("not-a-plugin")).unwrap();
        let plugins = runtime().block_on(resolve_plugins(&root, None)).unwrap();
        let names: Vec<String> = plugins.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["kc-alpha", "kc-beta"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_plugins_index_toml_is_authoritative() {
        let root = tmp();
        std::fs::create_dir_all(root.join("kc-alpha")).unwrap();
        std::fs::write(
            root.join("kc-alpha/plugin.toml"),
            "name = \"kc-alpha\"\ncommand = [\"/kc-alpha\"]\n",
        )
        .unwrap();
        // A plugin dir with no index entry is invisible when an index exists.
        std::fs::create_dir_all(root.join("kc-hidden")).unwrap();
        std::fs::write(
            root.join("kc-hidden/plugin.toml"),
            "name = \"kc-hidden\"\ncommand = [\"/kc-hidden\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("index.toml"),
            "[[plugins]]\nname = \"kc-alpha\"\npath = \"kc-alpha\"\ndescription = \"Alpha\"\n",
        )
        .unwrap();
        let plugins = runtime().block_on(resolve_plugins(&root, None)).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "kc-alpha");
        assert_eq!(plugins[0].rel_dir, PathBuf::from("kc-alpha"));
        assert_eq!(plugins[0].description.as_deref(), Some("Alpha"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_plugins_index_name_must_match_manifest() {
        let root = tmp();
        std::fs::create_dir_all(root.join("kc-alpha")).unwrap();
        std::fs::write(
            root.join("kc-alpha/plugin.toml"),
            "name = \"actually-beta\"\ncommand = [\"/x\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("index.toml"),
            "[[plugins]]\nname = \"kc-alpha\"\npath = \"kc-alpha\"\n",
        )
        .unwrap();
        let err = runtime()
            .block_on(resolve_plugins(&root, None))
            .unwrap_err();
        assert!(err.to_string().contains("actually-beta"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_plugins_single_plugin_repo_root() {
        let root = tmp();
        std::fs::write(
            root.join("plugin.toml"),
            "name = \"wordy\"\ncommand = [\"/opt/wordy\"]\n",
        )
        .unwrap();
        let plugins = runtime().block_on(resolve_plugins(&root, None)).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "wordy");
        assert!(plugins[0].rel_dir.as_os_str().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_plugins_without_a_name_needs_a_directory_name() {
        let root = tmp();
        std::fs::write(root.join("plugin.toml"), "command = [\"/opt/x\"]\n").unwrap();
        let err = runtime()
            .block_on(resolve_plugins(&root, None))
            .unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_plugins_subdir_scopes_the_scan() {
        let root = tmp();
        for name in ["one", "two"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("plugin.toml"),
                format!("name = \"{name}\"\ncommand = [\"/{name}\"]\n"),
            )
            .unwrap();
        }
        let plugins = runtime()
            .block_on(resolve_plugins(&root, Some("one")))
            .unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "one");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_manifest_strips_operator_only_keys_from_the_written_text() {
        // The three operator-only grants: each is stripped from the manifest
        // text that gets written into the install (the install still happens,
        // with a warning), mirroring how the host treats a disk manifest.
        for (body, dropped) in [
            (
                "name = \"x\"\ncommand = [\"/x\"]\npermissions = [\"*\"]\n",
                "permissions",
            ),
            (
                "name = \"x\"\ncommand = [\"/x\"]\n[settings]\ntrust = true\n",
                "trust",
            ),
            (
                "name = \"x\"\ncommand = [\"/x\"]\n[settings]\nread_only_tools = [\"a\"]\n",
                "read_only_tools",
            ),
            (
                "name = \"x\"\ncommand = [\"/x\"]\n[settings]\nread_only_tools = [\n  \"a\",\n  \"b\",\n]\n",
                "read_only_tools",
            ),
        ] {
            let dir = tmp();
            let path = dir.join("plugin.toml");
            std::fs::write(&path, body).unwrap();
            let plugin = read_manifest(&path, PathBuf::from("x"), None, None).unwrap();
            assert!(
                !plugin.manifest_text.contains(dropped),
                "`{dropped}` must not survive into the written manifest:\n{}",
                plugin.manifest_text
            );
            assert!(plugin.manifest_text.contains("command"));
            assert!(plugin.manifest_text.contains("name"));
            // The stripped text is still valid TOML.
            let reparsed: PluginFileConfig = toml::from_str(&plugin.manifest_text).unwrap();
            assert!(!reparsed.permissions.contains(&"*".to_string()));
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn read_manifest_keeps_narrow_permissions_and_unknown_host_keys() {
        // `permissions = ["files.read"]` is a reviewed grant, kept as written.
        // `panels`/`ui` are host-side keys this installer never inspects; they
        // must round-trip, or a plugin using them could not be installed.
        let body = "name = \"x\"\ncommand = [\"/x\"]\npermissions = [\"files.read\"]\n\
                    panels = [{ id = \"words\", title = \"Words\" }]\n";
        let dir = tmp();
        let path = dir.join("plugin.toml");
        std::fs::write(&path, body).unwrap();
        let plugin = read_manifest(&path, PathBuf::from("x"), None, None).unwrap();
        assert_eq!(plugin.manifest.permissions, vec!["files.read"]);
        assert!(plugin.manifest_text.contains("panels"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn strip_operator_only_lines_only_touches_the_security_keys() {
        let original = "name = \"x\"\npermissions = [\"shell\", \"network\"]\n\n\
                        [settings]\ntrust = true\ncall_timeout_secs = 50\n\n\
                        [install]\nbuild = [\"cargo\", \"build\"]\n";
        let stripped = strip_operator_only_lines(original);
        assert!(!stripped.contains("trust"), "{stripped}");
        assert!(stripped.contains("permissions = [\"shell\", \"network\"]"));
        assert!(stripped.contains("call_timeout_secs = 50"));
        assert!(stripped.contains("[install]"));
        // A `permissions` list without the wildcard is untouched.
        assert!(!stripped.contains('*'));
    }

    #[test]
    fn read_manifest_rejects_a_missing_command() {
        let dir = tmp();
        let path = dir.join("plugin.toml");
        std::fs::write(&path, "name = \"x\"\n").unwrap();
        let err = read_manifest(&path, PathBuf::from("x"), None, None).unwrap_err();
        assert!(err.to_string().contains("command"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_manifest_reads_the_install_table() {
        let dir = tmp();
        let path = dir.join("plugin.toml");
        std::fs::write(
            &path,
            "name = \"x\"\ncommand = [\"/x\"]\n[install]\nbuild = [\"cargo\", \"build\", \"--release\"]\n\
             build_timeout_secs = 120\nexclude = [\"examples\"]\n",
        )
        .unwrap();
        let plugin = read_manifest(&path, PathBuf::from("x"), None, None).unwrap();
        let spec = plugin.install_spec().expect("install table");
        assert_eq!(spec.build, vec!["cargo", "build", "--release"]);
        assert_eq!(spec.build_timeout_secs, Some(120));
        assert_eq!(spec.exclude, vec!["examples"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_manifest_pulls_the_description_line() {
        let dir = tmp();
        let path = dir.join("plugin.toml");
        std::fs::write(
            &path,
            "name = \"x\"\ndescription = \"Counts words\"\ncommand = [\"/x\"]\n",
        )
        .unwrap();
        let plugin = read_manifest(&path, PathBuf::from("x"), None, None).unwrap();
        assert_eq!(plugin.description.as_deref(), Some("Counts words"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- copy_tree -----------------------------------------------------------

    #[test]
    fn copy_tree_skips_build_outputs_and_caches() {
        let src = tmp();
        std::fs::write(src.join("plugin.toml"), "command = [\"x\"]\n").unwrap();
        std::fs::create_dir_all(src.join("target").join("release")).unwrap();
        std::fs::write(src.join("target/release/stale"), "old").unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref").unwrap();
        std::fs::create_dir_all(src.join("src")).unwrap();
        std::fs::write(src.join("src/main.rs"), "fn main() {}\n").unwrap();
        let dest = tmp();
        copy_tree(&src, &dest, &["extra-junk".into()]).unwrap();
        assert!(dest.join("plugin.toml").is_file());
        assert!(dest.join("src/main.rs").is_file());
        assert!(!dest.join("target").exists());
        assert!(!dest.join(".git").exists());
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn copy_tree_follows_and_flattens_symlinks() {
        let src = tmp();
        std::fs::write(src.join("real.txt"), "content\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("real.txt", src.join("link.txt")).unwrap();
        let dest = tmp();
        copy_tree(&src, &dest, &[]).unwrap();
        #[cfg(unix)]
        {
            assert!(!dest.join("link.txt").metadata().unwrap().is_symlink());
            assert_eq!(
                std::fs::read_to_string(dest.join("link.txt")).unwrap(),
                "content\n"
            );
        }
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dest).ok();
    }

    // -- command rewriting ----------------------------------------------------

    /// A `PluginInstaller` whose repository maps an origin tree onto a
    /// staging copy, plus the `dest` the staging dir will become.
    fn installer_for(staging: &Path, origin: &Path) -> PluginInstaller<'static> {
        // A minimal repository whose only role is path coordinates.
        let repository = Repository {
            source_label: "test".into(),
            root: staging.to_path_buf(),
            origin: Some(origin.to_path_buf()),
            resolved: None,
            tarball_sha256: None,
            plugins: Vec::new(),
        };
        let options = Box::leak(Box::new(PluginInstallOptions::default()));
        let reporter: &'static dyn InstallReporter = &NullReporter;
        PluginInstaller {
            options,
            repository: Box::leak(Box::new(repository)),
            reporter,
        }
    }

    /// `dest` in these tests: the final install directory the staging tree is
    /// renamed into. Distinct from staging so a path confusion is visible.
    fn install_dest(staging: &Path) -> PathBuf {
        staging.parent().unwrap().join("final-install")
    }

    #[test]
    fn rewrite_command_maps_relative_files_into_the_install_dir() {
        let origin = tmp();
        std::fs::write(origin.join("atlas.py"), "#!/usr/bin/env python3\n").unwrap();
        // The staging copy holds the file; the destination is elsewhere.
        let staging = tmp();
        std::fs::write(staging.join("atlas.py"), "#!/usr/bin/env python3\n").unwrap();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let out = installer
            .rewrite_command(
                &["python3".into(), "atlas.py".into()],
                &staging,
                &origin,
                &dest,
            )
            .unwrap();
        assert_eq!(out[0], "python3");
        assert_eq!(
            out[1],
            dest.join("atlas.py").display().to_string(),
            "the argument names its location after the rename, not the staging path"
        );
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_maps_absolute_paths_inside_the_plugin() {
        let origin = tmp();
        let bin = origin.join("target").join("release").join("wordy");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "binary").unwrap();
        let staging = tmp();
        // The build would produce the binary inside staging.
        std::fs::create_dir_all(staging.join("target").join("release")).unwrap();
        std::fs::write(staging.join("target/release/wordy"), "binary").unwrap();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let out = installer
            .rewrite_command(&[bin.display().to_string()], &staging, &origin, &dest)
            .unwrap();
        assert_eq!(
            out[0],
            dest.join("target/release/wordy").display().to_string()
        );
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_refuses_an_absolute_path_the_build_did_not_produce() {
        let origin = tmp();
        let bin = origin.join("target").join("release").join("wordy");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "binary").unwrap();
        let staging = tmp(); // empty: no build output
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let err = installer
            .rewrite_command(&[bin.display().to_string()], &staging, &origin, &dest)
            .unwrap_err();
        assert!(
            err.to_string().contains("missing from the staged copy"),
            "{err}"
        );
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_refuses_absolute_paths_outside_the_plugin() {
        let origin = tmp();
        let staging = tmp();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let err = installer
            .rewrite_command(&["/usr/local/bin/thing".into()], &staging, &origin, &dest)
            .unwrap_err();
        assert!(err.to_string().contains("not relocatable"), "{err}");
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_refuses_tilde_paths() {
        let origin = tmp();
        let staging = tmp();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let err = installer
            .rewrite_command(&["~/bin/wordy".into()], &staging, &origin, &dest)
            .unwrap_err();
        assert!(err.to_string().contains('~'), "{err}");
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_refuses_a_relative_program_path() {
        let origin = tmp();
        let staging = tmp();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let err = installer
            .rewrite_command(&["./run.sh".into()], &staging, &origin, &dest)
            .unwrap_err();
        // `./run.sh` exists neither in staging nor in the origin, so it falls
        // through to the relative-program refusal.
        assert!(err.to_string().contains("command[0]"), "{err}");
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_maps_an_existing_relative_program() {
        let origin = tmp();
        std::fs::write(origin.join("run.sh"), "#!/bin/sh\n").unwrap();
        let staging = tmp();
        std::fs::write(staging.join("run.sh"), "#!/bin/sh\n").unwrap();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let out = installer
            .rewrite_command(&["./run.sh".into()], &staging, &origin, &dest)
            .unwrap();
        assert_eq!(out[0], dest.join("run.sh").display().to_string());
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_refuses_a_file_the_install_excludes() {
        let origin = tmp();
        std::fs::write(origin.join("helper.py"), "# helper\n").unwrap();
        let staging = tmp(); // excludes it: the file is not staged
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let err = installer
            .rewrite_command(
                &["python3".into(), "helper.py".into()],
                &staging,
                &origin,
                &dest,
            )
            .unwrap_err();
        assert!(err.to_string().contains("excludes"), "{err}");
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn rewrite_command_keeps_plain_arguments() {
        let origin = tmp();
        let staging = tmp();
        let dest = install_dest(&staging);
        let installer = installer_for(&staging, &origin);
        let out = installer
            .rewrite_command(
                &[
                    "uvx".into(),
                    "--from".into(),
                    "pkg==1.0".into(),
                    "run".into(),
                ],
                &staging,
                &origin,
                &dest,
            )
            .unwrap();
        assert_eq!(out, vec!["uvx", "--from", "pkg==1.0", "run"]);
        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&staging).ok();
    }
}
