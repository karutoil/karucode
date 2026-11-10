//! Build-time frontend bundling.
//!
//! The server embeds `web/dist` with `rust-embed` (feature `embed-ui`), so the
//! built binary is the whole product: no Node, no `web/dist` directory and no
//! `KARUCODE_WEB_DIR` at runtime. That only holds if `web/dist` exists *before*
//! this crate is compiled — a plain source checkout has no `web/dist` (it is
//! generated), which is why this script runs the frontend build for you.
//!
//! Order of preference:
//!
//! 1. `KARUCODE_WEB_DIST=<dir>` — a prebuilt bundle (release tarball, CI
//!    artifact, package feed) is copied to `web/dist`. No Node needed anywhere.
//! 2. `web/dist` is fresh relative to the frontend sources — nothing to do.
//! 3. `npm` is available — `npm ci` (only when `node_modules` is missing or the
//!    lockfile moved) then `npm run build`, and the fresh `web/dist` is embedded.
//! 4. `npm` is missing but a `web/dist` from an earlier build exists — it is
//!    embedded with a warning, because a stale UI beats no UI.
//!
//! Otherwise the build stops with an actionable message instead of producing a
//! binary that silently serves the "UI is not built" placeholder.
//!
//! Set `KARUCODE_SKIP_WEB_BUILD=1` to opt out entirely (Rust-only iteration on a
//! crate that does not touch the frontend, or a build host without Node).
//!
//! It also stamps the **build number**: `git rev-list --count HEAD`, the version
//! karucode actually reports. There are no releases with invented version
//! numbers - every commit is the release, and the Nth commit is build N. See
//! `src/version.rs` for why, and for the environment overrides a git-less build
//! (a release tarball, a Docker build context) uses instead.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=KARUCODE_SKIP_WEB_BUILD");
    println!("cargo:rerun-if-env-changed=KARUCODE_WEB_DIST");
    println!("cargo:rerun-if-env-changed=KARUCODE_NPM");
    stamp_version();

    // Without the embed feature the binary only ever serves KARUCODE_WEB_DIR, so
    // bundling the frontend here would be wasted work.
    if env::var_os("CARGO_FEATURE_EMBED_UI").is_none() {
        println!(
            "cargo:warning=kc-server: feature `embed-ui` is off; the binary serves the UI only \
             from KARUCODE_WEB_DIR and nothing is embedded"
        );
        return;
    }

    let web = match web_dir() {
        Some(dir) => dir,
        None => {
            println!(
                "cargo:warning=kc-server: no `web/` directory next to the crates; the embedded UI \
                 will be whatever `web/dist` contains (usually nothing)"
            );
            return;
        }
    };
    let dist = web.join("dist");

    // Cargo must re-run this script when the frontend sources change, otherwise a
    // UI edit would never reach the binary. Watching the directories (not a file
    // list) also covers brand-new files.
    for input in watched_inputs(&web) {
        println!("cargo:rerun-if-changed={}", input.display());
    }

    if let Some(prebuilt) = prebuilt_dist() {
        sync_dist(&prebuilt, &dist);
        require_index(&dist, "KARUCODE_WEB_DIST");
        return;
    }

    if skip_requested() {
        if !dist.join("index.html").is_file() {
            println!(
                "cargo:warning=kc-server: KARUCODE_SKIP_WEB_BUILD is set and {} has no \
                 index.html; the binary will serve the `UI is not built` placeholder",
                dist.display()
            );
        }
        return;
    }

    if !needs_build(&web, &dist) {
        return;
    }

    let npm = npm_command();
    if !npm_available(&npm, &web) {
        if dist.join("index.html").is_file() {
            println!(
                "cargo:warning=kc-server: npm not found; embedding the existing {} as-is. \
                 It may not match the current frontend sources.",
                dist.display()
            );
            return;
        }
        panic!(
            "kc-server: cannot bundle the web UI and npm was not found on PATH.\n\
             \n\
             The binary embeds the frontend, so `web/dist` must exist before it compiles.\n\
             Pick one:\n\
             * install Node.js 20+ and rebuild — `cargo build` then runs `npm ci` + \
             `npm run build` for you;\n\
             * build once elsewhere and pass the bundle: \
             `KARUCODE_WEB_DIST=/path/to/web/dist cargo build`;\n\
             * build a Rust-only binary without a UI: \
             `cargo build -p kc-server --no-default-features` (serve one with \
             KARUCODE_WEB_DIR at runtime);\n\
             * skip embedding for now: `KARUCODE_SKIP_WEB_BUILD=1 cargo build` \
             (placeholder page until the frontend is built).\n\
             \n\
             Expected bundle location: {}",
            dist.display()
        );
    }

    if needs_install(&web) {
        run(&npm, &["ci"], &web);
    }
    run(&npm, &["run", "build"], &web);
    require_index(&dist, "`npm run build`");
}

