//! Supervision of one plugin process: spawn, JSON-RPC framing, exit handling
//! and the restart policy.
//!
//! # Wire format
//!
//! Newline-delimited JSON-RPC 2.0. The host writes requests to the plugin's
//! stdin, one JSON object per line, and reads responses/notifications from
//! stdout. stdout is reserved for the protocol; anything a plugin wants a human
//! to read goes to stderr, which is drained into `tracing` at debug level.
//!
//! # Task layout per process
//!
//! * *writer*  - owns stdin, serialises outgoing lines from an unbounded channel.
//! * *reader*  - owns stdout, routes responses to their `oneshot` waiter and
//!   logs notifications.
//! * *stderr*  - drains stderr into `tracing::debug!`.
//! * *supervisor* - owns the `Child`, waits for it (or for a kill signal) and
//!   reports the exit status.
//!
//! Every task holds an `Arc<PluginShared>`; nothing is `unsafe`, nothing is
//! panicking. A plugin that dies, hangs or writes garbage can only affect its
//! own slot.

use kc_core::config::PluginConfig;
use kc_core::plugin::{
    methods, valid_panel_id, PluginDiagnostics, PluginError, PluginManifest, PluginPanel,
    PluginPanelData, PluginStatus, PluginToolDef, PluginToolResult, ToolCallContext,
    PANEL_SOURCE_STATIC, PANEL_SOURCE_TOOL, PLUGIN_PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::PluginTimeouts;

/// How many automatic restarts one plugin slot may perform per process lifetime.
pub(crate) const MAX_RESTARTS: u32 = 3;

/// Timeouts, shared between the host and every slot so `set_timeouts` applies
/// to plugins that are already discovered.
#[derive(Debug)]
pub(crate) struct TimeoutRefs {
    handshake_ms: AtomicU64,
    call_ms: AtomicU64,
    hook_ms: AtomicU64,
    shutdown_ms: AtomicU64,
}

impl Default for TimeoutRefs {
    fn default() -> Self {
        Self::from(&PluginTimeouts::default())
    }
}

impl TimeoutRefs {
    pub(crate) fn from(t: &PluginTimeouts) -> Self {
        Self {
            handshake_ms: AtomicU64::new(millis(t.handshake)),
            call_ms: AtomicU64::new(millis(t.call)),
            hook_ms: AtomicU64::new(millis(t.hook)),
            shutdown_ms: AtomicU64::new(millis(t.shutdown)),
        }
    }

    pub(crate) fn apply(&self, t: &PluginTimeouts) {
        self.handshake_ms
            .store(millis(t.handshake), Ordering::Relaxed);
        self.call_ms.store(millis(t.call), Ordering::Relaxed);
        self.hook_ms.store(millis(t.hook), Ordering::Relaxed);
        self.shutdown_ms
            .store(millis(t.shutdown), Ordering::Relaxed);
    }

    pub(crate) fn handshake(&self) -> Duration {
        Duration::from_millis(self.handshake_ms.load(Ordering::Relaxed).max(1))
    }

    pub(crate) fn call(&self) -> Duration {
        Duration::from_millis(self.call_ms.load(Ordering::Relaxed).max(1))
    }

    pub(crate) fn hook(&self) -> Duration {
        Duration::from_millis(self.hook_ms.load(Ordering::Relaxed).max(1))
    }

    pub(crate) fn shutdown(&self) -> Duration {
        Duration::from_millis(self.shutdown_ms.load(Ordering::Relaxed).max(1))
    }
}

fn millis(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

/// JSON-RPC "method not found", the conventional answer for an unimplemented
/// method (the SDK and every example plugin use it).
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;

/// One panel payload, ready to be serialised for the HTTP route.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelPayload {
    pub title: String,
    /// Opaque JSON. The host never reads it; the browser renders it.
    pub data: Value,
}

impl PanelPayload {
    /// A payload built by the host itself (for example after the plugin died).
    #[allow(dead_code)]
    pub fn empty(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            data: Value::Null,
        }
    }
}

/// Failure of a JSON-RPC interaction with a plugin.
#[derive(Debug)]
pub(crate) enum RpcError {
    /// The process is not running (never started, stopped, disabled, crashed).
    NotReady(String),
    /// stdin/stdout framing broke, or the process exited mid-request.
    Transport(String),
    /// The plugin answered with a JSON-RPC error object.
    Remote(PluginError),
    /// No answer within the configured budget.
    Timeout(Duration),
    /// The caller cancelled the request (run cancelled, tool aborted).
    Cancelled,
}

impl RpcError {
    /// One-line description for logs and diagnostics.
    pub(crate) fn describe(&self) -> String {
        match self {
            RpcError::NotReady(m) | RpcError::Transport(m) => m.clone(),
            RpcError::Remote(e) => match e.code {
                0 => e.message.clone(),
                code => format!("{} (code {code})", e.message),
            },
            RpcError::Timeout(d) => format!("timed out after {}s", fmt_secs(*d)),
            RpcError::Cancelled => "cancelled".to_string(),
        }
    }

    /// Longer, actionable text for a tool result handed back to the model.
    pub(crate) fn tool_message(&self, plugin: &str, tool: &str, budget: Duration) -> String {
        match self {
            RpcError::NotReady(m) => format!(
                "plugin tool `{tool}` is unavailable: {m}. The plugin process is not running; \
                 fix the plugin (see the server log) or disable it in the plugin settings."
            ),
            RpcError::Transport(m) => format!(
                "plugin tool `{tool}` failed: the plugin `{plugin}` process stopped or broke the \
                 protocol ({m}). The call was not retried further. Check the plugin's stderr in \
                 the server log, then call again or disable the plugin."
            ),
            RpcError::Remote(e) => format!(
                "plugin `{plugin}` reported an error for tool `{tool}`: {}{}",
                e.message,
                code_suffix(e.code)
            ),
            RpcError::Timeout(d) => format!(
                "plugin tool `{tool}` timed out after {}s (limit {}s). The plugin `{plugin}` is \
                 still running; retry with a narrower request or raise the timeout.",
                fmt_secs(*d),
                fmt_secs(budget)
            ),
            RpcError::Cancelled => format!(
                "plugin tool `{tool}` was cancelled before it answered; the plugin `{plugin}` \
                 keeps running but the result was discarded."
            ),
        }
    }
}

fn code_suffix(code: i32) -> String {
    match code {
        0 => String::new(),
        code => format!(" (code {code})"),
    }
}

fn fmt_secs(d: Duration) -> String {
    let ms = d.as_millis();
    if ms.is_multiple_of(1000) {
        format!("{}", ms / 1000)
    } else {
        format!("{:.1}", d.as_secs_f64())
    }
}

/// Reply delivered through the pending-request map.
enum Reply {
    Ok(Value),
    Remote(PluginError),
    Transport(String),
}

struct Inner {
    status: PluginStatus,
    manifest: Option<PluginManifest>,
    tools: Vec<PluginToolDef>,
    /// Panels the manifest declared, already validated and de-duplicated.
    panels: Vec<PluginPanel>,
    error: Option<String>,
    pid: Option<u32>,
    out: Option<mpsc::Sender<String>>,
    pending: HashMap<u64, oneshot::Sender<Reply>>,
    kill: Option<oneshot::Sender<()>>,
    supervisor: Option<JoinHandle<()>>,
    /// Set while the host is intentionally stopping the process, so the exit is
    /// reported as `stopped` instead of `failed`.
    shutting_down: bool,
    restart_count: u32,
}

/// Map a failed `try_send` onto the error the caller sees.
///
/// `Full` is the bounded-queue case: the plugin stopped reading its stdin, so
/// the writer task is blocked in `write_all` and the queue is full. Buffering
/// more would grow without limit, so the request fails instead.
fn queue_error(plugin: &str, e: mpsc::error::TrySendError<String>) -> RpcError {
    match e {
        mpsc::error::TrySendError::Full(_) => {
            RpcError::Transport(format!("plugin `{plugin}` is not reading its stdin"))
        }
        mpsc::error::TrySendError::Closed(_) => {
            RpcError::Transport(format!("plugin `{plugin}` closed its stdin"))
        }
    }
}

/// Removes a request id from `Inner::pending` when it goes out of scope.
///
/// Every exit path of [`PluginShared::request_cancellable`] — including a
/// dropped future — runs this `Drop`, so a cancelled or abandoned call cannot
/// leave a dead waiter behind.
struct PendingGuard<'a> {
    shared: &'a PluginShared,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.shared.lock().pending.remove(&self.id);
    }
}

