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
}

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
        let pid = child.id();
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let msg = format!("could not capture stdin of plugin `{}`", self.name);
                let _ = child.start_kill();
                self.fail(msg.clone());
                return Err(msg);
            }
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let msg = format!("could not capture stdout of plugin `{}`", self.name);
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
            inner.pid = pid;
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
        let supervisor = self.spawn_supervisor(epoch, child, kill_rx);
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
            let result = pump_lines(stderr, MAX_LOG_LINE, |line| {
                debug!(plugin = %name, "stderr: {line}");
            })
            .await;
            if let Err(e) = result {
                debug!(plugin = %name, "stopped draining plugin stderr: {e}");
            }
        });
    }

    fn spawn_reader(self: &Arc<Self>, epoch: u64, stdout: ChildStdout) {
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            let result =
                pump_lines(stdout, MAX_PROTOCOL_LINE, |line| shared.handle_line(line)).await;
            let protocol_error = match result {
                Ok(()) => None,
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
        mut child: Child,
        kill_rx: oneshot::Receiver<()>,
    ) -> JoinHandle<()> {
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    shared.on_exit(epoch, status);
                }
                _ = kill_rx => {
                    let _ = child.start_kill();
                    let status = child.wait().await;
                    shared.on_exit(epoch, status);
                }
            }
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

/// Read `reader` line by line, calling `on_line` for each complete line, with a
/// hard per-line cap. Chunked read instead of `BufReader::lines` so one endless
/// line cannot exhaust memory.
async fn pump_lines<R, F>(mut reader: R, max_line: usize, mut on_line: F) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(&str),
{
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // Only scan bytes that were appended since the last newline search; a long
    // line would otherwise be rescanned on every read (quadratic).
    let mut searched: usize = 0;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        while let Some(rel) = pending[searched..].iter().position(|b| *b == b'\n') {
            let end = searched + rel + 1;
            let line: Vec<u8> = pending.drain(..end).collect();
            on_line(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']));
            searched = 0;
        }
        searched = pending.len();
        if pending.len() > max_line {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line exceeds {max_line} bytes; the process is not speaking the protocol"),
            ));
        }
    }
    if !pending.is_empty() {
        on_line(String::from_utf8_lossy(&pending).trim_end_matches(['\n', '\r']));
    }
    Ok(())
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
        pump_lines(&mut cursor, 1024, |line| lines.push(line.to_string()))
            .await
            .unwrap();
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}", "", "last"]);

        // One endless line is refused instead of buffered forever.
        let mut grow = vec![b'x'; 64 * 1024];
        grow.extend_from_slice(b"\n");
        let mut cursor = std::io::Cursor::new(grow);
        let err = pump_lines(&mut cursor, 1024, |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("exceeds"));
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
