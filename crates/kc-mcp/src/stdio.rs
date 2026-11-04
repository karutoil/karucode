//! stdio transport: the MCP server is a child process speaking newline-delimited
//! JSON-RPC 2.0 on stdin/stdout.
//!
//! Layout mirrors the plugin host: a *writer* task owns stdin, a *reader* task
//! owns stdout and routes responses to their waiter, a *stderr* task drains
//! diagnostics into `tracing` at debug level, and a *supervisor* reaps the child
//! and fails anything still in flight.

use crate::rpc::{notification_body, request_body, take_response, McpError, Pending, Reply};
use crate::transport::{bounded, Transport};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub(crate) struct StdioTransport {
    inner: Arc<Inner>,
    command: Vec<String>,
    env: BTreeMap<String, String>,
}

struct Inner {
    name: String,
    pending: Pending,
    state: Mutex<State>,
    /// False once the child is gone; used for honest status reporting.
    alive: AtomicBool,
}

#[derive(Default)]
struct State {
    out: Option<mpsc::Sender<String>>,
    kill: Option<oneshot::Sender<()>>,
    pid: Option<u32>,
    error: Option<String>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn exit_reason(&self) -> String {
        let st = self.lock();
        match &st.error {
            Some(e) => e.clone(),
            None => format!("MCP server `{}` is not running", self.name),
        }
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
                    server = %self.name,
                    "ignoring invalid JSON on MCP stdout ({}); servers must log to stderr",
                    truncate(line, 200)
                );
                return;
            }
        };
        let Some(id) = msg.get("id").and_then(|v| v.as_u64()) else {
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                // Notifications (progress, logs) and server→client requests: the
                // client implements no server→client methods, so log at debug.
                debug!(server = %self.name, method, "MCP inbound message");
            } else {
                debug!(server = %self.name, "unrecognised MCP message: {}", truncate(line, 200));
            }
            return;
        };
        let reply = match take_response(&msg, id) {
            Some(Ok(result)) => Reply::Ok(result),
            Some(Err(McpError::Remote { code, message })) => Reply::Remote { code, message },
            Some(Err(other)) => Reply::Transport(other.describe()),
            None => {
                debug!(server = %self.name, "ignoring inbound MCP message: {}", truncate(line, 200));
                return;
            }
        };
        if !self.pending.resolve(id, reply) {
            debug!(server = %self.name, "response for unknown request id {id}");
        }
    }

    fn on_exit(&self, status: std::io::Result<std::process::ExitStatus>) {
        self.alive.store(false, Ordering::SeqCst);
        let detail = match &status {
            Ok(s) => format!("process exited ({s})"),
            Err(e) => format!("waiting for the process failed: {e}"),
        };
        let message = format!("MCP server `{}` {detail}", self.name);
        {
            let mut st = self.lock();
            st.out = None;
            st.kill = None;
            st.pid = None;
            // Keep a more specific earlier error (e.g. a protocol violation)
            // instead of replacing it with the generic exit message.
            if st.error.is_none() {
                st.error = Some(message.clone());
            }
        }
        self.pending.fail_all(&message);
        debug!(server = %self.name, "MCP stdio server finished: {detail}");
    }

    /// The server broke the framing contract (e.g. an over-long line). Fail
    /// everything in flight with that protocol error and stop the child: a
    /// server that cannot frame JSON-RPC is not usable.
    fn fail_protocol(&self, message: String) {
        let kill = {
            let mut st = self.lock();
            st.out = None;
            st.error = Some(message.clone());
            st.kill.take()
        };
        self.pending.fail_all(&message);
        if let Some(tx) = kill {
            let _ = tx.send(());
        }
    }
}

/// Environment variables an MCP stdio server inherits from karucode.
///
/// The child gets a **cleared** environment plus this allowlist: the karucode
/// process holds `KARUCODE_AUTH_TOKEN` and the operator's provider API key, and
/// an `npx`-launched server (or its `postinstall` script) must not receive them.
/// Everything listed here is what ordinary tooling needs to start at all.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TERM", "TMPDIR", "PWD", "TZ",
];