/// State shared between the host, the tool objects and the per-process tasks.
pub(crate) struct PluginShared {
    pub(crate) name: String,
    config: PluginConfig,
    timeouts: Arc<TimeoutRefs>,
    inner: Mutex<Inner>,
    /// Serialises (re)starts so two concurrent calls cannot spawn two processes.
    start_lock: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    /// Incremented on every spawn; stale tasks from a previous process compare
    /// their epoch before touching state.
    epoch: AtomicU64,
    /// Where an inbound `notify` frame goes.
    ///
    /// A plugin cannot reach a browser — there is no route mounting and no script
    /// execution — so the only way a plugin-raised notification can reach a human
    /// is through this channel, which the server drains into the same
    /// `Event::Notification` the runtime uses. `None` means "nobody is listening",
    /// which is what the unit tests that do not care about notifications use.
    notify_tx: Mutex<Option<mpsc::UnboundedSender<PluginNotice>>>,
    /// Rate limiting for plugin-raised notifications: a plugin that floods `notify`
    /// must not be able to flood a person's desktop.
    notify_budget: Mutex<NotifyBudget>,
}

/// One notification a plugin asked for, already validated by the host.
///
/// Validated here rather than at the server so a malformed frame can never reach
/// the event stream: the host is the only component that reads plugin output.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginNotice {
    /// The plugin that raised it.
    pub plugin: String,
    pub kind: kc_core::notify::NotifyKind,
    pub title: String,
    pub body: String,
    /// The session the plugin was called for, when it named one. Empty means
    /// "not about a particular session".
    pub session_id: String,
    pub sound: Option<String>,
    pub sticky: bool,
}

/// Token-bucket rate limit for plugin notifications.
///
/// One per second with a burst of five: enough for a plugin that reports a handful
/// of related events, nowhere near enough to be a nuisance. Exceeding it drops the
/// notification and warns once, because the alternative — a plugin in a loop
/// pinning notifications to the screen — is exactly the failure mode this feature
/// must not have.
#[derive(Debug)]
struct NotifyBudget {
    /// Milliseconds of "credit" left, refilled at `REFILL_MS` per millisecond.
    credit_ms: f64,
    last_refill_ms: i64,
    /// Set once the first drop has been logged, so a flooding plugin produces one
    /// warning instead of a megabyte of them.
    warned: bool,
}

/// Burst allowance, in milliseconds of credit (five notifications).
const NOTIFY_BURST_MS: f64 = 5_000.0;
/// Refill rate: one notification per second.
const NOTIFY_REFILL_PER_MS: f64 = 0.001;
/// Cost of one notification, in milliseconds of credit.
const NOTIFY_COST_MS: f64 = 1_000.0;

