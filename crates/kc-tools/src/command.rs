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

/// How long the output pipes may stay open after the leader has exited.
///
/// A command may deliberately leave a child running (`sh -c 'build & echo
/// started; exit 0'`), and that child inherits the stdout/stderr pipes. The
/// leader is finished, so its exit status is the command's result; waiting for
/// EOF instead means a *successful* command is reported as a timeout as soon as
/// the background child outlives the timeout. The pipe is therefore given this
/// short grace to drain whatever the leader wrote, and anything still open after
/// it is reported as possibly-truncated output rather than blocking the call.
/// While the leader is *alive* the wait stays unbounded: a foreground command
/// that writes slowly must still stream to the end.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Bound queued pipe output; a full queue backpressures the child via its pipes.
const OUTPUT_QUEUE_CHUNKS: usize = 32;
const OUTPUT_CHUNK_BYTES: usize = 8192;

/// Environment variables a command inherits from the server.
///
/// The child gets a **cleared** environment plus this allowlist, never the
/// server's own environment: the server holds `KARUCODE_AUTH_TOKEN` and the
/// operator's provider API key, and a prompt-injected `curl` in a shell command
/// would otherwise exfiltrate them. Everything here is needed to run ordinary
/// tooling (find a compiler, resolve `~`, pick a locale/terminal/temp dir).
///
/// Operators can extend this per install with `limits.command_env_passthrough`
/// (names of extra variables to pass through); those names are filtered by
/// [`is_secret_like`] as well, so the list cannot be used to re-leak a secret.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TERM", "TMPDIR", "PWD", "TZ",
];

/// Locale forced on every command, as before the allowlist existed.
const FORCED_LOCALE: &str = "C";

/// Whether an environment variable name looks like it carries a credential.
///
/// Defence in depth on top of [`ENV_ALLOWLIST`]: even if a name reaches the
/// passthrough list (or a future allowlist entry), it is dropped here. Matching
/// is case-insensitive and anchored on `_` boundaries, so `MY_TOKENIZER` is not
/// mistaken for a token while `GITHUB_TOKEN` and `OPENAI_API_KEY` are caught.
fn is_secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const SECRET_PREFIXES: &[&str] = &["KARUCODE_", "AWS_", "OPENAI_", "ANTHROPIC_"];
    const SECRET_NAMES: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "NPM_TOKEN"];
    const SECRET_SUFFIXES: &[&str] = &["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];

    SECRET_PREFIXES.iter().any(|p| upper.starts_with(p))
        || SECRET_NAMES.contains(&upper.as_str())
        || SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// Clear the inherited environment and re-add only what a command may see.
///
/// `passthrough` holds extra variable *names* from
/// `limits.command_env_passthrough`; a name is only added when it is actually
/// present in the parent environment, so no empty values are invented.
fn apply_env_allowlist(cmd: &mut tokio::process::Command, passthrough: &[String]) {
    cmd.env_clear();
    for name in ENV_ALLOWLIST
        .iter()
        .copied()
        .chain(passthrough.iter().map(|s| s.as_str()))
    {
        if is_secret_like(name) {
            continue;
        }
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    // Set last so it wins over an inherited `LANG`/`LC_*`.
    cmd.env("LC_ALL", FORCED_LOCALE);
}

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
    /// True when the leader exited but something it started still held the
    /// output pipes open past [`DRAIN_GRACE`], so the captured output may be
    /// incomplete even though the command itself succeeded.
    pub background_output: bool,
    pub duration: Duration,
    pub stdout: FinishedStream,
    pub stderr: FinishedStream,
}

#[derive(Debug)]
pub struct FinishedStream {
    pub text: String,
    pub truncated: bool,
    pub total_bytes: usize,
    // Raw retained segments let the tool refit the stream to the *final* output
    // budget without truncating our own marker instead of command output.
    raw_head: Vec<u8>,
    raw_tail: Vec<u8>,
}

impl FinishedStream {
    /// Render one, self-describing head+tail cut within exactly `max_bytes`.
    pub(crate) fn render_with_budget(&self, max_bytes: usize) -> String {
        if self.text.len() <= max_bytes {
            return self.text.clone();
        }
        render_raw_stream(
            &self.raw_head,
            &self.raw_tail,
            self.total_bytes,
            self.truncated,
            max_bytes,
        )
    }
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
                raw_head: self.head.clone(),
                raw_tail: Vec::new(),
            };
        }
        // The cuts are byte offsets, so they can land inside a multi-byte
        // character. Trim both sides to char boundaries first: slicing there and
        // then decoding would emit U+FFFD garbage at the seam.
        let head = &self.head[..floor_char_boundary(&self.head, self.head.len())];
        let tail = &self.tail[ceil_char_boundary(&self.tail, 0)..];
        let dropped = self.total.saturating_sub(head.len() + tail.len());
        let text = format!(
            "{}\n{}\n{}",
            String::from_utf8_lossy(head),
            capture_marker(
                dropped,
                self.total,
                head.len(),
                tail.len(),
                &format!("limits.max_command_output_bytes = {}", self.max),
                self.max,
            ),
            String::from_utf8_lossy(tail),
        );
        FinishedStream {
            text,
            truncated: true,
            total_bytes: self.total,
            raw_head: self.head.clone(),
            raw_tail: self.tail.clone(),
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.total
    }
}

