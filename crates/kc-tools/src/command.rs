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
        // A nonzero exit is a failed command, not a successful tool call: the UI
        // and the runtime must not paint a green check for a failed build.
        !self.timed_out && !self.cancelled && self.exit_code == Some(0)
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
        if !self.truncated {
            return FinishedStream {
                text: String::from_utf8_lossy(&self.head).into_owned(),
                truncated: false,
                total_bytes: self.total,
            };
        }
        // The cuts are byte offsets, so they can land inside a multi-byte
        // character. Trim both sides to char boundaries first: slicing there and
        // then decoding would emit U+FFFD garbage at the seam.
        let head = &self.head[..floor_char_boundary(&self.head, self.head.len())];
        let tail = &self.tail[ceil_char_boundary(&self.tail, 0)..];
        let dropped = self.total.saturating_sub(head.len() + tail.len());
        let text = format!(
            "{}\n[... {dropped} of {} bytes omitted (kept the first {} and last {}) ...]\n{}",
            String::from_utf8_lossy(head),
            self.total,
            head.len(),
            tail.len(),
            String::from_utf8_lossy(tail),
        );
        FinishedStream {
            text,
            truncated: true,
            total_bytes: self.total,
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.total
    }
}

/// Largest index `<= idx` that is a UTF-8 char boundary in `bytes`.
fn floor_char_boundary(bytes: &[u8], idx: usize) -> usize {
    let idx = idx.min(bytes.len());
    match std::str::from_utf8(&bytes[..idx]) {
        Ok(_) => idx,
        Err(e) => e.valid_up_to(),
    }
}

/// Smallest index `>= idx` that is a UTF-8 char boundary in `bytes`.
fn ceil_char_boundary(bytes: &[u8], idx: usize) -> usize {
    let mut i = idx.min(bytes.len());
    while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
        i += 1;
    }
    i
}

/// Kills the whole process group if the `run` future is dropped before the
/// leader was reaped (task abort, subagent hard timeout).
///
/// `kill_on_drop(true)` only signals the leader, which called `setsid()` and is
/// therefore the group leader: its children (`sleep` under a shell, a build's
/// compiler, ...) would survive. The guard is disarmed as soon as the leader is
/// reaped — after that the pid may be reused and `kill(-pid)` would hit an
/// unrelated process group.
struct GroupGuard {
    pid: u32,
    armed: bool,
}

