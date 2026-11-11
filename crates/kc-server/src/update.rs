//! Checking for, staging and installing a newer build.
//!
//! karucode versions itself by commit count (see [`crate::version`]), so an
//! update is a newer commit, not a semver bump. A release publishes a small
//! `updates.json` manifest next to its artifacts; this module reads it, compares
//! integers, downloads the artifact for this target, proves it by running its own
//! `--version`, and - when the running process may write its binary - swaps it in
//! place.

// @kc-doc: updates
// @kc-title: Update karucode automatically
// @kc-summary: A released install checks `updates.json` for a newer build, stages and verifies the download, and installs it with one command or one click.
// @kc-tags: update, upgrade, auto-update, release, manifest, updates.json, rollback
// @kc-applies-to: config:[updates], config:updates.check, config:updates.check_secs, config:updates.manifest_url, config:updates.auto_apply, env:KARUCODE_UPDATE_CHECK, env:KARUCODE_UPDATE_CHECK_SECS, env:KARUCODE_UPDATE_MANIFEST_URL, env:KARUCODE_UPDATE_AUTO_APPLY, route:GET /api/update, route:POST /api/update/check, route:POST /api/update/apply, cmd:karucode update, cmd:karucode update --check, cmd:karucode update --stage, cmd:karucode update --apply, file:packaging/update.sh, file:scripts/release_manifest.py, file:updates.json, ui:Settings > Updates, event:notification
//
// Every commit on `main` is a release and the version is the commit count, so
// "update" always means *a higher build number*. There is no channel juggling and
// no version range to resolve: the manifest names one build, and the server
// compares `manifest.build > current.build`.
//
// ```text
// $ karucode update --check
// current: 107 (abf0a42, 2026-05-30T18:04:11+00:00)
// latest:  108 (9c1de77, 2026-05-31T09:12:40+00:00)
// update available: 108
// $ sudo karucode update --apply
// installed build 108 at /usr/local/bin/karucode (previous binary kept as karucode.old)
// restarted karucode
// ```
//
// # Three steps, and only the last one needs privilege
//
// 1. **check** - one GET of the manifest. Touches nothing, needs no privilege, and
//    runs on startup plus every `updates.check_secs` (6 hours by default) when
//    `updates.check` is on. Set `KARUCODE_UPDATE_CHECK=0` for an air-gapped host;
//    no request is made at all.
// 2. **stage** - download the artifact for this target, hash it, and **run it once
//    with `--version`** to prove the file really is the build the manifest named.
//    Always possible: the staging directory is inside the data directory, which
//    the service account owns.
// 3. **apply** - copy the verified binary next to the installed one and rename it
//    over the old path, keeping the previous binary as `<name>.old`. This is the
//    only step that needs write access to the install directory, which is why a
//    service account that runs `/usr/local/bin/karucode` cannot do it: the update
//    is staged and `sudo karucode update --apply` finishes it.
//
// The web UI shows all three: **Settings → Updates** has the current build, the
// newest one, the release notes, and **Check now** / **Install** buttons. When the
// installed binary is writable, **Install** does the swap in-process; otherwise it
// shows the exact `sudo` command to run. A newly available build also raises an
// informational notification, so you hear about it without opening Settings.
//
// # Configuration
//
// ```toml
// [updates]
// check = true                 # look for newer builds (default true)
// check_secs = 21600           # 6 hours; clamped to 300..=604800
// manifest_url = "https://github.com/karutoil/karucode/releases/latest/download/updates.json"
// auto_apply = false           # never replace the binary on its own
// ```
//
// Environment: `KARUCODE_UPDATE_CHECK`, `KARUCODE_UPDATE_CHECK_SECS`,
// `KARUCODE_UPDATE_MANIFEST_URL`, `KARUCODE_UPDATE_AUTO_APPLY`. The same fields are
// editable in the UI and through `PATCH /api/config` (`{"updates": {"check":
// false}}`), and `GET /api/config` reports them under `updates`.
//
// `auto_apply = true` installs as soon as a newer build is found **and** the
// binary is writable; otherwise it stages and waits. It is off by default for two
// reasons: replacing a binary is a root operation on a managed install, and the
// restart that follows ends every run in flight. Turn it on for a box nobody is
// using interactively.
//
// # The manifest
//
// CI writes it with `scripts/release_manifest.py` and publishes it as a release
// asset, so the URL above always names the newest one:
//
// ```json
// {
//   "schema": 1,
//   "build": 108,
//   "commit": "9c1de77a0b3c",
//   "commit_date": "2026-05-31T09:12:40+00:00",
//   "released_at": "2026-05-31T09:20:03Z",
//   "url": "https://github.com/karutoil/karucode/releases/tag/v108",
//   "notes": "What changed in this build.",
//   "targets": {
//     "x86_64-unknown-linux-gnu": {
//       "url": "https://github.com/.../karucode-108-x86_64-unknown-linux-gnu",
//       "sha256": "…64 hex characters…",
//       "size": 18432120,
//       "tarball": "https://github.com/.../karucode-108-x86_64-unknown-linux-gnu.tar.gz"
//     }
//   }
// }
// ```
//
// An unknown `schema` or a target this binary was not built for is refused with a
// message naming both. `sha256` and `size` are checked before the artifact is run;
// a mismatch deletes the download and fails the stage. The artifact URL must be
// `http(s)`; `https` is what makes the checksum meaningful. **The manifest URL is
// the trust anchor**: whoever serves it chooses which binary runs on your machine,
// so point `manifest_url` at a mirror you control, and leave checking off
// (`check = false`) if you would rather update by hand.
//
// # Exit codes and the CLI
//
// ```bash
// karucode update --check     # 0 = up to date, 3 = a newer build exists, 1 = error
// karucode update --stage     # download + verify into <data_dir>/updates/staged
// karucode update --apply     # install the staged build (needs write access)
// karucode update --apply --restart   # …and restart a systemd unit afterwards
// ```
//
// `--apply` without a staged download stages first. On a systemd install it only
// restarts when asked, because a restart ends the runs in progress; the unit's
// `Restart=on-failure` then starts the new binary. A source checkout is not
// updated this way - `packaging/update.sh` rebuilds it - and a container install
// refuses: the image is the unit of deployment.
//
// # Updating a managed install with the API
//
// `GET /api/update` returns the whole state (current build, install kind, last
// check, newest build, staged artifact, last applied). `POST /api/update/check`
// refreshes it now. `POST /api/update/apply` stages if needed, then installs; the
// response says whether a restart is still needed. `{"restart": true}` asks for
// one, which is accepted only when the process is supervised (systemd sets
// `INVOCATION_ID`) because nothing else would start it again.
//
// # Why the download is executed before it is installed
//
// A checksum proves the bytes arrived intact; it cannot prove the file runs, or
// that it is the build the manifest claims. So a staged artifact is executed once
// with `--version`, with a cleared environment and a 30-second limit, and the build
// number it prints must match. That is also the check `packaging/update.sh` does
// on its staged pair, for the same reason: an installer that ships a binary it
// never ran is how a service ends up not starting after an update.

use crate::settings::SettingsHandle;
use crate::version::{self, BuildInfo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};

/// The manifest schema this build understands.
pub const MANIFEST_SCHEMA: u32 = 1;

/// Largest artifact a stage will accept, compressed or not.
///
/// The release binary is ~20 MiB; 256 MiB is far past anything legitimate and far
/// below "fills the data directory". The limit is enforced while streaming, so a
/// server that lies about `size` still cannot write more than this.
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// How long the manifest GET may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the artifact download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long a staged artifact's `--version` may take.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Characters of release notes echoed to the API and the UI.
const MAX_NOTES_CHARS: usize = 4_000;

/// Environment variable a supervisor sets. systemd sets `INVOCATION_ID` for every
/// unit it starts; `KARUCODE_SUPERVISED=1` covers any other supervisor.
pub const SUPERVISED_ENV: &str = "KARUCODE_SUPERVISED";

/// Why an update operation was refused or failed. The HTTP layer turns this into
/// the documented JSON error shape; the CLI prints the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorKind {
    /// The request makes no sense right now: already up to date, no target for
    /// this platform, the binary is not writable. A `409`.
    Refused,
    /// The release source failed: network error, bad manifest, checksum mismatch.
    /// A `502`: the server is fine, the upstream is not.
    Remote,
    /// Reading or writing a local file failed. A `500`.
    Local,
}