/// Whether an environment variable name looks like it carries a credential.
///
/// Defence in depth on top of [`ENV_ALLOWLIST`]. Matching is case-insensitive
/// and anchored on `_` boundaries, so `MY_TOKENIZER` is not mistaken for a token
/// while `GITHUB_TOKEN` and `OPENAI_API_KEY` are caught.
///
/// Duplicated from `kc-tools/src/command.rs` on purpose: the two crates share no
/// module, and `kc-core` exposes no equivalent helper. Keep the two copies in
/// step (both have the same unit tests).
fn is_secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const SECRET_PREFIXES: &[&str] = &["KARUCODE_", "AWS_", "OPENAI_", "ANTHROPIC_"];
    const SECRET_NAMES: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "NPM_TOKEN"];
    const SECRET_SUFFIXES: &[&str] = &["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];

    SECRET_PREFIXES.iter().any(|p| upper.starts_with(p))
        || SECRET_NAMES.contains(&upper.as_str())
        || SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// Clear the inherited environment, add the allowlist, then the server's own
/// configured `env` table.
///
/// The configured table is applied last so it wins over the allowlist, and it is
/// **not** filtered by [`is_secret_like`]: a variable an operator explicitly
/// configured for that server is an intentional choice (that is how a server
/// receives its own credentials), unlike an inherited one.
///
/// Unlike `run_command`, `LC_ALL` is not forced to `C`: an MCP server is a
/// long-lived protocol peer, and pinning its locale is not needed to stop the
/// leak.
fn apply_env(command: &mut Command, configured: &BTreeMap<String, String>) {
    command.env_clear();
    for name in ENV_ALLOWLIST {
        if is_secret_like(name) {
            continue;
        }
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (name, value) in configured {
        command.env(name, value);
    }
}

impl StdioTransport {
    pub(crate) fn new(name: String, command: Vec<String>, env: BTreeMap<String, String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                name,
                pending: Pending::new(),
                state: Mutex::new(State::default()),
                alive: AtomicBool::new(false),
            }),
            command,
            env,
        }
    }

    fn exit_reason(&self) -> String {
        self.inner.exit_reason()
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn open(&self) -> Result<(), McpError> {
        if self.inner.lock().out.is_some() {
            return Ok(());
        }
        if self.command.is_empty() {
            return Err(McpError::Transport(format!(
                "MCP server `{}` has no command configured",
                self.inner.name
            )));
        }
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]);
        // The server must not inherit karucode's environment (bearer token,
        // provider API key): allowlist plus the server's own configured `env`.
        apply_env(&mut command, &self.env);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own session => own process group, so teardown reaches every helper the
        // server starts (an `npx`-launched `node`, a wrapper script, a tool call's
        // children). `kill_on_drop` only signals the leader.
        crate::procgroup::detach_into_own_group(&mut command);
        debug!(server = %self.inner.name, command = ?self.command, "starting MCP stdio server");
        let mut child: Child = command.spawn().map_err(|e| {
            McpError::Transport(format!(
                "failed to start MCP server `{}` with command `{}`: {e}. Check that the program \
                 exists and is executable.",
                self.inner.name,
                self.command.join(" ")
            ))
        })?;
        let pid = child.id().ok_or_else(|| {
            McpError::Transport(format!(
                "spawned MCP server `{}` has no pid; cannot manage its process group",
                self.inner.name
            ))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::Transport(format!(
                "could not capture stdin of MCP server `{}`",
                self.inner.name
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::Transport(format!(
                "could not capture stdout of MCP server `{}`",
                self.inner.name
            ))
        })?;
        let stderr = child.stderr.take();

        let (out_tx, mut out_rx) = mpsc::channel::<String>(OUTGOING_CAPACITY);
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        {
            let mut st = self.inner.lock();
            st.out = Some(out_tx);
            st.kill = Some(kill_tx);
            st.pid = Some(pid);
            st.error = None;
        }
        self.inner.alive.store(true, Ordering::SeqCst);

        // writer: owns stdin
        {
            let inner = Arc::clone(&self.inner);
            let mut stdin = stdin;
            tokio::spawn(async move {
                while let Some(line) = out_rx.recv().await {
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
                debug!(server = %inner.name, "MCP stdin closed");
            });
        }

        // stderr drain
        if let Some(stderr) = stderr {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                // A log line past the cap is truncated, not fatal: closing this
                // read end used to make the server's next stderr write fail with
                // `EPIPE` and take the whole process down.
                let result = pump_lines(stderr, MAX_LOG_LINE, OverLong::Truncate, |line| {
                    debug!(server = %inner.name, "stderr: {line}");
                })
                .await;
                match result {
                    Ok(report) if !report.is_empty() => debug!(
                        server = %inner.name,
                        "{}",
                        report.describe(MAX_LOG_LINE, "the server's stderr")
                    ),
                    Ok(_) => {}
                    Err(e) => debug!(server = %inner.name, "stopped draining stderr: {e}"),
                }
            });
        }

        // reader: owns stdout
        {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                // `OverLong::Fail`: an over-long JSON-RPC line cannot be parsed,
                // so that server's in-flight requests are failed rather than left
                // waiting. stderr is truncated instead - see above.
                let result = pump_lines(stdout, MAX_PROTOCOL_LINE, OverLong::Fail, |line| {
                    inner.handle_line(line)
                })
                .await;
                if let Err(e) = result {
                    let message = if e.kind() == std::io::ErrorKind::InvalidData {
                        format!(
                            "MCP server `{}` sent a protocol line larger than the {} byte limit; \
                             the connection was closed because the server is not speaking the \
                             protocol",
                            inner.name, MAX_PROTOCOL_LINE
                        )
                    } else {
                        format!("reading MCP server `{}` stdout failed: {e}", inner.name)
                    };
                    warn!(server = %inner.name, "{message}");
                    inner.fail_protocol(message);
                }
            });
        }

        // supervisor: owns the child
        {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                // Backstop: the group must not outlive this task either.
                let _group = crate::procgroup::guard(pid);
                let status = tokio::select! {
                    status = child.wait() => {
                        // The leader exited on its own; helpers it started are
                        // still in the group and would keep the pipes open.
                        crate::procgroup::kill_group_after_exit(pid);
                        status
                    }
                    _ = kill_rx => crate::procgroup::terminate_group(&mut child, pid).await,
                };
                inner.on_exit(status);
            });
        }
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        let sender = self.inner.lock().out.clone();
        let Some(sender) = sender else {
            return Err(McpError::Transport(self.exit_reason()));
        };
        bounded(
            async {
                // Reserve capacity before registering or serializing, so blocked senders
                // do not add entries to the pending map or fill an unbounded queue.
                let permit = sender
                    .reserve()
                    .await
                    .map_err(|_| McpError::Transport(self.exit_reason()))?;
                let (id, rx) = self.inner.pending.register();
                let _guard = PendingGuard {
                    pending: &self.inner.pending,
                    id,
                };
                permit.send(request_body(id, method, params).to_string());
                match rx.await {
                    Ok(reply) => reply.into_result(),
                    Err(_) => Err(McpError::Transport(format!(
                        "MCP server `{}` dropped the response for `{method}`",
                        self.inner.name
                    ))),
                }
            },
            timeout,
            cancel,
        )
        .await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let sender = self.inner.lock().out.clone();
        let Some(sender) = sender else {
            return Err(McpError::Transport(self.exit_reason()));
        };
        let permit = sender
            .reserve()
            .await
            .map_err(|_| McpError::Transport(self.exit_reason()))?;
        permit.send(notification_body(method, params).to_string());
        Ok(())
    }

    async fn close(&self) {
        let (kill, had_process) = {
            let mut st = self.inner.lock();
            let had = st.out.take().is_some();
            (st.kill.take(), had)
        };
        if let Some(tx) = kill {
            let _ = tx.send(());
        }
        if had_process {
            // Give the supervisor a moment to reap the child.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.inner
            .pending
            .fail_all(&format!("MCP server `{}` was closed", self.inner.name));
        self.inner.alive.store(false, Ordering::SeqCst);
    }

    fn alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }

    fn kind(&self) -> &'static str {
        "stdio"
    }
}

