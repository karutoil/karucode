// @kc-doc: terminals
// @kc-title: Use the terminal tab
// @kc-summary: A real shell in the browser that survives a reload, another browser and a cleared cache - and the honest limit of what survives a restart.
// @kc-tags: terminal, shell, pty, websocket, ui, tabs
// @kc-applies-to: route:GET /api/terminals, route:POST /api/terminals, route:PATCH /api/terminals/:id, route:DELETE /api/terminals/:id, route:POST /api/terminals/:id/restart, route:GET /api/terminals/:id/socket, config:[terminals], config:terminals.enabled, config:terminals.shell, config:terminals.scrollback_bytes, config:terminals.max_terminals, config:terminals.idle_timeout_secs, config:limits.command_env_passthrough, env:KARUCODE_TERMINALS, env:KARUCODE_TERMINALS_SHELL, env:KARUCODE_TERMINALS_SCROLLBACK_BYTES, env:KARUCODE_TERMINALS_MAX, env:KARUCODE_TERMINAL_IDLE_SECS, env:KARUCODE_ALLOWED_HOSTS, ui:TerminalTab, file:web/src/components/TerminalTab.tsx, file:crates/kc-server/src/terminals.rs
//
// The **Terminal** tab in the chat area runs a real shell in a real PTY as the
// server user, in the current project's directory. It is a shell for *you*, the
// person operating the machine: it is not a tool the model can call and it does
// not ask for approval, because the human typing at it is already authorised -
// which is exactly what `run_command` asks them about.
//
// # What "survives" means
//
// | Event | Terminals |
// | --- | --- |
// | Reload the page, open another browser, clear site data | **all of them**, reattached to the same live shells with their scrollback replayed |
// | Restart karucode, reboot the machine | the **tabs** - same ids, titles, order and directory - each reopening a fresh shell |
//
// The distinction is deliberate and it is the one thing to understand about
// this feature. A shell is a child process of the server. Keeping it alive
// across a server restart would need a detached daemon, and a terminal that
// survives `systemctl restart karucode` is a terminal that outlives the thing
// that could kill it. What is durable is the **session row** in SQLite: the tab
// strip, the titles, the working directory and the position are global to the
// install, not to a browser profile or a `localStorage` entry.
//
// # Creating and naming
//
// ```bash
// # A new shell in the default workspace
// curl -s -X POST localhost:8787/api/terminals \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{}'
//
// # A name and a directory (the directory must be inside a workspace root)
// curl -s -X POST localhost:8787/api/terminals \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"title":"deploy","cwd":"/home/me/app","project":"p_123"}'
//
// curl -s -X PATCH localhost:8787/api/terminals/t_abc \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"title":"build watcher"}'      # {"title":null} restores the default
//
// curl -s -X DELETE localhost:8787/api/terminals/t_abc -H 'X-Karucode-Client: web'
// curl -s -X POST localhost:8787/api/terminals/t_abc/restart -H 'X-Karucode-Client: web'
// ```
//
// `POST` also accepts `cols` and `rows` (default 80x24), clamped to 20..=500 and
// 5..=200. They only decide the initial PTY size; the browser resizes it as soon
// as the terminal renders.
//
// # Attaching: `GET /api/terminals/:id/socket`
//
// This is the only websocket in karucode. It is a raw byte pipe with a small
// JSON control channel:
//
// * **binary frames** carry PTY bytes verbatim in both directions - the browser
//   sends what you type, the server sends what the shell printed. Nothing is
//   interpreted, so a program that emits arbitrary bytes cannot inject a control
//   message;
// * **text frames** are JSON and carry only control: `{"type":"resize","cols":120,"rows":40}`,
//   and, from the server, `{"type":"ready","replay":true,"pid":1234}`,
//   `{"type":"exit","code":0}`, `{"type":"error","message":"..."}`. Sending
//   `{"type":"ping"}` answers only that socket with text `{"type":"pong"}`;
//   control JSON is never broadcast or disguised as binary PTY output.
//
// On attach the server first sends the tail of the replay buffer (the newest
// `terminals.scrollback_bytes` of output) as one binary frame, then streams live
// output. That is what makes a reconnect - a reload, a network blip, a second
// browser tab - land on a terminal that already looks like it did.
//
// Because a browser cannot set a header on a websocket handshake, this route
// also accepts `?token=<auth token>` exactly like `GET /api/events`. The
// `Origin` header must still match `Host` (a cross-origin page cannot open it),
// and the origin check is enforced even when no token is configured.
//
// "Match" is exact - scheme, host and **port**. The one exception is a reverse
// proxy, and there it is not a loopback shortcut: the `Origin`'s authority must
// appear as an explicit `KARUCODE_ALLOWED_HOSTS` entry. So
// `Origin: http://localhost:9999` against `Host: 127.0.0.1:8787` is refused
// (they are different origins, whatever the port is doing), while a proxy
// fronting `https://karucode.example.com` still works because that name is
// listed. Vite development and preview servers proxy `/api` **with `ws: true`**,
// so terminals work there too; the origin check is still enforced on the upgrade.
// A missing `Origin` is a non-browser client and is accepted (the `Host` guard and
// the bearer token are the gate for those), never a web page.
//
// # The shell
//
// `terminals.shell`, or `$SHELL`, or `/bin/sh` - in that order. The value is
// validated when settings are saved, so a bad path is an error at that moment
// rather than a websocket that fails every time the tab opens.
//
// The child gets a **cleared environment** plus the same allowlist
// `run_command` uses (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`,
// `LC_ALL`, `TERM`, `TMPDIR`, `PWD`, `TZ`) and anything in
// `limits.command_env_passthrough`, using each name's current value only when set.
// Credential-shaped names (`KARUCODE_*`, `AWS_*`, `OPENAI_*`, `ANTHROPIC_*`,
// `*_API_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD`, and the named GitHub/npm tokens)
// are dropped even if configured. `KARUCODE_AUTH_TOKEN` and provider keys therefore
// do not reach a shell's environment: an interactive session is exactly where
// `env | curl` is easiest to type.
//
// Each shell is its own session and process group, so closing the tab, deleting
// the terminal or stopping the server kills the whole tree - a `make watch` left
// running in a terminal does not survive it. Shutdown sends SIGHUP, escalates to
// SIGKILL after two seconds when needed, and waits for the leader and PTY close;
// a shell that ignores SIGHUP therefore cannot escape through process bootstrap.
//
// # Limits
//
// | Key | Default | What it bounds |
// | --- | --- | --- |
// | `terminals.enabled` | `true` | `false` removes the surface entirely |
// | `terminals.max_terminals` | `16` | concurrent PTYs, `1`-`64` |
// | `terminals.scrollback_bytes` | `262144` | replay per terminal, 4 KiB-4 MiB |
// | `terminals.idle_timeout_secs` | `43200` | reap a shell with no browser for this long; `0` keeps it |
//
// When `max_terminals` is reached a create is a `400 bad_request` naming the
// limit and telling you to close one - it never silently replaces a running
// shell.
//
// # Gotchas
//
// * **There is no sandbox.** A terminal runs as the server user with that user's
//   whole filesystem and network. Approvals apply to the *model*, not to you.
//   Set `KARUCODE_TERMINALS=0` on a deployment that must not offer one.
// * The terminal is **not confined to the project** the way file tools are: you
//   can `cd /` in it. The `cwd` a terminal starts in is checked against the
//   workspace roots, but the shell itself is an ordinary shell.
// * **One attached browser per terminal.** A second tab attaching to the same
//   terminal takes over the stream and the first is told so with
//   `{"type":"detached"}`. Two independent views of one PTY would fight over
//   the window size and replay, so they are not offered.
// * `idle_timeout_secs` closes the *process* of a terminal with no connected
//   browser. The tab stays; opening it starts a fresh shell in the same place.
// * Resize is per attached browser. Closing the laptop lid does not resize
//   anything; the next attach sends its own size.
//
// Full reference: `docs/API.md` (terminals), `config.example.toml`.