impl PluginShared {
    pub(crate) fn new(
        config: PluginConfig,
        timeouts: Arc<TimeoutRefs>,
        status: PluginStatus,
    ) -> Arc<Self> {
        let name = config.name.clone();
        Arc::new(Self {
            name,
            config,
            timeouts,
            inner: Mutex::new(Inner {
                status,
                manifest: None,
                tools: Vec::new(),
                panels: Vec::new(),
                error: None,
                pid: None,
                out: None,
                pending: HashMap::new(),
                kill: None,
                supervisor: None,
                shutting_down: false,
                restart_count: 0,
            }),
            start_lock: tokio::sync::Mutex::new(()),
            next_id: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            notify_tx: Mutex::new(None),
            notify_budget: Mutex::new(NotifyBudget {
                credit_ms: NOTIFY_BURST_MS,
                last_refill_ms: 0,
                warned: false,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic inside one of the small critical sections must not poison the
        // plugin slot for the rest of the process.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn status(&self) -> PluginStatus {
        self.lock().status.clone()
    }

    pub(crate) fn manifest(&self) -> Option<PluginManifest> {
        self.lock().manifest.clone()
    }

    pub(crate) fn tool_defs(&self) -> Vec<PluginToolDef> {
        self.lock().tools.clone()
    }

    /// The validated panels this plugin declared, in manifest order.
    pub(crate) fn panel_defs(&self) -> Vec<PluginPanel> {
        self.lock().panels.clone()
    }

    /// One declared panel by its bare (un-namespaced) id.
    pub(crate) fn panel_def(&self, panel: &str) -> Option<PluginPanel> {
        self.lock().panels.iter().find(|p| p.id == panel).cloned()
    }

    pub(crate) fn config(&self) -> PluginConfig {
        self.config.clone()
    }

    /// Point this slot's inbound `notify` frames at a listener (or nowhere).
    pub(crate) fn set_notify_sender(&self, tx: Option<mpsc::UnboundedSender<PluginNotice>>) {
        *self.notify_tx.lock().unwrap_or_else(|e| e.into_inner()) = tx;
    }

    /// Number of in-flight requests. Test-only: the pending map must not grow
    /// when calls are cancelled, timed out or answered.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.lock().pending.len()
    }

    pub(crate) fn diagnostics(&self) -> PluginDiagnostics {
        let inner = self.lock();
        PluginDiagnostics {
            name: self.name.clone(),
            status: inner.status.clone(),
            manifest: inner.manifest.clone(),
            tools: inner.tools.clone(),
            error: inner.error.clone(),
            pid: inner.pid,
            restart_count: inner.restart_count,
        }
    }

    pub(crate) fn reset_restarts(&self) {
        self.lock().restart_count = 0;
    }

    /// Clear the `disabled` marker so a start is allowed (used by
    /// `set_enabled(name, true)`).
    pub(crate) fn mark_stopped(&self) {
        let mut inner = self.lock();
        if inner.status == PluginStatus::Disabled {
            inner.status = PluginStatus::Stopped;
        }
    }

    /// Budget for one `tools/call`.
    pub(crate) fn call_timeout(&self) -> Duration {
        self.timeouts.call()
    }

    fn fail(&self, message: String) {
        let mut inner = self.lock();
        inner.status = PluginStatus::Failed;
        inner.error = Some(message);
        inner.tools.clear();
        inner.panels.clear();
        inner.manifest = None;
        inner.pid = None;
        inner.out = None;
    }

    /// Send one request and wait for the matching response.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        self.request_cancellable(method, params, timeout, None)
            .await
    }

    /// [`request`](Self::request), but abandoned as soon as `cancel` fires.
    ///
    /// The pending entry is removed on **every** exit path — response, remote
    /// error, transport error, timeout, cancellation and future-drop — by
    /// [`PendingGuard`]. Without that, a cancelled call would leave a dead
    /// `oneshot::Sender` in `Inner::pending` forever.
    pub(crate) async fn request_cancellable(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, RpcError> {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut inner = self.lock();
            let sender = match inner.out.as_ref() {
                Some(sender) => sender.clone(),
                None => {
                    let msg = inner
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("plugin `{}` is not running", self.name));
                    return Err(RpcError::NotReady(msg));
                }
            };
            let line = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string();
            inner.pending.insert(id, tx);
            if let Err(e) = sender.try_send(line) {
                inner.pending.remove(&id);
                return Err(queue_error(&self.name, e));
            }
        }

        // From here on the entry is removed by the guard, whatever happens.
        let _guard = PendingGuard { shared: self, id };

        let wait = async {
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(Reply::Ok(v))) => Ok(v),
                Ok(Ok(Reply::Remote(e))) => Err(RpcError::Remote(e)),
                Ok(Ok(Reply::Transport(m))) => Err(RpcError::Transport(m)),
                Ok(Err(_)) => Err(RpcError::Transport(format!(
                    "plugin `{}` dropped the response for `{method}`",
                    self.name
                ))),
                Err(_) => Err(RpcError::Timeout(timeout)),
            }
        };
        match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => Err(RpcError::Cancelled),
                reply = wait => reply,
            },
            None => wait.await,
        }
    }

    /// Ask the plugin for a hook contribution. Returns `None` when the plugin
    /// does not advertise the hook, is not ready, or the hook failed.
    pub(crate) async fn hook_output(
        self: &Arc<Self>,
        hook: &str,
        payload: &Value,
    ) -> Option<Value> {
        let advertised = self
            .lock()
            .manifest
            .as_ref()
            .map(|m| m.hooks.iter().any(|h| h == hook))
            .unwrap_or(false);
        if !advertised {
            return None;
        }
        let params = json!({ "hook": hook, "payload": payload });
        match self
            .request(methods::HOOKS_RUN, params, self.timeouts.hook())
            .await
        {
            Ok(v) => {
                let out = v.get("output").cloned().unwrap_or(Value::Null);
                if out.is_null() {
                    None
                } else {
                    Some(out)
                }
            }
            Err(e) => {
                // Hooks are best-effort: a broken hook must not fail a run.
                debug!(plugin = %self.name, hook, "hook failed: {}", e.describe());
                None
            }
        }
    }

    /// Ask the plugin for a panel payload.
    ///
    /// The plugin answers `panels/read` with `{"title": "...", "data": <any
    /// JSON>}`; the host never interprets `data`. A plugin that does not declare
    /// `panels/read` support falls back to running the panel's `tool` through
    /// `tools/call` and using its `meta` — that is what `source = "tool"` meant
    /// before this method existed, so an older plugin keeps working. Errors are
    /// returned (not swallowed): the caller shows them to the user.
    pub(crate) async fn panel_output(
        self: &Arc<Self>,
        panel: &PluginPanel,
        context: &ToolCallContext,
    ) -> Result<PanelPayload, RpcError> {
        let params = json!({
            "panel": panel.id,
            "session_id": context.session_id,
            "project": context.project,
            "workspace": context.workspace,
        });
        let reply = self
            .request(methods::PANELS_READ, params, self.timeouts.call())
            .await;
        match reply {
            Ok(value) => {
                let parsed: PluginPanelData = serde_json::from_value(value).map_err(|e| {
                    RpcError::Transport(format!(
                        "plugin `{}` answered `panels/read` for `{}` with something that is not \
                         {{\"title\", \"data\"}}: {e}",
                        self.name, panel.id
                    ))
                })?;
                Ok(PanelPayload {
                    title: parsed
                        .title
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| panel.title.clone()),
                    data: parsed.data,
                })
            }
            // An older plugin answers `-32601: unknown method`. If the panel named
            // a tool, fall back to calling it: that is exactly what `source =
            // "tool"` documented before `panels/read` existed.
            Err(RpcError::Remote(e)) if e.code == METHOD_NOT_FOUND => {
                self.panel_via_tool(panel, context).await
            }
            Err(e) => Err(e),
        }
    }

    async fn panel_via_tool(
        self: &Arc<Self>,
        panel: &PluginPanel,
        context: &ToolCallContext,
    ) -> Result<PanelPayload, RpcError> {
        let Some(tool) = panel.tool.as_deref() else {
            return Err(RpcError::Remote(PluginError::new(
                METHOD_NOT_FOUND,
                format!(
                    "the plugin does not implement `panels/read` and panel `{}` names no tool to \
                     fall back to; reinstall it with a karucode build that supports panels",
                    panel.id
                ),
            )));
        };
        let params = json!({
            "name": tool,
            "arguments": {},
            "context": context,
        });
        match self
            .request(methods::TOOLS_CALL, params, self.timeouts.call())
            .await
        {
            Ok(value) => {
                let result: PluginToolResult = serde_json::from_value(value).map_err(|e| {
                    RpcError::Transport(format!(
                        "plugin `{}` returned an invalid `tools/call` result for the panel tool \
                         `{tool}`: {e}",
                        self.name
                    ))
                })?;
                if !result.ok {
                    return Err(RpcError::Remote(PluginError::new(
                        0,
                        format!("the panel tool `{tool}` failed: {}", result.content),
                    )));
                }
                Ok(PanelPayload {
                    title: panel.title.clone(),
                    data: result.meta.unwrap_or(Value::Null),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Start the process (no-op when already ready). Serialised by `start_lock`.
    pub(crate) async fn start(self: &Arc<Self>) -> Result<(), String> {
        let _guard = self.start_lock.lock().await;
        let status = self.status();
        if status == PluginStatus::Ready {
            return Ok(());
        }
        if status == PluginStatus::Disabled {
            return Err(format!("plugin `{}` is disabled", self.name));
        }
        // Reap anything left over from a previous incarnation.
        self.terminate().await;
        self.spawn_and_handshake().await
    }

    /// Used by tool calls: start (or restart, at most [`MAX_RESTARTS`] times)
    /// when the process is known to be dead.
    pub(crate) async fn ensure_ready(self: &Arc<Self>) -> Result<(), String> {
        match self.status() {
            PluginStatus::Ready => return Ok(()),
            PluginStatus::Disabled => {
                return Err(format!(
                    "plugin `{}` is disabled; enable it in the plugin settings to use its tools",
                    self.name
                ))
            }
            _ => {}
        }
        {
            let mut inner = self.lock();
            if inner.restart_count >= MAX_RESTARTS {
                let reason = inner
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".into());
                return Err(format!(
                    "plugin `{}` is not running ({reason}) and has already been restarted \
                     {MAX_RESTARTS} times; fix the plugin or restart the server",
                    self.name
                ));
            }
            inner.restart_count += 1;
        }
        debug!(plugin = %self.name, "restarting plugin after a failed call");
        self.start().await
    }

    /// Graceful stop: `shutdown` request, then SIGKILL, then status `Stopped`.
    pub(crate) async fn stop(self: &Arc<Self>) {
        let _guard = self.start_lock.lock().await;
        self.stop_locked(PluginStatus::Stopped).await;
    }

    /// Stop and mark the slot disabled (used by `set_enabled(name, false)`).
    pub(crate) async fn disable(self: &Arc<Self>) {
        let _guard = self.start_lock.lock().await;
        self.stop_locked(PluginStatus::Disabled).await;
    }

    async fn stop_locked(&self, final_status: PluginStatus) {
        if self.status() == PluginStatus::Ready {
            let _ = self
                .request(methods::SHUTDOWN, json!({}), self.timeouts.shutdown())
                .await;
        }
        self.terminate().await;
        let mut inner = self.lock();
        inner.status = final_status;
        inner.error = None;
        inner.tools.clear();
        inner.panels.clear();
        inner.manifest = None;
        inner.pid = None;
    }

    /// Kill the child (if any) and wait for the supervisor task to finish.
    async fn terminate(&self) {
        let (kill, supervisor) = {
            let mut inner = self.lock();
            inner.shutting_down = true;
            inner.out = None;
            (inner.kill.take(), inner.supervisor.take())
        };
        if let Some(tx) = kill {
            let _ = tx.send(());
        }
        if let Some(handle) = supervisor {
            let _ = handle.await;
        }
        let pending = {
            let mut inner = self.lock();
            std::mem::take(&mut inner.pending)
        };
        let msg = format!("plugin `{}` was stopped", self.name);
        for (_, tx) in pending {
            let _ = tx.send(Reply::Transport(msg.clone()));
        }
    }

    /// Stop the child, then record a startup failure.
    async fn abort(&self, message: String) {
        self.terminate().await;
        self.fail(message);
    }

    async fn spawn_and_handshake(self: &Arc<Self>) -> Result<(), String> {
        let cfg = &self.config;
        if cfg.command.is_empty() {
            let msg = format!(
                "plugin `{}` has no command configured: set `command = [\"program\", \"args\"]` \
                 in the [[plugins]] entry (config.toml) or in {}/plugins/{}/plugin.toml",
                self.name, "<data_dir>", self.name
            );
            self.fail(msg.clone());
            return Err(msg);
        }

        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let mut command = Command::new(&cfg.command[0]);
        command.args(&cfg.command[1..]);
        // The child gets a cleared environment plus an allowlist: the server
        // process holds `KARUCODE_AUTH_TOKEN` and the operator's provider API
        // keys, and a plugin must not inherit them.
        apply_env_allowlist(&mut command);
        command.env("KARUCODE_PLUGIN_NAME", &self.name);
        command.env("KARUCODE_PLUGIN_PROTOCOL", PLUGIN_PROTOCOL_VERSION);
        if !cfg.permissions.is_empty() {
            command.env("KARUCODE_PLUGIN_PERMISSIONS", cfg.permissions.join(","));
        }
        if !cfg.settings.is_empty() {
            if let Ok(settings) = serde_json::to_string(&cfg.settings) {
                command.env("KARUCODE_PLUGIN_SETTINGS", settings);
            }
        }
        // Applied last so the operator's explicit table wins over everything
        // above, including the `KARUCODE_PLUGIN_*` variables.
        for (k, v) in &cfg.env {
            command.env(k, v);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own session => own process group, so teardown reaches every helper the
        // plugin starts (`kill_on_drop` and `start_kill` only signal the leader).
        // See `crate::procgroup`.
        crate::procgroup::detach_into_own_group(&mut command);

        debug!(plugin = %self.name, command = ?cfg.command, "spawning plugin");
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let msg = format!(
                    "failed to start plugin `{}` with command `{}`: {e}. Check that the program \
                     exists, is executable and is on PATH.",
                    self.name,
                    cfg.command.join(" ")
                );
                self.fail(msg.clone());
                return Err(msg);
            }
        };
        let Some(pid) = child.id() else {
            let msg = format!(
                "spawned plugin `{}` has no pid; cannot manage its process group",
                self.name
            );
            let _ = child.start_kill();
            self.fail(msg.clone());
            return Err(msg);
        };
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let msg = format!("could not capture stdin of plugin `{}`", self.name);
                crate::procgroup::signal_group(pid, crate::procgroup::SIGKILL);
                let _ = child.start_kill();
                self.fail(msg.clone());
                return Err(msg);
            }
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let msg = format!("could not capture stdout of plugin `{}`", self.name);
                crate::procgroup::signal_group(pid, crate::procgroup::SIGKILL);
                let _ = child.start_kill();
                self.fail(msg.clone());
                return Err(msg);
            }
        };
        let stderr = child.stderr.take();

        let (out_tx, out_rx) = mpsc::channel::<String>(OUT_QUEUE_DEPTH);
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        {
            let mut inner = self.lock();
            inner.status = PluginStatus::Starting;
            inner.error = None;
            inner.tools.clear();
            inner.panels.clear();
            inner.manifest = None;
            inner.pid = Some(pid);
            inner.out = Some(out_tx);
            inner.kill = Some(kill_tx);
            inner.shutting_down = false;
            inner.pending.clear();
        }

        self.spawn_writer(out_rx, stdin);
        if let Some(stderr) = stderr {
            self.spawn_stderr(stderr);
        }
        self.spawn_reader(epoch, stdout);
        let supervisor = self.spawn_supervisor(epoch, pid, child, kill_rx);
        self.lock().supervisor = Some(supervisor);

        match self.handshake().await {
            Ok(()) => Ok(()),
            Err(msg) => {
                let msg = format!("plugin `{}` failed to start: {msg}", self.name);
                self.abort(msg.clone()).await;
                Err(msg)
            }
        }
    }

    async fn handshake(&self) -> Result<(), String> {
        let budget = self.timeouts.handshake();
        let init = self
            .request(
                methods::INITIALIZE,
                json!({ "protocol": PLUGIN_PROTOCOL_VERSION }),
                budget,
            )
            .await
            .map_err(|e| format!("the `initialize` handshake failed: {}", e.describe()))?;
        let manifest: PluginManifest = serde_json::from_value(init)
            .map_err(|e| format!("the `initialize` result is not a valid plugin manifest: {e}"))?;
        if manifest.name != self.name {
            return Err(format!(
                "the plugin reported the name `{}` but the host expects `{}`; rename the plugin or \
                 its [[plugins]].name / plugin.toml entry so they match",
                manifest.name, self.name
            ));
        }
        if manifest.protocol != PLUGIN_PROTOCOL_VERSION {
            return Err(format!(
                "the plugin speaks protocol `{}` but this host speaks `{}`",
                manifest.protocol, PLUGIN_PROTOCOL_VERSION
            ));
        }
        let list = self
            .request(methods::TOOLS_LIST, json!({}), budget)
            .await
            .map_err(|e| format!("`tools/list` failed: {}", e.describe()))?;
        let tools = parse_tools(&self.name, list)?;
        // Panels are validated against the tools that actually exist, so a
        // declaration that names an unknown tool is a discovery error the
        // operator can read in `PluginDiagnostics.error`, not a crash and not a
        // tab that fails every time it is opened.
        let panels = parse_panels(&self.name, &manifest, &tools)?;
        let mut inner = self.lock();
        inner.manifest = Some(PluginManifest {
            panels: panels.clone(),
            ..manifest
        });
        inner.panels = panels;
        inner.tools = tools;
        inner.status = PluginStatus::Ready;
        inner.error = None;
        Ok(())
    }

    fn spawn_writer(&self, mut rx: mpsc::Receiver<String>, mut stdin: ChildStdin) {
        let name = self.name.clone();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
            let _ = stdin.shutdown().await;
            debug!(plugin = %name, "plugin stdin closed");
        });
    }

    fn spawn_stderr(&self, stderr: ChildStderr) {
        let name = self.name.clone();
        tokio::spawn(async move {
            // `OverLong::Truncate`: an over-long stderr line is trimmed and the
            // drain continues. Closing the read end here used to make the
            // plugin's next write fail with `EPIPE` and take the whole process
            // down, which is exactly what must not happen on a log channel.
            let result = pump_lines(stderr, MAX_LOG_LINE, OverLong::Truncate, |line| {
                debug!(plugin = %name, "stderr: {line}");
            })
            .await;
            match result {
                Ok(report) if !report.is_empty() => {
                    // One line for the whole run rather than one per offending
                    // line: a plugin in a loop must not be able to flood the log.
                    debug!(
                        plugin = %name,
                        "{}",
                        report.describe(MAX_LOG_LINE, "the plugin's stderr")
                    );
                }
                Ok(_) => {}
                Err(e) => debug!(plugin = %name, "stopped draining plugin stderr: {e}"),
            }
        });
    }

    fn spawn_reader(self: &Arc<Self>, epoch: u64, stdout: ChildStdout) {
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            // `OverLong::Fail`: an over-long *protocol* line is not JSON and
            // cannot be recovered, so that peer's in-flight requests are failed
            // instead of waiting for an answer that will never come. stderr is
            // truncated instead — see `spawn_stderr`.
            let result = pump_lines(stdout, MAX_PROTOCOL_LINE, OverLong::Fail, |line| {
                shared.handle_line(line)
            })
            .await;
            let protocol_error = match result {
                Ok(_) => None,
                Err(e) => {
                    warn!(plugin = %shared.name, "reading plugin stdout stopped: {e}");
                    Some(format!("plugin `{}` broke the protocol: {e}", shared.name))
                }
            };
            shared.on_stdout_closed(epoch, protocol_error);
        });
    }

    fn spawn_supervisor(
        self: &Arc<Self>,
        epoch: u64,
        pid: u32,
        mut child: Child,
        kill_rx: oneshot::Receiver<()>,
    ) -> JoinHandle<()> {
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            // The group guard is the backstop: if this task is aborted (or the
            // runtime drops it) before either arm finishes, the plugin's helpers
            // must still not survive.
            let _group = crate::procgroup::guard(pid);
            let status = tokio::select! {
                status = child.wait() => {
                    // The leader exited on its own. Its helpers, if any, are
                    // still in the group and would otherwise keep the pipes open
                    // (and outlive the plugin).
                    crate::procgroup::kill_group_after_exit(pid);
                    status
                }
                _ = kill_rx => crate::procgroup::terminate_group(&mut child, pid).await,
            };
            shared.on_exit(epoch, status);
        })
    }

    fn handle_line(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                warn!(
                    plugin = %self.name,
                    "ignoring invalid JSON on plugin stdout ({}); plugins must log to stderr",
                    truncate(line, 200)
                );
                return;
            }
        };
        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
            let waiter = self.lock().pending.remove(&id);
            let reply = match (msg.get("error"), msg.get("result")) {
                (Some(err), _) if !err.is_null() => Reply::Remote(parse_error(err)),
                (None, Some(result)) => Reply::Ok(result.clone()),
                _ => Reply::Transport(format!(
                    "plugin `{}` sent a malformed response for request {id}",
                    self.name
                )),
            };
            match waiter {
                Some(tx) => {
                    let _ = tx.send(reply);
                }
                None => debug!(plugin = %self.name, "response for unknown request id {id}"),
            }
            return;
        }
        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
            let params = msg
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            // `notify` is the one notification with a meaning beyond logging: it is
            // how a plugin asks a human to look at something. It is handled before
            // the generic debug line so a plugin raising a notification does not
            // look like a protocol curiosity in the log.
            if method == kc_core::plugin::methods::NOTIFY {
                self.handle_notify(params);
                return;
            }
            debug!(
                plugin = %self.name,
                method,
                params = %truncate(&params.to_string(), 300),
                "plugin notification"
            );
            return;
        }
        debug!(plugin = %self.name, "unrecognised plugin message: {}", truncate(line, 200));
    }

    /// Handle an inbound `notify` frame.
    ///
    /// Everything about this method is defensive, because the input is another
    /// process: a malformed frame is logged and dropped, never a panic, never a
    /// host failure, and never a partially-valid notification. The caps that
    /// [`kc_core::notify::Notify::new`] applies are applied again here so an
    /// over-long title cannot even reach the channel.
    fn handle_notify(&self, params: Value) {
        let Some(object) = params.as_object() else {
            debug!(plugin = %self.name, "ignoring a notify frame whose params are not an object");
            return;
        };
        let text = |key: &str| {
            object
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let kind_name = text("kind");
        let Some(kind) = kc_core::notify::NotifyKind::parse(&kind_name) else {
            // An unknown kind is a plugin bug, not something to guess at: guessing
            // would let a plugin choose a cue it was never given.
            debug!(
                plugin = %self.name,
                kind = %truncate(&kind_name, 40),
                "ignoring a notify frame with an unknown kind"
            );
            return;
        };
        let title = text("title");
        if title.is_empty() {
            debug!(plugin = %self.name, "ignoring a notify frame with no title");
            return;
        }
        let notice = PluginNotice {
            plugin: self.name.clone(),
            kind,
            title: kc_core::notify::truncate_chars(&title, kc_core::notify::MAX_NOTIFY_TITLE_CHARS),
            body: kc_core::notify::truncate_chars(
                &text("body"),
                kc_core::notify::MAX_NOTIFY_BODY_CHARS,
            ),
            session_id: text("session_id"),
            sound: object
                .get("sound")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    kc_core::notify::truncate_chars(s, kc_core::notify::MAX_NOTIFY_SOUND_CHARS)
                }),
            sticky: object
                .get("sticky")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        };

        if !self.spend_notify_budget() {
            return;
        }
        let tx = self.notify_tx.lock().unwrap_or_else(|e| e.into_inner());
        match tx.as_ref() {
            Some(tx) => {
                if tx.send(notice).is_err() {
                    debug!(plugin = %self.name, "notification listener is gone; dropping notify");
                }
            }
            // No listener: the host was built without one (a unit test) or the
            // server has not wired it yet. Silent, not an error.
            None => debug!(plugin = %self.name, "no notification listener; dropping notify"),
        }
    }

    /// Take one notification's worth of credit, or refuse.
    fn spend_notify_budget(&self) -> bool {
        let now = crate::now_ms();
        let mut budget = self.notify_budget.lock().unwrap_or_else(|e| e.into_inner());
        if budget.last_refill_ms == 0 {
            budget.last_refill_ms = now;
        }
        let elapsed = (now - budget.last_refill_ms).max(0) as f64;
        budget.credit_ms = (budget.credit_ms + elapsed * NOTIFY_REFILL_PER_MS).min(NOTIFY_BURST_MS);
        budget.last_refill_ms = now;
        if budget.credit_ms >= NOTIFY_COST_MS {
            budget.credit_ms -= NOTIFY_COST_MS;
            return true;
        }
        if !budget.warned {
            budget.warned = true;
            warn!(
                plugin = %self.name,
                "dropping notify frames: the plugin is raising notifications faster than \
                 one per second (burst of {})",
                (NOTIFY_BURST_MS / NOTIFY_COST_MS) as u32
            );
        }
        false
    }

    /// stdout closed: the process is exiting (or has already exited). Close the
    /// write side immediately, then give the supervisor a short grace period to
    /// reap the child so pending requests can report the real exit status. If the
    /// process is still alive after the grace period, fail what is left.
    ///
    /// `protocol_error` is `Some` when the reader stopped because the plugin
    /// broke the framing contract (an over-long line, a read error) rather than
    /// because it exited. Such a plugin is not usable, so the kill sender is
    /// fired and the supervisor reaps it — mirroring `fail_protocol` in the MCP
    /// stdio transport. Without this the process would linger forever.
    fn on_stdout_closed(self: &Arc<Self>, epoch: u64, protocol_error: Option<String>) {
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let kill = {
            let mut inner = self.lock();
            inner.out = None;
            if let Some(message) = &protocol_error {
                inner.error = Some(message.clone());
            }
            // Only a protocol violation kills the child here; a plain EOF means
            // the process is already on its way out and the supervisor will see
            // the exit on its own.
            if protocol_error.is_some() {
                inner.kill.take()
            } else {
                None
            }
        };
        if let Some(tx) = kill {
            let _ = tx.send(());
        }
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if shared.epoch.load(Ordering::SeqCst) != epoch {
                return;
            }
            let msg = protocol_error
                .unwrap_or_else(|| format!("plugin `{}` closed its stdout", shared.name));
            let pending_left = {
                let mut inner = shared.lock();
                if inner.shutting_down {
                    return;
                }
                if !inner.pending.is_empty() {
                    inner.status = PluginStatus::Failed;
                    inner.error = Some(msg.clone());
                    inner.tools.clear();
                    inner.panels.clear();
                    inner.manifest = None;
                }
                inner.pending.is_empty()
            };
            if !pending_left {
                shared.fail_pending(&msg);
            }
            debug!(plugin = %shared.name, "plugin stdout closed");
        });
    }

    fn on_exit(&self, epoch: u64, status: std::io::Result<ExitStatus>) {
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let detail = match &status {
            Ok(s) => format!("process exited ({s})"),
            Err(e) => format!("waiting for the process failed: {e}"),
        };
        // Update the slot *before* waking the waiters, so a caller that sees the
        // failure also sees a consistent `failed` status.
        let pending = {
            let mut inner = self.lock();
            inner.out = None;
            inner.kill = None;
            inner.pid = None;
            inner.supervisor = None;
            inner.tools.clear();
            inner.panels.clear();
            inner.manifest = None;
            if inner.shutting_down {
                inner.status = PluginStatus::Stopped;
            } else {
                inner.status = PluginStatus::Failed;
                // Keep a more specific earlier error (e.g. the protocol
                // violation that caused the kill) instead of replacing it with
                // the generic exit message.
                if inner.error.is_none() {
                    inner.error = Some(match &status {
                        Ok(s) => match s.code() {
                            Some(code) => format!("plugin process exited with exit code {code}"),
                            None => format!("plugin process was terminated by a signal ({s})"),
                        },
                        Err(e) => format!("could not wait for the plugin process: {e}"),
                    });
                }
            }
            std::mem::take(&mut inner.pending)
        };
        let msg = format!("plugin `{}` {detail}", self.name);
        for (_, tx) in pending {
            let _ = tx.send(Reply::Transport(msg.clone()));
        }
        debug!(plugin = %self.name, "plugin process finished: {detail}");
    }

    fn fail_pending(&self, message: &str) {
        let pending = {
            let mut inner = self.lock();
            std::mem::take(&mut inner.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(Reply::Transport(message.to_string()));
        }
    }
}