// A dropped request future must also remove its waiter, not only explicit timeouts.
struct PendingGuard<'a> {
    pending: &'a Pending,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.pending.forget(self.id);
    }
}

/// Maximum queued messages waiting for the stdin writer.
const OUTGOING_CAPACITY: usize = 32;

/// Largest protocol line buffered from a server. Anything bigger means the
/// server is not speaking the protocol; the channel is closed instead of growing.
const MAX_PROTOCOL_LINE: usize = 8 * 1024 * 1024;
/// Largest stderr line kept for logging.
const MAX_LOG_LINE: usize = 64 * 1024;

/// What to do with a line that exceeds the reader's cap.
///
/// See the identical reasoning in `kc_plugin_host::rpc`: the two streams are not
/// symmetric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverLong {
    /// A **log** stream (stderr): truncate the line, report it once, keep
    /// draining. Fatal truncation here was a bug — the closed read end made the
    /// server's next stderr write fail with `EPIPE`, which killed the process,
    /// and every later call failed with "process stopped or broke the protocol".
    Truncate,
    /// A **protocol** stream (stdout): the line cannot be JSON, so the server
    /// broke the framing contract. Failing is the honest answer.
    Fail,
}

/// What [`pump_lines`] saw beyond an ordinary end of stream.
#[derive(Debug, Default, PartialEq, Eq)]
struct PumpReport {
    /// Lines that exceeded the cap: truncated to it and handed to `on_line`
    /// anyway, with the remainder discarded.
    over_long_lines: u64,
}