use crate::error::{ApiError, ApiResult};
use crate::host::HostAllowlist;
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use kc_core::config::TerminalConfig;
use kc_core::store::TerminalSession;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::Instant;

/// Output kept for a reconnecting browser, in bytes, when no setting is
/// available. Matches [`kc_core::config::TerminalConfig::default`].
#[allow(dead_code)]
const FALLBACK_SCROLLBACK: usize = 256 * 1024;

/// Broadcast capacity for live output. A viewer this many chunks behind is
/// dropped from the stream (and resynchronised from the replay buffer) rather
/// than slowing the shell down: PTY output must never block on a browser.
const OUTPUT_CHANNEL: usize = 256;

/// Bound per-terminal replay snapshots and stalled websocket tasks.
const MAX_ATTACHMENTS: usize = 4;
const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a `pty` write may block before the reader is treated as gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Grace between SIGHUP and SIGKILL when a shell is closed deliberately.
///
/// A shell that is asked to hang up usually exits; one that traps SIGHUP - or is
/// in the middle of a build - gets two seconds and then the whole group is
/// killed. Without the second signal a stubborn child would keep its PTY open
/// and leak the descriptor.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Terminal sizes below this are not worth rendering and usually mean a
/// hidden element measured as zero.
const MIN_COLS: u16 = 20;
const MAX_COLS: u16 = 500;
const MIN_ROWS: u16 = 5;
const MAX_ROWS: u16 = 200;
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// A shell reading and writing one PTY.
///
/// The master side lives in a blocking thread: `tokio::io::unix::AsyncFd` would
/// work, but the read loop also has to write, resize and notice a dead child,
/// and one thread per terminal that spends its life in `read(2)` is both simpler
/// and cheaper than three tasks plus an interest register. A terminal is an
/// inherently thread-per-shell thing; the server's thread count is
/// `max_terminals`, which the settings bound.
pub struct Terminal {
    pub id: String,
    /// The replay buffer: newest bytes win.
    replay: Arc<Replay>,
    /// Live output. Senders other than the reader only push control messages.
    output: broadcast::Sender<Output>,
    /// Writes to the PTY master, and resizes. One mutex because a `TIOCSWINSZ`
    /// interleaved with a write is not a problem, but a *closed* fd is: the
    /// writer task holds the lock while it writes, and shutdown takes it before
    /// closing.
    io: Arc<Mutex<IoState>>,
    /// Set once the shell has exited or been killed; a later attach starts a new
    /// one.
    dead: Arc<AtomicBool>,
    /// `kill(2)` target for the leader, and the process group. `0` for a shell
    /// that never started.
    pid: Arc<AtomicU32>,
    /// When a browser last attached or detached, in unix millis.
    last_seen: Arc<AtomicI64>,
    /// How many browsers are attached right now.
    attached: Arc<AtomicU32>,
}

/// One frame the shell sends to attached browsers.
#[derive(Clone, Debug)]
pub enum Output {
    /// Raw PTY bytes.
    Data(Vec<u8>),
    /// The shell exited with this status.
    Exit(Option<i32>),
}

struct IoState {
    /// Duplicated master fd. `None` after shutdown, which is what makes a write
    /// after close an error instead of a write into a recycled descriptor.
    master: Option<std::fs::File>,
    /// The child, waited on by whoever notices the exit first.
    child: Option<std::process::Child>,
}

/// Bounded newest-wins byte ring for the attach replay.
///
/// A `VecDeque<u8>` would be simpler but a byte-at-a-time push is slow for a
/// build log, and trimming from the front of a `Vec` is O(n). This keeps chunks
/// and drops whole chunks from the front, so appends are a push and the total is
/// bounded by `capacity + largest chunk`.
struct Replay {
    capacity: usize,
    chunks: Mutex<ReplayChunks>,
}

#[derive(Default)]
struct ReplayChunks {
    /// `(bytes, start)` - `start` lets a partial trim of the oldest chunk happen
    /// without shifting every later one.
    chunks: std::collections::VecDeque<(Vec<u8>, usize)>,
    /// Total live bytes across every chunk.
    len: usize,
}

impl Replay {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1024),
            chunks: Mutex::new(ReplayChunks::default()),
        }
    }

    fn push(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut state = self.chunks.lock().unwrap_or_else(|e| e.into_inner());
        // One chunk larger than the whole buffer is trimmed on read; keeping it
        // whole is what makes a single huge redraw cheap.
        state.chunks.push_back((bytes.to_vec(), 0));
        state.len += bytes.len();
        while state.len > self.capacity {
            let excess = state.len - self.capacity;
            let Some(front) = state.chunks.front_mut() else {
                break;
            };
            let live = front.0.len() - front.1;
            if live <= excess {
                state.len -= live;
                state.chunks.pop_front();
            } else {
                front.1 += excess;
                state.len -= excess;
                break;
            }
        }
    }

    /// The newest bytes that fit in `capacity`, oldest first.
    fn snapshot(&self) -> Vec<u8> {
        let state = self.chunks.lock().unwrap_or_else(|e| e.into_inner());
        let skip = state.len.saturating_sub(self.capacity);
        let mut out = Vec::with_capacity(state.len.min(self.capacity));
        let mut skipped = 0usize;
        for (bytes, start) in state.chunks.iter() {
            let mut live = &bytes[*start..];
            // Drop the oldest bytes first, so the answer is always the *newest*
            // `capacity` bytes in their original order.
            if skipped < skip {
                let drop = (skip - skipped).min(live.len());
                skipped += drop;
                live = &live[drop..];
            }
            if live.is_empty() {
                continue;
            }
            out.extend_from_slice(live);
        }
        // One last clamp: the loop above can briefly exceed the cap by less than
        // one chunk when several chunks partially overlapped the boundary.
        if out.len() > self.capacity {
            out.drain(..out.len() - self.capacity);
        }
        out
    }

    fn len(&self) -> usize {
        self.chunks.lock().unwrap_or_else(|e| e.into_inner()).len
    }
}

impl Terminal {
    /// Start a shell for `session`, in `session.cwd`, at `cols`x`rows`.
    ///
    /// `settings` is passed rather than read from a handle so a unit test can
    /// exercise the PTY without a whole `AppState`.
    pub fn spawn(
        id: &str,
        session: &TerminalSession,
        settings: &TerminalConfig,
        env_passthrough: &[String],
        cols: u16,
        rows: u16,
    ) -> kc_core::Result<Arc<Self>> {
        let (cols, rows) = (clamp_cols(cols), clamp_rows(rows));
        let (master, slave) = open_pty(cols, rows)?;

        // The slave fd is what the child gets on 0/1/2. It must not stay open in
        // the parent - an open slave keeps the master from ever reading EOF.
        let slave_stdout = dup_fd(&slave)?;
        let slave_stderr = dup_fd(&slave)?;

        let shell = settings.shell_program();
        let mut command = std::process::Command::new(&shell);
        command
            .current_dir(&session.cwd)
            .stdin(Stdio::from(slave))
            .stdout(Stdio::from(slave_stdout))
            .stderr(Stdio::from(slave_stderr));
        apply_environment(&mut command, env_passthrough);
        // New session => new process group, so closing the terminal reaches
        // every child (a `make watch`, a `sleep`), not just the leader.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // The controlling terminal for `setsid`'s new session. Without
                // this, job control (Ctrl+C, Ctrl+Z) has no foreground group to
                // signal and the shell prints "no job control in this shell".
                if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|e| {
            kc_core::Error::Tool(format!("could not start `{shell}` in {}: {e}", session.cwd))
        })?;
        let pid = child.id();

