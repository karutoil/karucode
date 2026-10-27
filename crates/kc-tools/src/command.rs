//! Shell execution.
//!
//! **Not a sandbox.** Commands run as the server user through `/bin/sh -c`, in
//! their own session/process group so that a timeout or a cancelled run can
//! terminate the whole tree. Output is streamed to the event sink and kept
//! head+tail bounded so a noisy build cannot exhaust memory.

use kc_core::events::Event;
use kc_core::tool::ToolCtx;
use kc_core::{Error, Result};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// Grace period between SIGTERM and SIGKILL when a command must be stopped.
const KILL_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub call_id: String,
    pub command: String,
    pub cwd: PathBuf,
    /// Workspace-relative cwd used in events and the result header.
    pub cwd_label: String,
    pub timeout: Duration,
    /// Per-stream byte budget kept for the model (head and tail).
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    fn label(self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

#[derive(Debug)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration: Duration,
    pub stdout: FinishedStream,
    pub stderr: FinishedStream,
}

#[derive(Debug)]
pub struct FinishedStream {
    pub text: String,
    pub truncated: bool,
    pub total_bytes: usize,
}

impl CommandOutcome {
    pub fn ok(&self) -> bool {
        !self.timed_out && !self.cancelled
    }

    pub fn truncated(&self) -> bool {
        self.stdout.truncated || self.stderr.truncated
    }
}

/// Head + tail ring buffer. Keeps the first and last `max/2` bytes once the
/// budget is exceeded and records how much was dropped.
#[derive(Debug, Clone)]
pub struct BoundedOutput {
    max: usize,
    /// Bytes kept so far while still under budget.
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
    truncated: bool,
}

