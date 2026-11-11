//! `karucode plugin install` over HTTP: the web UI's half of the installer.
//!
//! The CLI prints progress as it happens; a browser needs the same run as a
//! pollable job. These routes wrap [`crate::plugin_install`] with a tiny
//! in-memory registry:
//!
//! * `POST /api/plugins/install/preview` — resolve a source and return what it
//!   offers (names, permissions, whether each builds, whether each is already
//!   installed) without installing anything. Synchronous: resolution is one
//!   download.
//! * `POST /api/plugins/install` — start an install job, answer `202` with
//!   its id. Names, `--force`, `--ref`, `--no-build`, `--build-timeout-secs`
//!   ride in the body.
//! * `GET  /api/plugins/install/:id` — poll: status, every progress event so
//!   far, the rescan report when it lands.
//! * `POST /api/plugins/install/:id/cancel` — abort a running job (the build
//!   child is killed; anything already swapped in stays, which is only ever
//!   "complete plugins", because the swap is atomic per plugin).
//!
//! Security, in one paragraph: installing a plugin means downloading a
//! repository and **running its build command** — arbitrary code, with the
//! server's own environment. That is exactly what `POST /api/update/apply`
//! does, so these routes take the same admin gate
//! ([`crate::settings::authorize_admin`]) *before* anything is fetched.
//! Resolved-shape problems (unknown source form, hostile archive) are 400/502
//! from the installer's own errors; a running server never trusts the
//! repository's manifest (see [`crate::plugin_install`] for the strip rules).
//!
//! The job registry is per-process and unbounded-free: entries are dropped
//! from memory once a terminal job has been fetched and is older than an
//! hour, so a browser tab left open cannot pin memory forever.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::plugin_install::{self, InstallOutcome, InstallReporter, PluginInstallOptions};
use crate::routes::parse_required;
use crate::state::AppState;

/// How long a finished job stays queryable after its last read.
const JOB_TTL_AFTER_READ: Duration = Duration::from_secs(3600);
/// How many finished jobs to keep even unread, so a poll that races a restart
/// still finds its answer.
const MAX_FINISHED_JOBS: usize = 64;

/// The request body both preview and install accept: the same vocabulary the
/// CLI flags spell.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequestBody {
    /// `owner/repo`, a github.com URL, `owner/repo@ref`, or a local directory.
    #[serde(default)]
    pub source: Option<String>,
    /// Specific plugin names; empty = every plugin the source offers.
    #[serde(default)]
    pub names: Vec<String>,
    /// Branch, tag or commit; default `HEAD`.
    #[serde(default)]
    pub git_ref: Option<String>,
    /// Replace an existing install instead of refusing.
    #[serde(default)]
    pub force: bool,
    /// Skip the build step.
    #[serde(default)]
    pub no_build: bool,
    /// Override `[install] build_timeout_secs`.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

impl InstallRequestBody {
    fn into_options(self, data_dir: PathBuf) -> anyhow::Result<PluginInstallOptions> {
        let source = self.source.unwrap_or_default().trim().to_string();
        if source.is_empty() {
            anyhow::bail!("`source` is required: a GitHub repository or a local directory");
        }
        Ok(PluginInstallOptions {
            source,
            names: self.names,
            git_ref: self.git_ref,
            list: false,
            force: self.force,
            no_build: self.no_build,
            build_timeout_secs: self.build_timeout_secs,
            data_dir: Some(data_dir),
            config: None,
        })
    }
}

