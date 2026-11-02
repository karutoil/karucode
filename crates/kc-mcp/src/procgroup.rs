//! Process-group handling for a supervised MCP server process.
//!
//! `kill_on_drop(true)` and `Child::start_kill()` only signal the child that was
//! spawned, not the helpers it starts: an `npx` server spawns a `node` child, a
//! wrapper script spawns the real server, and a tool call can start anything at
//! all. Those helpers survive teardown, keep the stdout/stderr pipes open (so the
//! pump tasks never see EOF) and accumulate as orphans on every reconnect.
//!
//! So the child is spawned as a **session leader** (`setsid`) and teardown
//! signals the **process group** (`kill(-pgid)`) instead of the pid. Same pattern
//! as `kc-tools/src/command.rs` and `kc-server/src/terminals.rs`; POSIX-only, and
//! a no-op elsewhere, where `kill_on_drop` on the leader stays the backstop.

use std::time::Duration;
use tokio::process::{Child, Command};

/// Grace between `SIGTERM` to the group and `SIGKILL` to what is left.
pub(crate) const KILL_GRACE: Duration = Duration::from_secs(2);

/// `SIGTERM`, the polite one.
pub(crate) const SIGTERM: i32 = 15;
/// `SIGKILL`, which cannot be caught.
pub(crate) const SIGKILL: i32 = 9;

/// Put the child in its own process group (and session). Call before spawning.
#[cfg(unix)]
pub(crate) fn detach_into_own_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe; `pre_exec` is the one place a call
    // between fork and exec is legal.
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

/// Send `signal` to every process in the group `pgid` (the child's pid).
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
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Nothing to inspect without POSIX process groups.
#[cfg(not(unix))]
pub(crate) fn group_alive(_pgid: u32) -> bool {
    false
}

/// `SIGTERM` the group, wait up to [`KILL_GRACE`], then `SIGKILL` what is left.
///
/// The escalation is unconditional: a leader that dies on `SIGTERM` says nothing
/// about its children, and a survivor keeps the pipes open. The group id stays
/// valid while any member lives, so signalling it after the leader is reaped
/// cannot hit an unrelated group.
pub(crate) async fn terminate_group(
    child: &mut Child,
    pgid: u32,
) -> std::io::Result<std::process::ExitStatus> {
    signal_group(pgid, SIGTERM);
    if let Ok(status) = tokio::time::timeout(KILL_GRACE, child.wait()).await {
        if group_alive(pgid) {
            signal_group(pgid, SIGKILL);
        }
        return status;
    }
    signal_group(pgid, SIGKILL);
    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(status) => status,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the process did not die after SIGKILL",
        )),
    }
}

/// Kill whatever is left of `pgid` once the leader has been reaped.
pub(crate) fn kill_group_after_exit(pgid: u32) {
    if group_alive(pgid) {
        signal_group(pgid, SIGKILL);
    }
}

/// Kills the whole process group when dropped.
///
/// The backstop for a supervisor task that is aborted before it can terminate the
/// child itself: `kill_on_drop` only reaches the leader.
struct GroupGuard {
    pgid: u32,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        kill_group_after_exit(self.pgid);
    }
}

/// A guard that keeps the group from outliving the supervisor task.
pub(crate) fn guard(pgid: u32) -> impl Drop {
    GroupGuard { pgid }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_group_is_never_signalled() {
        assert!(!group_alive(0));
        signal_group(0, SIGKILL);
    }

    #[test]
    fn a_reaped_process_is_not_a_live_group() {
        let Ok(mut child) = std::process::Command::new("true").spawn() else {
            return;
        };
        let pid = child.id();
        let _ = child.wait();
        assert!(!group_alive(pid));
    }
}