impl GroupGuard {
    fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            signal_group(self.pid, libc::SIGKILL);
        }
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
    // Declared after `child`, so it drops first: kill the group while the leader
    // pid is still guaranteed to be ours, then let `kill_on_drop` handle the
    // leader itself.
    let mut group = GroupGuard::new(pid);

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
                    Ok(status) => {
                        exit_status = Some(status);
                        // The leader is reaped; the pid may be reused now, so the
                        // drop guard must never signal this group again.
                        group.disarm();
                    }
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
        terminate_group(&mut child, pid, exit_status).await
    } else {
        exit_status
    };
    if status.is_some() {
        group.disarm();
    }
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
///
/// Signalling a process group by pid is only safe while the leader has not been
/// reaped: once it has, the kernel may reuse that pid for an unrelated process
/// (group), and `kill(-pid)` would hit innocent processes. So the group is only
/// signalled after `try_wait()` confirms the leader is still running, and never
/// again once it has been reaped.
async fn terminate_group(
    child: &mut tokio::process::Child,
    pid: u32,
    reaped: Option<std::process::ExitStatus>,
) -> Option<std::process::ExitStatus> {
    if let Some(status) = reaped {
        return Some(status);
    }
    match child.try_wait() {
        Ok(Some(status)) => return Some(status),
        Ok(None) => {}
        // The state is unknown; never risk signalling a reused pid.
        Err(_) => return None,
    }

    signal_group(pid, libc::SIGTERM);
    if let Ok(Ok(status)) = tokio::time::timeout(KILL_GRACE, child.wait()).await {
        return Some(status);
    }

    // Escalate only if the leader is still unreaped; a reaped leader means the
    // pid may already belong to somebody else and must not be signalled.
    match child.try_wait() {
        Ok(Some(status)) => return Some(status),
        Ok(None) => signal_group(pid, libc::SIGKILL),
        Err(_) => return None,
    }
    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(_)) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kc_core::approval::{ApprovalDecision, ApprovalRequest};
    use kc_core::config::{ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
    use kc_core::ids::{RunId, SessionId};
    use kc_core::paths::Workspace;
    use kc_core::settings::Settings;
    use kc_core::tool::{ApprovalGate, ApprovalHandle};
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct TestSettings;

    #[async_trait]
    impl Settings for TestSettings {
        async fn model(&self) -> ModelConfig {
            ModelConfig::default()
        }
        async fn limits(&self) -> Limits {
            Limits::default()
        }
        async fn approval_policy(&self) -> ApprovalPolicy {
            ApprovalPolicy::default()
        }
        async fn subagent_config(&self) -> SubagentConfig {
            SubagentConfig::default()
        }
    }

    struct AllowGate;

    #[async_trait]
    impl ApprovalGate for AllowGate {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::AllowOnce
        }
    }

    fn test_ctx(root: &Path) -> ToolCtx {
        ToolCtx {
            workspace: Arc::new(Workspace::new(root).unwrap()),
            cancel: CancellationToken::new(),
            approvals: ApprovalHandle::new(
                Arc::new(AllowGate),
                SessionId::new(),
                RunId::new(),
                None,
            ),
            events: kc_core::events::noop_sink(),
            settings: Arc::new(TestSettings),
            depth: 0,
        }
    }

    fn temp_workspace(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "kc-tools-command-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    async fn wait_for_pid(path: &Path) -> Option<u32> {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return Some(pid);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    async fn wait_until_gone(pid: u32, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !alive(pid)
    }

    async fn spawn_own_group(script: &str) -> (tokio::process::Child, u32) {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        unsafe {
            cmd.as_std_mut().pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("spawn /bin/sh");
        let pid = child.id().expect("child pid");
        (child, pid)
    }

    #[tokio::test]
    async fn an_already_reaped_child_is_never_signalled() {
        let (mut child, pid) = spawn_own_group("exit 0").await;
        let status = child.wait().await.unwrap();
        assert_eq!(status.code(), Some(0));

        // The leader is reaped: cleanup must not signal the (possibly reused) pid
        // and must return the status it was given.
        let cleaned = terminate_group(&mut child, pid, Some(status)).await;
        assert_eq!(cleaned.and_then(|s| s.code()), Some(0));
        assert!(cleaned.is_none() || cleaned.unwrap().signal().is_none());
    }

    #[tokio::test]
    async fn a_quickly_exiting_child_is_reaped_without_a_stray_kill() {
        let (mut child, pid) = spawn_own_group("exit 3").await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // No exit status was collected by the caller, but the child has already
        // exited: cleanup must reap it instead of signalling its group.
        let cleaned = terminate_group(&mut child, pid, None).await;
        let status = cleaned.expect("the exited child must be reaped");
        assert_eq!(status.code(), Some(3));
        assert!(
            status.signal().is_none(),
            "the child must not have been killed, got {status:?}"
        );
    }

    #[tokio::test]
    async fn a_running_group_is_terminated_and_reaped() {
        let (mut child, pid) = spawn_own_group("sleep 30").await;
        let started = Instant::now();
        let cleaned = terminate_group(&mut child, pid, None).await;
        assert!(
            started.elapsed() < KILL_GRACE + Duration::from_secs(1),
            "termination must be prompt, took {:?}",
            started.elapsed()
        );
        let status = cleaned.expect("the killed child must be reaped");
        assert!(status.signal().is_some(), "expected a signal exit");
    }

    #[test]
    fn bounded_output_keeps_head_and_tail() {
        let mut out = BoundedOutput::new(64);
        out.push(b"abcdefghij");
        out.push(&[b'x'; 200]);
        let finished = out.finish();
        assert!(finished.truncated);
        assert!(finished.text.contains("bytes omitted"));
        assert!(finished.text.starts_with("abcdefghij"));
    }

    #[test]
    fn truncated_output_is_trimmed_to_char_boundaries() {
        // 3-byte characters so both the head cut (32 bytes) and the tail cut
        // land in the middle of a character.
        let mut out = BoundedOutput::new(64);
        out.push(b"a");
        out.push("\u{20ac}".repeat(40).as_bytes());
        let finished = out.finish();
        assert!(finished.truncated);
        assert!(
            !finished.text.contains('\u{FFFD}'),
            "truncation must not split multi-byte characters: {}",
            finished.text
        );
        // Both retained segments decode cleanly and keep complete characters.
        let (head, tail) = finished.text.split_once("[...").expect("head/tail marker");
        assert!(head.starts_with('a'));
        assert!(head.trim_end().ends_with('\u{20ac}'), "{head:?}");
        assert!(tail.contains('\u{20ac}'), "{tail:?}");
    }

    #[tokio::test]
    async fn dropping_the_run_future_kills_the_whole_process_group() {
        let dir = temp_workspace("drop-guard");
        let leader_pid_file = dir.join("leader.pid");
        let child_pid_file = dir.join("child.pid");
        // The leader keeps a `sleep` child alive and waits for it, so a leader-only
        // kill would leave the child running - the failure mode this guard fixes.
        let command = format!(
            "echo $$ > '{}'; sleep 30 & echo $! > '{}'; wait",
            leader_pid_file.display(),
            child_pid_file.display()
        );
        let ctx = test_ctx(&dir);
        let request = CommandRequest {
            call_id: "drop".into(),
            command,
            cwd: dir.clone(),
            cwd_label: ".".into(),
            timeout: Duration::from_secs(60),
            max_output_bytes: 4096,
        };
        // A subagent hard timeout aborts the task, dropping the `run` future.
        let task = tokio::spawn(async move { run(request, &ctx).await });
        let leader = wait_for_pid(&leader_pid_file)
            .await
            .expect("the command should start and write its pid");
        let child = wait_for_pid(&child_pid_file)
            .await
            .expect("the command should start its sleep child");
        assert!(alive(leader) && alive(child));

        task.abort();
        let _ = task.await;

        assert!(
            wait_until_gone(leader, Duration::from_secs(2)).await,
            "leader {leader} survived the dropped run future"
        );
        assert!(
            wait_until_gone(child, Duration::from_secs(2)).await,
            "child {child} survived the dropped run future (group was not killed)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
