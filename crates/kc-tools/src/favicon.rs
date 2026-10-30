//! The project's own `favicon.ico`, for the rail's per-project icon.
//!
//! The rail draws one square per project. Without this module that square is the
//! project's initials; with it, the square becomes the icon the project already
//! ships, which is how a user tells two checkouts of the same shape apart.
//!
//! The search lives here rather than in `kc-server` because walking a tree while
//! honouring `.gitignore` is what [`crate::file_ops::list_dir`] and
//! [`crate::search`] already do, and the `ignore` crate is already a dependency
//! of this crate, not of the server.
//!
//! The search is deliberately narrow, because `GET /api/projects/:id/favicon` is
//! fetched for every project on every load of the rail:
//!
//! * one file name (`favicon.ico`, matched case-insensitively);
//! * `.gitignore` rules apply — a project's own, nested ones, and
//!   `.git/info/exclude` — because a favicon inside an ignored directory is build
//!   output or a vendored copy, not the icon the project means. `.gitignore` is
//!   honoured even outside a git repository, and the operator's *global* gitignore
//!   is not read, so the answer does not depend on which machine answers;
//! * hidden entries are skipped, along with `node_modules` and `target`;
//! * at most [`MAX_DEPTH`] levels and [`MAX_ENTRIES`] entries are visited;
//! * the shallowest match wins and the walk is sorted, so the same tree always
//!   answers the same path (`favicon.ico` at the project root beats
//!   `public/favicon.ico`, which beats `web/public/favicon.ico`).
//!
//! Nothing is cached: the file is small and read on demand, so replacing a
//! project's favicon shows up on the next load of the rail instead of surviving
//! until a restart.

use kc_core::paths::Workspace;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

/// The only file name that counts as a project icon.
pub const FILE_NAME: &str = "favicon.ico";
/// How deep the search goes. A favicon lives at the root or one or two levels
/// down (`public/`, `web/public/`, `static/`); deeper than this is a vendored
/// copy or a test fixture.
pub const MAX_DEPTH: usize = 6;
/// Ceiling on visited entries, so a checkout with a huge unignored tree answers
/// in bounded time instead of walking it all.
pub const MAX_ENTRIES: usize = 20_000;
/// A favicon is a few kilobytes. Anything past this is not one, and the response
/// is capped so the rail cannot pull a 40 MB file over the connection.
pub const MAX_ICON_BYTES: u64 = 512 * 1024;

/// Never searched: dependency and build-artifact trees. Hidden directories
/// (including `.git`) are skipped by the walker itself.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target"];

/// The bytes of a project icon, ready to be served.
pub struct ProjectIcon {
    pub bytes: Vec<u8>,
    /// Sniffed from the bytes, not from the extension: plenty of `favicon.ico`
    /// files in the wild are really PNG or SVG.
    pub content_type: &'static str,
    /// Project-relative path of the file that was used, for logging.
    pub path: String,
}

/// The first usable `favicon.ico` under `workspace`, or `None`.
///
/// `None` is the normal case (most projects have no icon) and is not an error:
/// the rail falls back to initials.
pub fn load(workspace: &Workspace) -> Option<ProjectIcon> {
    let found = find(workspace)?;
    let rel = workspace.relative(&found);
    // Canonicalise and re-check containment: a symlinked `favicon.ico` that
    // points outside the project is refused here, exactly like a file tool's
    // read would be.
    let abs = workspace.resolve_existing(&rel).ok()?;
    let meta = std::fs::metadata(&abs).ok()?;
    if !meta.is_file() || meta.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = read_capped(&abs)?;
    Some(ProjectIcon {
        content_type: sniff(&bytes),
        bytes,
        path: rel,
    })
}