impl PumpReport {
    fn is_empty(&self) -> bool {
        self.over_long_lines == 0
    }

    fn describe(&self, max_line: usize, what: &str) -> String {
        format!(
            "{what} sent {} line(s) longer than the {max_line} byte limit; each was truncated to \
             that limit and the remainder discarded, and reading continued",
            self.over_long_lines
        )
    }
}

/// Chunked, length-capped line reader (see the plugin host for the rationale).
///
/// Bytes past `max_line` are dropped as they arrive instead of being buffered, so
/// an endless line cannot exhaust memory. What happens *then* depends on
/// [`OverLong`]: a log line is truncated and the drain continues; a protocol line
/// is a framing violation. The only `Err` a log stream can produce is a genuine
/// read failure from the OS.
async fn pump_lines<R, F>(
    mut reader: R,
    max_line: usize,
    over_long: OverLong,
    mut on_line: F,
) -> std::io::Result<PumpReport>
where
    R: AsyncReadExt + Unpin,
    F: FnMut(&str),
{
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // Offset up to which `pending` has already been searched for a newline, so a
    // long line is scanned once instead of once per read (quadratic).
    let mut searched: usize = 0;
    let mut report = PumpReport::default();
    // The current line already hit the cap: drop everything up to its newline.
    let mut discarding = false;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if discarding {
            match chunk[..read].iter().position(|b| *b == b'\n') {
                Some(pos) => {
                    discarding = false;
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
            // The remaining bytes were not searched yet.
            searched = 0;
        }
        searched = pending.len();
        if pending.len() > max_line {
            if over_long == OverLong::Fail {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "line exceeds {max_line} bytes; the server is not speaking the protocol"
                    ),
                ));
            }
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
    fn transport_reports_not_running_before_open() {
        let t = StdioTransport::new("x".into(), vec!["sh".into()], BTreeMap::new());
        assert!(!t.alive());
        assert_eq!(t.kind(), "stdio");
        assert!(t.exit_reason().contains("not running"));
    }

    #[test]
    fn pending_count_tracks_in_flight_requests() {
        let t = StdioTransport::new("x".into(), vec!["sh".into()], BTreeMap::new());
        assert_eq!(t.inner.pending.len(), 0);
        let (_id, _rx) = t.inner.pending.register();
        assert_eq!(t.inner.pending.len(), 1);
    }

    #[tokio::test]
    async fn outgoing_backpressure_and_dropped_requests_clean_up() {
        let t = Arc::new(StdioTransport::new("x".into(), vec![], BTreeMap::new()));
        let (tx, mut rx) = mpsc::channel(OUTGOING_CAPACITY);
        t.inner.lock().out = Some(tx.clone());
        for _ in 0..OUTGOING_CAPACITY {
            tx.try_send("full".into()).unwrap();
        }
        let task = {
            let t = Arc::clone(&t);
            tokio::spawn(async move {
                t.request("test", Value::Null, Duration::from_secs(10), None)
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        assert_eq!(t.inner.pending.len(), 0);
        assert!(matches!(
            tx.try_send("overflow".into()),
            Err(mpsc::error::TrySendError::Full(_))
        ));
        rx.recv().await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(t.inner.pending.len(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(t.inner.pending.len(), 0);
    }

    #[tokio::test]
    async fn timeout_and_cancellation_cover_queue_wait() {
        let t = StdioTransport::new("x".into(), vec![], BTreeMap::new());
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send("full".into()).unwrap();
        t.inner.lock().out = Some(tx);
        assert!(t
            .request("test", Value::Null, Duration::from_millis(10), None)
            .await
            .is_err());
        assert_eq!(t.inner.pending.len(), 0);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(t
            .request("test", Value::Null, Duration::from_secs(10), Some(&cancel))
            .await
            .is_err());
        assert_eq!(t.inner.pending.len(), 0);
    }

    #[tokio::test]
    async fn pump_lines_is_bounded() {
        let mut cursor = std::io::Cursor::new(b"one\ntwo\n".to_vec());
        let mut lines = Vec::new();
        let report = pump_lines(&mut cursor, 64, OverLong::Fail, |line| {
            lines.push(line.to_string())
        })
        .await
        .unwrap();
        assert!(report.is_empty());
        assert_eq!(lines, vec!["one", "two"]);

        // Protocol stream: an endless line is a framing violation.
        let mut cursor = std::io::Cursor::new(vec![b'y'; 4096]);
        let err = pump_lines(&mut cursor, 64, OverLong::Fail, |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    /// One over-long stderr line used to end the drain for good, which killed the
    /// server and every later call with it. It must be truncated and discarded,
    /// and the following lines must still arrive.
    #[tokio::test]
    async fn an_over_long_log_line_is_truncated_and_the_drain_continues() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"before\n");
        input.extend(std::iter::repeat_n(b'y', 200 * 1024));
        input.extend_from_slice(b"\nafter\n");
        input.extend(std::iter::repeat_n(b'z', 150 * 1024));

        let mut lines: Vec<String> = Vec::new();
        let report = pump_lines(
            &mut std::io::Cursor::new(input),
            1024,
            OverLong::Truncate,
            |line| lines.push(line.to_string()),
        )
        .await
        .unwrap();

        assert_eq!(report.over_long_lines, 2, "{report:?}");
        assert_eq!(lines[0], "before");
        assert_eq!(lines[1].len(), 1024);
        assert!(lines[1].bytes().all(|b| b == b'y'));
        assert_eq!(lines[2], "after");
        assert_eq!(lines[3].len(), 1024);
        assert!(lines[3].bytes().all(|b| b == b'z'));
    }

    /// The cap must be applied on a UTF-8 boundary so a multi-byte log line is
    /// not mangled into replacement characters.
    #[tokio::test]
    async fn an_over_long_utf8_log_line_is_truncated_on_a_char_boundary() {
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
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].contains('\u{FFFD}'), "{:?}", &lines[0].len());
        assert_eq!(lines[0].len(), 1024);
        assert_eq!(lines[1], "tail");
    }

    #[tokio::test]
    async fn open_without_command_is_an_error() {
        let t = StdioTransport::new("x".into(), Vec::new(), BTreeMap::new());
        let err = t.open().await.unwrap_err();
        assert!(err.describe().contains("no command"));
    }

    #[tokio::test]
    async fn open_with_a_missing_program_is_an_error() {
        let t = StdioTransport::new(
            "x".into(),
            vec!["kc-mcp-no-such-program-xyz".into()],
            BTreeMap::new(),
        );
        let err = t.open().await.unwrap_err();
        assert!(
            err.describe().contains("failed to start"),
            "{}",
            err.describe()
        );
    }

    /// Sets a variable in the *process* environment for the duration of a test.
    ///
    /// Environment mutation is process-global and the test harness runs tests in
    /// parallel threads, so every test that uses this must pick a name no other
    /// test touches (the names below are unique to this module) and must restore
    /// the previous value even when the test panics.
    struct EnvGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    fn env_of(command: &Command) -> Vec<(String, Option<String>)> {
        command
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn is_secret_like_flags_credentials() {
        for name in [
            "KARUCODE_AUTH_TOKEN",
            "KARUCODE_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "NPM_TOKEN",
            "MY_SERVICE_TOKEN",
            "DB_PASSWORD",
            "SIGNING_SECRET",
            "openai_api_key",
        ] {
            assert!(is_secret_like(name), "{name} must be treated as a secret");
        }
    }

    #[test]
    fn is_secret_like_ignores_ordinary_names() {
        for name in [
            "PATH",
            "HOME",
            "TERM",
            "LANG",
            "TMPDIR",
            "MY_TOKENIZER",
            "TOKEN_COUNT",
            "PASSWORD_FILE_NAME",
            "AWSOME_THING",
        ] {
            assert!(
                !is_secret_like(name),
                "{name} must not be treated as a secret"
            );
        }
    }

    #[test]
    fn apply_env_clears_the_inherited_environment() {
        let _guard = EnvGuard::set("KARUCODE_TEST_MCP_ALLOWLIST_TOKEN", "leak-me");
        let mut command = Command::new("sh");
        apply_env(&mut command, &BTreeMap::new());

        let env = env_of(&command);
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !names.contains(&"KARUCODE_TEST_MCP_ALLOWLIST_TOKEN"),
            "a secret-like variable must never be inherited: {names:?}"
        );
        assert!(
            !names.iter().any(|n| is_secret_like(n)),
            "no secret-like name may survive the allowlist: {names:?}"
        );
        assert!(names.contains(&"PATH"), "{names:?}");
        for name in ["CARGO_HOME", "RUSTUP_HOME", "KARUCODE_CONFIG"] {
            assert!(
                !names.contains(&name),
                "{name} must not be inherited: {names:?}"
            );
        }
    }

    #[test]
    fn configured_env_wins_over_the_allowlist() {
        let mut configured = BTreeMap::new();
        configured.insert("PATH".to_string(), "/configured/bin".to_string());
        // An operator-configured credential is intentional and must be applied.
        configured.insert("SERVER_API_KEY".to_string(), "server-key".to_string());
        let mut command = Command::new("sh");
        apply_env(&mut command, &configured);

        let env = env_of(&command);
        let get = |name: &str| {
            env.iter()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| v.clone())
        };
        assert_eq!(get("PATH").as_deref(), Some("/configured/bin"));
        assert_eq!(get("SERVER_API_KEY").as_deref(), Some("server-key"));
    }

    #[tokio::test]
    async fn a_secret_in_the_parent_environment_does_not_reach_the_server() {
        // The failure this fixes: an `npx`-launched server (and its postinstall
        // script) used to inherit karucode's bearer token and provider key.
        let _guard = EnvGuard::set("KARUCODE_TEST_MCP_LEAK_TOKEN", "leak-me");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("kc-mcp-stdio-env-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("env.txt");
        let script = format!(
            "echo \"token=${{KARUCODE_TEST_MCP_LEAK_TOKEN:-ABSENT}}\" > '{}'; \
             echo \"path=${{PATH:+PATH-SET}}\" >> '{}'; sleep 30",
            out.display(),
            out.display()
        );
        let t = StdioTransport::new(
            "env-probe".into(),
            vec!["sh".into(), "-c".into(), script],
            BTreeMap::new(),
        );
        t.open().await.expect("the probe server should start");

        let mut text = String::new();
        for _ in 0..200 {
            if let Ok(read) = std::fs::read_to_string(&out) {
                if read.contains("path=") {
                    text = read;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        t.close().await;

        assert!(
            text.contains("token=ABSENT"),
            "the parent's secret leaked into the MCP server: {text:?}"
        );
        assert!(
            text.contains("path=PATH-SET"),
            "PATH must still be passed through: {text:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