        let replay = Arc::new(Replay::new(settings.replay_capacity()));
        let (output, _) = broadcast::channel(OUTPUT_CHANNEL);
        let io = Arc::new(Mutex::new(IoState {
            master: Some(master),
            child: Some(child),
        }));
        let dead = Arc::new(AtomicBool::new(false));
        let pid_cell = Arc::new(AtomicU32::new(pid));
        let last_seen = Arc::new(AtomicI64::new(crate::state::now_ms()));
        let attached = Arc::new(AtomicU32::new(0));

        let terminal = Arc::new(Self {
            id: id.to_string(),
            replay,
            output,
            io,
            dead,
            pid: pid_cell,
            last_seen,
            attached,
        });
        spawn_reader(Arc::clone(&terminal));
        Ok(terminal)
    }

    /// True when the shell has exited and a new one is needed.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    pub fn pid(&self) -> u32 {
        self.pid.load(Ordering::Relaxed)
    }

    /// Subscribe to live output. Take this *before* reading [`Terminal::replay`]
    /// so bytes produced in between are neither lost nor duplicated twice.
    pub fn subscribe(&self) -> broadcast::Receiver<Output> {
        self.output.subscribe()
    }

    /// The replay buffer, oldest first.
    pub fn replay(&self) -> Vec<u8> {
        self.replay.snapshot()
    }

    pub fn replay_len(&self) -> usize {
        self.replay.len()
    }

    /// Write bytes from a browser to the shell.
    pub fn write(&self, bytes: &[u8]) -> bool {
        if self.is_dead() {
            return false;
        }
        // Track partial writes: write_all may write a prefix before WouldBlock,
        // and retrying the entire frame would duplicate that prefix.
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut pending = bytes;
        while !pending.is_empty() {
            if self.is_dead() || Instant::now() > deadline {
                return false;
            }
            let result = {
                let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
                let Some(master) = io.master.as_mut() else {
                    return false;
                };
                master.write(pending)
            };
            match result {
                Ok(0) => return false,
                Ok(n) => pending = &pending[n..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
        true
    }

    /// Tell the kernel the window size changed, so `vi`, `less`, `top` and every
    /// readline prompt redraw at the right width.
    pub fn resize(&self, cols: u16, rows: u16) -> bool {
        let cols = clamp_cols(cols);
        let rows = clamp_rows(rows);
        let io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let Some(master) = io.master.as_ref() else {
            return false;
        };
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // `TIOCSWINSZ` also delivers SIGWINCH to the foreground group.
        unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as libc::c_ulong, &size) == 0 }
    }

    /// Terminal size the kernel currently reports, for a test and for a
    /// diagnostic message.
    pub fn size(&self) -> Option<(u16, u16)> {
        let io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let master = io.master.as_ref()?;
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::ioctl(
                master.as_raw_fd(),
                libc::TIOCGWINSZ as libc::c_ulong,
                &mut size,
            )
        };
        (rc == 0).then_some((size.ws_col, size.ws_row))
    }

    pub fn mark_attached(&self) {
        self.attached.fetch_add(1, Ordering::AcqRel);
        self.last_seen
            .store(crate::state::now_ms(), Ordering::Release);
    }

    pub fn mark_detached(&self) {
        // Publish the new idle timestamp before the final attachment disappears.
        // An acquire load observing zero must also observe this timestamp.
        self.last_seen
            .store(crate::state::now_ms(), Ordering::Release);
        // `Atomic::fetch_update` is `try_update` from Rust 1.100. The workspace's
        // MSRV is 1.89, where only the old name exists, so the old name stays (and
        // the deprecation is allowed) until the MSRV moves.
        #[allow(deprecated)]
        let _ = self
            .attached
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
    }

    pub fn attached(&self) -> u32 {
        self.attached.load(Ordering::Acquire)
    }

    /// Unix millis of the last attach, detach or activity.
    pub fn last_seen(&self) -> i64 {
        self.last_seen.load(Ordering::Acquire)
    }

    /// A browser typed something: this counts as use for the idle reaper.
    pub fn record_activity(&self) {
        self.last_seen
            .store(crate::state::now_ms(), Ordering::Release);
    }

    /// Hang up and reap the shell: SIGHUP to the group, SIGKILL after
    /// [`KILL_GRACE`].
    ///
    /// Closing the master is what actually ends the child's `read`, so the
    /// process is signalled first (so it can run its exit handlers) and the fd
    /// is closed after the grace period or as soon as it exits. This call does not
    /// return until escalation, `wait(2)`, and the close have completed; spawning
    /// an unjoined cleanup thread let the process exit while a HUP-ignoring shell
    /// was still alive.
    pub fn shutdown(&self) {
        if self.dead.swap(true, Ordering::AcqRel) {
            return;
        }
        let pid = self.pid.load(Ordering::Relaxed);
        if pid != 0 {
            signal_group(pid, libc::SIGHUP);
        }
        let deadline = Instant::now() + KILL_GRACE;
        loop {
            if pid == 0 {
                break;
            }
            {
                let mut state = self.io.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(child) = state.child.as_mut() {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            // `try_wait` has reaped an exited leader. Descendants,
                            // if any, still keep the process group alive below.
                            state.child.take();
                        }
                        Ok(None) => {}
                    }
                }
            }
            if !group_alive(pid) {
                break;
            }
            if Instant::now() >= deadline {
                signal_group(pid, libc::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let mut state = self.io.lock().unwrap_or_else(|e| e.into_inner());
        // Reap the leader before closing the PTY. After SIGKILL this wait cannot
        // be survived by a HUP handler, and by this point the whole group got it.
        if let Some(mut child) = state.child.take() {
            let _ = child.wait();
        }
        state.master = None;
        drop(state);
        let _ = self.output.send(Output::Exit(None));
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // A terminal dropped without an explicit shutdown (a deleted tab, a
        // failed restart) must not leave a shell behind.
        if !self.dead.swap(true, Ordering::AcqRel) {
            let pid = self.pid.load(Ordering::Relaxed);
            if pid != 0 {
                signal_group(pid, libc::SIGHUP);
            }
            if let Ok(mut state) = self.io.lock() {
                if let Some(mut child) = state.child.take() {
                    // Non-blocking best effort: a `try_wait` after a short sleep
                    // would stall the drop. The SIGKILL below is what matters.
                    let _ = child.try_wait();
                }
                if let Some(pid) = (self.pid.load(Ordering::Relaxed) != 0)
                    .then(|| self.pid.load(Ordering::Relaxed))
                {
                    signal_group(pid, libc::SIGKILL);
                }
                state.master = None;
            }
        }
    }
}