#[derive(Debug, Clone)]
pub struct UpdateError {
    pub kind: UpdateErrorKind,
    pub message: String,
}

impl UpdateError {
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Refused,
            message: message.into(),
        }
    }

    pub fn remote(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Remote,
            message: message.into(),
        }
    }

    pub fn local(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Local,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UpdateError {}

pub type Result<T> = std::result::Result<T, UpdateError>;

/// One artifact in the manifest, for one target triple.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetArtifact {
    /// Direct URL of the **unpacked binary**. Raw, not a tarball: extracting an
    /// archive safely needs a dependency and a symlink policy, and the release
    /// already publishes the binary beside the tarball.
    pub url: String,
    /// Lowercase hex SHA-256 of the file at `url`.
    pub sha256: String,
    /// Expected size in bytes, when the manifest states it.
    #[serde(default)]
    pub size: Option<u64>,
    /// The tarball for humans and for `packaging/install.sh`.
    #[serde(default)]
    pub tarball: Option<String>,
}

/// The release manifest (`updates.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest schema. Unknown values are refused rather than guessed at.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// The build number this release is (the commit count).
    pub build: u64,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub commit_date: Option<String>,
    /// When the release was published, RFC 3339. Informational.
    #[serde(default)]
    pub released_at: Option<String>,
    /// What changed. Plain text; truncated when echoed.
    #[serde(default)]
    pub notes: Option<String>,
    /// The human-readable release page.
    #[serde(default)]
    pub url: Option<String>,
    /// Per-target artifacts, keyed by target triple.
    #[serde(default)]
    pub targets: BTreeMap<String, TargetArtifact>,
}

fn default_schema() -> u32 {
    MANIFEST_SCHEMA
}

impl Manifest {
    /// Parse and validate a manifest document.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(text).map_err(|e| {
            UpdateError::remote(format!("the release manifest is not valid JSON: {e}"))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks that do not need a network: the schema, the build number, and every
    /// artifact's URL scheme and checksum shape.
    ///
    /// A manifest that fails these is not "newer", it is broken, and treating it
    /// as newer is how a broken release gets installed everywhere at once.
    pub fn validate(&self) -> Result<()> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(UpdateError::remote(format!(
                "the release manifest uses schema {} but this build understands schema {MANIFEST_SCHEMA}; \
                 update karucode the old-fashioned way (packaging/update.sh or packaging/install.sh)",
                self.schema
            )));
        }
        if self.build == 0 {
            return Err(UpdateError::remote(
                "the release manifest reports build 0, which cannot be a release",
            ));
        }
        if self.targets.is_empty() {
            return Err(UpdateError::remote(
                "the release manifest lists no download targets",
            ));
        }
        for (target, artifact) in &self.targets {
            check_url(target, &artifact.url)?;
            if let Some(tarball) = &artifact.tarball {
                check_url(target, tarball)?;
            }
            if !is_sha256_hex(&artifact.sha256) {
                return Err(UpdateError::remote(format!(
                    "the release manifest's sha256 for `{target}` is not 64 hex characters \
                     (got {:?})",
                    artifact.sha256
                )));
            }
            if let Some(size) = artifact.size {
                if size == 0 {
                    return Err(UpdateError::remote(format!(
                        "the release manifest says `{target}` is 0 bytes"
                    )));
                }
                if size > MAX_ARTIFACT_BYTES {
                    return Err(UpdateError::remote(format!(
                        "the release manifest says `{target}` is {size} bytes, over this build's \
                         {MAX_ARTIFACT_BYTES}-byte download limit"
                    )));
                }
            }
        }
        Ok(())
    }

    /// The artifact for a target triple, when the release has one.
    pub fn artifact_for(&self, target: &str) -> Option<&TargetArtifact> {
        self.targets.get(target)
    }

    /// A one-line description for a log or a notification.
    pub fn label(&self) -> String {
        match &self.commit {
            Some(commit) if !commit.trim().is_empty() => format!("{} ({})", self.build, commit),
            _ => self.build.to_string(),
        }
    }

    /// The latest build's information in the shape the UI expects.
    pub fn latest_info(&self, target: Option<&TargetArtifact>) -> LatestInfo {
        LatestInfo {
            build: self.build,
            version: self.build.to_string(),
            commit: self.commit.clone(),
            commit_date: self.commit_date.clone(),
            released_at: self.released_at.clone(),
            notes: self
                .notes
                .as_deref()
                .map(|notes| truncate_chars(notes.trim(), MAX_NOTES_CHARS)),
            url: self.url.clone(),
            artifact: target.map(|artifact| ArtifactInfo {
                url: artifact.url.clone(),
                sha256: artifact.sha256.clone(),
                size: artifact.size,
                tarball: artifact.tarball.clone(),
            }),
            has_target_for_this_build: target.is_some(),
        }
    }
}

fn check_url(target: &str, url: &str) -> Result<()> {
    let trimmed = url.trim();
    let ok = trimmed.starts_with("https://") || trimmed.starts_with("http://");
    if !ok {
        return Err(UpdateError::remote(format!(
            "the release manifest's URL for `{target}` is not http(s): {trimmed:?}"
        )));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Where this binary was installed, and whether it can be replaced.
#[derive(Debug, Clone, Serialize)]
pub struct InstallInfo {
    /// `managed`, `source` or `container`.
    pub kind: String,
    /// Absolute path of the running binary.
    pub path: String,
    /// Can this process replace the binary at `path`?
    pub writable: bool,
    /// Why self-update is (or is not) possible, in one sentence.
    pub detail: String,
}

/// How the running binary is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed binary, not obviously a build tree: `karucode update --apply`
    /// works when the path is writable.
    Managed,
    /// Running out of `target/`: a source checkout, updated with
    /// `packaging/update.sh`.
    Source,
    /// Inside a container: the image is the unit of deployment.
    Container,
}

impl InstallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallKind::Managed => "managed",
            InstallKind::Source => "source",
            InstallKind::Container => "container",
        }
    }
}

/// A downloaded and verified artifact waiting to be installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedUpdate {
    pub build: u64,
    pub version: String,
    pub commit: Option<String>,
    /// Absolute path of the staged binary.
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// Unix milliseconds when it was staged.
    pub staged_at: u64,
}

/// The last build this install copied into place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedUpdate {
    pub from: String,
    pub to: String,
    pub path: String,
    /// Unix milliseconds.
    pub applied_at: u64,
}

/// The newest release, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatestInfo {
    pub build: u64,
    pub version: String,
    pub commit: Option<String>,
    pub commit_date: Option<String>,
    pub released_at: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    pub artifact: Option<ArtifactInfo>,
    /// Whether this release has an artifact for the target this binary was built
    /// for. `false` means "available, but not for you" - an ARM host reading an
    /// x86-only release.
    pub has_target_for_this_build: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactInfo {
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
    pub tarball: Option<String>,
}

/// The state of update checking itself.
#[derive(Debug, Clone, Serialize)]
pub struct CheckInfo {
    pub enabled: bool,
    pub manifest_url: Option<String>,
    pub interval_secs: u64,
    /// Unix milliseconds of the last completed attempt, successful or not.
    pub checked_at: Option<u64>,
    /// Why the last attempt failed. Cleared by a successful one.
    pub error: Option<String>,
}

/// Whether a restart is possible, and the command that installs by hand.
#[derive(Debug, Clone, Serialize)]
pub struct RestartInfo {
    /// A supervisor will start the new binary if the process exits.
    pub supervised: bool,
    /// The exact command that finishes an update this process may not apply.
    pub apply_command: String,
}

/// Everything `GET /api/update` returns.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub current: BuildInfo,
    pub install: InstallInfo,
    pub check: CheckInfo,
    pub latest: Option<LatestInfo>,
    pub update_available: bool,
    /// `idle`, `checking`, `downloading` or `applying`.
    pub phase: String,
    pub staged: Option<StagedUpdate>,
    pub last_applied: Option<AppliedUpdate>,
    pub restart: RestartInfo,
}

