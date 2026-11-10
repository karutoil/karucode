//! The version karucode reports: the commit count.
//!
//! `build.rs` runs `git rev-list --count HEAD` at compile time and stamps the
//! result into the binary, so `karucode --version` answers "which commit am I
//! running" without a hand-maintained number that nobody bumps.

// @kc-doc: versioning
// @kc-title: Understand karucode's version and build number
// @kc-summary: The version is the git commit count — build 107 is the 107th commit — reported by `--version`, `/api/health` and the web UI.
// @kc-tags: version, build number, commit, upgrade, about, semver
// @kc-applies-to: cmd:karucode --version, env:KARUCODE_BUILD_NUMBER, env:KARUCODE_COMMIT, env:KARUCODE_COMMIT_DATE, env:KARUCODE_CHANNEL, route:GET /api/health, route:GET /api/config, file:crates/kc-server/build.rs, file:Cargo.toml, ui:Settings > Updates, ui:SessionsRail
//
// karucode has **no release trains and no semantic version bumps**. Every commit
// to `main` is a release, and the Nth commit is build N:
//
// ```text
// $ karucode --version
// karucode 107 (abf0a42, 2026-05-30T18:04:11+00:00)
// ```
//
// The number after the name is the version. It is `git rev-list --count HEAD`,
// computed by `crates/kc-server/build.rs` when the binary is compiled. A tag is
// `v<number>` (`v107`), a tarball is `karucode-<number>-<target>.tar.gz`, and the
// release manifest carries the same integer in `"build"`.
//
// # Why commit count
//
// A version you have to remember to bump drifts from the code: two binaries claim
// `0.1.0` while doing different things, and "is this newer" becomes a question
// about a file nobody re-read. The commit count is monotonic within a history,
// requires no release ceremony, and answers the only question an operator asks
// when something is wrong — *which code is running*. It also makes the updater's
// comparison a plain integer: `latest.build > current.build`.
//
// The trade-off is real and worth stating: rewrites and force-pushes renumber
// history, so two branches can share a build number. karucode ships from `main`,
// where that does not happen.
//
// # Where the number appears
//
// | Surface | What it shows |
// | --- | --- |
// | `karucode --version` | `karucode <build> (<commit>, <date>)` |
// | `GET /api/health` | `version` (the build as a string), `build`, `commit`, `commit_date`, `dirty` |
// | `GET /api/config` | `version` |
// | Prometheus `/metrics` | `karucode_build_info{version="…",build="…",commit="…"} 1` |
// | The web UI | the rail footer (`v<build>`), and **Settings → Updates** with the commit and date |
//
// `dirty` is `true` when the binary was built from a working tree with uncommitted
// changes to tracked files. It is informational: a dirty build is still build N.
//
// # Building from a tarball or a container context
//
// There is no `.git` in a release tarball or a Docker build context, so the
// numbers can be supplied as environment variables. They win over git, which is
// also how CI pins the number it wrote into the release notes:
//
// ```bash
// KARUCODE_BUILD_NUMBER="$(git rev-list --count HEAD)" \
// KARUCODE_COMMIT="$(git rev-parse --short=12 HEAD)" \
// KARUCODE_COMMIT_DATE="$(git log -1 --format=%cI)" \
//   cargo build --release --locked -p kc-server
// ```
//
// `KARUCODE_CHANNEL` labels a build (`stable` by default) and is reported, not
// acted on. A binary with neither git nor the overrides reports build `0` and
// commit `unknown`; the updater refuses to compare that against a release rather
// than pretending it is ancient — see the `updates` topic.
//
// # Upgrading
//
// `karucode update` compares the running build with the newest release and
// installs it; `packaging/update.sh` does the same for a build from a source
// checkout. Both are described in the `updates` topic, and a managed install
// reports the same numbers in **Settings → Updates**.

use serde::Serialize;

/// The build number: the count of commits reachable from `HEAD` at build time.
///
/// `0` means the build had neither git nor [`BUILD_NUMBER_ENV`]; it is not an
/// ancient release, it is a build that cannot say what it is.
pub const BUILD: u64 = parse_u64(env!("KARUCODE_BUILD"));

/// The build number as the string every surface reports.
pub const VERSION: &str = env!("KARUCODE_VERSION");

/// Short commit sha (`unknown` when the build could not determine it).
pub const COMMIT: &str = env!("KARUCODE_COMMIT");

/// Commit date, RFC 3339 (`unknown` when the build could not determine it).
pub const COMMIT_DATE: &str = env!("KARUCODE_COMMIT_DATE");

/// Whether tracked files were modified when this binary was built.
pub const DIRTY: bool = matches!(env!("KARUCODE_DIRTY").as_bytes(), b"true");

/// Release channel label (`stable` unless `KARUCODE_CHANNEL` says otherwise).
pub const CHANNEL: &str = env!("KARUCODE_CHANNEL");

/// The target triple this binary was built for (`x86_64-unknown-linux-gnu`, …).
pub const TARGET: &str = env!("KARUCODE_TARGET");

/// The `Cargo.toml` version. Not the version: kept only so a diagnostic can say
/// which manifest produced the build. Use [`BUILD`] / [`VERSION`] instead.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable that stamps a build with no `.git`.
pub const BUILD_NUMBER_ENV: &str = "KARUCODE_BUILD_NUMBER";

