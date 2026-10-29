//! Source discovery, shared by `build.rs` and the `kc-docs` CLI.
//!
//! Like [`crate::parse`] this module is compiled twice (library and build
//! script), so it uses `super::` and only `std`.

use super::parse::{self, DocError, ParsedDoc};
use std::fs;
use std::path::{Path, PathBuf};

/// Directories (relative to the workspace root) that may contain doc blocks.
/// `examples/` is excluded on purpose: it is sample *user* workspace, not
/// karucode. `web/dist` and `node_modules` never contain hand-written source.
pub const SCAN_ROOTS: &[&str] = &["crates", "plugins", "web/src"];

pub const SOURCE_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "js", "jsx"];

pub const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", ".git"];

/// Parse every `@kc-doc` block under `root`, in deterministic file order.
pub fn collect(root: &Path) -> (Vec<ParsedDoc>, Vec<DocError>) {
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    for dir in SCAN_ROOTS {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        walk(&base, &mut files);
        files.sort();
        for file in files {
            let Ok(text) = fs::read_to_string(&file) else {
                continue; // non-UTF-8 files cannot contain our markers
            };
            let rel = relative_path(root, &file);
            let (mut found, mut file_errors) = parse::extract(&rel, &text);
            docs.append(&mut found);
            errors.append(&mut file_errors);
        }
    }
    (docs, errors)
}

/// Recursively collect source files, skipping build output and hidden dirs.
pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.') {
                continue;
            }
            walk(&path, out);
        } else if meta.is_file() {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if SOURCE_EXTENSIONS.contains(&ext.as_str()) {
                out.push(path);
            }
        }
    }
}

/// Workspace-relative path with forward slashes, for stable output across hosts.
pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