fn parse_error(value: &Value) -> PluginError {
    let code = value.get("code").and_then(|c| c.as_i64()).unwrap_or(-32000) as i32;
    let message = value
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unspecified plugin error")
        .to_string();
    PluginError {
        code,
        message,
        data: value.get("data").cloned(),
    }
}

fn parse_tools(plugin: &str, value: Value) -> Result<Vec<PluginToolDef>, String> {
    let raw = value.get("tools").cloned().unwrap_or_else(|| json!([]));
    let defs: Vec<PluginToolDef> = serde_json::from_value(raw)
        .map_err(|e| format!("`tools/list` returned a malformed tool list: {e}"))?;
    for def in &defs {
        if def.name.trim().is_empty() {
            return Err(format!(
                "plugin `{plugin}` advertised a tool with an empty name"
            ));
        }
        if def.name.contains(char::is_whitespace) {
            return Err(format!(
                "plugin `{plugin}` advertised the tool `{}`; tool names may not contain whitespace",
                def.name
            ));
        }
    }
    Ok(defs)
}

/// Validate the panel declarations of a manifest.
///
/// Rejected (as a handshake error, so it lands in `PluginDiagnostics.error`):
///
/// * a panel id that is empty or contains characters unusable in a URL path
///   segment or a DOM id;
/// * two panels that collide after namespacing — the same `id` twice (Rust
///   would silently keep one, and the tab order would depend on serde);
/// * a `source` that is neither `tool` nor `static`;
/// * `source = "tool"` naming a tool that `tools/list` did not advertise.
///
/// A panel whose `title` is blank is *not* an error: the host falls back to the
/// id, because a missing label is cosmetic while the other cases are a broken
/// tab.
pub(crate) fn parse_panels(
    plugin: &str,
    manifest: &PluginManifest,
    tools: &[PluginToolDef],
) -> Result<Vec<PluginPanel>, String> {
    let mut out: Vec<PluginPanel> = Vec::with_capacity(manifest.panels.len());
    for panel in &manifest.panels {
        if !valid_panel_id(&panel.id) {
            return Err(format!(
                "plugin `{plugin}` declared the panel `{}`; a panel id must be non-empty and may \
                 contain only lowercase ASCII letters, digits, `-`, `_` and `.`",
                panel.id
            ));
        }
        if out.iter().any(|existing| existing.id == panel.id) {
            return Err(format!(
                "plugin `{plugin}` declared the panel id `{}` twice; panel ids must be unique \
                 within a plugin (they are hosted as `plugin__{plugin}__<id>`)",
                panel.id
            ));
        }
        let source = panel.source.trim();
        if source != PANEL_SOURCE_TOOL && source != PANEL_SOURCE_STATIC {
            return Err(format!(
                "plugin `{plugin}` declared panel `{}` with source `{source}`; the supported \
                 sources are `{PANEL_SOURCE_TOOL}` and `{PANEL_SOURCE_STATIC}`",
                panel.id
            ));
        }
        if source == PANEL_SOURCE_TOOL {
            match panel.tool.as_deref().map(str::trim) {
                None | Some("") => {
                    // No tool is fine: `panels/read` is the primary path and an
                    // empty payload is better than refusing to show the tab.
                }
                Some(tool) => {
                    if !tools.iter().any(|def| def.name == tool) {
                        return Err(format!(
                            "plugin `{plugin}` declared panel `{}` with tool `{tool}`, which it \
                             does not advertise in `tools/list`; declare the tool or point the \
                             panel at `source = \"{PANEL_SOURCE_STATIC}\"`",
                            panel.id
                        ));
                    }
                }
            }
        }
        out.push(PluginPanel {
            title: if panel.title.trim().is_empty() {
                panel.id.clone()
            } else {
                panel.title.clone()
            },
            refresh_ms: kc_core::plugin::clamp_refresh_ms(panel.refresh_ms),
            ..panel.clone()
        });
    }
    Ok(out)
}