/// The shared view of one job: written by the task, read by the poll.
struct JobInner {
    status: JobStatus,
    events: Vec<plugin_install::InstallEvent>,
    /// The error message when the job failed.
    error: Option<String>,
    /// The outcome + rescan report when it succeeded.
    outcome: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Running,
    Completed,
    Failed,
    #[allow(dead_code)]
    Cancelled,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

/// One install job: the reporter the task emits into, the state the poll
/// reads, and the cancel signal between them.
struct InstallJob {
    inner: Mutex<JobInner>,
    cancel: tokio_util::sync::CancellationToken,
    started: Instant,
    /// Last poll time, for the TTL sweep.
    last_read: Mutex<Instant>,
}

/// A sink that appends events to a job under its mutex. The task and the poll
/// are on the same process, so a lock-bounded push is all the synchronization
/// the progress needs.
struct JobReporter {
    job: std::sync::Arc<InstallJob>,
}

impl InstallReporter for JobReporter {
    fn emit(&self, event: plugin_install::InstallEvent) {
        let mut inner = self.job.inner.lock().expect("install job lock");
        inner.events.push(event);
    }
}

/// The registry answerable at the state. Small and in-process: a job that
/// outlives the server is gone, and that is fine — its filesystem effect is
/// already applied and visible in `GET /api/plugins`.
#[derive(Default)]
pub struct InstallJobRegistry {
    jobs: Mutex<HashMap<String, std::sync::Arc<InstallJob>>>,
}

impl InstallJobRegistry {
    /// Drop terminal, stale jobs so the map cannot grow without bound.
    fn sweep(&self) {
        let mut jobs = self.jobs.lock().expect("install jobs lock");
        let now = Instant::now();
        jobs.retain(|_, job| {
            let status = job.inner.lock().expect("install job lock").status;
            if matches!(
                status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
            ) {
                let last_read = *job.last_read.lock().expect("install job read lock");
                let age = now.duration_since(job.started);
                // Kept when recently read, unread-but-recent, or while under
                // the finished cap.
                if now.duration_since(last_read) <= JOB_TTL_AFTER_READ || age <= JOB_TTL_AFTER_READ
                {
                    return true;
                }
                return false;
            }
            true
        });
        // Hard cap on terminal entries regardless of clocks: keep the newest.
        let finished: Vec<(String, Instant)> = jobs
            .iter()
            .filter(|(_, job)| {
                let status = job.inner.lock().expect("install job lock").status;
                matches!(
                    status,
                    JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
                )
            })
            .map(|(id, job)| (id.clone(), job.started))
            .collect();
        if finished.len() > MAX_FINISHED_JOBS {
            let mut finished = finished;
            finished.sort_by_key(|(_, started)| *started);
            let excess = finished.len() - MAX_FINISHED_JOBS;
            for (id, _) in finished.into_iter().take(excess) {
                jobs.remove(&id);
            }
        }
    }
}

/// `POST /api/plugins/install/preview` — what does this source offer?
///
/// Answers synchronously: one repository fetch (a download for a remote
/// source, a local copy otherwise) and the resolve. The caller sees every
/// plugin's name, description, declared permissions, whether it will be
/// built, and whether an install of it already exists — the review surface
/// before anything runs.
pub async fn preview_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    authorize(&state, &headers, "previewing a plugin install")?;
    let request: InstallRequestBody = parse_required(&body, "POST /api/plugins/install/preview")?;
    let options = request
        .into_options(effective_data_dir(&state))
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    let data_dir = options.data_dir.clone().unwrap_or_default();
    // Resolution can take a download (codeload): off the request path but
    // awaited, because the answer *is* the preview.
    let preview = plugin_install::preview(&options, &data_dir)
        .await
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    Ok(Json(serde_json::to_value(preview).map_err(|e| {
        ApiError::internal(format!("could not encode the preview: {e}"))
    })?))
}