impl BoundedOutput {
    pub fn new(max: usize) -> Self {
        let max = max.max(64);
        Self {
            max,
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            truncated: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total = self.total.saturating_add(chunk.len());
        let head_cap = self.max / 2;
        let tail_cap = self.max - head_cap;

        if !self.truncated {
            if self.head.len() + chunk.len() <= self.max {
                self.head.extend_from_slice(chunk);
                return;
            }
            self.truncated = true;
            let mut all = std::mem::take(&mut self.head);
            all.extend_from_slice(chunk);
            let head_len = head_cap.min(all.len());
            self.head = all[..head_len].to_vec();
            let rest = &all[head_len..];
            let start = rest.len().saturating_sub(tail_cap);
            self.tail = rest[start..].to_vec();
            return;
        }

        self.tail.extend_from_slice(chunk);
        if self.tail.len() > tail_cap {
            let drop = self.tail.len() - tail_cap;
            self.tail.drain(..drop);
        }
    }

    pub fn finish(&self) -> FinishedStream {
        let dropped = self.total.saturating_sub(self.head.len() + self.tail.len());
        if !self.truncated {
            FinishedStream {
                text: String::from_utf8_lossy(&self.head).into_owned(),
                truncated: false,
                total_bytes: self.total,
            }
        } else {
            let text = format!(
                "{}\n[... {dropped} of {} bytes omitted (kept the first {} and last {}) ...]\n{}",
                String::from_utf8_lossy(&self.head),
                self.total,
                self.head.len(),
                self.tail.len(),
                String::from_utf8_lossy(&self.tail),
            );
            FinishedStream {
                text,
                truncated: true,
                total_bytes: self.total,
            }
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.total
    }
}

/// Spawn, stream, bound and (if needed) kill a shell command.
pub async fn run(request: CommandRequest, ctx: &ToolCtx) -> Result<CommandOutcome> {
    if ctx.cancelled() {
        return Err(Error::Cancelled);
    }

    let started = Instant::now();
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(&request.command)
        .current_dir(&request.cwd)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Own session => own process group, so `kill(-pid)` reaches every child.
    unsafe {
        cmd.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(|e| {
        Error::Tool(format!(
            "failed to spawn `/bin/sh -c {:?}` in {}: {e}",
            request.command, request.cwd_label
        ))
    })?;
    let pid = child.id().ok_or_else(|| {
        Error::Internal("spawned command has no pid; cannot manage its process group".into())
    })?;

    ctx.emit(Event::CommandStarted {
        call_id: request.call_id.clone(),
        command: request.command.clone(),
        cwd: request.cwd_label.clone(),
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<(Stream, Vec<u8>)>();
    let mut pumps = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        pumps.push(tokio::spawn(pump(stdout, Stream::Stdout, tx.clone())));
    }
    if let Some(stderr) = child.stderr.take() {
        pumps.push(tokio::spawn(pump(stderr, Stream::Stderr, tx.clone())));
    }
    drop(tx);

    let mut stdout_acc = BoundedOutput::new(request.max_output_bytes);
    let mut stderr_acc = BoundedOutput::new(request.max_output_bytes);
    let mut timed_out = false;
    let mut cancelled = false;

    let deadline = tokio::time::sleep(request.timeout);
    tokio::pin!(deadline);
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut streams_open = true;
    loop {
        if !streams_open && exit_status.is_some() {
            break;
        }
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                cancelled = true;
                break;
            }
            _ = &mut deadline => {
                timed_out = true;
                break;
            }
            chunk = rx.recv(), if streams_open => match chunk {
                Some((stream, bytes)) => {
                    ctx.emit(Event::CommandOutput {
                        call_id: request.call_id.clone(),
                        stream: stream.label().to_string(),
                        chunk: String::from_utf8_lossy(&bytes).into_owned(),
                    });
                    match stream {
                        Stream::Stdout => stdout_acc.push(&bytes),
                        Stream::Stderr => stderr_acc.push(&bytes),
                    }
                }
                // Both pipes closed: the process (and its children) are done writing.
                None => streams_open = false,
            },
            status = child.wait(), if exit_status.is_none() => {
                // `Child::wait` is cancellation-safe, so re-creating it per loop
                // iteration cannot lose the exit status.
                match status {
                    Ok(status) => exit_status = Some(status),
                    Err(e) => {
                        return Err(Error::Tool(format!(
                            "failed while waiting for `{}`: {e}",
                            request.command
                        )));
                    }
                }
            },
        }
    }

    let stopped_early = timed_out || cancelled;
    let status = if stopped_early {
        terminate_group(&mut child, pid).await
    } else {
        exit_status
    };
    for pump in pumps {
        pump.abort();
    }

    let duration = started.elapsed();
    let exit_code = status.and_then(|s| s.code());
    let outcome = CommandOutcome {
        exit_code,
        timed_out,
        cancelled,
        duration,
        stdout: stdout_acc.finish(),
        stderr: stderr_acc.finish(),
    };

    ctx.emit(Event::CommandFinished {
        call_id: request.call_id.clone(),
        exit_code,
        duration_ms: duration.as_millis() as u64,
        truncated: outcome.truncated(),
    });

    Ok(outcome)
}

async fn pump<R>(mut reader: R, stream: Stream, tx: mpsc::UnboundedSender<(Stream, Vec<u8>)>)
where
    R: AsyncReadExt + Unpin,
{
    let mut buf = vec![0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send((stream, buf[..n].to_vec())).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn signal_group(pid: u32, signal: i32) {
    // The child called setsid(), so its group id equals its pid.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// SIGTERM the group, wait up to [`KILL_GRACE`], then SIGKILL. Returns the exit
/// status when it could be collected.
async fn terminate_group(
    child: &mut tokio::process::Child,
    pid: u32,
) -> Option<std::process::ExitStatus> {
    signal_group(pid, libc::SIGTERM);
    let status = match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => None,
    };
    // Sweep any survivors (grandchildren) even if the leader already exited.
    signal_group(pid, libc::SIGKILL);
    if status.is_some() {
        return status;
    }
    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => None,
    }
}
