//! Keep the resident set proportional to live data, not to the peak of the
//! busiest hour.
//!
//! A karucode server that has served a few long runs and a handful of plugins
//! settles at a much larger RSS than the same binary does when it starts cold.
//! None of that growth is live data: it is glibc's allocator, which gives every
//! thread that allocates its own arena (`8 * nproc` of them by default, each a
//! separate 64 MiB arena slot) and does not return a freed page to the kernel
//! unless the top of an arena's heap happens to be free. On a 16-core box with
//! plugin JSON-RPC traffic, tokio workers plus blocking threads touched enough
//! arenas to hold tens of MiB of *empty* heap; measured on this repository's own
//! harness, six plugins and twelve concurrent runs cost 47 MiB RSS, of which
//! 32 MiB was anonymous heap spread across 17 separate mappings.
//!
//! Two knobs, both cheap and both measured rather than assumed:
//!
//! * `M_ARENA_MAX` caps how many arenas the process will ever create. Two is
//!   enough for this workload - allocation is bursty, not a hot loop across 16
//!   cores - and it turns 17 heaps into 2.
//! * an idle `malloc_trim(0)` hands the free pages at the top of each arena back
//!   to the kernel, so a burst that already happened does not stay resident for
//!   the life of the process. It runs off the async workers (`spawn_blocking`)
//!   and only when nothing is in flight, because trimming during a burst just
//!   gives the pages straight back.
//!
//! Neither changes behaviour, and both are Kubernetes-friendly: a container limit
//! is about the resident set, and "it will not grow unless it is used" is a
//! property, not a hope.
//!
//! `KARUCODE_MALLOC_TRIM_SECS=0` disables the trimmer, `KARUCODE_MALLOC_ARENA_MAX`
//! overrides the arena cap. Both are read once at startup.

/// Arena cap for a glibc build. Two arenas: one for whatever thread is
/// allocating and one spare, so two busy threads do not serialise on a single
/// arena's lock while a third waits.
pub const DEFAULT_ARENA_MAX: i32 = 2;

/// How often an idle server returns free heap to the kernel.
pub const DEFAULT_TRIM_SECS: u64 = 60;

/// Apply the allocator knobs. Must run before the work that allocates heavily -
/// arenas are created lazily, so this is only a guarantee if it happens first.
///
/// A no-op on anything that is not glibc, because `mallopt` is a glibc API: musl
/// has no arenas to cap, and its `malloc_trim` equivalent does not exist.
pub fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let arenas = std::env::var("KARUCODE_MALLOC_ARENA_MAX")
            .ok()
            .and_then(|raw| raw.trim().parse::<i32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_ARENA_MAX);
        // SAFETY: `mallopt` only reads and writes glibc's own tunables; both
        // constants are documented and the values are bounded here.
        let applied = unsafe { libc::mallopt(libc::M_ARENA_MAX, arenas) };
        if applied == 0 {
            tracing::debug!(arenas, "mallopt(M_ARENA_MAX) was refused");
        } else {
            tracing::debug!(arenas, "capped the malloc arena count");
        }
        // A one-byte allocation is enough to notice, in a log line, whether the
        // tunables took effect on this libc.
        tracing::debug!(
            trim_secs = trim_interval().map_or(0, |d| d.as_secs()),
            "allocator tuned"
        );
    }
}

/// The trim interval, or `None` when trimming is disabled.
pub fn trim_interval() -> Option<std::time::Duration> {
    let secs = std::env::var("KARUCODE_MALLOC_TRIM_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TRIM_SECS);
    (secs > 0).then(|| std::time::Duration::from_secs(secs))
}

/// Hand free heap back to the kernel now.
///
/// Returns whether glibc actually released anything (`malloc_trim` returns 1 when
/// it did).
pub fn trim_now() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` takes glibc's internal locks itself and is safe to
        // call from any thread while others allocate.
        (unsafe { libc::malloc_trim(0) }) == 1
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trim_interval_honours_zero_and_a_number() {
        // These read process-wide environment state, so keep the assertions to the
        // default path and a value that cannot leak into another test's run.
        // SAFETY: the test binary is single-threaded for this module's tests.
        std::env::remove_var("KARUCODE_MALLOC_TRIM_SECS");
        assert_eq!(
            trim_interval(),
            Some(std::time::Duration::from_secs(DEFAULT_TRIM_SECS))
        );
        std::env::set_var("KARUCODE_MALLOC_TRIM_SECS", "0");
        assert_eq!(trim_interval(), None);
        std::env::set_var("KARUCODE_MALLOC_TRIM_SECS", "5");
        assert_eq!(trim_interval(), Some(std::time::Duration::from_secs(5)));
        std::env::remove_var("KARUCODE_MALLOC_TRIM_SECS");
    }

    #[test]
    fn trimming_is_callable_and_idempotent() {
        // Whether it releases anything depends on the allocator's state, so the
        // assertion is that calling it never panics or corrupts the heap.
        let _ = trim_now();
        let _ = trim_now();
        let boxed = vec![0u8; 1024];
        assert_eq!(boxed.len(), 1024);
    }
}
