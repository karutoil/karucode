//! Workspace path confinement.
//!
//! Every file tool resolves paths through [`Workspace`]. The rules are:
//!
//! * the workspace root is canonicalised once at startup;
//! * absolute input paths must already live inside the root;
//! * `..` segments may not climb above the root (lexical check);
//! * for existing paths the fully canonicalised path (symlinks resolved) must
//!   still be inside the root, which blocks symlink escapes;
//! * for writes, the deepest existing ancestor is canonicalised and checked, so
//!   a symlinked directory cannot be used to escape either.
//!
//! These checks apply to the file tools only. They do **not** sandbox shell
//! commands, which run as the server user and can touch anything that user can.

use crate::{Error, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let canon = std::fs::canonicalize(root).map_err(|e| Error::Io {
            path: root.to_path_buf(),
            source: e,
        })?;
        if !canon.is_dir() {
            return Err(Error::Config(format!(
                "workspace root {} is not a directory",
                canon.display()
            )));
        }
        Ok(Self { root: canon })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a user/model supplied path against the root without touching the
    /// filesystem. Rejects absolute paths outside the root and `..` escapes.
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.lexical_join(input)?;
        if !candidate.starts_with(&self.root) {
            return Err(Error::PathEscape {
                path: candidate,
                root: self.root.clone(),
            });
        }
        Ok(candidate)
    }

    /// Resolve a path that is expected to exist and verify the *canonical*
    /// location is inside the workspace (blocks symlink escapes).
    pub fn resolve_existing(&self, input: &str) -> Result<PathBuf> {
        let lexical = self.resolve(input)?;
        let canon = std::fs::canonicalize(&lexical).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!("path does not exist: {input}"))
            } else {
                Error::Io {
                    path: lexical.clone(),
                    source: e,
                }
            }
        })?;
        if !canon.starts_with(&self.root) {
            return Err(Error::PathEscape {
                path: canon,
                root: self.root.clone(),
            });
        }
        Ok(canon)
    }

    /// Resolve a path for writing. The file itself may not exist yet, so the
    /// deepest existing ancestor is canonicalised instead.
    pub fn resolve_for_write(&self, input: &str) -> Result<PathBuf> {
        let lexical = self.resolve(input)?;
        let mut ancestor = lexical.clone();
        let mut missing: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match std::fs::symlink_metadata(&ancestor) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let name = ancestor.file_name().map(|n| n.to_os_string());
                    match (name, ancestor.parent()) {
                        (Some(n), Some(parent)) => {
                            missing.push(n);
                            ancestor = parent.to_path_buf();
                        }
                        _ => {
                            return Err(Error::Io {
                                path: lexical,
                                source: e,
                            })
                        }
                    }
                }
                Err(e) => {
                    return Err(Error::Io {
                        path: ancestor,
                        source: e,
                    })
                }
            }
        }
        let canon_ancestor = std::fs::canonicalize(&ancestor).map_err(|e| Error::Io {
            path: ancestor.clone(),
            source: e,
        })?;
        if !canon_ancestor.starts_with(&self.root) {
            return Err(Error::PathEscape {
                path: canon_ancestor,
                root: self.root.clone(),
            });
        }
        // If the target exists and is a symlink, refuse to write through it.
        if let Ok(meta) = std::fs::symlink_metadata(&lexical) {
            if meta.file_type().is_symlink() {
                let resolved = std::fs::canonicalize(&lexical).map_err(|e| Error::Io {
                    path: lexical.clone(),
                    source: e,
                })?;
                if !resolved.starts_with(&self.root) {
                    return Err(Error::PathEscape {
                        path: resolved,
                        root: self.root.clone(),
                    });
                }
            }
        }
        let mut out = canon_ancestor;
        for name in missing.iter().rev() {
            out.push(name);
        }
        Ok(out)
    }

    /// Path relative to the workspace root, for display. Never panics.
    pub fn relative(&self, abs: &Path) -> String {
        match abs.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => abs.to_string_lossy().to_string(),
        }
    }

    fn lexical_join(&self, input: &str) -> Result<PathBuf> {
        if input.contains('\0') {
            return Err(Error::BadRequest("path contains NUL byte".into()));
        }
        let raw = Path::new(input);
        let mut out = if raw.is_absolute() {
            PathBuf::new()
        } else {
            self.root.clone()
        };
        for comp in raw.components() {
            match comp {
                Component::Prefix(_) | Component::RootDir => {
                    if raw.is_absolute() {
                        out.push(Path::new("/"));
                    }
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(seg) => out.push(seg),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kc-core-paths-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/file.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn relative_paths_resolve_inside_root() {
        let dir = tempdir("rel");
        let ws = Workspace::new(&dir).unwrap();
        let p = ws.resolve("sub/file.txt").unwrap();
        assert_eq!(p, std::fs::canonicalize(&dir).unwrap().join("sub/file.txt"));
        assert_eq!(ws.relative(&p), "sub/file.txt");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn traversal_is_rejected() {
        let dir = tempdir("trav");
        let ws = Workspace::new(&dir).unwrap();
        let err = ws.resolve("../../etc/passwd").unwrap_err();
        assert!(matches!(err, Error::PathEscape { .. }));
        let err = ws.resolve("sub/../../../etc/passwd").unwrap_err();
        assert!(matches!(err, Error::PathEscape { .. }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn absolute_path_outside_root_is_rejected() {
        let dir = tempdir("abs");
        let ws = Workspace::new(&dir).unwrap();
        let err = ws.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, Error::PathEscape { .. }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn absolute_path_inside_root_is_allowed() {
        let dir = tempdir("absin");
        let ws = Workspace::new(&dir).unwrap();
        let inside = ws.root().join("sub/file.txt");
        let p = ws.resolve(inside.to_str().unwrap()).unwrap();
        assert_eq!(p, inside);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let dir = tempdir("sym");
        let outside = std::env::temp_dir().join(format!(
            "kc-core-outside-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let ws = Workspace::new(&dir).unwrap();
        assert!(ws.resolve_existing("link/secret.txt").is_err());
        assert!(ws.resolve_for_write("link/new.txt").is_err());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn write_resolution_handles_missing_parents() {
        let dir = tempdir("write");
        let ws = Workspace::new(&dir).unwrap();
        let p = ws.resolve_for_write("new/dir/file.txt").unwrap();
        assert!(p.starts_with(ws.root()));
        assert!(p.ends_with("new/dir/file.txt"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn nul_bytes_are_rejected() {
        let dir = tempdir("nul");
        let ws = Workspace::new(&dir).unwrap();
        assert!(ws.resolve("a\0b").is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