/// How many outgoing protocol lines may be queued for the writer task.
///
/// Bounded on purpose: a plugin that stops reading its stdin fills the OS pipe
/// buffer, the writer blocks in `write_all`, and an unbounded queue would then
/// grow until the server runs out of memory. When the queue is full the request
/// fails with `RpcError::Transport("plugin is not reading its stdin")` instead.
const OUT_QUEUE_DEPTH: usize = 64;

/// Environment variables a plugin inherits from karucode.
///
/// The child gets a **cleared** environment plus this allowlist. The server
/// process holds `KARUCODE_AUTH_TOKEN` and the operator's provider API keys, and
/// a plugin (or a `postinstall` script of something it launches) must not
/// receive them. Everything listed here is what ordinary tooling needs to start
/// at all; the `KARUCODE_PLUGIN_*` variables and the configured `env` table are
/// added separately by the caller.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TERM", "TMPDIR", "PWD", "TZ",
];

/// Whether an environment variable name looks like it carries a credential.
///
/// Defence in depth on top of [`ENV_ALLOWLIST`]. Matching is case-insensitive
/// and anchored on `_` boundaries, so `MY_TOKENIZER` is not mistaken for a token
/// while `GITHUB_TOKEN` and `OPENAI_API_KEY` are caught.
///
/// Duplicated from `kc-mcp/src/stdio.rs` on purpose: the two crates share no
/// module and `kc-core` exposes no equivalent helper. Keep the copies in step.
fn is_secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const SECRET_PREFIXES: &[&str] = &["KARUCODE_", "AWS_", "OPENAI_", "ANTHROPIC_"];
    const SECRET_NAMES: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "NPM_TOKEN"];
    const SECRET_SUFFIXES: &[&str] = &["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];

    SECRET_PREFIXES.iter().any(|p| upper.starts_with(p))
        || SECRET_NAMES.contains(&upper.as_str())
        || SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// Clear the inherited environment and add only the allowlisted variables.
///
/// The configured `env` table is applied by the caller *after* this, so it wins
/// over the allowlist and is deliberately **not** filtered by
/// [`is_secret_like`]: a variable an operator explicitly configured for that
/// plugin is an intentional choice (that is how a plugin receives its own
/// credentials), unlike an inherited one.
fn apply_env_allowlist(command: &mut Command) {
    command.env_clear();
    for name in ENV_ALLOWLIST {
        if is_secret_like(name) {
            continue;
        }
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

/// Largest protocol line the host will buffer from a plugin. A plugin that
/// writes more than this is broken (or hostile); the host closes the channel
/// instead of growing without bound.
const MAX_PROTOCOL_LINE: usize = 8 * 1024 * 1024;
/// Largest stderr line kept for logging.
const MAX_LOG_LINE: usize = 64 * 1024;

/// What to do with a line that exceeds the reader's cap.
///
/// The two streams are not symmetric, and pretending otherwise is what caused the
/// bug this enum exists to document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverLong {
    /// A **log** stream (stderr). Truncate the line to the cap, hand the head to
    /// `on_line`, throw the rest away as it arrives, and keep reading.
    ///
    /// This must not be fatal: the plugin's stderr pipe is a diagnostic channel,
    /// and a single verbose line was enough to stop the drain for good. Once the
    /// read end was closed the plugin's next stderr write failed with `EPIPE`,
    /// which killed the process, and every later tool call reported "process
    /// stopped or broke the protocol".
    Truncate,
    /// A **protocol** stream (stdout). A line past the cap cannot be JSON, so the
    /// peer is not speaking the framing contract: stop with `InvalidData` and let
    /// the caller fail that peer's in-flight requests. A truncated protocol line
    /// could never be parsed, and silently discarding it would leave the caller
    /// waiting for a response that will never come.
    Fail,
}

/// What [`pump_lines`] found beyond an ordinary end of stream.
///
/// Only an over-long line is reported, and it is reported **after** the reader
/// has been kept draining: it is news for the log, not a reason to stop.
#[derive(Debug, Default, PartialEq, Eq)]
struct PumpReport {
    /// Complete lines that exceeded the per-line cap. Their text was truncated
    /// to the cap and handed to `on_line` anyway; the rest of each line was
    /// discarded without ever being buffered.
    over_long_lines: u64,
}

impl PumpReport {
    fn is_empty(&self) -> bool {
        self.over_long_lines == 0
    }

    /// Describe the over-long lines as one line of log text.
    fn describe(&self, max_line: usize, what: &str) -> String {
        format!(
            "{what} sent {} line(s) longer than the {max_line} byte limit; each was truncated to \
             that limit and the remainder discarded, and reading continued",
            self.over_long_lines
        )
    }
}

/// Read `reader` line by line, calling `on_line` for each complete line, with a
/// per-line cap that is **not** fatal.
///
/// Chunked read instead of `BufReader::lines` so one endless line cannot exhaust
/// memory: bytes past `max_line` are dropped as they arrive rather than
/// buffered.
///
/// An over-long line is truncated to `max_line` (on a UTF-8 boundary) and passed
/// to `on_line`, the rest of it is discarded, and reading **continues**. Making it
/// fatal was a bug: a single 64 KiB stderr line stopped the drain, the closed
/// read end then made the child's next write fail with `EPIPE`, and the process
/// died — after which every later tool call failed with "process stopped or broke
/// the protocol", for the rest of the plugin's life. A plugin with verbose logging
/// must not be able to kill itself that way, so the line is reported back in a
/// [`PumpReport`] (for one log line, not one per offending line) and the pipe
/// keeps draining.
///
/// The only `Err` is a genuine read failure from the OS.
async fn pump_lines<R, F>(
    mut reader: R,
    max_line: usize,
    over_long: OverLong,
    mut on_line: F,
) -> std::io::Result<PumpReport>
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(&str),
{
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // Only scan bytes that were appended since the last newline search; a long
    // line would otherwise be rescanned on every read (quadratic).
    let mut searched: usize = 0;
    let mut report = PumpReport::default();
    // Set when the current line already hit the cap and the rest of it must be
    // thrown away until its newline arrives.
    let mut discarding = false;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if discarding {
            // Only look for the end of the line; never buffer its tail.
            match chunk[..read].iter().position(|b| *b == b'\n') {
                Some(pos) => {
                    discarding = false;
                    // The cap-sized prefix was emitted when it filled up.
                    pending.clear();
                    searched = 0;
                    pending.extend_from_slice(&chunk[pos + 1..read]);
                }
                None => continue,
            }
        } else {
            pending.extend_from_slice(&chunk[..read]);
        }
        while let Some(rel) = pending[searched..].iter().position(|b| *b == b'\n') {
            let end = searched + rel + 1;
            let line: Vec<u8> = pending.drain(..end).collect();
            on_line(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']));
            searched = 0;
        }
        searched = pending.len();
        if pending.len() > max_line {
            if over_long == OverLong::Fail {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "line exceeds {max_line} bytes; the process is not speaking the protocol"
                    ),
                ));
            }
            // Truncate to the cap on a char boundary, hand it over, and drop the
            // rest of the line as it arrives. `searched` is reset because the
            // buffer is emptied here; the next newline clears `discarding`.
            let cut = floor_char_boundary(&pending, max_line);
            let head = String::from_utf8_lossy(&pending[..cut]).into_owned();
            on_line(head.trim_end_matches(['\n', '\r']));
            pending.clear();
            searched = 0;
            discarding = true;
            report.over_long_lines += 1;
        }
    }
    if !discarding && !pending.is_empty() {
        // A final line with no trailing newline still counts, unless it was the
        // truncated tail of an over-long one (already reported above).
        on_line(String::from_utf8_lossy(&pending).trim_end_matches(['\n', '\r']));
    }
    Ok(report)
}