/// Locate the file [`load`] reads. Public so a caller can report *where* the
/// icon came from without reading it.
pub fn find(workspace: &Workspace) -> Option<PathBuf> {
    let walker = ignore::WalkBuilder::new(workspace.root())
        .max_depth(Some(MAX_DEPTH))
        // `.gitignore` applies outside a git repository too; the operator's
        // global excludes do not, so two machines answer alike.
        .require_git(false)
        .git_global(false)
        .follow_links(false)
        // Sorted siblings: two runs over the same tree agree on the winner.
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| entry.depth() == 0 || !is_skipped_dir(entry.file_name()))
        .build();

    let mut best: Option<(usize, PathBuf)> = None;
    let mut visited = 0usize;
    for entry in walker {
        visited += 1;
        if visited > MAX_ENTRIES {
            break;
        }
        let Ok(entry) = entry else { continue };
        // Directories are never icons; everything else (including a symlink,
        // which `load` resolves and re-checks) may be.
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(true) {
            continue;
        }
        if !entry.file_name().eq_ignore_ascii_case(FILE_NAME) {
            continue;
        }
        let depth = entry.depth();
        if best
            .as_ref()
            .is_none_or(|(best_depth, _)| depth < *best_depth)
        {
            best = Some((depth, entry.into_path()));
        }
        // Nothing can be shallower than a match directly under the root.
        if depth <= 1 {
            break;
        }
    }
    best.map(|(_, path)| path)
}

/// Read at most [`MAX_ICON_BYTES`] and refuse anything larger, so a file that
/// grew between the metadata check and the read cannot be read whole.
fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_ICON_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ICON_BYTES {
        return None;
    }
    Some(bytes)
}

fn is_skipped_dir(name: &std::ffi::OsStr) -> bool {
    SKIPPED_DIRS
        .iter()
        .any(|skipped| name.eq_ignore_ascii_case(skipped))
}

/// The MIME type of the icon, from its magic bytes.
///
/// The fallback is `image/x-icon` rather than `application/octet-stream`: the
/// browser then tries to decode it as an image, and a file that is not one
/// simply fails to render (the rail's `<img>` error handler puts the initials
/// back).
pub fn sniff(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x00\x00\x01\x00") {
        "image/x-icon"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if looks_like_svg(bytes) {
        "image/svg+xml"
    } else {
        "image/x-icon"
    }
}