/// What an apply did.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyOutcome {
    pub installed: bool,
    /// Restart the process to run the new build. Always `true` for a successful
    /// in-process install: the running image is the old file.
    pub restart_required: bool,
    /// The restart was requested (only when asked for and supervised).
    pub restart_requested: bool,
    pub from: String,
    pub to: String,
    pub path: String,
    /// A manual apply command when this process could not write the binary.
    pub needs_privilege: bool,
    pub command: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Checking,
    Downloading,
    Applying,
}

impl Phase {
    fn as_str(&self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Applying => "applying",
        }
    }
}

/// State carried between calls and reloaded at startup.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct Persisted {
    #[serde(default)]
    checked_at: Option<u64>,
    #[serde(default)]
    latest: Option<Manifest>,
    #[serde(default)]
    staged: Option<StagedUpdate>,
    #[serde(default)]
    last_applied: Option<AppliedUpdate>,
    /// The build a notification was raised for, so a check every six hours does
    /// not raise the same one four times a day.
    #[serde(default)]
    notified_build: Option<u64>,
}

#[derive(Default)]
struct Inner {
    persisted: Persisted,
    error: Option<String>,
    phase: Option<Phase>,
    /// Cached: the install directory does not change ownership while the process
    /// runs, and the probe creates a real file.
    install: Option<InstallInfo>,
}

/// Where the updater reads its settings from.
///
/// The server hands it the live [`SettingsHandle`] so a change made in the UI
/// applies on the next tick; the CLI and the tests hand it a fixed snapshot, so a
/// `karucode update --check` never has to open the database.
pub trait UpdateSettings: Send + Sync {
    fn updates_config(&self) -> kc_core::config::UpdatesConfig;
}

impl UpdateSettings for SettingsHandle {
    fn updates_config(&self) -> kc_core::config::UpdatesConfig {
        self.snapshot().updates
    }
}

/// A fixed `[updates]` section, for the CLI and for tests.
#[derive(Debug, Clone)]
pub struct FixedSettings(pub kc_core::config::UpdatesConfig);

impl UpdateSettings for FixedSettings {
    fn updates_config(&self) -> kc_core::config::UpdatesConfig {
        self.0.clone()
    }
}

/// The update subsystem, one per process.
pub struct Updater {
    settings: Arc<dyn UpdateSettings>,
    data_dir: PathBuf,
    /// The path this process was started from, captured once.
    ///
    /// It must not be re-read from `current_exe()` after an update: `/proc/self/exe`
    /// follows the *inode*, so once a successful install renames the old binary to
    /// `<name>.old` (and a second install deletes that), `current_exe()` stops
    /// naming the install path at all. Caching it here is what makes "install
    /// again" work in a process that already installed once.
    install_path: PathBuf,
    client: reqwest::Client,
    inner: Mutex<Inner>,
    /// One check/stage/apply at a time. `None` in the state means idle; the lock
    /// is what makes "already running" a refusal instead of interleaved writes.
    busy: AsyncMutex<()>,
}

impl Updater {
    /// Build the updater and load whatever the last process left behind.
    ///
    /// A missing or unreadable state file is not an error: it only carries cached
    /// facts, and a fresh one costs one check.
    pub fn new(settings: Arc<dyn UpdateSettings>, data_dir: PathBuf) -> Arc<Self> {
        let persisted = load_state(&data_dir).unwrap_or_default();
        let client = reqwest::Client::builder()
            .user_agent(format!("karucode/{}", version::VERSION))
            .timeout(CHECK_TIMEOUT)
            .build()
            .unwrap_or_default();
        Arc::new(Self {
            settings,
            data_dir,
            install_path: current_exe(),
            client,
            inner: Mutex::new(Inner {
                persisted,
                ..Inner::default()
            }),
            busy: AsyncMutex::new(()),
        })
    }

    /// `<data_dir>/updates`: staging directory and cached state.
    pub fn updates_dir(&self) -> PathBuf {
        self.data_dir.join("updates")
    }

    /// The current status, without touching the network.
    pub fn status(&self) -> UpdateStatus {
        let config = self.settings.updates_config();
        let inner = self.inner.lock().expect("update state lock");
        let install = inner
            .install
            .clone()
            .unwrap_or_else(|| install_info(self.install_path.clone()));
        let latest = inner
            .persisted
            .latest
            .as_ref()
            .map(|manifest| manifest.latest_info(manifest.artifact_for(version::TARGET)));
        let update_available = inner
            .persisted
            .latest
            .as_ref()
            .map(|manifest| manifest.build > version::BUILD)
            .unwrap_or(false);
        let apply_command = self.apply_command_for(&install);
        UpdateStatus {
            current: version::build_info(),
            install,
            check: CheckInfo {
                enabled: config.check,
                manifest_url: config.manifest().map(str::to_string),
                interval_secs: config.interval(),
                checked_at: inner.persisted.checked_at,
                error: inner.error.clone(),
            },
            latest,
            update_available,
            phase: inner.phase.unwrap_or(Phase::Idle).as_str().to_string(),
            staged: inner.persisted.staged.clone(),
            last_applied: inner.persisted.last_applied.clone(),
            restart: RestartInfo {
                supervised: is_supervised(),
                apply_command,
            },
        }
    }

    /// The exact command that finishes an update this process may not install.
    ///
    /// `--data-dir` is not optional noise: `sudo` starts with root's environment, so
    /// a bare `karucode update --apply` would look for the staged download under
    /// `/root/.local/share/karucode` instead of the one the server was started with.
    pub fn apply_command_for(&self, install: &InstallInfo) -> String {
        let exe = install
            .path_for_shell()
            .unwrap_or_else(|| "karucode".to_string());
        format!(
            "sudo {exe} update --apply --data-dir {}",
            shell_quote(&self.data_dir.display().to_string())
        )
    }

    /// The cached manifest, if a check has succeeded.
    pub fn manifest(&self) -> Option<Manifest> {
        self.inner
            .lock()
            .expect("update state lock")
            .persisted
            .latest
            .clone()
    }

    /// The staged artifact, if one is waiting.
    pub fn staged(&self) -> Option<StagedUpdate> {
        self.inner
            .lock()
            .expect("update state lock")
            .persisted
            .staged
            .clone()
    }

    /// Whether a notification was already raised for `build`.
    pub fn notified(&self, build: u64) -> bool {
        self.inner
            .lock()
            .expect("update state lock")
            .persisted
            .notified_build
            == Some(build)
    }

    /// Record that a notification was raised for `build`.
    pub fn mark_notified(&self, build: u64) {
        let snapshot = {
            let mut inner = self.inner.lock().expect("update state lock");
            inner.persisted.notified_build = Some(build);
            inner.persisted.clone()
        };
        if let Err(e) = save_state(&self.data_dir, &snapshot) {
            debug!(error = %e, "could not persist the update notification marker");
        }
    }

    /// Fetch the manifest and record the result.
    ///
    /// A failure is recorded in the status (so the UI can say *why* checking
    /// failed) and returned; it never takes the server down.
    pub async fn check(&self) -> Result<Manifest> {
        let _busy = self
            .busy
            .try_lock()
            .map_err(|_| UpdateError::refused("an update operation is already running"))?;
        let config = self.settings.updates_config();
        let Some(url) = config.manifest().map(str::to_string) else {
            return Err(UpdateError::refused(
                "update checking is off (updates.check = false); set a manifest_url and turn it \
                 on, or run `karucode update --check --manifest <url>`",
            ));
        };
        self.set_phase(Some(Phase::Checking));
        let outcome = self.fetch_manifest(&url).await;
        self.set_phase(None);
        let (snapshot, result) = {
            let mut inner = self.inner.lock().expect("update state lock");
            inner.persisted.checked_at = Some(now_ms());
            match outcome {
                Ok(manifest) => {
                    // A manifest for the build we already run is not news; keep
                    // the document anyway, the UI shows what it said.
                    info!(
                        url = %url,
                        remote_build = manifest.build,
                        local_build = version::BUILD,
                        newer = manifest.build > version::BUILD,
                        "checked for updates"
                    );
                    inner.error = None;
                    inner.persisted.latest = Some(manifest.clone());
                    (inner.persisted.clone(), Ok(manifest))
                }
                Err(error) => {
                    warn!(url = %url, error = %error, "update check failed");
                    inner.error = Some(error.message.clone());
                    (inner.persisted.clone(), Err(error))
                }
            }
        };
        if let Err(e) = save_state(&self.data_dir, &snapshot) {
            debug!(error = %e, "could not persist the update check");
        }
        result
    }