/// Largest index `<= max` that is a UTF-8 character boundary in `bytes`.
fn floor_char_boundary(bytes: &[u8], max: usize) -> usize {
    let mut i = max.min(bytes.len());
    while i > 0 && !str_boundary(bytes, i) {
        i -= 1;
    }
    i
}

/// Whether `index` is a valid slice boundary in `bytes`.
fn str_boundary(bytes: &[u8], index: usize) -> bool {
    index == 0 || index >= bytes.len() || (bytes[index] & 0xC0) != 0x80
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_refs_apply() {
        let refs = TimeoutRefs::default();
        assert_eq!(refs.call(), Duration::from_secs(60));
        refs.apply(&PluginTimeouts {
            call: Duration::from_millis(1500),
            ..PluginTimeouts::default()
        });
        assert_eq!(refs.call(), Duration::from_millis(1500));
    }

    #[test]
    fn rpc_error_messages_are_actionable() {
        let e = RpcError::Timeout(Duration::from_secs(60));
        let msg = e.tool_message("demo", "hello", Duration::from_secs(60));
        assert!(msg.contains("timed out after 60s"), "{msg}");
        assert!(msg.contains("demo"), "{msg}");
        let e = RpcError::Remote(PluginError::new(-32601, "unknown tool `x`"));
        let msg = e.tool_message("demo", "x", Duration::from_secs(60));
        assert!(msg.contains("unknown tool `x`"), "{msg}");
        assert!(msg.contains("code -32601"), "{msg}");
    }

    #[test]
    fn tool_list_validation_rejects_bad_names() {
        let v = json!({"tools": [{"name": "", "description": "x"}]});
        assert!(parse_tools("p", v).is_err());
        let v = json!({"tools": [{"name": "two words", "description": "x"}]});
        assert!(parse_tools("p", v).is_err());
        let v = json!({"tools": [{"name": "ok", "description": "x"}]});
        assert_eq!(parse_tools("p", v).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pump_lines_splits_lines_and_caps_length() {
        let input = b"{\"a\":1}\n{\"b\":2}\n\nlast";
        let mut lines = Vec::new();
        let mut cursor = std::io::Cursor::new(input.to_vec());
        let report = pump_lines(&mut cursor, 1024, OverLong::Fail, |line| {
            lines.push(line.to_string())
        })
        .await
        .unwrap();
        assert!(report.is_empty());
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}", "", "last"]);

        // On the protocol stream an endless line is refused instead of buffered
        // forever: a truncated protocol line could never be parsed.
        let mut grow = vec![b'x'; 64 * 1024];
        grow.extend_from_slice(b"\n");
        let mut cursor = std::io::Cursor::new(grow);
        let err = pump_lines(&mut cursor, 1024, OverLong::Fail, |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    /// The bug FIX 2 is about: one over-long stderr line used to end the drain
    /// for good. It must be truncated and discarded instead, and the lines after
    /// it must still arrive — in the same read chunk and in later ones.
    #[tokio::test]
    async fn an_over_long_log_line_is_truncated_and_the_drain_continues() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"before\n");
        // One 200 KiB line with no newline for a long stretch, then more lines.
        input.extend(std::iter::repeat_n(b'y', 200 * 1024));
        input.extend_from_slice(b"\nafter-one\n");
        // A second over-long line, to prove the state resets between them.
        input.extend(std::iter::repeat_n(b'z', 150 * 1024));
        input.extend_from_slice(b"\nafter-two\n");
        // A third that is over-long and also the last thing written (no newline).
        input.extend(std::iter::repeat_n(b'w', 100 * 1024));

        let mut lines: Vec<String> = Vec::new();
        let mut cursor = std::io::Cursor::new(input);
        let report = pump_lines(&mut cursor, 1024, OverLong::Truncate, |line| {
            lines.push(line.to_string())
        })
        .await
        .unwrap();

        assert_eq!(report.over_long_lines, 3, "{report:?}");
        assert_eq!(lines.len(), 6, "{:?}", lines.iter().map(|l| l.len()));
        assert_eq!(lines[0], "before");
        assert_eq!(lines[1].len(), 1024, "the head is capped, not dropped");
        assert!(lines[1].bytes().all(|b| b == b'y'));
        assert_eq!(lines[2], "after-one");
        assert_eq!(lines[3].len(), 1024);
        assert!(lines[3].bytes().all(|b| b == b'z'));
        assert_eq!(lines[4], "after-two");
        // The trailing line never saw a newline: it was truncated when it hit the
        // cap, and emitting it once is the whole of its output.
        assert_eq!(lines[5].len(), 1024);
        assert!(lines[5].bytes().all(|b| b == b'w'));

        // A single endless line still terminates (the reader sees EOF) rather
        // than hanging or erroring.
        let mut endless = vec![b'q'; 512 * 1024];
        let mut cursor = std::io::Cursor::new(std::mem::take(&mut endless));
        let mut count = 0;
        let report = pump_lines(&mut cursor, 256, OverLong::Truncate, |_| count += 1)
            .await
            .unwrap();
        assert_eq!(report.over_long_lines, 1);
        assert_eq!(count, 1);
    }

    /// The truncation point must land on a UTF-8 boundary, or the plugin's UTF-8
    /// log line would come out as replacement characters.
    #[tokio::test]
    async fn an_over_long_utf8_log_line_is_truncated_on_a_char_boundary() {
        // "é" is two bytes: a cap of 1025 would split the 513th one. The line is
        // long enough to arrive with no newline in the first read, which is the
        // only path that truncates (a complete line is delivered whole).
        let mut input: Vec<u8> = Vec::new();
        input.extend("é".repeat(20_000).into_bytes());
        input.push(b'\n');
        input.extend_from_slice(b"tail\n");
        let mut lines: Vec<String> = Vec::new();
        let report = pump_lines(
            &mut std::io::Cursor::new(input),
            1025,
            OverLong::Truncate,
            |line| lines.push(line.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(report.over_long_lines, 1);
        assert_eq!(lines.len(), 2, "{:?}", lines.iter().map(|l| l.len()));
        assert!(
            !lines[0].contains('\u{FFFD}'),
            "truncation split a character: {:?}",
            &lines[0][lines[0].len() - 8..]
        );
        assert_eq!(lines[0].len(), 1024);
        // The line after the truncated one still arrives: the drain continued.
        assert_eq!(lines[1], "tail");
    }

    #[test]
    fn parse_error_falls_back_to_generic_message() {
        let e = parse_error(&json!({"code": -1}));
        assert_eq!(e.code, -1);
        assert!(e.message.contains("unspecified"));
    }

    fn shared_with_queue() -> (Arc<PluginShared>, mpsc::Receiver<String>) {
        let config = PluginConfig {
            name: "p".into(),
            enabled: true,
            command: vec!["true".into()],
            env: Default::default(),
            permissions: Vec::new(),
            settings: Default::default(),
        };
        let shared = PluginShared::new(
            config,
            Arc::new(TimeoutRefs::default()),
            PluginStatus::Ready,
        );
        let (tx, rx) = mpsc::channel(OUT_QUEUE_DEPTH);
        shared.lock().out = Some(tx);
        (shared, rx)
    }

    #[tokio::test]
    async fn a_cancelled_request_leaves_no_pending_entry() {
        let (shared, _rx) = shared_with_queue();
        let token = CancellationToken::new();
        let waiter = {
            let shared = Arc::clone(&shared);
            let token = token.clone();
            tokio::spawn(async move {
                shared
                    .request_cancellable(
                        methods::TOOLS_CALL,
                        json!({}),
                        Duration::from_secs(30),
                        Some(&token),
                    )
                    .await
            })
        };
        // Let the request register itself, then cancel it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(shared.pending_len(), 1, "the request must be in flight");
        token.cancel();
        let result = waiter.await.unwrap();
        assert!(matches!(result, Err(RpcError::Cancelled)), "{result:?}");
        assert_eq!(
            shared.pending_len(),
            0,
            "a cancelled call must not leave a dead waiter behind"
        );
    }

    #[tokio::test]
    async fn a_dropped_request_future_leaves_no_pending_entry() {
        let (shared, _rx) = shared_with_queue();
        // `timeout` drops the inner future, which is what the old
        // `tokio::select!` in `PluginTool::invoke` did on cancellation.
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            shared.request(methods::TOOLS_CALL, json!({}), Duration::from_secs(30)),
        )
        .await;
        assert!(result.is_err(), "the outer timeout must fire");
        assert_eq!(
            shared.pending_len(),
            0,
            "dropping the future must remove the pending entry"
        );
    }

    #[tokio::test]
    async fn a_timed_out_request_leaves_no_pending_entry() {
        let (shared, _rx) = shared_with_queue();
        let result = shared
            .request(methods::TOOLS_CALL, json!({}), Duration::from_millis(20))
            .await;
        assert!(matches!(result, Err(RpcError::Timeout(_))), "{result:?}");
        assert_eq!(shared.pending_len(), 0);
    }

    #[tokio::test]
    async fn a_full_outgoing_queue_fails_instead_of_buffering() {
        let (shared, _rx) = shared_with_queue();
        // Fill the queue without ever draining it: the writer is "blocked".
        for _ in 0..OUT_QUEUE_DEPTH {
            shared
                .request(methods::TOOLS_CALL, json!({}), Duration::from_millis(1))
                .await
                .ok();
        }
        let result = shared
            .request(methods::TOOLS_CALL, json!({}), Duration::from_millis(1))
            .await;
        match result {
            Err(RpcError::Transport(m)) => {
                assert!(m.contains("not reading its stdin"), "{m}");
            }
            other => panic!("expected a transport error, got {other:?}"),
        }
        // The failed request must not be left in the map either.
        assert_eq!(shared.pending_len(), 0);
    }

    #[test]
    fn queue_errors_name_the_actual_problem() {
        let (tx, rx) = mpsc::channel::<String>(1);
        tx.try_send("x".into()).unwrap();
        let full = tx.try_send("y".into()).unwrap_err();
        let e = queue_error("p", full);
        assert!(e.describe().contains("not reading its stdin"), "{e:?}");
        drop(rx);
        let closed = tx.try_send("z".into()).unwrap_err();
        let e = queue_error("p", closed);
        assert!(e.describe().contains("closed its stdin"), "{e:?}");
    }

    #[test]
    fn secret_like_names_are_recognised() {
        for name in [
            "KARUCODE_AUTH_TOKEN",
            "KARUCODE_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "MY_TOKEN",
            "DB_PASSWORD",
            "SIGNING_SECRET",
        ] {
            assert!(is_secret_like(name), "{name} must be treated as a secret");
        }
        for name in ["PATH", "HOME", "LANG", "MY_TOKENIZER", "TERM"] {
            assert!(!is_secret_like(name), "{name} must not be filtered");
        }
    }

    #[test]
    fn the_env_allowlist_contains_no_secrets() {
        for name in ENV_ALLOWLIST {
            assert!(!is_secret_like(name), "{name} must not be allowlisted");
        }
        assert!(ENV_ALLOWLIST.contains(&"PATH"));
        assert!(!ENV_ALLOWLIST.contains(&"KARUCODE_AUTH_TOKEN"));
    }

    #[tokio::test]
    async fn a_protocol_violation_fires_the_kill_sender() {
        let (shared, _rx) = shared_with_queue();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        shared.lock().kill = Some(kill_tx);

        shared.on_stdout_closed(
            0,
            Some("plugin `p` broke the protocol: line exceeds 8388608 bytes".into()),
        );

        // The supervisor reaps the child when this fires; without it the plugin
        // process would linger forever after a framing violation.
        tokio::time::timeout(Duration::from_secs(1), kill_rx)
            .await
            .expect("the kill sender must fire on a protocol violation")
            .expect("the kill sender must not be dropped");
        assert!(
            shared.lock().kill.is_none(),
            "the kill sender must be consumed"
        );
        let err = shared.lock().error.clone().unwrap_or_default();
        assert!(err.contains("broke the protocol"), "{err}");
    }

    #[tokio::test]
    async fn a_plain_eof_does_not_kill_the_child() {
        let (shared, _rx) = shared_with_queue();
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        shared.lock().kill = Some(kill_tx);

        // A plain EOF means the process is already on its way out; the
        // supervisor sees the exit on its own and must not be raced by a kill.
        shared.on_stdout_closed(0, None);

        assert!(kill_rx.try_recv().is_err(), "no kill on a plain EOF");
        assert!(shared.lock().kill.is_some());
    }
}

#[cfg(test)]
mod notify_tests {
    //! The `notify` frame and its rate limiter, exercised without a subprocess.

    use super::*;

    fn shared() -> Arc<PluginShared> {
        let config = PluginConfig {
            name: "p".into(),
            enabled: true,
            command: vec!["true".into()],
            env: Default::default(),
            permissions: Vec::new(),
            settings: Default::default(),
        };
        PluginShared::new(
            config,
            Arc::new(TimeoutRefs::default()),
            PluginStatus::Ready,
        )
    }

    #[test]
    fn a_well_formed_frame_becomes_a_notice() {
        let slot = shared();
        let (tx, mut rx) = mpsc::unbounded_channel();
        slot.set_notify_sender(Some(tx));
        slot.handle_notify(json!({
            "kind": "needs_input",
            "title": "  Deploy needs a yes  ",
            "body": "staging is approved",
            "session_id": "s_1",
            "sound": "ping",
            "sticky": true,
        }));
        let notice = rx.try_recv().expect("a notice");
        assert_eq!(notice.plugin, "p");
        assert_eq!(notice.kind, kc_core::notify::NotifyKind::NeedsInput);
        assert_eq!(notice.title, "Deploy needs a yes", "trimmed");
        assert_eq!(notice.body, "staging is approved");
        assert_eq!(notice.session_id, "s_1");
        assert_eq!(notice.sound.as_deref(), Some("ping"));
        assert!(notice.sticky);
    }

    #[test]
    fn defaults_are_the_quiet_ones() {
        // A minimal frame must be legal: a plugin should not have to spell out
        // every field to say one thing.
        let slot = shared();
        let (tx, mut rx) = mpsc::unbounded_channel();
        slot.set_notify_sender(Some(tx));
        slot.handle_notify(json!({"kind": "info", "title": "hello"}));
        let notice = rx.try_recv().expect("a notice");
        assert_eq!(notice.body, "");
        assert_eq!(notice.session_id, "");
        assert_eq!(notice.sound, None);
        assert!(!notice.sticky);
    }

    #[test]
    fn malformed_frames_are_dropped_rather_than_guessed_at() {
        let cases = [
            json!("not an object"),
            json!({"title": "no kind"}),
            json!({"kind": "critical", "title": "unknown kind"}),
            json!({"kind": "info"}),
            json!({"kind": "info", "title": "   "}),
            json!({"kind": "info", "title": 42}),
            json!(null),
        ];
        for frame in cases {
            let slot = shared();
            let (tx, mut rx) = mpsc::unbounded_channel();
            slot.set_notify_sender(Some(tx));
            slot.handle_notify(frame.clone());
            assert!(rx.try_recv().is_err(), "{frame} must not produce a notice");
        }
    }

    #[test]
    fn an_over_long_title_is_capped_before_it_reaches_the_channel() {
        let slot = shared();
        let (tx, mut rx) = mpsc::unbounded_channel();
        slot.set_notify_sender(Some(tx));
        slot.handle_notify(json!({
            "kind": "info",
            "title": "x".repeat(kc_core::notify::MAX_NOTIFY_TITLE_CHARS * 4),
            "body": "y".repeat(kc_core::notify::MAX_NOTIFY_BODY_CHARS * 4),
        }));
        let notice = rx.try_recv().expect("a notice");
        assert!(notice.title.chars().count() <= kc_core::notify::MAX_NOTIFY_TITLE_CHARS + 1);
        assert!(notice.body.chars().count() <= kc_core::notify::MAX_NOTIFY_BODY_CHARS + 1);
    }

    #[test]
    fn the_rate_limiter_allows_a_burst_then_refuses() {
        let slot = shared();
        // The first call seeds `last_refill_ms`, so spend until it stops.
        let mut allowed = 0;
        for _ in 0..20 {
            if slot.spend_notify_budget() {
                allowed += 1;
            }
        }
        // Burst of five, then refusals: the exact count can be five or six
        // depending on how much wall clock the loop burned, and the property that
        // matters is "a burst, not twenty".
        assert!(
            (5..=6).contains(&allowed),
            "expected a burst of 5, got {allowed}"
        );
    }

    #[test]
    fn a_flood_warns_once_not_once_per_drop() {
        let slot = shared();
        for _ in 0..20 {
            slot.spend_notify_budget();
        }
        let budget = slot.notify_budget.lock().unwrap_or_else(|e| e.into_inner());
        assert!(budget.warned, "the first drop must set the warn flag");
    }

    #[test]
    fn a_slot_with_no_listener_drops_the_notice_without_panicking() {
        let slot = shared();
        slot.set_notify_sender(None);
        slot.handle_notify(json!({"kind": "info", "title": "nobody home"}));
        // Nothing to assert but the absence of a panic and a stuck budget.
        assert!(slot.spend_notify_budget());
    }

    #[test]
    fn the_notify_method_is_a_well_known_name() {
        assert_eq!(methods::NOTIFY, "notify");
        assert_eq!(
            kc_core::plugin::methods::NOTIFY,
            "notify",
            "the SDK and the host must agree on the spelling"
        );
    }
}
