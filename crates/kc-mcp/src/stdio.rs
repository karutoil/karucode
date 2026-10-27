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
    out: Option<mpsc::UnboundedSender<String>>,
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
        for (k, v) in &self.env {
            command.env(k, v);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        debug!(server = %self.inner.name, command = ?self.command, "starting MCP stdio server");
        let mut child: Child = command.spawn().map_err(|e| {
            McpError::Transport(format!(
                "failed to start MCP server `{}` with command `{}`: {e}. Check that the program \
                 exists and is executable.",
                self.inner.name,
                self.command.join(" ")
            ))
        })?;
        let pid = child.id();
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

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        {
            let mut st = self.inner.lock();
            st.out = Some(out_tx);
            st.kill = Some(kill_tx);
            st.pid = pid;
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
                let result = pump_lines(stderr, MAX_LOG_LINE, |line| {
                    debug!(server = %inner.name, "stderr: {line}");
                })
                .await;
                if let Err(e) = result {
                    debug!(server = %inner.name, "stopped draining stderr: {e}");
                }
            });
        }

        // reader: owns stdout
        {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let result =
                    pump_lines(stdout, MAX_PROTOCOL_LINE, |line| inner.handle_line(line)).await;
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
                tokio::select! {
                    status = child.wait() => inner.on_exit(status),
                    _ = kill_rx => {
                        let _ = child.start_kill();
                        let status = child.wait().await;
                        inner.on_exit(status);
                    }
                }
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
        let (id, rx) = self.inner.pending.register();
        let line = request_body(id, method, params).to_string();
        let sender = self.inner.lock().out.clone();
        let Some(sender) = sender else {
            self.inner.pending.forget(id);
            return Err(McpError::Transport(self.exit_reason()));
        };
        if sender.send(line).is_err() {
            self.inner.pending.forget(id);
            return Err(McpError::Transport(self.exit_reason()));
        }
        let wait = async {
            match rx.await {
                Ok(reply) => reply.into_result(),
                Err(_) => Err(McpError::Transport(format!(
                    "MCP server `{}` dropped the response for `{method}`",
                    self.inner.name
                ))),
            }
        };
        let result = bounded(wait, timeout, cancel).await;
        // Reclaim the slot for timeouts/cancellations (a no-op when resolved).
        self.inner.pending.forget(id);
        result
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let sender = self.inner.lock().out.clone();
        let Some(sender) = sender else {
            return Err(McpError::Transport(self.exit_reason()));
        };
        sender
            .send(notification_body(method, params).to_string())
            .map_err(|_| McpError::Transport(self.exit_reason()))
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

/// Largest protocol line buffered from a server. Anything bigger means the
/// server is not speaking the protocol; the channel is closed instead of growing.
const MAX_PROTOCOL_LINE: usize = 8 * 1024 * 1024;
/// Largest stderr line kept for logging.
const MAX_LOG_LINE: usize = 64 * 1024;

/// Chunked, length-capped line reader (see the plugin host for the rationale).
async fn pump_lines<R, F>(mut reader: R, max_line: usize, mut on_line: F) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    F: FnMut(&str),
{
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // Offset up to which `pending` has already been searched for a newline, so a
    // long line is scanned once instead of once per read (quadratic).
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
            // The remaining bytes were not searched yet.
            searched = 0;
        }
        searched = pending.len();
        if pending.len() > max_line {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line exceeds {max_line} bytes; the server is not speaking the protocol"),
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
    async fn pump_lines_is_bounded() {
        let mut cursor = std::io::Cursor::new(b"one\ntwo\n".to_vec());
        let mut lines = Vec::new();
        pump_lines(&mut cursor, 64, |line| lines.push(line.to_string()))
            .await
            .unwrap();
        assert_eq!(lines, vec!["one", "two"]);

        let mut cursor = std::io::Cursor::new(vec![b'y'; 4096]);
        let err = pump_lines(&mut cursor, 64, |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("exceeds"));
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
}