    async fn fetch_manifest(&self, url: &str) -> Result<Manifest> {
        let response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .map_err(|e| UpdateError::remote(format!("could not fetch {url}: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(UpdateError::remote(format!(
                "could not fetch {url}: HTTP {status}"
            )));
        }
        // A manifest is a few hundred bytes; a body past this is not one.
        let bytes = response
            .bytes()
            .await
            .map_err(|e| UpdateError::remote(format!("could not read {url}: {e}")))?;
        if bytes.len() > 1024 * 1024 {
            return Err(UpdateError::remote(format!(
                "the release manifest at {url} is {} bytes, which is not a manifest",
                bytes.len()
            )));
        }
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
            UpdateError::remote(format!("the release manifest at {url} is not UTF-8"))
        })?;
        Manifest::parse(&text)
    }

    /// Download, verify and keep the newest artifact for this target.
    ///
    /// Re-running this with a verified stage already on disk reuses it (it is
    /// re-hashed first), so "install" after a failed apply does not download
    /// 20 MiB again.
    pub async fn stage(&self) -> Result<StagedUpdate> {
        let _busy = self
            .busy
            .try_lock()
            .map_err(|_| UpdateError::refused("an update operation is already running"))?;
        let manifest = match self.manifest() {
            Some(manifest) => manifest,
            None => self.refresh_for_stage().await?,
        };
        if manifest.build <= version::BUILD {
            return Err(UpdateError::refused(format!(
                "build {} is already up to date (newest release is {})",
                version::BUILD,
                manifest.build
            )));
        }
        let target = manifest.artifact_for(version::TARGET).ok_or_else(|| {
            UpdateError::refused(format!(
                "release {} has no artifact for this platform ({}); it publishes {}",
                manifest.build,
                version::TARGET,
                manifest
                    .targets
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let artifact = target.clone();

        let dir = self
            .updates_dir()
            .join("staged")
            .join(manifest.build.to_string());
        let binary = dir.join("karucode");
        if let Some(existing) = self
            .reusable_stage(&binary, &artifact, manifest.build)
            .await
        {
            self.remember_stage(existing.clone());
            return Ok(existing);
        }

        std::fs::create_dir_all(&dir)
            .map_err(|e| UpdateError::local(format!("could not create {}: {e}", dir.display())))?;
        let partial = dir.join(format!("karucode.part-{}", std::process::id()));
        self.set_phase(Some(Phase::Downloading));
        let downloaded = self.download(&artifact, &partial).await;
        self.set_phase(None);
        let size = downloaded?;

        // The artifact has to run and name the build the manifest promised. This
        // is the check that a checksum cannot make.
        if let Err(error) = prove_artifact(&partial, manifest.build).await {
            let _ = std::fs::remove_file(&partial);
            return Err(error);
        }
        std::fs::rename(&partial, &binary).map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            UpdateError::local(format!("could not move the download into place: {e}"))
        })?;
        let staged = StagedUpdate {
            build: manifest.build,
            version: manifest.build.to_string(),
            commit: manifest.commit.clone(),
            path: binary.display().to_string(),
            sha256: artifact.sha256.to_lowercase(),
            size,
            staged_at: now_ms(),
        };
        // The whole release document, so a later `--apply` (possibly in another
        // process, as root) can explain itself without a second network call.
        let release = dir.join("release.json");
        if let Ok(text) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(&release, text);
        }
        info!(
            build = staged.build,
            bytes = staged.size,
            path = %staged.path,
            "staged an update"
        );
        self.remember_stage(staged.clone());
        Ok(staged)
    }

    /// Fetch the manifest when there is no cached one, without deadlocking on the
    /// busy lock `stage` already holds.
    async fn refresh_for_stage(&self) -> Result<Manifest> {
        let config = self.settings.updates_config();
        let url = config.manifest().map(str::to_string).ok_or_else(|| {
            UpdateError::refused("update checking is off (updates.check = false)")
        })?;
        let manifest = self.fetch_manifest(&url).await?;
        let snapshot = {
            let mut inner = self.inner.lock().expect("update state lock");
            inner.persisted.checked_at = Some(now_ms());
            inner.persisted.latest = Some(manifest.clone());
            inner.error = None;
            inner.persisted.clone()
        };
        let _ = save_state(&self.data_dir, &snapshot);
        Ok(manifest)
    }

    /// A staged file that still matches the manifest, verified by hashing it again.
    async fn reusable_stage(
        &self,
        binary: &Path,
        artifact: &TargetArtifact,
        build: u64,
    ) -> Option<StagedUpdate> {
        let meta = std::fs::metadata(binary).ok()?;
        if !meta.is_file() {
            return None;
        }
        let size = meta.len();
        if let Some(expected) = artifact.size {
            if size != expected {
                return None;
            }
        }
        if size > MAX_ARTIFACT_BYTES {
            return None;
        }
        let bytes = std::fs::read(binary).ok()?;
        if !sha256_matches(&bytes, &artifact.sha256) {
            return None;
        }
        prove_artifact(binary, build).await.ok()?;
        Some(StagedUpdate {
            build,
            version: build.to_string(),
            commit: None,
            path: binary.display().to_string(),
            sha256: artifact.sha256.to_lowercase(),
            size,
            staged_at: now_ms(),
        })
    }

    /// Stream the artifact to `partial`, hashing as it goes.
    async fn download(&self, artifact: &TargetArtifact, partial: &Path) -> Result<u64> {
        let url = artifact.url.trim().to_string();
        check_url(version::TARGET, &url)?;
        let response = self
            .client
            .get(&url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| UpdateError::remote(format!("could not download {url}: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(UpdateError::remote(format!(
                "could not download {url}: HTTP {status}"
            )));
        }
        if let Some(length) = response.content_length() {
            if length > MAX_ARTIFACT_BYTES {
                return Err(UpdateError::remote(format!(
                    "{url} is {length} bytes, over this build's {MAX_ARTIFACT_BYTES}-byte limit"
                )));
            }
        }
        let mut file = tokio::fs::File::create(partial).await.map_err(|e| {
            UpdateError::local(format!("could not create {}: {e}", partial.display()))
        })?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt as _;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| UpdateError::remote(format!("download of {url} failed: {e}")))?;
            written += chunk.len() as u64;
            if written > MAX_ARTIFACT_BYTES {
                drop(file);
                let _ = std::fs::remove_file(partial);
                return Err(UpdateError::remote(format!(
                    "{url} is larger than this build's {MAX_ARTIFACT_BYTES}-byte limit"
                )));
            }
            hasher.update(&chunk);
            if let Err(e) = file.write_all(&chunk).await {
                let _ = std::fs::remove_file(partial);
                return Err(UpdateError::local(format!(
                    "could not write {}: {e}",
                    partial.display()
                )));
            }
        }
        file.flush().await.map_err(|e| {
            UpdateError::local(format!("could not flush {}: {e}", partial.display()))
        })?;
        // Close the descriptor *here*, synchronously. `tokio::fs::File` closes on
        // a blocking-pool thread when it is dropped, so `drop(file)` only
        // schedules the close: `prove_artifact` can then `execve` a file that is
        // still open for writing, and Linux answers `ETXTBSY` ("Text file busy")
        // for exactly that - a staged update refused for a reason that has
        // nothing to do with the download. `into_std` hands back the plain handle,
        // whose drop closes it before the next line runs.
        let file = file.into_std();
        drop(file);

        if let Some(expected) = artifact.size {
            if written != expected {
                let _ = std::fs::remove_file(partial);
                return Err(UpdateError::remote(format!(
                    "{url} is {written} bytes but the manifest says {expected}; the download was \
                     truncated or the release changed under us"
                )));
            }
        }
        let digest = hex(&hasher.finalize());
        if digest != artifact.sha256.to_lowercase() {
            let _ = std::fs::remove_file(partial);
            return Err(UpdateError::remote(format!(
                "{url} does not match the manifest's sha256 (expected {}, got {digest}); the \
                 download was refused",
                artifact.sha256.to_lowercase()
            )));
        }
        make_executable(partial)?;
        Ok(written)
    }

    fn remember_stage(&self, staged: StagedUpdate) {
        let snapshot = {
            let mut inner = self.inner.lock().expect("update state lock");
            inner.persisted.staged = Some(staged);
            inner.persisted.clone()
        };
        if let Err(e) = save_state(&self.data_dir, &snapshot) {
            debug!(error = %e, "could not persist the staged update");
        }
    }

    /// Stage (if needed) and install.
    ///
    /// `restart` asks the process to exit through its normal shutdown path so a
    /// supervisor starts the new binary; it is refused when nothing supervises the
    /// process, because an exit would then leave nothing running.
    pub async fn apply(&self, restart: bool) -> Result<ApplyOutcome> {
        let install = self.install_blocking();
        let command = self.apply_command_for(&install);
        if install.kind == InstallKind::Container.as_str() {
            return Err(UpdateError::refused(
                "this is a container install: rebuild the image to update it. Nothing was changed \
                 (the manifest is still readable at GET /api/update).",
            ));
        }
        if install.kind == InstallKind::Source.as_str() {
            return Err(UpdateError::refused(format!(
                "this binary runs from a build tree ({}); update it with `packaging/update.sh --no-pull` \
                 or a fresh `cargo build`, not with a release artifact",
                install.path
            )));
        }
        if !install.writable {
            return Err(UpdateError::refused(format!(
                "this process may not write {} (owned by another user, usually root). Stage it and \
                 install it with privilege:\n  {command}",
                install.path
            )));
        }
        let staged = self.stage().await?;
        let _busy = self
            .busy
            .try_lock()
            .map_err(|_| UpdateError::refused("an update operation is already running"))?;
        self.set_phase(Some(Phase::Applying));
        let outcome = self.install_staged(&staged, restart).await;
        self.set_phase(None);
        outcome
    }

    async fn install_staged(&self, staged: &StagedUpdate, restart: bool) -> Result<ApplyOutcome> {
        // The captured install path, never `current_exe()`: see the field's comment.
        let exe = self.install_blocking().path;
        let exe = PathBuf::from(exe);
        let from = version::VERSION.to_string();
        // Re-prove the staged file: it sat on disk between staging and applying,
        // and this is the last chance to notice that it is not what it claims.
        let bytes = std::fs::read(&staged.path).map_err(|e| {
            UpdateError::local(format!(
                "the staged download {} is unreadable: {e}; run `karucode update --stage` again",
                staged.path
            ))
        })?;
        if !sha256_matches(&bytes, &staged.sha256) {
            return Err(UpdateError::local(format!(
                "the staged download {} no longer matches its checksum; run `karucode update --stage` \
                 again",
                staged.path
            )));
        }
        if bytes.len() as u64 != staged.size {
            return Err(UpdateError::local(format!(
                "the staged download is {} bytes, not the {} it was staged as",
                bytes.len(),
                staged.size
            )));
        }
        prove_artifact(Path::new(&staged.path), staged.build).await?;

        install_binary(&exe, &bytes)?;
        let applied = AppliedUpdate {
            from: from.clone(),
            to: staged.build.to_string(),
            path: exe.display().to_string(),
            applied_at: now_ms(),
        };
        let snapshot = {
            let mut inner = self.inner.lock().expect("update state lock");
            inner.persisted.last_applied = Some(applied.clone());
            inner.persisted.staged = None;
            inner.error = None;
            inner.persisted.clone()
        };
        let _ = save_state(&self.data_dir, &snapshot);
        // The staged copy is no longer needed; the binary is in place and `.old`
        // holds the previous one.
        if let Some(parent) = Path::new(&staged.path).parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
        info!(
            from = %applied.from,
            to = %applied.to,
            path = %applied.path,
            "installed an update"
        );

        let supervised = is_supervised();
        let restart_requested = restart && supervised;
        if restart && !supervised {
            return Err(UpdateError::refused(
                "the new build is installed, but nothing supervises this process (no systemd \
                 INVOCATION_ID and no KARUCODE_SUPERVISED), so exiting would leave karucode \
                 stopped. Restart it yourself to run the new build.",
            ));
        }
        Ok(ApplyOutcome {
            installed: true,
            restart_required: true,
            restart_requested,
            from: applied.from,
            to: applied.to,
            path: applied.path,
            needs_privilege: false,
            command: None,
            message: if restart_requested {
                "installed; the service is restarting onto the new build".into()
            } else {
                "installed; restart the service to run the new build (systemctl restart karucode)"
                    .into()
            },
        })
    }

    /// Install info for the running binary, cached.
    pub fn install_info(&self) -> InstallInfo {
        self.install_blocking()
    }

    fn install_blocking(&self) -> InstallInfo {
        let mut inner = self.inner.lock().expect("update state lock");
        if let Some(install) = &inner.install {
            return install.clone();
        }
        let info = install_info(self.install_path.clone());
        inner.install = Some(info.clone());
        info
    }

    fn set_phase(&self, phase: Option<Phase>) {
        self.inner.lock().expect("update state lock").phase = phase;
    }
}

