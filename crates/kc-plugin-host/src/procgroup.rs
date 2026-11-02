//! Process-group handling for a supervised child process.
//!
//! [`kill_on_drop(true)`](tokio::process::Command::kill_on_drop) only signals the
//! child the host spawned, and `Child::start_kill()` does the same: the *leader*.
//! A plugin that runs a helper (`Popen`, `sh -c 'make watch'`, a language
//! server) leaves that helper behind, because it is a separate process that
//! keeps running — and keeps the stdout/stderr pipes open — after its parent is
//! gone. `docs/PLUGINS.md` promises the host kills the process; before this
//! module it killed one process and orphaned the rest, so every reload
//! accumulated strays.
//!
//! The fix is the same one `kc-tools/src/command.rs` and
//! `kc-server/src/terminals.rs` already use: spawn the child as a **session
//! leader** (`setsid`), which makes it the leader of a new process group whose id
//! equals its pid, and on teardown signal the **group** (`kill(-pgid)`), not the
//! pid.
//!
//! POSIX-only, like those two: on platforms without POSIX process groups the
//! helpers compile to no-ops and `kill_on_drop` on the leader stays the backstop.

use std::time::Duration;
use tokio::process::{Child, Command};

/// Grace period between `SIGTERM` to the group and `SIGKILL` to what is left.
///
/// Two seconds: long enough for a plugin to close a file or a socket, short
/// enough that a reload of a hung plugin does not feel like a hang. The same
/// value the command runner and the terminals use.
pub(crate) const KILL_GRACE: Duration = Duration::from_secs(2);

/// `SIGTERM`, the polite one.
pub(crate) const SIGTERM: i32 = 15;
/// `SIGKILL`, which cannot be caught.
pub(crate) const SIGKILL: i32 = 9;

/// Put the child in its own process group (and session), so that a signal to
/// `-pid` reaches every process it starts.
///
/// Call this **before** spawning. `setsid` fails only when the caller is already
/// a process-group leader, which a freshly forked child never is.
#[cfg(unix)]
pub(crate) fn detach_into_own_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe and touches nothing but the calling
    // process's own session and process-group ids. `pre_exec` runs in the child
    // between fork and exec, which is the only place it is legal.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// No POSIX process groups on this platform; `kill_on_drop` is the backstop.
#[cfg(not(unix))]
pub(crate) fn detach_into_own_group(_command: &mut Command) {}

/// Send `signal` to every process in the group `pgid`.
///
/// A negative pid means "the whole process group". `pgid` is the pid of the
/// child that called `setsid` (see [`detach_into_own_group`]), and the kernel
/// keeps that group id allocated while any member lives, so signalling it cannot
/// hit an unrelated process group.
#[cfg(unix)]
pub(crate) fn signal_group(pgid: u32, signal: i32) {
    if pgid == 0 {
        return;
    }
    // SAFETY: `kill` only reads its arguments.
    unsafe {
        libc::kill(-(pgid as libc::pid_t), signal);
    }
}

/// No-op on platforms without POSIX process groups.
#[cfg(not(unix))]
pub(crate) fn signal_group(_pgid: u32, _signal: i32) {}

/// Whether any process is still a member of the group `pgid`.
///
/// `kill(-pgid, 0)` returns `ESRCH` once the group is empty. While any member
/// lives the kernel keeps the group id allocated, so the id cannot have been
/// reused by an unrelated process group.
#[cfg(unix)]
pub(crate) fn group_alive(pgid: u32) -> bool {
    if pgid == 0 {
        return false;
    }
    // SAFETY: `kill` only reads its arguments.
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    if rc == 0 {
        return true;
    }
    // `EPERM` means the group exists but is not ours to signal: still alive.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Nothing to inspect without POSIX process groups.
#[cfg(not(unix))]
pub(crate) fn group_alive(_pgid: u32) -> bool {
    false
}

/// `SIGTERM` the group, wait up to [`KILL_GRACE`] for the leader, then `SIGKILL`
/// whatever is left and reap it.
///
/// The escalation is unconditional. A leader that dies on `SIGTERM` says nothing
/// about its children: they may ignore the signal, and a surviving one keeps the
/// pipes open so the pump tasks never see EOF. Killing the group after the leader
/// is reaped is safe because the group id stays valid while any member lives.
pub(crate) async fn terminate_group(
    child: &mut Child,
    pgid: u32,
) -> std::io::Result<std::process::ExitStatus> {
    signal_group(pgid, SIGTERM);
    if let Ok(status) = tokio::time::timeout(KILL_GRACE, child.wait()).await {
        // The leader is gone; its helpers may not be.
        if group_alive(pgid) {
            signal_group(pgid, SIGKILL);
        }
        return status;
    }
    // The leader ignored SIGTERM (or is wedged): take the group down.
    signal_group(pgid, SIGKILL);
    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(status) => status,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the process did not die after SIGKILL",
        )),
    }
}

/// Kills the whole process group when dropped, unless it is already gone.
///
/// Belt and braces next to `kill_on_drop`: that one only reaches the leader, and
/// the supervisor task can be dropped (aborted) on a path that never runs
/// [`terminate_group`].
struct GroupGuard {
    pgid: u32,
    armed: bool,
}

impl GroupGuard {
    fn new(pgid: u32) -> Self {
        Self {
            pgid,
            armed: pgid != 0,
        }
    }

    /// Kill whatever is left of the group and disarm.
    fn kill_remaining(&mut self) {
        if self.armed && group_alive(self.pgid) {
            signal_group(self.pgid, SIGKILL);
        }
        self.armed = false;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill_remaining();
    }
}

/// Kill the process group of `pgid` once the leader has been reaped.
///
/// Used by a supervisor that waited for the child instead of terminating it: the
/// leader exiting on its own (a crash, a `shutdown` answer) must not leave its
/// helpers running.
pub(crate) fn kill_group_after_exit(pgid: u32) {
    let mut guard = GroupGuard::new(pgid);
    guard.kill_remaining();
}

/// A guard that keeps the group from outliving the supervisor task.
///
/// Returned by [`guard`] so a supervisor can hold one across its `select!`.
pub(crate) fn guard(pgid: u32) -> impl Drop {
    GroupGuard::new(pgid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_group_is_never_signalled() {
        // `kill(-0, sig)` would signal the *caller's* group, which is why pid 0
        // is filtered out everywhere in this module.
        assert!(!group_alive(0));
        signal_group(0, SIGKILL);
    }

    #[test]
    fn a_group_with_no_members_is_not_alive() {
        // Spawn and reap a child that is not a group leader: its pid is not a
        // live process group once it is gone.
        let child = std::process::Command::new("true").spawn();
        let Ok(mut child) = child else { return };
        let pid = child.id();
        let _ = child.wait();
        assert!(
            !group_alive(pid),
            "a reaped process must not look like a live group"
        );
    }
}