/// The reader thread: `read(2)` the master until EOF, publish and replay.
fn spawn_reader(terminal: Arc<Terminal>) {
    let reader = Arc::clone(&terminal);
    std::thread::Builder::new()
        .name(format!("kc-terminal-{}", terminal.id))
        .spawn(move || {
            let mut buffer = vec![0u8; 8192];
            loop {
                if reader.dead.load(Ordering::Acquire) {
                    return;
                }
                let read = {
                    let io = reader.io.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(master) = io.master.as_ref() else {
                        return;
                    };
                    // The fd is non-blocking so this lock is never held across a
                    // blocking read; a short sleep between `EAGAIN`s is cheaper
                    // than a `poll` loop for an interactive terminal.
                    unsafe {
                        libc::read(
                            master.as_raw_fd(),
                            buffer.as_mut_ptr() as *mut libc::c_void,
                            buffer.len(),
                        )
                    }
                };
                if read > 0 {
                    let chunk = &buffer[..read as usize];
                    reader.replay.push(chunk);
                    // A lagging viewer is dropped from the broadcast; it gets the
                    // replay buffer on its next attach instead of a stalled shell.
                    let _ = reader.output.send(Output::Data(chunk.to_vec()));
                    continue;
                }
                if read == 0 {
                    return finish(&reader);
                }
                let error = std::io::Error::last_os_error();
                match error.kind() {
                    std::io::ErrorKind::WouldBlock => {
                        // Gone for good when the child has exited: `read` on a
                        // master whose slave side is closed returns EAGAIN on
                        // Linux until the fd itself is closed.
                        if child_exited(&reader) {
                            return finish(&reader);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    std::io::ErrorKind::Interrupted => continue,
                    _ => return finish(&reader),
                }
            }
        })
        .ok();
}

/// Reap the child if it has exited and publish the status once.
fn child_exited(terminal: &Terminal) -> bool {
    let mut io = terminal.io.lock().unwrap_or_else(|e| e.into_inner());
    let Some(child) = io.child.as_mut() else {
        return true;
    };
    match child.try_wait() {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Close the read loop: wait for the child, publish the exit code, mark dead.
fn finish(terminal: &Terminal) {
    if terminal.dead.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut status = None;
    {
        let mut io = terminal.io.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = io.child.take() {
            if let Ok(exit) = child.wait() {
                status = exit.code();
            }
        }
        io.master = None;
    }
    let _ = terminal.output.send(Output::Exit(status));
}

/// `openpty(3)`: a master/slave pair with `cols`x`rows` already set, so the very
/// first prompt is the right width.
fn open_pty(cols: u16, rows: u16) -> kc_core::Result<(std::fs::File, std::fs::File)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // `openpty` writes the two fds through the pointers. The termios/winsize
    // arguments are read-only on Linux (`*const`) but are written back on macOS
    // (`*mut`), so each target passes the pointer type its libc binding expects.
    #[cfg(target_os = "macos")]
    let rc = {
        let mut size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        }
    };
    #[cfg(not(target_os = "macos"))]
    let rc = {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        }
    };
    if rc != 0 {
        return Err(kc_core::Error::Tool(format!(
            "could not open a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        )));
    }
    // Non-blocking on the master: the websocket task must never be stuck in
    // `read`, and the reader thread uses `EAGAIN` + a short sleep.
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        if flags != -1 {
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    // `from_raw_fd` takes ownership, so both descriptors are closed exactly once.
    let master_file = unsafe { std::fs::File::from_raw_fd(master) };
    let slave_file = unsafe { std::fs::File::from_raw_fd(slave) };
    Ok((master_file, slave_file))
}

fn dup_fd(file: &std::fs::File) -> kc_core::Result<std::fs::File> {
    let duplicated = unsafe { libc::dup(file.as_raw_fd()) };
    if duplicated == -1 {
        return Err(kc_core::Error::Tool(format!(
            "could not duplicate the pty slave descriptor: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { std::fs::File::from_raw_fd(duplicated) })
}

/// The environment a shell may see.
///
/// The same allowlist and secret-shaped-name filter `run_command` uses, for the
/// same reason: the server holds `KARUCODE_AUTH_TOKEN` and provider keys, and an
/// interactive shell is where `env | curl -d @-` is easiest to type. Extra names
/// come from the live `limits.command_env_passthrough`; absent variables are never
/// invented as empty values.
fn apply_environment(command: &mut std::process::Command, passthrough: &[String]) {
    command.env_clear();
    const ALLOWLIST: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TMPDIR", "PWD", "TZ",
    ];
    for name in ALLOWLIST
        .iter()
        .copied()
        .chain(passthrough.iter().map(String::as_str))
    {
        if is_secret_like(name) || !is_valid_environment_name(name) {
            continue;
        }
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // A terminal *is* interactive, so `TERM` must be a terminal type: the shell
    // would otherwise think it is talking to a pipe and refuse to do line
    // editing, colours or job control. Set after the allowlist so an inherited
    // `TERM=dumb` (a CI runner, a systemd unit) cannot cross it out.
    command.env("TERM", "xterm-256color");
    // A shell needs a usable locale; "C" is what `run_command` forces, but an
    // interactive one with `LC_ALL=C` cannot render a UTF-8 prompt properly.
    // `$LANG` is passed through above; this only fills the gap when it is unset.
    if std::env::var_os("LANG").is_none() {
        command.env("LANG", "C.UTF-8");
    }
}

/// Match kc-tools' filter exactly: passthrough is not a way to reauthorise a
/// credential. Matching is case-insensitive and uses real name boundaries.
fn is_secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const SECRET_PREFIXES: &[&str] = &["KARUCODE_", "AWS_", "OPENAI_", "ANTHROPIC_"];
    const SECRET_NAMES: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "NPM_TOKEN"];
    const SECRET_SUFFIXES: &[&str] = &["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];
    SECRET_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
        || SECRET_NAMES.contains(&upper.as_str())
        || SECRET_SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

fn is_valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn clamp_cols(cols: u16) -> u16 {
    cols.clamp(MIN_COLS, MAX_COLS)
}

fn clamp_rows(rows: u16) -> u16 {
    rows.clamp(MIN_ROWS, MAX_ROWS)
}

fn signal_group(pid: u32, signal: libc::c_int) {
    if pid == 0 {
        return;
    }
    unsafe {
        // Negative pid = the whole process group. The shell is a group leader
        // (`setsid`), so this reaches every child.
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

fn group_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(-(pid as libc::pid_t), 0) == 0 }
}

/// The live terminals, keyed by id.
///
/// Deliberately not persisted: this is the process table, and it is empty after
/// a restart. The durable half is the `terminals` table in SQLite.
#[derive(Default)]
pub struct TerminalRegistry {
    live: Mutex<HashMap<String, Arc<Terminal>>>,
    // Weak entries preserve the same budget across process generations while
    // any old socket still owns a permit, without retaining deleted IDs forever.
    slots: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Semaphore>>>,
    /// Serialize row lifecycle operations across async store calls.
    pub(crate) operation: tokio::sync::Mutex<()>,
}

impl TerminalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn attachment_slots(&self, id: &str) -> Arc<tokio::sync::Semaphore> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.retain(|_, value| value.strong_count() > 0);
        if let Some(existing) = slots.get(id).and_then(std::sync::Weak::upgrade) {
            return existing;
        }
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_ATTACHMENTS));
        slots.insert(id.to_string(), Arc::downgrade(&semaphore));
        semaphore
    }

    pub fn get(&self, id: &str) -> Option<Arc<Terminal>> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub fn insert(&self, terminal: Arc<Terminal>) {
        let replaced = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(terminal.id.clone(), Arc::clone(&terminal));
        if let Some(old) = replaced {
            if !Arc::ptr_eq(&old, &terminal) {
                old.shutdown();
            }
        }
    }

    /// Remove and return a terminal. The caller decides whether to shut it down.
    pub fn remove(&self, id: &str) -> Option<Arc<Terminal>> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
    }

    pub fn len(&self) -> usize {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ids(&self) -> Vec<String> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Every live terminal, for the idle reaper.
    pub fn all(&self) -> Vec<Arc<Terminal>> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Hang up every shell. Called on server shutdown so no child outlives us.
    pub fn shutdown_all(&self) {
        let terminals = self.all();
        for terminal in terminals {
            terminal.shutdown();
        }
        self.live.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Reap shells that have had no browser for `idle_secs`.
    ///
    /// The *row* stays: a reaped terminal keeps its tab, its title and its
    /// directory, and attaching starts a fresh shell in the same place. `0`
    /// disables the reaper.
    pub fn reap_idle(&self, idle_secs: u64) -> Vec<String> {
        if idle_secs == 0 {
            return Vec::new();
        }
        let cutoff = crate::state::now_ms() - (idle_secs as i64) * 1000;
        // Reserve attachments and remove idle entries under the same lock. Never
        // remove by stale id after releasing it: an entry may have been replaced.
        let removed: Vec<_> = {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<_> = live
                .iter()
                .filter(|(_, t)| t.attached() == 0 && t.last_seen() <= cutoff)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter().filter_map(|id| live.remove(&id)).collect()
        };
        let reaped = removed.iter().map(|t| t.id.clone()).collect();
        for terminal in removed {
            terminal.shutdown();
        }
        reaped
    }
}

/// Read the terminal settings, falling back to the defaults when the settings
/// handle cannot be read (which only happens during a torn shutdown).
pub fn terminal_settings(state: &AppState) -> TerminalConfig {
    state.settings().snapshot().terminals
}

/// Spawn with one coherent live settings snapshot, including the operator's
/// current extra environment names. This is used by create, restart and attach so
/// no path silently falls back to the fixed allowlist.
pub fn spawn_for_state(
    state: &AppState,
    id: &str,
    row: &TerminalSession,
    cols: u16,
    rows: u16,
) -> kc_core::Result<Arc<Terminal>> {
    let snapshot = state.settings().snapshot();
    Terminal::spawn(
        id,
        row,
        &snapshot.terminals,
        &snapshot.limits.command_env_passthrough,
        cols,
        rows,
    )
}

/// The router-facing part of a terminal: `GET /api/terminals` and friends.
///
/// The rows come from SQLite, so the answer is the same in every browser; each
/// row carries `running` so the UI can say "reattach" instead of "start".
pub async fn list(state: &AppState) -> ApiResult<serde_json::Value> {
    let settings = terminal_settings(state);
    if !settings.enabled {
        return Ok(json!({ "enabled": false, "terminals": [], "max_terminals": 0 }));
    }
    let rows = state.store().list_terminals().await?;
    let registry = state.terminals();
    let terminals: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| terminal_json(row, registry.get(&row.id).as_deref()))
        .collect();
    Ok(json!({
        "enabled": true,
        "terminals": terminals,
        "max_terminals": settings.terminal_budget(),
        "shell": settings.shell_program(),
        "scrollback_bytes": settings.replay_capacity(),
    }))
}

fn terminal_json(row: &TerminalSession, live: Option<&Terminal>) -> serde_json::Value {
    json!({
        "id": row.id,
        "title": row.title,
        "cwd": row.cwd,
        "project": row.project_id,
        "position": row.position,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "last_used_at": row.last_used_at,
        // `running` is the process half; the row above is the durable half.
        "running": live.map(|t| !t.is_dead()).unwrap_or(false),
        "pid": live.map(Terminal::pid).unwrap_or(0),
        "attached": live.map(Terminal::attached).unwrap_or(0),
    })
}

/// The public shape of one terminal: the durable row plus the process state.
///
/// Kept in one place so the list, the create, the rename and the restart all
/// answer identically - a client that reads `terminal.running` from a create
/// response gets the same field a list would have given it.
pub fn terminal_json_value(row: &TerminalSession, live: Option<&Terminal>) -> serde_json::Value {
    terminal_json(row, live)
}

/// Ensure a live shell for `row`, starting one if the process is gone.
pub fn ensure_live(
    state: &AppState,
    row: &TerminalSession,
    cols: u16,
    rows: u16,
) -> ApiResult<Arc<Terminal>> {
    let settings = terminal_settings(state);
    if !settings.enabled {
        return Err(ApiError::forbidden(
            "the terminal is disabled on this server (terminals.enabled, or KARUCODE_TERMINALS=0)",
        ));
    }
    let registry = state.terminals();
    // Keep lookup and spawn atomic, including for callers outside the HTTP routes.
    let mut live = registry.live.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = live.get(&row.id) {
        if !existing.is_dead() {
            return Ok(Arc::clone(existing));
        }
    }
    let terminal = spawn_for_state(state, &row.id, row, cols, rows)?;
    if let Some(old) = live.insert(row.id.clone(), Arc::clone(&terminal)) {
        old.shutdown();
    }
    Ok(terminal)
}

/// The websocket route: attach, replay, then pump both directions.
pub async fn socket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::routes::QueryMap>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !terminal_settings(&state).enabled {
        return Err(ApiError::forbidden(
            "the terminal is disabled on this server (terminals.enabled, or KARUCODE_TERMINALS=0)",
        ));
    }
    // A browser cannot set a header on the websocket handshake, so `?token=` is
    // accepted here exactly as on `/api/events`.
    check_origin(&headers, &state)?;
    let _operation = state.terminals().operation.lock().await;
    let row = state
        .store()
        .get_terminal(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no terminal with id `{id}`")))?;

    let cols = crate::routes::query_usize(&query, "cols")?
        .and_then(|v| u16::try_from(v).ok())
        .map(clamp_cols)
        .unwrap_or(DEFAULT_COLS);
    let rows = crate::routes::query_usize(&query, "rows")?
        .and_then(|v| u16::try_from(v).ok())
        .map(clamp_rows)
        .unwrap_or(DEFAULT_ROWS);

    let attachment = loop {
        let terminal = ensure_live(&state, &row, cols, rows)?;
        let registry = state.terminals();
        let live = registry.live.lock().unwrap_or_else(|e| e.into_inner());
        if !live
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &terminal))
        {
            continue;
        }
        let permit = registry
            .attachment_slots(&id)
            .try_acquire_owned()
            .map_err(|_| ApiError::bad_request("too many terminal attachments"))?;
        terminal.mark_attached();
        break Attachment {
            terminal,
            _permit: permit,
        };
    };
    let _ = state.store().touch_terminal(&id).await;
    let shutdown = state.shutdown_token();
    Ok(ws
        .max_message_size(1024 * 1024)
        .on_upgrade(move |socket| pump(socket, attachment, shutdown)))
}

/// A cross-origin page must not be able to open a shell.
///
/// A websocket handshake is not covered by the CSRF middleware (it is a `GET`,
/// and the browser sends `Origin` itself), so this check is the whole defence. A
/// missing `Origin` is accepted only when the `Host` guard is already active
/// (loopback, no token) - which means a non-browser client on the machine, not a
/// web page.
fn check_origin(headers: &HeaderMap, state: &AppState) -> ApiResult<()> {
    decide_origin(headers, state.host_allowlist()).map_err(ApiError::forbidden)
}

/// The whole `Origin` decision, split out from [`check_origin`] so a test can
/// exercise the fallback (the production path needs an `AppState`; this needs
/// only an allowlist).
///
/// `SameOrigin` and `NotABrowser` pass. A `Mismatch` is accepted only behind a
/// reverse proxy, and then only when **both** halves are trusted: the `Host` the
/// request arrived with is one the allowlist accepts, and the `Origin`'s
/// authority matches an *explicit* `KARUCODE_ALLOWED_HOSTS` rule
/// ([`HostAllowlist::allows_explicit_authority`]).
///
/// That second half must not use [`HostAllowlist::check`]: it treats any
/// loopback name on any port as allowed, so `Origin: http://localhost:9999`
/// against `Host: 127.0.0.1:8787` would pass, and any page on any local port
/// (`npm run dev`, a docs server, an attacker's own localhost listener) could
/// open a shell. There is no dev-server exemption to protect - `web/vite.config.ts`
/// proxies `/api` without `ws: true`, so `npm run dev` cannot reach this socket
/// by design. An operator who really fronts karucode with a proxy names it in
/// `KARUCODE_ALLOWED_HOSTS`, which is exactly the explicit rule required here.
fn decide_origin(headers: &HeaderMap, allowlist: &HostAllowlist) -> Result<(), String> {
    match origin_verdict(headers) {
        OriginVerdict::SameOrigin => Ok(()),
        OriginVerdict::NotABrowser => Ok(()),
        OriginVerdict::Mismatch(origin, host) => {
            // Behind a reverse proxy the `Host` a browser sends is the public name
            // while the server may already trust that name; `origin_matches_host`
            // covers the ordinary case, and the host allowlist covers the rest.
            if allowlist.check(&host).is_ok()
                && origin_is_explicitly_allowlisted(&origin, allowlist)
            {
                return Ok(());
            }
            Err(
                "cross-origin websocket rejected: the Origin header does not match the Host header"
                    .to_string(),
            )
        }
    }
}

/// What the `Origin`/`Host` comparison says about a handshake.
pub enum OriginVerdict {
    /// The `Origin` matches the `Host` the request dialled.
    SameOrigin,
    /// No `Origin` at all: a non-browser client, covered by the `Host` guard.
    NotABrowser,
    /// A cross-origin page. Carries both halves for the allowlist fallback.
    Mismatch(String, String),
}

/// The whole defence for an ordinary deployment, split out so a unit test can
/// exercise it without an `AppState`.
pub fn origin_verdict(headers: &HeaderMap) -> OriginVerdict {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return OriginVerdict::NotABrowser;
    };
    let Ok(origin) = origin.to_str() else {
        // A malformed `Origin` is refused, never treated as absent: values with
        // whitespace or control characters cannot be compared.
        return OriginVerdict::Mismatch(String::new(), String::new());
    };
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if crate::host::origin_matches_host(origin, host) {
        OriginVerdict::SameOrigin
    } else {
        OriginVerdict::Mismatch(origin.to_string(), host.to_string())
    }
}

/// True when the origin's authority matches an explicit allowlist rule.
///
/// Not [`HostAllowlist::check`]: that accepts every loopback name on every port
/// (see [`decide_origin`]), which would turn any page on a local port into a
/// shell. A malformed origin matches nothing.
fn origin_is_explicitly_allowlisted(origin: &str, allowlist: &HostAllowlist) -> bool {
    let Some(rest) = origin.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    match crate::host::parse_authority(authority) {
        Some(authority) => allowlist.allows_explicit_authority(&authority),
        None => false,
    }
}

/// Counts upgrades as attachments too; dropping a failed upgrade releases both
/// the idle-reaper reservation and the replay-memory slot.
struct Attachment {
    terminal: Arc<Terminal>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        self.terminal.mark_detached();
    }
}

/// Both directions of the websocket until either side goes away.
async fn pump(
    mut socket: WebSocket,
    attachment: Attachment,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let terminal = &attachment.terminal;
    // Subscribe before snapshotting the replay so a byte written in between is
    // delivered once: it is either in the snapshot or on the channel.
    let mut output = terminal.subscribe();
    let replay = terminal.replay();
    let ready = json!({
        "type": "ready",
        "replay": !replay.is_empty(),
        "replay_bytes": replay.len(),
        "pid": terminal.pid(),
        "running": !terminal.is_dead(),
    });
    if tokio::time::timeout(
        SOCKET_SEND_TIMEOUT,
        socket.send(Message::Text(ready.to_string())),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .is_none()
    {
        return;
    }
    if !replay.is_empty()
        && tokio::time::timeout(SOCKET_SEND_TIMEOUT, socket.send(Message::Binary(replay)))
            .await
            .ok()
            .and_then(Result::ok)
            .is_none()
    {
        return;
    }

    let (mut sender, mut receiver) = socket.split();
    use futures_util::{SinkExt, StreamExt};
    // Control replies belong only to the socket that asked. They never enter the
    // PTY byte broadcast: that would turn JSON into binary terminal data and
    // leak it to every other attached browser.
    let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<String>(8);

    // Shell -> browser.
    let to_browser = {
        async move {
            loop {
                tokio::select! {
                    biased;
                    control = control_rx.recv() => {
                        let Some(frame) = control else { return };
                        if tokio::time::timeout(
                            SOCKET_SEND_TIMEOUT,
                            sender.send(Message::Text(frame)),
                        )
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .is_none()
                        {
                            return;
                        }
                    }
                    received = output.recv() => match received {
                    Ok(Output::Data(bytes)) => {
                        if tokio::time::timeout(
                            SOCKET_SEND_TIMEOUT,
                            sender.send(Message::Binary(bytes)),
                        )
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .is_none()
                        {
                            return;
                        }
                    }
                    Ok(Output::Exit(code)) => {
                        let frame = json!({ "type": "exit", "code": code });
                        let _ = tokio::time::timeout(
                            SOCKET_SEND_TIMEOUT,
                            sender.send(Message::Text(frame.to_string())),
                        )
                        .await;
                        return;
                    }
                    // A lagging browser: it missed frames, so tell it to resync
                    // rather than pretend the stream is continuous. The client
                    // reconnects and gets the replay buffer.
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        let frame = json!({ "type": "lagged", "missed": missed });
                        if tokio::time::timeout(
                            SOCKET_SEND_TIMEOUT,
                            sender.send(Message::Text(frame.to_string())),
                        )
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .is_none()
                        {
                            return;
                        }
                        return;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    };

    // Browser -> shell.
    let from_browser = {
        let terminal = Arc::clone(terminal);
        async move {
            while let Some(frame) = receiver.next().await {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(_) => return,
                };
                match frame {
                    Message::Binary(bytes) => {
                        terminal.record_activity();
                        let writer = Arc::clone(&terminal);
                        if !tokio::task::spawn_blocking(move || writer.write(&bytes))
                            .await
                            .unwrap_or(false)
                        {
                            return;
                        }
                    }
                    Message::Text(text) => {
                        if let Some(action) = parse_control(&text) {
                            terminal.record_activity();
                            match action {
                                Control::Resize { cols, rows } => {
                                    terminal.resize(cols, rows);
                                }
                                Control::Input { data } => {
                                    let writer = Arc::clone(&terminal);
                                    if !tokio::task::spawn_blocking(move || {
                                        writer.write(data.as_bytes())
                                    })
                                    .await
                                    .unwrap_or(false)
                                    {
                                        return;
                                    }
                                }
                                Control::Ping => {
                                    let pong = json!({ "type": "pong" }).to_string();
                                    if tokio::time::timeout(
                                        SOCKET_SEND_TIMEOUT,
                                        control_tx.send(pong),
                                    )
                                    .await
                                    .ok()
                                    .and_then(Result::ok)
                                    .is_none()
                                    {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Message::Close(_) => return,
                    // Ping/pong are handled by axum's websocket layer.
                    _ => {}
                }
            }
        }
    };

    tokio::select! {
        _ = to_browser => {}
        _ = from_browser => {}
        _ = shutdown.cancelled() => {}
    }
}

/// A text frame from the browser. Unknown types are ignored rather than treated
/// as an error: an older client may send something a newer server dropped.
enum Control {
    Resize { cols: u16, rows: u16 },
    Input { data: String },
    Ping,
}

fn parse_control(text: &str) -> Option<Control> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value.get("type")?.as_str()? {
        "resize" => Some(Control::Resize {
            cols: value
                .get("cols")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .map(clamp_cols)
                .unwrap_or(DEFAULT_COLS),
            rows: value
                .get("rows")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .map(clamp_rows)
                .unwrap_or(DEFAULT_ROWS),
        }),
        "input" => Some(Control::Input {
            data: value.get("data")?.as_str()?.to_string(),
        }),
        "ping" => Some(Control::Ping),
        _ => Some(Control::Ping),
    }
}

// `std::os::unix::io::AsRawFd` is needed for the ioctl calls above.
use std::os::unix::io::AsRawFd;

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> TerminalConfig {
        TerminalConfig {
            shell: "/bin/sh".into(),
            // Small, so a test can overflow it on purpose.
            scrollback_bytes: kc_core::config::MIN_TERMINAL_SCROLLBACK,
            ..TerminalConfig::default()
        }
    }

    fn session(cwd: &str) -> TerminalSession {
        TerminalSession {
            id: "t_test".into(),
            title: "test-1".into(),
            cwd: cwd.into(),
            project_id: None,
            position: 1,
            created_at: 0,
            updated_at: 0,
            last_used_at: 0,
        }
    }

    fn cwd() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    fn handshake(origin: &str, host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            axum::http::HeaderValue::from_str(origin).expect("origin header"),
        );
        headers.insert(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_str(host).expect("host header"),
        );
        headers
    }

    fn allowlist(extra: Option<&str>) -> HostAllowlist {
        HostAllowlist::new("127.0.0.1", extra)
    }

    /// The reported bug: `Origin: http://localhost:9999` (and any other loopback
    /// name on another port) was accepted against `Host: 127.0.0.1:PORT`,
    /// because the fallback asked the `Host` allowlist - which treats every
    /// loopback name on every port as trusted. That is a shell for any page on
    /// any local port.
    #[test]
    fn a_cross_port_loopback_origin_is_rejected() {
        let allow = allowlist(None);
        for origin in [
            "http://localhost:9999",
            "http://127.0.0.1:5173",
            "http://127.0.0.1:9999",
            "http://[::1]:8080",
            "http://localhost:8787",
        ] {
            let result = decide_origin(&handshake(origin, "127.0.0.1:8787"), &allow);
            assert!(
                result.is_err(),
                "{origin} must not be allowed to open a shell on 127.0.0.1:8787"
            );
        }
    }

    /// The ordinary case still works: the app's own page dials its own host.
    #[test]
    fn the_same_origin_is_accepted() {
        let allow = allowlist(None);
        for (origin, host) in [
            ("http://127.0.0.1:8787", "127.0.0.1:8787"),
            ("http://localhost:8787", "localhost:8787"),
            ("http://[::1]:8787", "[::1]:8787"),
        ] {
            assert!(
                decide_origin(&handshake(origin, host), &allow).is_ok(),
                "{origin} == {host} must be accepted"
            );
        }
    }

    /// A reverse proxy is the one supported cross-origin case, and it works for
    /// the documented reason: the public name is an *explicit* allowlist entry.
    #[test]
    fn an_explicitly_allowlisted_origin_behind_a_proxy_is_accepted() {
        let allow = allowlist(Some("karucode.example.com, karucode.example.com:443"));
        assert!(
            decide_origin(
                &handshake("https://karucode.example.com", "karucode.example.com"),
                &allow
            )
            .is_ok(),
            "the allowlisted proxy host must still open the terminal"
        );
        // A Host/Origin pair the operator never named is still refused.
        assert!(decide_origin(
            &handshake("https://evil.example", "karucode.example.com"),
            &allow
        )
        .is_err());
        // An allowlisted *Host* with a loopback *Origin* is exactly the exfiltration
        // shape the fallback used to allow: the port differs, so it is a stranger.
        assert!(decide_origin(
            &handshake("http://localhost:9999", "karucode.example.com"),
            &allow
        )
        .is_err());
    }

    /// No `Origin` is a non-browser client; it is accepted (the `Host` guard and
    /// the bearer token are the gate for those clients).
    #[test]
    fn a_missing_origin_is_accepted_as_a_non_browser_client() {
        let allow = allowlist(None);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("127.0.0.1:8787"),
        );
        assert!(decide_origin(&headers, &allow).is_ok());
    }

    /// A cross-origin page behind an allowlisted proxy still cannot use the
    /// websocket, and the error names the actual rule.
    #[test]
    fn the_rejection_message_names_the_origin_rule() {
        let allow = allowlist(None);
        let error = decide_origin(
            &handshake("http://localhost:9999", "127.0.0.1:8787"),
            &allow,
        )
        .expect_err("a cross-port loopback origin must be rejected");
        assert!(error.contains("Origin"), "{error}");
        assert!(error.contains("Host"), "{error}");
    }

    /// A real shell runs in a real PTY and its output comes back through the
    /// reader. This is the whole feature in one test.
    #[tokio::test]
    async fn a_shell_runs_and_its_output_is_replayed() {
        let terminal = Terminal::spawn("t_test", &session(&cwd()), &settings(), &[], 100, 30)
            .expect("spawn shell");
        assert!(!terminal.is_dead());
        assert!(terminal.pid() > 0);
        assert_eq!(terminal.size(), Some((100, 30)));

        // The shell must echo something identifiable, and only after it has
        // started - so the write goes to the PTY and the answer comes back
        // through the same descriptor.
        assert!(terminal.write(b"echo kc-terminal-probe-42\n"));
        let mut seen = Vec::new();
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            seen = terminal.replay();
            if String::from_utf8_lossy(&seen).contains("kc-terminal-probe-42") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.contains("kc-terminal-probe-42"),
            "the shell never echoed the probe: {text:?}"
        );

        // Resizing reaches the kernel, which is what makes `vi` redraw.
        assert!(terminal.resize(120, 40));
        assert_eq!(terminal.size(), Some((120, 40)));
        // ...and out-of-range values are clamped, not passed through.
        assert!(terminal.resize(1, 1));
        assert_eq!(terminal.size(), Some((MIN_COLS, MIN_ROWS)));

        terminal.shutdown();
        // The shell is really gone: the group no longer exists.
        let pid = terminal.pid();
        for _ in 0..100 {
            if !group_alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!group_alive(pid), "the shell outlived its terminal");
    }

    #[tokio::test]
    async fn shutdown_kills_and_reaps_a_shell_that_ignores_sighup_before_returning() {
        let terminal = Terminal::spawn("t_stubborn", &session(&cwd()), &settings(), &[], 80, 24)
            .expect("spawn stubborn shell");
        assert!(
            terminal.write(b"trap '' HUP; printf 'KC_HUP_IGNORED\\n'; while :; do sleep 1; done\n")
        );
        for _ in 0..200 {
            if String::from_utf8_lossy(&terminal.replay()).contains("KC_HUP_IGNORED") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(String::from_utf8_lossy(&terminal.replay()).contains("KC_HUP_IGNORED"));
        let pid = terminal.pid();
        let started = std::time::Instant::now();
        terminal.shutdown();
        assert!(
            started.elapsed() >= KILL_GRACE,
            "HUP ignored must escalate to KILL"
        );
        assert!(started.elapsed() < KILL_GRACE + Duration::from_secs(2));
        assert!(
            terminal.io.lock().unwrap().child.is_none(),
            "leader was not reaped"
        );
        assert!(
            terminal.io.lock().unwrap().master.is_none(),
            "PTY was not closed"
        );
        // The leader is our child and has been waited on, so its id cannot still be
        // alive. Group members received SIGKILL before the same return point.
        assert_ne!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0);
        for _ in 0..40 {
            if !group_alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            !group_alive(pid),
            "a stubborn process group survived shutdown"
        );
    }

    /// A dead shell is reported as dead, which is what makes the route start a
    /// new one instead of attaching to a closed fd.
    #[tokio::test]
    async fn a_shell_that_exits_marks_the_terminal_dead() {
        let terminal = Terminal::spawn("t_test", &session(&cwd()), &settings(), &[], 80, 24)
            .expect("spawn shell");
        assert!(terminal.write(b"exit 7\n"));
        for _ in 0..200 {
            if terminal.is_dead() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(terminal.is_dead(), "the terminal did not notice the exit");
        // Writing to a dead terminal is refused rather than panicking.
        assert!(!terminal.write(b"echo alive\n"));
    }

    /// The replay buffer is bounded: a terminal that printed more than the
    /// setting must not hold all of it.
    #[test]
    fn the_replay_buffer_keeps_only_the_newest_bytes() {
        let replay = Replay::new(1024);
        replay.push(&vec![b'a'; 900]);
        replay.push(&vec![b'b'; 900]);
        let snapshot = replay.snapshot();
        assert!(
            snapshot.len() <= 1024,
            "replay kept {} bytes, cap is 1024",
            snapshot.len()
        );
        // Newest bytes win: the tail is the 'b' run.
        assert_eq!(snapshot.last(), Some(&b'b'));
        assert!(replay.len() <= 1024);
    }

    /// A single chunk larger than the buffer is trimmed on read instead of being
    /// dropped, so a huge redraw still replays its last screenful.
    #[test]
    fn a_huge_chunk_is_trimmed_and_not_dropped() {
        let replay = Replay::new(1024);
        replay.push(b"hello");
        replay.push(&vec![b'z'; 4096]);
        let snapshot = replay.snapshot();
        assert_eq!(snapshot.len(), 1024);
        assert!(snapshot.iter().all(|byte| *byte == b'z'));
    }

    /// Order is preserved: a naive ring would reverse the chunks.
    #[test]
    fn the_replay_preserves_order() {
        let replay = Replay::new(1024);
        replay.push(b"first ");
        replay.push(b"second");
        assert_eq!(replay.snapshot(), b"first second".to_vec());
    }

    #[test]
    fn control_frames_are_parsed_and_clamped() {
        match parse_control(r#"{"type":"resize","cols":10,"rows":2}"#) {
            Some(Control::Resize { cols, rows }) => {
                assert_eq!(cols, MIN_COLS);
                assert_eq!(rows, MIN_ROWS);
            }
            _ => panic!("resize was not parsed"),
        }
        match parse_control(r#"{"type":"resize","cols":9999,"rows":9999}"#) {
            Some(Control::Resize { cols, rows }) => {
                assert_eq!(cols, MAX_COLS);
                assert_eq!(rows, MAX_ROWS);
            }
            _ => panic!("resize was not parsed"),
        }
        assert!(matches!(
            parse_control(r#"{"type":"input","data":"ls\r"}"#),
            Some(Control::Input { .. })
        ));
        // Garbage and unknown types never panic and never become input.
        assert!(parse_control("not json").is_none());
        assert!(parse_control(r#"{"type":"nonsense"}"#).is_some());
    }

    #[test]
    fn attachment_slots_are_bounded_and_drop_releases_reservation() {
        let terminal =
            Terminal::spawn("t_slots", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        let registry = TerminalRegistry::new();
        let slots = registry.attachment_slots("t_slots");
        let mut attachments = Vec::new();
        for _ in 0..MAX_ATTACHMENTS {
            let permit = Arc::clone(&slots).try_acquire_owned().unwrap();
            terminal.mark_attached();
            attachments.push(Attachment {
                terminal: Arc::clone(&terminal),
                _permit: permit,
            });
        }
        assert!(Arc::clone(&slots).try_acquire_owned().is_err());
        assert_eq!(terminal.attached(), MAX_ATTACHMENTS as u32);
        drop(attachments.pop());
        assert_eq!(terminal.attached(), MAX_ATTACHMENTS as u32 - 1);
        assert!(Arc::clone(&slots).try_acquire_owned().is_ok());
        drop(attachments);
        terminal.shutdown();
    }

    #[test]
    fn attachment_budget_survives_terminal_replacement() {
        let registry = TerminalRegistry::new();
        let old =
            Terminal::spawn("t_generation", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        registry.insert(Arc::clone(&old));
        let permits: Vec<_> = (0..MAX_ATTACHMENTS)
            .map(|_| {
                registry
                    .attachment_slots("t_generation")
                    .try_acquire_owned()
                    .unwrap()
            })
            .collect();
        let new =
            Terminal::spawn("t_generation", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        registry.insert(new);
        assert!(registry
            .attachment_slots("t_generation")
            .try_acquire_owned()
            .is_err());
        drop(permits);
        assert!(registry
            .attachment_slots("t_generation")
            .try_acquire_owned()
            .is_ok());
        registry.shutdown_all();
    }

    #[test]
    fn detach_refreshes_idle_before_reaper_can_observe_zero() {
        let registry = Arc::new(TerminalRegistry::new());
        let terminal =
            Terminal::spawn("t_detach", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        registry.insert(Arc::clone(&terminal));
        for _ in 0..1000 {
            terminal.mark_attached();
            terminal
                .last_seen
                .store(crate::state::now_ms() - 120_000, Ordering::Release);
            let barrier = Arc::new(std::sync::Barrier::new(2));
            std::thread::scope(|scope| {
                let barrier2 = Arc::clone(&barrier);
                let registry = Arc::clone(&registry);
                scope.spawn(move || {
                    barrier2.wait();
                    registry.reap_idle(60);
                });
                barrier.wait();
                terminal.mark_detached();
            });
            assert!(registry.get("t_detach").is_some());
        }
        registry.shutdown_all();
    }

    #[test]
    fn replacing_entry_shuts_down_old_shell() {
        let registry = TerminalRegistry::new();
        let old = Terminal::spawn("t_replace", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        let new = Terminal::spawn("t_replace", &session(&cwd()), &settings(), &[], 80, 24).unwrap();
        registry.insert(Arc::clone(&old));
        registry.insert(Arc::clone(&new));
        assert!(old.is_dead());
        assert!(!new.is_dead());
        assert!(Arc::ptr_eq(&registry.get("t_replace").unwrap(), &new));
        registry.shutdown_all();
    }

    /// The registry reaps only terminals nobody is attached to *and* that have
    /// been idle past the cutoff; an attached one is never taken.
    #[tokio::test]
    async fn the_idle_reaper_spares_attached_terminals() {
        let registry = TerminalRegistry::new();
        let idle = Terminal::spawn("t_idle", &session(&cwd()), &settings(), &[], 80, 24)
            .expect("spawn idle");
        let busy = Terminal::spawn("t_busy", &session(&cwd()), &settings(), &[], 80, 24)
            .expect("spawn busy");
        busy.mark_attached();
        // Pretend the idle one was last seen two days ago.
        idle.last_seen.store(
            crate::state::now_ms() - 2 * 24 * 3600 * 1000,
            Ordering::Release,
        );
        registry.insert(idle);
        registry.insert(Arc::clone(&busy));

        // 0 disables the reaper entirely.
        assert!(registry.reap_idle(0).is_empty());
        assert_eq!(registry.len(), 2);

        let reaped = registry.reap_idle(3600);
        assert_eq!(reaped, vec!["t_idle".to_string()]);
        assert!(registry.get("t_idle").is_none());
        assert!(
            registry.get("t_busy").is_some(),
            "an attached shell was reaped"
        );
        registry.shutdown_all();
        assert!(registry.is_empty());
    }

    /// The shell gets a cleared environment: the server's own secrets must not
    /// be readable from an interactive prompt.
    #[tokio::test]
    async fn the_shell_environment_is_cleared_of_secrets() {
        std::env::set_var("KARUCODE_AUTH_TOKEN", "super-secret-token");
        std::env::set_var("OPENAI_API_KEY", "sk-secret");
        let terminal = Terminal::spawn("t_test", &session(&cwd()), &settings(), &[], 200, 50)
            .expect("spawn shell");
        assert!(terminal.write(b"env\n"));
        let mut seen = String::new();
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            seen = String::from_utf8_lossy(&terminal.replay()).into_owned();
            if seen.contains("PATH=") {
                // Give the shell a moment to finish printing `env`.
                tokio::time::sleep(Duration::from_millis(100)).await;
                seen = String::from_utf8_lossy(&terminal.replay()).into_owned();
                break;
            }
        }
        terminal.shutdown();
        std::env::remove_var("KARUCODE_AUTH_TOKEN");
        std::env::remove_var("OPENAI_API_KEY");
        assert!(
            seen.contains("PATH="),
            "the shell never printed its env: {seen:?}"
        );
        assert!(
            !seen.contains("super-secret-token"),
            "KARUCODE_AUTH_TOKEN leaked into the shell environment"
        );
        assert!(
            !seen.contains("sk-secret"),
            "OPENAI_API_KEY leaked into the shell environment"
        );
        assert!(
            seen.contains("TERM=xterm-256color"),
            "TERM was not set: {seen:?}"
        );
    }
}