/// `<workspace>/web`: `crates/kc-server` -> `crates` -> root.
fn web_dir() -> Option<PathBuf> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let root = manifest.parent()?.parent()?;
    let web = root.join("web");
    web.is_dir().then_some(web)
}

/// Stamp the build number, commit, date, dirty flag, channel and target into the
/// binary as compile-time environment variables.
///
/// The build number is the count of commits reachable from `HEAD`. It is the
/// version karucode reports: the project does not cut semantic releases, so a
/// hand-maintained version in `Cargo.toml` would be a number nobody bumps and a
/// comparison that means nothing. "Which commit am I running" is the only version
/// question an operator actually asks, and `git rev-list --count` answers it
/// monotonically for every build in the same history.
///
/// A build with no `.git` (a release tarball, a Docker build context, a vendored
/// copy) must not silently report build `0`, so the values can be supplied:
///
/// * `KARUCODE_BUILD_NUMBER` - the commit count,
/// * `KARUCODE_COMMIT` - the short sha,
/// * `KARUCODE_COMMIT_DATE` - an RFC 3339 timestamp,
/// * `KARUCODE_CHANNEL` - `stable` (default) or a named pre-release channel.
///
/// With neither git nor the overrides the binary reports build `0` and says so;
/// the updater refuses to compare against it rather than treating it as ancient.
fn stamp_version() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    for name in [
        "KARUCODE_BUILD_NUMBER",
        "KARUCODE_COMMIT",
        "KARUCODE_COMMIT_DATE",
        "KARUCODE_CHANNEL",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    // A commit or a checkout moves one of these; watching them is what makes a
    // plain `cargo build` after `git pull` report the new build number instead of
    // the number from the last time the script happened to run. Cargo rescans a
    // watched directory, so `.git/refs` catches branch tips and tags too.
    let git_dir = git_dir(&root);
    if let Some(dir) = &git_dir {
        for input in [dir.join("HEAD"), dir.join("logs/HEAD"), dir.join("refs")] {
            if input.exists() {
                println!("cargo:rerun-if-changed={}", input.display());
            }
        }
    }

    let build = env_override("KARUCODE_BUILD_NUMBER")
        .or_else(|| git(&root, &["rev-list", "--count", "HEAD"]))
        .map(|value| value.trim().to_string())
        .filter(|value| value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty())
        .unwrap_or_else(|| "0".to_string());
    let commit = env_override("KARUCODE_COMMIT")
        .or_else(|| git(&root, &["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let commit_date = env_override("KARUCODE_COMMIT_DATE")
        .or_else(|| git(&root, &["log", "-1", "--format=%cI", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = if env_override("KARUCODE_COMMIT").is_some() {
        // A stamped build (CI) describes its commit, not the working tree it was
        // assembled in.
        "false".to_string()
    } else {
        let dirty = git(&root, &["status", "--porcelain", "--untracked-files=no"])
            .map(|out| !out.trim().is_empty())
            .unwrap_or(false);
        dirty.to_string()
    };
    let channel = env_override("KARUCODE_CHANNEL").unwrap_or_else(|| "stable".to_string());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=KARUCODE_VERSION={build}");
    println!("cargo:rustc-env=KARUCODE_BUILD={build}");
    println!("cargo:rustc-env=KARUCODE_COMMIT={commit}");
    println!("cargo:rustc-env=KARUCODE_COMMIT_DATE={commit_date}");
    println!("cargo:rustc-env=KARUCODE_DIRTY={dirty}");
    println!("cargo:rustc-env=KARUCODE_CHANNEL={channel}");
    println!("cargo:rustc-env=KARUCODE_TARGET={target}");
}

/// An override that is present and not blank. A blank variable is treated as
/// absent: `KARUCODE_BUILD_NUMBER=` in a CI step must not produce an empty
/// version string.
fn env_override(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The absolute git directory for `root`, when this really is a checkout.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dir = git(root, &["rev-parse", "--absolute-git-dir"])?;
    let dir = PathBuf::from(dir.trim());
    dir.is_dir().then_some(dir)
}

/// Run one git command in the repository. `None` on any failure (no git binary,
/// no repository, a command that exits non-zero) - never a build error, because
/// a source build without git is still a build.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn watched_inputs(web: &Path) -> Vec<PathBuf> {
    [
        "package.json",
        "package-lock.json",
        "tsconfig.json",
        "vite.config.ts",
        "index.html",
        "public",
        "src",
    ]
    .iter()
    .map(|name| web.join(name))
    .filter(|path| path.exists())
    .collect()
}

fn prebuilt_dist() -> Option<PathBuf> {
    let value = env::var_os("KARUCODE_WEB_DIST")?;
    let path = PathBuf::from(value);
    if path.is_dir() {
        Some(path)
    } else {
        panic!(
            "kc-server: KARUCODE_WEB_DIST={} is not a directory holding a built frontend",
            path.display()
        );
    }
}

fn skip_requested() -> bool {
    matches!(
        env::var("KARUCODE_SKIP_WEB_BUILD")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Overridable for hosts where npm is not on PATH or is a shim.
fn npm_command() -> String {
    env::var("KARUCODE_NPM").unwrap_or_else(|_| "npm".to_string())
}

fn npm_available(npm: &str, cwd: &Path) -> bool {
    Command::new(npm)
        .arg("--version")
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// True when the bundle is missing or older than the newest frontend source.
fn needs_build(web: &Path, dist: &Path) -> bool {
    let Ok(index) = fs::metadata(dist.join("index.html")).and_then(|m| m.modified()) else {
        return true;
    };
    let mut newest = index;
    for input in watched_inputs(web) {
        if let Some(time) = newest_mtime(&input, 0) {
            if time > newest {
                newest = time;
            }
        }
    }
    newest > index
}

/// `npm ci` only when the dependency tree is absent or was installed from a
/// different lockfile; `npm run build` on every rebuild.
fn needs_install(web: &Path) -> bool {
    let modules = web.join("node_modules");
    if !modules.is_dir() {
        return true;
    }
    let stamp = modules.join(".package-lock.json");
    let Ok(stamp_time) = fs::metadata(&stamp).and_then(|m| m.modified()) else {
        return true;
    };
    match fs::metadata(web.join("package-lock.json")).and_then(|m| m.modified()) {
        Ok(lock_time) => lock_time > stamp_time,
        // No lockfile: install only when the dependency tree is missing.
        Err(_) => false,
    }
}

fn newest_mtime(path: &Path, depth: usize) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    let mut newest = meta.modified().ok()?;
    if !meta.is_dir() || depth >= 8 {
        return Some(newest);
    }
    for entry in fs::read_dir(path).ok()?.flatten() {
        let name = entry.file_name();
        // Build output and dependencies are not inputs.
        if name == "dist" || name == "node_modules" || name == ".git" {
            continue;
        }
        if let Some(time) = newest_mtime(&entry.path(), depth + 1) {
            if time > newest {
                newest = time;
            }
        }
    }
    Some(newest)
}

fn run(npm: &str, args: &[&str], cwd: &Path) {
    let shown = format!("{npm} {}", args.join(" "));
    println!(
        "cargo:warning=kc-server: running `{shown}` in {}",
        cwd.display()
    );
    let status = Command::new(npm)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("kc-server: could not spawn `{shown}`: {e}"));
    if !status.success() {
        panic!(
            "kc-server: `{shown}` failed with {status}.\n\
             Fix the frontend build (the error is above) or build a Rust-only binary with \
             `--no-default-features`. The binary was not produced."
        );
    }
}

fn require_index(dist: &Path, origin: &str) {
    if !dist.join("index.html").is_file() {
        panic!(
            "kc-server: {origin} left no index.html in {}, so there is nothing to embed.\n\
             Check the Vite `build.outDir` setting in web/vite.config.ts.",
            dist.display()
        );
    }
}

/// Replace `dist` with `source` so the embedded snapshot is exactly the bundle
/// that was handed to the build (a stale leftover file must not survive).
fn sync_dist(source: &Path, dist: &Path) {
    let source = fs::canonicalize(source).unwrap_or_else(|e| {
        panic!(
            "kc-server: cannot read KARUCODE_WEB_DIST {}: {e}",
            source.display()
        )
    });
    if fs::canonicalize(dist).ok().as_deref() == Some(source.as_path()) {
        return;
    }
    if dist.exists() {
        fs::remove_dir_all(dist)
            .unwrap_or_else(|e| panic!("kc-server: cannot clear {}: {e}", dist.display()));
    }
    copy_tree(&source, dist);
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to)
        .unwrap_or_else(|e| panic!("kc-server: cannot create {}: {e}", to.display()));
    let entries = fs::read_dir(from)
        .unwrap_or_else(|e| panic!("kc-server: cannot list {}: {e}", from.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap_or_else(|e| {
                panic!(
                    "kc-server: cannot copy {} to {}: {e}",
                    path.display(),
                    dest.display()
                )
            });
        }
    }
}