/// The marker a bounded command stream carries where its middle bytes were
/// dropped.
///
/// It states how much is missing, names the key that bounds capture, and gives
/// the one recovery that actually works for a pipe: write the output to a file
/// and read *that* with `read_file`, whose `offset=`/`limit=` can page through it.
/// Re-running a command with a bigger cap is the other option, and the key name
/// is what makes it possible to ask for it.
fn capture_marker(
    dropped: usize,
    total: usize,
    head: usize,
    tail: usize,
    bounds: &str,
    budget: usize,
) -> String {
    if budget >= 1_200 {
        return format!(
            "[... {dropped} of {total} bytes omitted (kept the first {head} and the last {tail}; \
             {bounds}): re-run it with the output redirected to a file, e.g. \
             `cmd > out.txt 2>&1`, then read `out.txt` with read_file `offset=`/`limit=`, or raise \
             limits.max_command_output_bytes ...]"
        );
    }
    if budget >= 512 {
        // A tighter budget still names the key and the recovery, in fewer bytes.
        return format!(
            "[... {dropped} of {total} bytes omitted (kept {head}+{tail}); redirect the command \
             (`cmd > out.txt 2>&1`) and read `out.txt` with read_file offset=/limit=, or raise \
             limits.max_command_output_bytes ...]"
        );
    }
    // The tightest budget keeps only what makes the cut recoverable at all: the
    // amount and the key. Anything longer would evict the trailing error this
    // retention exists for.
    format!("[... {dropped} of {total} bytes omitted; raise limits.max_command_output_bytes]")
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

/// Refit raw retained bytes instead of head-truncating a pre-formatted stream.
///
/// The marker has a stable size, so the complete rendered string - including the
/// marker - fits the requested budget. For a stream that had not previously been
/// cut, both halves come from its full raw bytes; otherwise they extend the head
/// and tail that BoundedOutput retained at capture time.
fn render_raw_stream(
    raw_head: &[u8],
    raw_tail: &[u8],
    total: usize,
    was_truncated: bool,
    max_bytes: usize,
) -> String {
    // Both caps are in play here: capture kept `limits.max_command_output_bytes`
    // of each stream and this final fit stays inside the tool-output budget.
    const BOUNDS: &str =
        "limits.max_command_output_bytes caps capture; limits.max_tool_output_bytes caps this result";
    // The marker reports the geometry it produces and its own length decides that
    // geometry, so estimate the marker, then rebuild it from the split it actually
    // made. Each pass that fits is kept so the seam uses every byte available.
    let mut reserve = 420.min(max_bytes);
    let mut best: Option<String> = None;
    let mut last_marker = String::new();
    for _ in 0..4 {
        // `+ 2` for the newlines that separate the marker from each half.
        let payload = max_bytes.saturating_sub(reserve + 2);
        let (head, tail, dropped) = split_stream(raw_head, raw_tail, total, was_truncated, payload);
        let marker = capture_marker(dropped, total, head.len(), tail.len(), BOUNDS, max_bytes);
        if head.len() + marker.len() + tail.len() + 2 <= max_bytes {
            best = Some(format!("{head}\n{marker}\n{tail}"));
        }
        let next = marker.len();
        last_marker = marker;
        if next == reserve {
            break;
        }
        reserve = next;
    }
    if let Some(out) = best {
        return out;
    }
    // Even the marker alone does not fit: a clipped marker still says the result
    // was cut, which is better than a silent prefix.
    clip_text(&last_marker, max_bytes)
}

/// Split one stream's retained bytes into a head and a tail of at most `payload`
/// bytes together, reporting how many bytes of the original stream that drops.
fn split_stream(
    raw_head: &[u8],
    raw_tail: &[u8],
    total: usize,
    was_truncated: bool,
    payload: usize,
) -> (String, String, usize) {
    let head_budget = payload / 2;
    let tail_budget = payload - head_budget;
    // Before the first cut, `raw_head` is the complete stream and `raw_tail` is
    // empty. Create a genuine tail from its final bytes rather than duplicating
    // the prefix or dropping the trailing diagnostic.
    let tail_source = if was_truncated { raw_tail } else { raw_head };
    let head_end = floor_char_boundary(raw_head, head_budget.min(raw_head.len()));
    let tail_start = ceil_char_boundary(tail_source, tail_source.len().saturating_sub(tail_budget));
    let head = clip_text(&String::from_utf8_lossy(&raw_head[..head_end]), head_budget);
    let tail = clip_tail(
        &String::from_utf8_lossy(&tail_source[tail_start..]),
        tail_budget,
    );
    let dropped = total.saturating_sub(head.len() + tail.len());
    (head, tail, dropped)
}

fn clip_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn clip_tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

/// Incremental UTF-8 decoding for live output events.
///
/// from_utf8_lossy on each independent pipe read turns a character split between
/// reads into two replacement characters. Keep only an incomplete trailing
/// sequence; malformed complete sequences are emitted immediately and the buffer
/// remains bounded to three bytes even for an arbitrary byte stream.
#[derive(Default)]
struct LiveUtf8 {
    pending: Vec<u8>,
}

impl LiveUtf8 {
    fn push(&mut self, chunk: &[u8]) -> String {
        if chunk.is_empty() {
            return String::new();
        }
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        let mut consumed = 0usize;
        loop {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(valid) => {
                    out.push_str(valid);
                    consumed = self.pending.len();
                    break;
                }
                Err(err) => {
                    let next = consumed + err.valid_up_to();
                    if let Some(invalid_len) = err.error_len() {
                        let end = next + invalid_len;
                        out.push_str(&String::from_utf8_lossy(&self.pending[consumed..end]));
                        consumed = end;
                    } else {
                        out.push_str(&String::from_utf8_lossy(&self.pending[consumed..next]));
                        consumed = next;
                        break;
                    }
                }
            }
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        debug_assert!(self.pending.len() <= 3);
        out
    }

    fn finish(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        out
    }
}

/// Kills the whole process group if the `run` future is dropped before the
/// group is confirmed empty (task abort, subagent hard timeout).
///
/// `kill_on_drop(true)` only signals the leader, which called `setsid()` and is
/// therefore the group leader: its children (`sleep` under a shell, a build's
/// compiler, ...) would survive. The guard holds the **group id**, not the
/// leader pid, and stays armed until `kill(-pgid, 0)` reports the group empty:
/// a reaped leader says nothing about its children, and the kernel keeps the
/// group id allocated while any member lives, so signalling it cannot hit a
/// reused pid.
struct GroupGuard {
    pgid: u32,
    armed: bool,
}

impl GroupGuard {
    fn new(pgid: u32) -> Self {
        Self { pgid, armed: true }
    }

    /// Disarm only when the group has no members left.
    fn disarm_if_empty(&mut self) {
        if !group_alive(self.pgid) {
            self.armed = false;
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            signal_group(self.pgid, libc::SIGKILL);
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The child must not inherit the server's environment: it holds the harness
    // bearer token and the provider API key. Only the allowlist is passed on,
    // plus any names the operator opted into via `limits.command_env_passthrough`
    // (which still drops credential-shaped names).
    let passthrough = ctx.limits().await.command_env_passthrough;
    apply_env_allowlist(&mut cmd, &passthrough);
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

    let (tx, mut rx) = mpsc::channel::<(Stream, Vec<u8>)>(OUTPUT_QUEUE_CHUNKS);
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
    let mut live_stdout = LiveUtf8::default();
    let mut live_stderr = LiveUtf8::default();
    let mut timed_out = false;
    let mut cancelled = false;

    let deadline = tokio::time::sleep(request.timeout);
    tokio::pin!(deadline);
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut streams_open = true;
    let mut background_output = false;
    // Armed when the leader exits; while it is unarmed the pipe wait is unbounded.
    let mut drain_grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    loop {
        // Leaving the loop means "stop reading". Two things can end it: both
        // pipes reached EOF *and* the leader was reaped (nothing more can
        // arrive), or the leader exited and its pipes stayed open past the grace
        // period (a background child still holds them). The second case is not a
        // timeout: the leader's exit status is already known and is what the
        // caller asked for. While the leader is alive the wait is unbounded, so a
        // command that closes both pipes and keeps working still finishes.
        if !streams_open && exit_status.is_some() {
            break;
        }
        // The leader is gone and its drain grace is over (or it never armed one
        // because the pipes had already closed): stop reading. Note the reverse of
        // this rule is what used to be wrong - requiring EOF *and* an exit status
        // meant a backgrounded child kept the call open until the timeout.
        if exit_status.is_some() && drain_grace.is_none() {
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
            _ = async { drain_grace.as_mut().unwrap().as_mut().await }, if drain_grace.is_some() => {
                // The leader is done and its pipes are still open: something it
                // started in the background is holding them. Keep what was
                // captured and report success with a note.
                background_output = streams_open;
                break;
            }
            chunk = rx.recv(), if streams_open => match chunk {
                Some((stream, bytes)) => {
                    let text = match stream {
                        Stream::Stdout => live_stdout.push(&bytes),
                        Stream::Stderr => live_stderr.push(&bytes),
                    };
                    if !text.is_empty() {
                        ctx.emit(Event::CommandOutput {
                            call_id: request.call_id.clone(),
                            stream: stream.label().to_string(),
                            chunk: text,
                        });
                    }
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
                        // The leader is reaped, but its children may still be
                        // running: only disarm once the group is empty, so a
                        // dropped future still kills the survivors.
                        group.disarm_if_empty();
                        // Start the bounded wait for the pipe drain (see
                        // `DRAIN_GRACE`); `streams_open` false already broke out.
                        if streams_open {
                            drain_grace =
                                Some(Box::pin(tokio::time::sleep(DRAIN_GRACE)));
                        }
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
    // The leader is reaped; keep the guard armed until the group is empty so a
    // surviving grandchild cannot keep the pipes open past this call. On the
    // drain-grace path (leader exited, a background child still holds a pipe) this
    // is also what reaps that child: `group` is dropped at the end of `run`, never
    // disarmed while a member lives.
    group.disarm_if_empty();
    for pump in pumps {
        pump.abort();
    }

    // A command may end between the bytes of one character. Emit only one
    // replacement for genuinely incomplete trailing input, not one per pipe read.
    for (stream, text) in [
        (Stream::Stdout, live_stdout.finish()),
        (Stream::Stderr, live_stderr.finish()),
    ] {
        if !text.is_empty() {
            ctx.emit(Event::CommandOutput {
                call_id: request.call_id.clone(),
                stream: stream.label().to_string(),
                chunk: text,
            });
        }
    }

    let duration = started.elapsed();
    let exit_code = status.and_then(|s| s.code());
    let outcome = CommandOutcome {
        exit_code,
        timed_out,
        cancelled,
        background_output,
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

async fn pump<R>(mut reader: R, stream: Stream, tx: mpsc::Sender<(Stream, Vec<u8>)>)
where
    R: AsyncReadExt + Unpin,
{
    let mut buf = vec![0u8; OUTPUT_CHUNK_BYTES];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send((stream, buf[..n].to_vec())).await.is_err() {
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

/// Whether any process is still a member of the group `pgid`.
///
/// `kill(-pgid, 0)` returns ESRCH once the group is empty. While any member
/// lives the kernel keeps the group id allocated, so the id cannot have been
/// reused by an unrelated process group.
fn group_alive(pgid: u32) -> bool {
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    if rc == 0 {
        return true;
    }
    // EPERM means the group exists but is not ours to signal: still alive.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// SIGTERM the group, wait up to [`KILL_GRACE`], then SIGKILL. Returns the exit
/// status when it could be collected.
///
/// The escalation is unconditional: a leader that dies on SIGTERM says nothing
/// about its children. A grandchild that ignores SIGTERM keeps the stdout/stderr
/// pipes open, so the pump tasks never see EOF and the tool call hangs. The
/// group id stays valid while any member lives, so `kill(-pgid, SIGKILL)` is
/// safe even after the leader has been reaped.
async fn terminate_group(
    child: &mut tokio::process::Child,
    pgid: u32,
    reaped: Option<std::process::ExitStatus>,
) -> Option<std::process::ExitStatus> {
    if let Some(status) = reaped {
        // The leader is gone, but the group may not be: kill whatever is left.
        if group_alive(pgid) {
            signal_group(pgid, libc::SIGKILL);
        }
        return Some(status);
    }
    match child.try_wait() {
        Ok(Some(status)) => {
            if group_alive(pgid) {
                signal_group(pgid, libc::SIGKILL);
            }
            return Some(status);
        }
        Ok(None) => {}
        // The state is unknown; never risk signalling a reused pid.
        Err(_) => return None,
    }

    signal_group(pgid, libc::SIGTERM);
    if let Ok(Ok(status)) = tokio::time::timeout(KILL_GRACE, child.wait()).await {
        // The leader exited on SIGTERM. Its children may have ignored it, so
        // escalate anyway: this is the orphan case.
        if group_alive(pgid) {
            signal_group(pgid, libc::SIGKILL);
        }
        return Some(status);
    }

    // The leader survived the grace period: kill the whole group, then reap.
    signal_group(pgid, libc::SIGKILL);
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

    #[tokio::test]
    async fn pump_backpressures_and_stops_when_receiver_drops() {
        let (tx, mut rx) = mpsc::channel(1);
        let data = vec![b'x'; OUTPUT_CHUNK_BYTES * 3];
        let task = tokio::spawn(pump(std::io::Cursor::new(data), Stream::Stdout, tx));
        tokio::time::timeout(Duration::from_secs(1), async {
            while rx.len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "full queue must block the pump");
        let (stream, bytes) = rx.recv().await.unwrap();
        assert_eq!(stream, Stream::Stdout);
        assert_eq!(bytes.len(), OUTPUT_CHUNK_BYTES);
        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

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
            supports_images: false,
            checkpoints: None,
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
            // Case must not matter: a lower-case name is still a credential.
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
            // `*_TOKEN` must match on the suffix boundary, not as a substring.
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
    fn the_allowlist_replaces_the_inherited_environment() {
        let _guard = EnvGuard::set("KARUCODE_TEST_ALLOWLIST_TOKEN", "leak-me");
        let mut cmd = tokio::process::Command::new("/bin/sh");
        apply_env_allowlist(&mut cmd, &[]);

        let env: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        assert!(
            !names.contains(&"KARUCODE_TEST_ALLOWLIST_TOKEN"),
            "a secret-like variable must never be passed through: {names:?}"
        );
        assert!(
            !names.iter().any(|n| is_secret_like(n)),
            "no secret-like name may survive the allowlist: {names:?}"
        );
        // `LC_ALL` is always forced, and `PATH` is present in any test runner.
        assert!(names.contains(&"LC_ALL"), "{names:?}");
        assert!(names.contains(&"PATH"), "{names:?}");
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "LC_ALL")
                .unwrap()
                .1
                .as_deref(),
            Some("C")
        );
        // Nothing outside the allowlist leaks in.
        for name in ["CARGO_HOME", "RUSTUP_HOME", "KARUCODE_CONFIG"] {
            assert!(
                !names.contains(&name),
                "{name} must not be inherited: {names:?}"
            );
        }
    }

    #[test]
    fn passthrough_names_are_added_unless_they_look_like_secrets() {
        // Deliberately not `KARUCODE_*`: that prefix is filtered outright, and
        // this test is about the suffix rule and about absent names.
        let _guard = EnvGuard::set("KC_TEST_PASSTHROUGH_VALUE", "kept");
        let _secret = EnvGuard::set("KC_TEST_PASSTHROUGH_TOKEN", "leak-me");
        let mut cmd = tokio::process::Command::new("/bin/sh");
        apply_env_allowlist(
            &mut cmd,
            &[
                "KC_TEST_PASSTHROUGH_VALUE".to_string(),
                "KC_TEST_PASSTHROUGH_TOKEN".to_string(),
                // Absent from the parent: must not be invented as an empty value.
                "KC_TEST_PASSTHROUGH_MISSING".to_string(),
            ],
        );

        let env: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        assert!(names.contains(&"KC_TEST_PASSTHROUGH_VALUE"), "{names:?}");
        assert!(
            !names.contains(&"KC_TEST_PASSTHROUGH_TOKEN"),
            "the passthrough list must not be able to re-leak a secret: {names:?}"
        );
        assert!(
            !names.contains(&"KC_TEST_PASSTHROUGH_MISSING"),
            "an absent variable must not be passed as an empty value: {names:?}"
        );
    }

    #[tokio::test]
    async fn a_secret_in_the_parent_environment_does_not_reach_the_command() {
        // The exact failure this allowlist fixes: the server's own bearer token
        // and provider key used to be inherited by every shell command.
        let _guard = EnvGuard::set("KARUCODE_TEST_LEAK_TOKEN", "leak-me");
        let dir = temp_workspace("env-leak");
        let ctx = test_ctx(&dir);
        let request = CommandRequest {
            call_id: "env".into(),
            command: "echo \"token=${KARUCODE_TEST_LEAK_TOKEN:-ABSENT}\"; \
                      echo \"path=${PATH:+PATH-SET}\""
                .into(),
            cwd: dir.clone(),
            cwd_label: ".".into(),
            timeout: Duration::from_secs(30),
            max_output_bytes: 4096,
        };
        let outcome = run(request, &ctx).await.expect("the command should run");
        assert!(outcome.ok(), "stderr: {}", outcome.stderr.text);
        assert!(
            outcome.stdout.text.contains("token=ABSENT"),
            "the parent's secret leaked into the command: {:?}",
            outcome.stdout.text
        );
        // The allowlist must still let ordinary tooling work.
        assert!(
            outcome.stdout.text.contains("path=PATH-SET"),
            "PATH must still be passed through: {:?}",
            outcome.stdout.text
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn live_utf8_is_decoded_incrementally_without_split_replacements() {
        let original = "A€中🙂Z";
        let mut decoder = LiveUtf8::default();
        let mut rendered = String::new();
        for byte in original.as_bytes() {
            rendered.push_str(&decoder.push(std::slice::from_ref(byte)));
        }
        rendered.push_str(&decoder.finish());
        assert_eq!(rendered, original);
        assert!(decoder.pending.is_empty());

        // Each stream has its own state, and malformed complete bytes are flushed
        // immediately instead of growing `pending` with an unbounded suffix.
        let mut stdout = LiveUtf8::default();
        let mut stderr = LiveUtf8::default();
        assert_eq!(stdout.push(&[0xE2]), "");
        assert_eq!(stderr.push(&[b'X', 0xFF]), "X\u{fffd}");
        assert_eq!(stdout.push(&[0x82, 0xAC]), "€");
        assert_eq!(stdout.finish(), "");
        stdout.push(&vec![0x80; 8192]);
        assert!(stdout.pending.len() <= 3);
        assert_eq!(stdout.finish(), "");
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

    /// Bug 5: the leader dies on SIGTERM but a grandchild ignores it. The old
    /// code only escalated when `try_wait()` still reported the leader running,
    /// so the reaped leader meant SIGKILL was never sent and the grandchild
    /// survived - keeping the stdout/stderr pipes open forever.
    #[tokio::test]
    async fn a_term_ignoring_grandchild_does_not_survive_termination() {
        let dir = temp_workspace("term-ignoring");
        let gc_pid_file = dir.join("gc.pid");
        let script = dir.join("gc.sh");
        // The grandchild ignores SIGTERM and loops, so only SIGKILL can stop it.
        std::fs::write(
            &script,
            format!(
                "trap '' TERM\necho $$ > '{}'\nwhile true; do sleep 1; done\n",
                gc_pid_file.display()
            ),
        )
        .unwrap();

        // The leader does not trap SIGTERM, so it dies on the first signal while
        // the grandchild lives on: exactly the orphan case.
        let (mut child, pid) =
            spawn_own_group(&format!("/bin/sh '{}' & wait", script.display())).await;
        let grandchild = wait_for_pid(&gc_pid_file)
            .await
            .expect("the grandchild should start and write its pid");
        assert!(
            alive(grandchild),
            "grandchild {grandchild} should be running"
        );

        let cleaned = terminate_group(&mut child, pid, None).await;
        assert!(cleaned.is_some(), "the leader must be reaped");

        assert!(
            wait_until_gone(grandchild, Duration::from_secs(3)).await,
            "grandchild {grandchild} survived termination: SIGKILL was never sent \
             because the leader had already been reaped"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The group id stays valid while any member lives, so the guard must not
    /// disarm just because the leader was reaped.
    #[tokio::test]
    async fn the_group_guard_stays_armed_while_a_member_lives() {
        let dir = temp_workspace("guard-armed");
        let gc_pid_file = dir.join("gc.pid");
        let script = dir.join("gc.sh");
        std::fs::write(
            &script,
            format!(
                "trap '' TERM\necho $$ > '{}'\nwhile true; do sleep 1; done\n",
                gc_pid_file.display()
            ),
        )
        .unwrap();

        // The leader exits immediately, leaving the grandchild as the only
        // member of the group: the leader being reaped must not disarm the guard.
        let (mut child, pid) =
            spawn_own_group(&format!("/bin/sh '{}' & exit 0", script.display())).await;
        let grandchild = wait_for_pid(&gc_pid_file)
            .await
            .expect("the grandchild should start and write its pid");

        // Reap the leader only; the grandchild is still a group member.
        let _ = child.wait().await;
        assert!(alive(grandchild));
        assert!(
            group_alive(pid),
            "the group must still be reported alive while a member lives"
        );

        let mut guard = GroupGuard::new(pid);
        guard.disarm_if_empty();
        drop(guard);
        assert!(
            wait_until_gone(grandchild, Duration::from_secs(3)).await,
            "dropping the guard must kill the surviving group member"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