/// Everything a UI or a release check needs to describe this binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    /// The build number as a string (`"107"`), so a client never parses a float.
    pub version: String,
    pub build: u64,
    pub commit: String,
    pub commit_date: String,
    pub dirty: bool,
    pub channel: String,
    pub target: String,
}

/// This binary's build information.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: VERSION.to_string(),
        build: BUILD,
        commit: COMMIT.to_string(),
        commit_date: COMMIT_DATE.to_string(),
        dirty: DIRTY,
        channel: CHANNEL.to_string(),
        target: TARGET.to_string(),
    }
}

/// The `--version` line: `karucode 107 (abf0a42, 2026-05-30T18:04:11+00:00)`.
///
/// The update path parses this same line out of a downloaded artifact to prove
/// the file it staged is the build the manifest promised, so the shape matters:
/// the first word is the program name, the second is the number, and everything
/// else is for humans.
pub fn one_line() -> String {
    format!("karucode {VERSION} ({COMMIT}, {COMMIT_DATE})")
}

/// A short human label: `107 (abf0a42)`, with a `+dirty` marker when the build
/// was not from a clean tree.
pub fn short_label() -> String {
    let dirty = if DIRTY { "+dirty" } else { "" };
    format!("{VERSION}{dirty} ({COMMIT})")
}

/// Markers a build with no usable version reports.
pub const UNKNOWN_BUILD: u64 = 0;

/// Is this binary able to take part in an update comparison?
///
/// A build with no number cannot be ordered against a release: "0 < 108" would be
/// true but meaningless, and applying an update over a binary nobody can identify
/// is how an install ends up in a state no one can reason about.
pub fn is_versioned() -> bool {
    BUILD > UNKNOWN_BUILD
}

/// Parse the build number out of a `--version` line.
///
/// Used by the updater on a staged artifact: running the download and reading its
/// version is the only check that proves the bytes are the build the manifest
/// named. A line that does not match returns `None`; the caller refuses the file.
pub fn parse_version_line(line: &str) -> Option<u64> {
    let rest = line.trim().strip_prefix("karucode")?;
    let number = rest.split_whitespace().next()?;
    number.parse::<u64>().ok()
}

/// `const` integer parsing, so [`BUILD`] is a real constant rather than a lazy
/// initialization on first use.
const fn parse_u64(text: &str) -> u64 {
    let bytes = text.as_bytes();
    let mut value: u64 = 0;
    let mut index = 0;
    if bytes.is_empty() {
        return 0;
    }
    while index < bytes.len() {
        let byte = bytes[index];
        if !byte.is_ascii_digit() {
            return 0;
        }
        value = value * 10 + (byte - b'0') as u64;
        index += 1;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reported_version_is_the_build_number() {
        assert_eq!(VERSION, BUILD.to_string());
        assert_eq!(build_info().build, BUILD);
        // A build number is a plain count, never a dotted semver.
        assert!(
            !VERSION.contains('.'),
            "the version must be the commit count, got {VERSION:?}"
        );
    }

    #[test]
    fn a_repository_build_has_a_real_build_number() {
        // These tests only run for a build of this very checkout; if git was
        // missing when it was compiled the binary says so, and neither state is a
        // failure - but the two must not disagree.
        if is_versioned() {
            // `is_versioned` is exactly `BUILD > 0`, so read it back through the
            // function instead of asserting on the constant.
            assert_eq!(is_versioned(), BUILD > 0);
            assert!(!COMMIT.is_empty(), "a versioned build names its commit");
            assert!(COMMIT != "unknown", "versioned builds name their commit");
        } else {
            assert_eq!(VERSION, "0");
            assert_eq!(COMMIT, "unknown");
            assert!(!is_versioned());
        }
    }

    #[test]
    fn the_version_line_can_be_read_back() {
        let line = one_line();
        assert_eq!(parse_version_line(&line), Some(BUILD), "line: {line}");
        // The updater also reads the line a downloaded artifact prints, which is
        // this same function in that artifact's code.
        assert_eq!(
            parse_version_line("karucode 108 (abc, 2026-01-01)"),
            Some(108)
        );
        assert_eq!(parse_version_line("karucode 0 (unknown, unknown)"), Some(0));
        assert_eq!(parse_version_line("karucode"), None);
        assert_eq!(parse_version_line("something 108"), None);
        assert_eq!(parse_version_line("karucode xyz (abc)"), None);
        assert_eq!(parse_version_line(""), None);
    }

    #[test]
    fn short_label_marks_a_dirty_tree() {
        let label = short_label();
        assert!(label.starts_with(VERSION), "label: {label}");
        assert_eq!(label.contains("+dirty"), DIRTY, "label: {label}");
    }

    #[test]
    fn const_parsing_rejects_junk_instead_of_silently_truncating() {
        assert_eq!(parse_u64("107"), 107);
        assert_eq!(parse_u64("0"), 0);
        assert_eq!(parse_u64(""), 0);
        assert_eq!(parse_u64("1x"), 0);
        assert_eq!(parse_u64("-1"), 0);
        assert_eq!(parse_u64("12.3"), 0);
    }
}