/// `POST /api/plugins/install` — start an install job, answer `202 {id}`.
///
/// The job runs the same `install()` the CLI runs. On success it performs the
/// **rescan itself**, so the plugins answer `GET /api/plugins` as ready and
/// the model sees the new tools on its next turn — the web UI never needs to
/// fire a second "apply" step the CLI user had to remember.
pub async fn start_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    authorize(&state, &headers, "installing a plugin")?;
    let request: InstallRequestBody = parse_required(&body, "POST /api/plugins/install")?;
    let options = request
        .into_options(effective_data_dir(&state))
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;

    state.installs().sweep();
    let id = Uuid::new_v4().to_string();
    let job = std::sync::Arc::new(InstallJob {
        inner: Mutex::new(JobInner {
            status: JobStatus::Running,
            events: Vec::new(),
            error: None,
            outcome: None,
        }),
        cancel: tokio_util::sync::CancellationToken::new(),
        started: Instant::now(),
        last_read: Mutex::new(Instant::now()),
    });
    state
        .installs()
        .jobs
        .lock()
        .expect("install jobs lock")
        .insert(id.clone(), job.clone());

    let state_for_task = state.clone();
    let job_for_task = job.clone();
    let data_dir_for_task = effective_data_dir(&state);
    tokio::spawn(async move {
        let reporter = JobReporter {
            job: job_for_task.clone(),
        };
        let cancel = job_for_task.cancel.clone();
        // The install is a future; the cancel token races it. A finished
        // install wins the race (its swap already landed), a cancel wins only
        // while work is still in flight.
        let install = plugin_install::install(&options, &data_dir_for_task, &reporter);
        let outcome = tokio::select! {
            result = install => result,
            _ = cancel.cancelled() => Err(anyhow::anyhow!("cancelled")),
        };
        // The status transition happens in short-lived lock scopes: no guard
        // may cross the rescan await (a `MutexGuard` is not `Send`), and a
        // poll must be free to read events while the rescan runs.
        match outcome {
            Err(error) if error.to_string() == "cancelled" => {
                job_for_task.inner.lock().expect("install job lock").status = JobStatus::Cancelled;
            }
            Err(error) => {
                let mut inner = job_for_task.inner.lock().expect("install job lock");
                inner.status = JobStatus::Failed;
                inner.error = Some(format!("{error:#}"));
            }
            Ok(outcome) => {
                // The installer wrote disk manifests the host has not read
                // yet. Rescan now so "install → plugin ready" is one action;
                // the report is the answer the CLI user reads from jq. The
                // job stays `running` until the rescan lands, and Completed
                // is written in the same lock as the outcome, so a poll that
                // sees `completed` also sees the result it carries.
                let (report, _diagnostics) = state_for_task.reload_plugins().await;
                let mut inner = job_for_task.inner.lock().expect("install job lock");
                inner.outcome = Some(
                    serde_json::to_value(json!({
                        "installed": outcome.installed,
                        "paths": outcome
                            .paths
                            .iter()
                            .map(|(name, path)| json!({ "name": name, "path": path }))
                            .collect::<Vec<_>>(),
                        "rescan": report,
                    }))
                    .unwrap_or(Value::Null),
                );
                inner.status = JobStatus::Completed;
            }
        };
    });

    Ok(Json(json!({
        "id": id,
        "status": "running",
        "poll": format!("/api/plugins/install/{id}"),
    })))
}

/// `GET /api/plugins/install/:id` — the job's snapshot.
pub async fn get_install(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state.installs().sweep();
    let job = state
        .installs()
        .jobs
        .lock()
        .expect("install jobs lock")
        .get(&id)
        .cloned();
    let Some(job) = job else {
        return Err(ApiError::not_found(format!(
            "no plugin install job with id `{id}`; it finished over an hour ago or the server \
             restarted"
        )));
    };
    *job.last_read.lock().expect("install job read lock") = Instant::now();
    let inner = job.inner.lock().expect("install job lock");
    let elapsed_ms = job.started.elapsed().as_millis() as u64;
    Ok(Json(json!({
        "id": id,
        "status": inner.status.as_str(),
        "events": inner.events,
        "error": inner.error,
        "result": inner.outcome,
        "elapsed_ms": elapsed_ms,
    })))
}

/// `POST /api/plugins/install/:id/cancel` — stop a running install.
///
/// A completed job answers `409` (nothing to cancel); a cancelled one is
/// idempotent (`completed` again, so the poll shape never changes on the
/// caller).
pub async fn cancel_install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &headers, "cancelling a plugin install")?;
    let job = state
        .installs()
        .jobs
        .lock()
        .expect("install jobs lock")
        .get(&id)
        .cloned();
    let Some(job) = job else {
        return Err(ApiError::not_found(format!(
            "no plugin install job with id `{id}`"
        )));
    };
    let status = job.inner.lock().expect("install job lock").status;
    if status != JobStatus::Running {
        return Err(ApiError::conflict(format!(
            "the plugin install job is already {}",
            status.as_str()
        )));
    }
    job.cancel.cancel();
    Ok(Json(json!({
        "id": id,
        "status": "cancelling",
        "poll": format!("/api/plugins/install/{id}"),
    })))
}

/// The data directory the running server was started with — the one the
/// installer must target for the host to see its output. Derived the same way
/// the updater derives it: the database lives in the data dir.
fn effective_data_dir(state: &AppState) -> PathBuf {
    state
        .db_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// The admin gate, exactly like `POST /api/update/apply`: an install runs
/// build commands with the server's environment, which is the same class of
/// action as replacing the binary.
fn authorize(state: &AppState, headers: &HeaderMap, what: &str) -> ApiResult<()> {
    let config = state.settings().snapshot();
    let presented = headers
        .get(crate::settings::ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    crate::settings::authorize_admin(&config, presented, what).map_err(ApiError::from)
}

/// Keep the outcome type name used in the spawn handy for signature-checking.
#[allow(dead_code)]
fn _outcome_witness(outcome: &InstallOutcome) -> &InstallOutcome {
    outcome
}