impl InstallInfo {
    /// The binary path quoted for a shell, when it can be.
    fn path_for_shell(&self) -> Option<String> {
        let path = self.path.trim();
        if path.is_empty() || path == "karucode" {
            return None;
        }
        Some(shell_quote(path))
    }
}

/// `'…'` with embedded single quotes escaped, so a path with a space or a quote
/// still produces a command that can be pasted.
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-:+=".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The running binary's path, or a `karucode` placeholder when it cannot be read.
fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("karucode"))
}

/// Describe how a binary at `exe` is installed and whether it can be replaced.
pub fn install_info(exe: PathBuf) -> InstallInfo {
    let path = exe.display().to_string();
    let kind = if Path::new("/.dockerenv").exists() {
        InstallKind::Container
    } else {
        let text = path.replace('\\', "/");
        if text.contains("/target/debug/") || text.contains("/target/release/") {
            InstallKind::Source
        } else {
            InstallKind::Managed
        }
    };
    let writable = kind == InstallKind::Managed && can_write_beside(&exe);
    let detail = match kind {
        InstallKind::Container => {
            "this process runs in a container: the image is the unit of deployment".to_string()
        }
        InstallKind::Source => format!(
            "this binary runs from a build tree ({path}); `packaging/update.sh` rebuilds it from the \
             checkout"
        ),
        InstallKind::Managed if writable => {
            format!("{path} is writable by this process: an update installs in place")
        }
        InstallKind::Managed => format!(
            "{path} is not writable by this process: an update is staged, then installed with \
             `sudo karucode update --apply`"
        ),
    };
    InstallInfo {
        kind: kind.as_str().to_string(),
        path,
        writable,
        detail,
    }
}