/// A leading BOM or XML prolog is common in hand-written SVGs; only the opening
/// tag decides. The tag has to appear in the first kilobyte, so a text file that
/// merely mentions `<svg` is not mistaken for one.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "kc-favicon-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("project dir");
            Self { root }
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("parent dir");
            }
            std::fs::write(path, bytes).expect("write");
        }

        fn workspace(&self) -> Workspace {
            Workspace::new(&self.root).expect("workspace")
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A minimal but real ICO header (the walker and the sniffer only look at the
    /// magic bytes).
    const ICO: &[u8] = b"\x00\x00\x01\x00\x01\x00\x10\x10\x00\x00\x01\x00\x20\x00";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";

    #[test]
    fn a_root_icon_is_found() {
        let project = TempProject::new("root");
        project.write("favicon.ico", ICO);
        let icon = load(&project.workspace()).expect("icon");
        assert_eq!(icon.path, "favicon.ico");
        assert_eq!(icon.content_type, "image/x-icon");
        assert_eq!(icon.bytes, ICO);
    }

    #[test]
    fn a_nested_icon_is_found() {
        let project = TempProject::new("nested");
        project.write("web/public/favicon.ico", ICO);
        let icon = load(&project.workspace()).expect("icon");
        assert_eq!(icon.path, "web/public/favicon.ico");
    }

    #[test]
    fn the_shallowest_icon_wins() {
        // `assets/deep/...` sorts first in a depth-first walk, so a plain walk
        // would pick it; the depth rule is what makes the root icon win.
        let project = TempProject::new("shallow");
        project.write("assets/deep/nested/favicon.ico", ICO);
        project.write("public/favicon.ico", ICO);
        assert_eq!(
            find(&project.workspace()).unwrap(),
            project.root.join("public/favicon.ico")
        );

        project.write("favicon.ico", ICO);
        assert_eq!(
            find(&project.workspace()).unwrap(),
            project.root.join("favicon.ico")
        );
    }

    #[test]
    fn a_git_ignored_icon_is_skipped() {
        let project = TempProject::new("ignored");
        project.write(".gitignore", b"branding/\n*.generated.ico\n");
        project.write("branding/png/favicon.ico", ICO);
        // Ignored directly, and ignored by a nested `.gitignore`.
        project.write("public/favicon.ico", ICO);
        project.write("public/.gitignore", b"favicon.ico\n");
        assert!(
            find(&project.workspace()).is_none(),
            "both candidates are ignored: {:?}",
            find(&project.workspace())
        );

        // A non-ignored icon elsewhere is still found.
        project.write("static/favicon.ico", ICO);
        assert_eq!(
            find(&project.workspace()).unwrap(),
            project.root.join("static/favicon.ico")
        );
    }

    #[test]
    fn gitignore_applies_without_a_repository() {
        // `.require_git(false)`: no `.git` directory anywhere in sight.
        let project = TempProject::new("no-git");
        project.write(".gitignore", b"out/\n");
        project.write("out/favicon.ico", ICO);
        assert!(find(&project.workspace()).is_none());
        assert!(!project.root.join(".git").exists());
    }

    #[test]
    fn dependency_and_hidden_directories_are_skipped() {
        let project = TempProject::new("skipped");
        project.write("node_modules/pkg/favicon.ico", ICO);
        project.write("target/debug/favicon.ico", ICO);
        project.write(".cache/favicon.ico", ICO);
        assert!(find(&project.workspace()).is_none());

        project.write("src/favicon.ico", ICO);
        assert_eq!(
            find(&project.workspace()).unwrap(),
            project.root.join("src/favicon.ico")
        );
    }

    #[test]
    fn the_file_name_match_is_case_insensitive() {
        let project = TempProject::new("case");
        project.write("Favicon.ICO", ICO);
        assert_eq!(
            load(&project.workspace()).expect("icon").path,
            "Favicon.ICO"
        );
    }

    #[test]
    fn a_project_without_an_icon_answers_none() {
        let project = TempProject::new("none");
        project.write("index.html", b"<html></html>");
        project.write(
            "public/favicon.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        );
        assert!(find(&project.workspace()).is_none());
        assert!(load(&project.workspace()).is_none());
    }

    #[test]
    fn an_oversized_icon_is_refused() {
        let project = TempProject::new("oversized");
        let mut bytes = ICO.to_vec();
        bytes.resize(MAX_ICON_BYTES as usize + 1, 0);
        project.write("favicon.ico", &bytes);
        // Found — it is a `favicon.ico` — but not served.
        assert!(find(&project.workspace()).is_some());
        assert!(load(&project.workspace()).is_none());
    }

    #[test]
    fn an_empty_icon_is_refused() {
        let project = TempProject::new("empty");
        project.write("favicon.ico", b"");
        assert!(load(&project.workspace()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_escapes_the_project_is_refused() {
        let outside = TempProject::new("outside");
        outside.write("favicon.ico", ICO);
        let project = TempProject::new("symlink-out");
        std::os::unix::fs::symlink(
            outside.root.join("favicon.ico"),
            project.root.join("favicon.ico"),
        )
        .expect("symlink");
        assert!(load(&project.workspace()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_project_is_followed() {
        let project = TempProject::new("symlink-in");
        project.write("assets/icon.ico", ICO);
        std::os::unix::fs::symlink(
            project.root.join("assets/icon.ico"),
            project.root.join("favicon.ico"),
        )
        .expect("symlink");
        let icon = load(&project.workspace()).expect("icon");
        assert_eq!(icon.path, "favicon.ico");
    }

    #[test]
    fn the_content_type_follows_the_bytes_not_the_extension() {
        assert_eq!(sniff(ICO), "image/x-icon");
        assert_eq!(sniff(PNG), "image/png");
        assert_eq!(sniff(b"GIF89a..."), "image/gif");
        assert_eq!(sniff(b"\xff\xd8\xff\xe0...."), "image/jpeg");
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "image/webp");
        assert_eq!(
            sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            "image/svg+xml"
        );
        assert_eq!(
            sniff("\u{feff}<?xml version=\"1.0\"?>\n<svg/>".as_bytes()),
            "image/svg+xml"
        );
        // Not an image at all: still labelled as an icon so the browser has a
        // chance to decode it; the rail falls back to initials on failure.
        assert_eq!(sniff(b"<html>not an icon</html>"), "image/x-icon");
    }
}