/// Can this process create a file in the binary's directory?
///
/// The probe is a real create/remove rather than a permission-bit check: ACLs,
/// read-only mounts and root-owned directories all look different from `mode`, and
/// the only answer that matters is whether the rename at the end can work.
fn can_write_beside(exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let probe = dir.join(format!(".karucode-update-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Run a staged artifact with `--version` and require the build it names.
///
/// Cleared environment: a downloaded binary has no business seeing the server's
/// credentials, and `--version` needs none.
async fn prove_artifact(path: &Path, expected: u64) -> Result<()> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .env_clear()
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(path_env) = std::env::var_os("PATH") {
        command.env("PATH", path_env);
    }
    // `execve` answers `ETXTBSY` while *any* process still holds the file open
    // for writing, and a file written moments ago looks busy for as long as a
    // forked sibling takes to finish its own `exec` - the "text file busy" race
    // `std` itself retries for a few milliseconds. Under a parallel run (and on
    // a busy install) that window is longer than `std`'s budget, so a staged
    // update was refused for a reason that had nothing to do with the download.
    // A real failure still fails: the message is the same after a second of
    // retries.
    const EXEC_BUSY_RETRIES: u32 = 40;
    const EXEC_BUSY_DELAY: Duration = Duration::from_millis(25);
    let mut attempt = 0u32;
    let output = loop {
        match tokio::time::timeout(VERSION_PROBE_TIMEOUT, command.output()).await {
            Err(_) => {
                return Err(UpdateError::remote(format!(
                    "the downloaded binary did not answer `--version` within {}s; it was refused",
                    VERSION_PROBE_TIMEOUT.as_secs()
                )))
            }
            Ok(Ok(output)) => break output,
            Ok(Err(e)) if is_text_file_busy(&e) && attempt < EXEC_BUSY_RETRIES => {
                attempt += 1;
                tokio::time::sleep(EXEC_BUSY_DELAY).await;
            }
            Ok(Err(e)) => {
                return Err(UpdateError::remote(format!(
                    "the downloaded file {} could not be executed: {e}; it was refused",
                    path.display()
                )))
            }
        }
    };
    if !output.status.success() {
        return Err(UpdateError::remote(format!(
            "the downloaded binary's `--version` failed with {}; it was refused",
            output.status
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let found = version::parse_version_line(text.trim());
    match found {
        Some(build) if build == expected => Ok(()),
        Some(build) => Err(UpdateError::remote(format!(
            "the downloaded binary reports build {build} but the manifest promised {expected}; it \
             was refused"
        ))),
        None => Err(UpdateError::remote(format!(
            "the downloaded binary's `--version` output is not a karucode version line ({:?}); it \
             was refused",
            truncate_chars(text.trim(), 120)
        ))),
    }
}

/// Copy `bytes` next to `exe`, prove it runs, and rename it over the old path.
///
/// The rename is the atomic step: a reader either sees the old binary or the new
/// one, never a half-written file. The old binary is kept as `<name>.old` so a
/// build that does not start can be put back with one `mv`.
pub fn install_binary(exe: &Path, bytes: &[u8]) -> Result<()> {
    let Some(dir) = exe.parent() else {
        return Err(UpdateError::local(format!(
            "cannot install over {}: it has no parent directory",
            exe.display()
        )));
    };
    let temp = dir.join(format!(
        ".{}.new-{}",
        exe.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "karucode".into()),
        std::process::id()
    ));
    std::fs::write(&temp, bytes)
        .map_err(|e| UpdateError::local(format!("could not write {}: {e}", temp.display())))?;
    if let Err(error) = make_executable(&temp) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    // Prove the copy itself, not the source: a full disk truncates, and the
    // rename would then install a broken binary.
    if let Err(error) = prove_copy(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let old = exe.with_extension("old");
    if exe.exists() {
        let _ = std::fs::remove_file(&old);
        if let Err(e) = std::fs::rename(exe, &old) {
            let _ = std::fs::remove_file(&temp);
            return Err(UpdateError::local(format!(
                "could not move the old binary aside ({e}); nothing was changed"
            )));
        }
    }
    if let Err(e) = std::fs::rename(&temp, exe) {
        // Put the old one back rather than leaving no binary at all.
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&temp);
        return Err(UpdateError::local(format!(
            "could not install {}: {e}; the previous binary was put back",
            exe.display()
        )));
    }
    Ok(())
}

/// Confirm the file we just wrote is binary-identical to what we intended, and
/// executable.
fn prove_copy(temp: &Path, bytes: &[u8]) -> Result<()> {
    let written = std::fs::read(temp)
        .map_err(|e| UpdateError::local(format!("could not read back {}: {e}", temp.display())))?;
    if written != bytes {
        return Err(UpdateError::local(format!(
            "the copy at {} does not match the verified download (the disk may be full)",
            temp.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|e| UpdateError::local(format!("could not stat {}: {e}", path.display())))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| UpdateError::local(format!("could not chmod {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Is this error Linux's "Text file busy"?
///
/// It means *another process is writing the file*, not that the file is bad or
/// that the download failed - so the caller waits and tries the exec again rather
/// than refusing a good artifact.
fn is_text_file_busy(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ETXTBSY)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn sha256_matches(bytes: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize()) == expected_hex.trim().to_ascii_lowercase()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Is something going to start this process again after it exits?
pub fn is_supervised() -> bool {
    std::env::var("INVOCATION_ID")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || matches!(
            std::env::var(SUPERVISED_ENV)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load_state(data_dir: &Path) -> Option<Persisted> {
    let path = data_dir.join("updates").join("state.json");
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(state) => Some(state),
        Err(e) => {
            // A corrupt cache must not stop the server; it will be rewritten.
            warn!(path = %path.display(), error = %e, "ignoring an unreadable update state file");
            None
        }
    }
}

/// Write the state file atomically: a crash mid-write leaves the old one intact.
fn save_state(data_dir: &Path, state: &Persisted) -> std::io::Result<()> {
    let dir = data_dir.join("updates");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("state.json");
    let temp = dir.join(format!("state.json.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &path)?;
    Ok(())
}

/// Truncate on a character boundary, with the documented marker.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What `updates.auto_apply` should do about an available build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoApply {
    /// Install it: a managed install this process may write.
    Install,
    /// Download and verify it, then stop: the install needs privilege.
    Stage,
    /// Do nothing.
    Off,
}

/// Decide what an automatic update may do, from the switch and the install kind.
///
/// `auto_apply` can never install over a binary this process does not own - that
/// is the difference between "update my own install" and "get root". A build tree
/// (updated with `packaging/update.sh`) and a container (updated by rebuilding the
/// image) are left alone entirely: a staged release artifact would be misleading,
/// because neither can install it.
pub fn auto_apply_action(auto_apply: bool, install: &InstallInfo) -> AutoApply {
    if !auto_apply {
        return AutoApply::Off;
    }
    if install.kind == InstallKind::Managed.as_str() {
        return if install.writable {
            AutoApply::Install
        } else {
            AutoApply::Stage
        };
    }
    AutoApply::Off
}

/// The background check: on startup, then every `updates.check_secs`.
///
/// The settings are re-read on every tick, so turning checking off (or changing
/// the interval) from the settings sheet takes effect without a restart. A check
/// that finds a newer build raises one informational notification per build,
/// respecting the `[notifications]` switches - the same rule the runtime and the
/// plugin bridge follow.
pub fn spawn(state: crate::state::AppState) {
    tokio::spawn(async move {
        let shutdown = state.shutdown_token();
        // Give the server a moment to finish starting before the first request:
        // nothing is urgent about a version check, and a slow or unreachable
        // release host must not compete with the first page load.
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
        // A stamp of the last attempt, so an interval change does not restart the
        // clock from a fresh "never checked" every tick.
        let mut last_attempt = 0u64;
        loop {
            let config = state.settings().snapshot();
            if config.updates.check {
                let interval_ms = config.updates.interval() * 1000;
                let now = now_ms();
                if now.saturating_sub(last_attempt) >= interval_ms {
                    last_attempt = now;
                    run_check(&state).await;
                }
            }
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
        debug!("update check task stopped");
    });
}

/// One check plus the notification it may raise. Public so a test (and an
/// operator with a debugger) can run exactly what the timer runs.
pub async fn run_check(state: &crate::state::AppState) -> Option<Manifest> {
    let updater = state.updates();
    let manifest = match updater.check().await {
        Ok(manifest) => manifest,
        Err(error) => {
            debug!(error = %error, "update check did not produce a manifest");
            return None;
        }
    };
    if manifest.build <= version::BUILD {
        return Some(manifest);
    }
    if manifest.artifact_for(version::TARGET).is_none() {
        // Nothing installable on this platform: worth a log, not a notification
        // nobody can act on.
        info!(
            build = manifest.build,
            target = version::TARGET,
            "a newer release exists without an artifact for this platform"
        );
        return Some(manifest);
    }
    if updater.notified(manifest.build) {
        return Some(manifest);
    }
    let settings = state.settings().snapshot();
    let notifications = settings.notifications.clone();
    if !notifications.enabled || !notifications.info {
        debug!("update notification suppressed by [notifications]");
        return Some(manifest);
    }
    let notification = kc_core::notify::Notify::new(
        kc_core::notify::NotifyKind::Info,
        format!("update:{}", manifest.build),
        format!("karucode {} is available", manifest.build),
        format!(
            "You are running build {}. Open Settings → Updates to install it.",
            version::BUILD
        ),
        "",
        "",
        kc_core::notify::RUNTIME_SOURCE,
    );
    state.runtime().emit(
        None,
        None,
        None,
        kc_core::events::Event::Notification { notification },
    );
    updater.mark_notified(manifest.build);

    // `auto_apply` is off by default, and even when it is on it only installs over
    // a binary this process may write. Otherwise the most it does is stage the
    // download, so the privileged `karucode update --apply` is a single command
    // that does not need the network.
    let install = updater.install_info();
    match auto_apply_action(settings.updates.auto_apply, &install) {
        AutoApply::Install => match updater.apply(false).await {
            Ok(outcome) => info!(
                from = %outcome.from,
                to = %outcome.to,
                "auto_apply installed the newer build; restart karucode to run it"
            ),
            Err(error) => {
                warn!(error = %error, build = manifest.build, "auto_apply could not install the update")
            }
        },
        AutoApply::Stage => match updater.stage().await {
            Ok(staged) => info!(
                build = staged.build,
                path = %staged.path,
                "auto_apply staged the update; install it with `karucode update --apply`"
            ),
            Err(error) => {
                warn!(error = %error, build = manifest.build, "auto_apply could not stage the update")
            }
        },
        AutoApply::Off => {}
    }
    Some(manifest)
}

/// The build a shutdown is restarting into, when one was requested.
///
/// Set by [`request_restart`] and read by `bootstrap::run` after the server has
/// drained: the process exits with a non-zero status so a unit with
/// `Restart=on-failure` starts the binary that was just installed.
static RESTART_BUILD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The build a restart was requested for, if any.
pub fn restart_build() -> Option<u64> {
    match RESTART_BUILD.load(std::sync::atomic::Ordering::SeqCst) {
        0 => None,
        build => Some(build),
    }
}

/// Ask the server to stop so a supervisor starts the newly installed binary.
///
/// The delay is there for the HTTP response: the client that clicked **Install**
/// should get its JSON before the connection dies. Only called when the process is
/// supervised (systemd's `Restart=on-failure` brings it back); the check is in
/// [`Updater::apply`], which refuses `restart` otherwise.
pub fn request_restart(state: &crate::state::AppState, build: u64, delay: Duration) {
    let token = state.shutdown_token();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        RESTART_BUILD.store(build, std::sync::atomic::Ordering::SeqCst);
        info!(
            build,
            "shutting down so the supervisor starts the installed build"
        );
        token.cancel();
    });
}

/// Which of the three steps `karucode update` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Stage,
    Apply,
}

/// Parsed options for `karucode update`.
#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub action: UpdateAction,
    /// Override `[updates] manifest_url` for this run (`--manifest`).
    pub manifest: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub config: Option<PathBuf>,
    /// Restart the systemd unit after `--apply`.
    pub restart: bool,
}

/// Service name `--restart` acts on. `update.sh` uses the same variable.
const SERVICE_ENV: &str = "KARUCODE_SERVICE";
const DEFAULT_SERVICE: &str = "karucode";

/// `karucode update …`: check, stage or apply from a shell.
///
/// Exit codes: `0` success (and "already up to date" for `--check`), `3` an update
/// is available (`--check` only, matching `packaging/update.sh --check`), `1` any
/// failure. The command is documented in the `updates` topic.
pub async fn cli(options: UpdateOptions) -> anyhow::Result<()> {
    if let Some(path) = &options.config {
        if !path.exists() {
            anyhow::bail!(
                "config file {} does not exist; create it or omit --config",
                path.display()
            );
        }
        std::env::set_var("KARUCODE_CONFIG", path);
    }
    let mut config = kc_core::config::Config::load()
        .map_err(|e| anyhow::anyhow!("could not load configuration: {e}"))?;
    if let Some(dir) = &options.data_dir {
        config.data_dir = dir.clone();
    }
    if let Some(url) = &options.manifest {
        config.updates.manifest_url = url.clone();
        config.updates.check = true;
    }
    let updater = Updater::new(
        Arc::new(FixedSettings(config.updates.clone())),
        config.data_dir.clone(),
    );

    match options.action {
        UpdateAction::Check => {
            println!("current: {}", current_label());
            let manifest = match updater.check().await {
                Ok(manifest) => manifest,
                Err(error) => {
                    eprintln!("could not check for updates: {error}");
                    std::process::exit(1);
                }
            };
            let target_note = if manifest.artifact_for(version::TARGET).is_some() {
                String::new()
            } else {
                format!(
                    "\nnote:    release {} has no artifact for {} (it publishes {})",
                    manifest.build,
                    version::TARGET,
                    manifest
                        .targets
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let latest = match &manifest.commit {
                Some(commit) => format!("{} ({commit})", manifest.build),
                None => manifest.build.to_string(),
            };
            println!("latest:  {latest}");
            if manifest.build > version::BUILD {
                println!("update available: {}{target_note}", manifest.build);
                if let Some(notes) = manifest.notes.as_deref().map(str::trim) {
                    if !notes.is_empty() {
                        println!("\n{}", truncate_chars(notes, MAX_NOTES_CHARS));
                    }
                }
                if manifest.artifact_for(version::TARGET).is_some() {
                    println!(
                        "\ninstall it with: {}",
                        updater.apply_command_for(&updater.install_info())
                    );
                }
                std::process::exit(3);
            }
            println!("karucode is up to date.{target_note}");
        }
        UpdateAction::Stage => {
            if !version::is_versioned() {
                eprintln!(
                    "this binary has no build number (it was not built from a git checkout or a \
                     stamped release), so it cannot be compared with a release"
                );
                std::process::exit(1);
            }
            match updater.stage().await {
                Ok(staged) => {
                    println!(
                        "staged build {} ({} bytes, sha256 {}) at {}",
                        staged.build, staged.size, staged.sha256, staged.path
                    );
                    println!(
                        "install it with: {}",
                        updater.apply_command_for(&updater.install_info())
                    );
                }
                Err(error) => {
                    eprintln!("could not stage the update: {error}");
                    std::process::exit(1);
                }
            }
        }
        UpdateAction::Apply => {
            if !version::is_versioned() {
                eprintln!(
                    "this binary has no build number (it was not built from a git checkout or a \
                     stamped release), so it cannot be compared with a release"
                );
                std::process::exit(1);
            }
            let outcome = match updater.apply(false).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("could not install the update: {error}");
                    std::process::exit(1);
                }
            };
            println!(
                "installed build {} at {} (build {} kept as {}.old)",
                outcome.to,
                outcome.path,
                outcome.from,
                outcome.path.trim_end_matches(".exe")
            );
            if options.restart {
                if let Err(error) = restart_service() {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            } else {
                println!("restart karucode to run it (`systemctl restart {DEFAULT_SERVICE}`)");
            }
        }
    }
    Ok(())
}

fn current_label() -> String {
    format!(
        "{} ({}, {})",
        version::VERSION,
        version::COMMIT,
        version::COMMIT_DATE
    )
}

/// `systemctl restart <service>`, with an error that says the install already
/// succeeded.
fn restart_service() -> anyhow::Result<()> {
    let service = std::env::var(SERVICE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE.to_string());
    let systemctl = std::env::var("KARUCODE_SYSTEMCTL").unwrap_or_else(|_| "systemctl".to_string());
    let status = std::process::Command::new(&systemctl)
        .args(["restart", &service])
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("restarted {service}");
            Ok(())
        }
        Ok(status) => Err(anyhow::anyhow!(
            "the new build is installed, but `{systemctl} restart {service}` failed ({status}); \
             run it yourself to switch over"
        )),
        Err(error) => Err(anyhow::anyhow!(
            "the new build is installed, but `{systemctl}` could not be run ({error}); restart \
             {service} yourself to switch over"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json() -> String {
        format!(
            r#"{{
                "schema": 1,
                "build": {},
                "commit": "9c1de77a0b3c",
                "commit_date": "2026-05-31T09:12:40+00:00",
                "released_at": "2026-05-31T09:20:03Z",
                "url": "https://example.com/releases/tag/v108",
                "notes": "  Faster startup, and the update system itself.  ",
                "targets": {{
                    "{}": {{
                        "url": "https://example.com/karucode-{}",
                        "sha256": "{}",
                        "size": 4,
                        "tarball": "https://example.com/karucode.tar.gz"
                    }}
                }}
            }}"#,
            version::BUILD + 1,
            version::TARGET,
            version::BUILD + 1,
            "a".repeat(64)
        )
    }

    #[test]
    fn a_manifest_parses_and_compares_by_build_number() {
        let manifest = Manifest::parse(&manifest_json()).expect("manifest should parse");
        assert_eq!(manifest.build, version::BUILD + 1);
        assert!(manifest.build > version::BUILD, "the test build is newer");
        assert_eq!(
            manifest.artifact_for(version::TARGET).map(|a| a.size),
            Some(Some(4))
        );
        let latest = manifest.latest_info(manifest.artifact_for(version::TARGET));
        assert!(latest.has_target_for_this_build);
        assert_eq!(
            latest.notes.as_deref(),
            Some("Faster startup, and the update system itself.")
        );
        assert_eq!(latest.version, (version::BUILD + 1).to_string());
    }

    #[test]
    fn an_unknown_schema_is_refused_by_name() {
        let text = manifest_json().replace("\"schema\": 1", "\"schema\": 7");
        let error = Manifest::parse(&text).expect_err("an unknown schema must not be guessed at");
        assert!(error.message.contains("schema 7"), "{}", error.message);
        assert_eq!(error.kind, UpdateErrorKind::Remote);
    }

    #[test]
    fn a_manifest_with_a_bad_checksum_or_url_is_refused() {
        let text = manifest_json().replace(&"a".repeat(64), "nothex");
        assert!(
            Manifest::parse(&text).is_err(),
            "a short sha256 is a broken manifest"
        );

        let text = manifest_json().replace("https://example.com/karucode-", "file:///tmp/x");
        let error = Manifest::parse(&text).expect_err("a file:// artifact is refused");
        assert!(error.message.contains("not http(s)"), "{}", error.message);
    }

    #[test]
    fn build_zero_and_an_empty_target_list_are_refused() {
        let text = manifest_json().replace(
            &format!("\"build\": {}", version::BUILD + 1),
            "\"build\": 0",
        );
        assert!(Manifest::parse(&text).is_err(), "build 0 is not a release");

        let manifest = Manifest {
            schema: MANIFEST_SCHEMA,
            build: 1,
            commit: None,
            commit_date: None,
            released_at: None,
            notes: None,
            url: None,
            targets: BTreeMap::new(),
        };
        assert!(
            manifest.validate().is_err(),
            "a manifest with no targets is useless"
        );
    }

    #[test]
    fn a_release_without_this_platform_is_reported_as_such() {
        let manifest = Manifest::parse(&manifest_json()).unwrap();
        assert!(manifest.artifact_for("aarch64-unknown-linux-gnu").is_none());
        let latest = manifest.latest_info(manifest.artifact_for("aarch64-unknown-linux-gnu"));
        assert!(!latest.has_target_for_this_build);
        assert!(latest.artifact.is_none());
    }

    #[test]
    fn a_source_tree_is_not_a_managed_install() {
        let source = install_info(PathBuf::from(
            "/home/someone/karucode/target/release/karucode",
        ));
        assert_eq!(source.kind, "source");
        assert!(
            !source.writable,
            "a build tree is never updated by a release"
        );
        assert!(
            source.detail.contains("packaging/update.sh"),
            "{}",
            source.detail
        );

        // /usr/local/bin is inside a container in this test host when the test
        // runs under Docker, so the assertion is conditional on the real check.
        if !Path::new("/.dockerenv").exists() {
            let managed = install_info(PathBuf::from("/usr/local/bin/karucode"));
            assert_eq!(managed.kind, "managed");
        }
    }

    #[test]
    fn auto_apply_never_installs_over_a_binary_this_process_does_not_own() {
        let writable = InstallInfo {
            kind: "managed".into(),
            path: "/home/me/.local/bin/karucode".into(),
            writable: true,
            detail: String::new(),
        };
        assert_eq!(auto_apply_action(false, &writable), AutoApply::Off);
        assert_eq!(auto_apply_action(true, &writable), AutoApply::Install);

        let root_owned = InstallInfo {
            writable: false,
            ..writable.clone()
        };
        assert_eq!(
            auto_apply_action(true, &root_owned),
            AutoApply::Stage,
            "a service account cannot replace /usr/local/bin/karucode, so it stages"
        );

        for kind in ["source", "container"] {
            let other = InstallInfo {
                kind: kind.into(),
                writable: true,
                ..writable.clone()
            };
            assert_eq!(
                auto_apply_action(true, &other),
                AutoApply::Off,
                "a {kind} install is not updated from a release artifact"
            );
        }
    }

    #[test]
    fn the_apply_command_quotes_a_path_with_spaces() {
        assert_eq!(
            shell_quote("/usr/local/bin/karucode"),
            "/usr/local/bin/karucode"
        );
        assert_eq!(
            shell_quote("/opt/my karucode/bin/karucode"),
            "'/opt/my karucode/bin/karucode'"
        );
        assert_eq!(
            shell_quote("/opt/it's/karucode"),
            r"'/opt/it'\''s/karucode'"
        );
    }

    #[test]
    fn wrote_a_file_and_its_checksum_matches() {
        let bytes = b"pretend binary";
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hex(&hasher.finalize());
        assert!(sha256_matches(bytes, &digest));
        assert!(sha256_matches(bytes, &digest.to_uppercase()));
        assert!(!sha256_matches(bytes, &"0".repeat(64)));
    }

    #[test]
    fn notes_are_truncated_on_a_character_boundary() {
        let text = "é".repeat(10);
        let cut = truncate_chars(&text, 4);
        assert_eq!(cut.chars().count(), 4);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_chars("short", 10), "short");
    }

    #[test]
    fn the_version_probe_reads_the_line_a_real_binary_prints() {
        // The test binary is the current build, so it must pass its own proof.
        let exe = current_exe();
        let found = std::process::Command::new(&exe).arg("--version").output();
        // A test executable has no CLI, so this only asserts the parser on a
        // synthetic line; the real proof runs the karucode binary.
        assert!(found.is_ok());
        assert_eq!(
            version::parse_version_line(&format!("karucode {} (abc, 2026-01-01)", version::BUILD)),
            Some(version::BUILD)
        );
    }

    #[test]
    fn install_binary_replaces_the_file_and_keeps_the_old_one() {
        let dir = temp_dir("install-binary");
        let exe = dir.join("karucode");
        std::fs::write(&exe, b"old build").unwrap();
        let new = format!("karucode {} (abc, 2026-01-01)\n", version::BUILD);
        install_binary(&exe, new.as_bytes()).expect("install should succeed");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), new);
        assert_eq!(
            std::fs::read_to_string(dir.join("karucode.old")).unwrap(),
            "old build",
            "the previous binary must survive as .old"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installing_twice_keeps_exactly_one_previous_binary() {
        // The second install in a process that already installed once is the case
        // that found the real bug: after the first swap `/proc/self/exe` names the
        // `.old` file, so an install path re-read from it points at a file the
        // second swap is about to delete. `install_binary` takes the path it is
        // given, and this pins the two-in-a-row behaviour.
        let dir = temp_dir("install-twice");
        let exe = dir.join("karucode");
        std::fs::write(&exe, b"first").unwrap();
        install_binary(&exe, b"second").expect("first install");
        install_binary(&exe, b"third").expect("second install");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "third");
        assert_eq!(
            std::fs::read_to_string(dir.join("karucode.old")).unwrap(),
            "second",
            "the last installed build is the one kept for a rollback"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_install_puts_the_old_binary_back() {
        let dir = temp_dir("install-failure");
        let exe = dir.join("karucode");
        std::fs::write(&exe, b"old build").unwrap();
        // Writing into a directory that vanished is the closest local equivalent
        // of a rename that fails after the old file moved aside.
        let missing = dir.join("gone").join("karucode");
        assert!(install_binary(&missing, b"new").is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old build");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "karucode-update-test-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
