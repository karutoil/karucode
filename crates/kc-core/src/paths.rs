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
//! # These are checks, not handles
//!
//! Resolution returns a *path*, and the caller then opens or writes that path by
//! name. That is check-then-use: a component swapped for a symlink between the
//! check and the `open`/`rename` defeats it. Closing that window needs
//! `openat2(RESOLVE_BENEATH)` or `cap-std` style passed handles, not a second
//! `stat` - a re-check just narrows the race.
//!
//! The window is deliberately not treated as a vulnerability of its own here,
//! for the same reason [`crate::config::Config::allow_temp_dir`] is not: the
//! agent can already run `run_command` as this user, so anything able to win
//! that race can simply read the file directly. The rules below stop a *model*
//! wandering out of its project with a normal `../` or a committed symlink; they
//! are not a containment guarantee against a process racing the server.
//!
//! # Extra roots
//!
//! A workspace may carry a small allowlist of *additional* roots (see
//! [`Workspace::with_extra_roots`]) and every rule above then holds against the
//! whole set: a path is allowed when it is inside the project root **or** inside
//! one of the extra roots, and `..`/symlink checks are made against the root the
//! path actually belongs to. A `/tmp` that is allowed therefore stays `/tmp`: a
//! path may not climb from `/tmp` up into `/etc`, and a symlink inside `/tmp`
//! that points at `/etc/shadow` is still refused.
//!
//! The one rule is that a declared extra root cannot be the filesystem root `/`:
//! that single value would make every absolute path "inside" a root and really
//! would turn confinement off. A root that merely *contains* the workspace (a
//! workspace under `/tmp`, as in CI) is fine - the temp dir being an ancestor is
//! exactly what such a layout asks for. See
//! [`Config::allow_temp_dir`](crate::config::Config::allow_temp_dir).
//!
//! These checks apply to the file tools only. They do **not** sandbox shell
//! commands, which run as the server user and can touch anything that user can.

use crate::{Error, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    /// Additional allowed roots, canonicalised and de-duplicated. Always
    /// disjoint from `root` and never an ancestor of it (enforced on build).
    extra: Vec<PathBuf>,
}

impl Workspace {
    /// A workspace confined to exactly `root`.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        Self::with_extra_roots(root, Vec::new())
    }

    /// A workspace confined to `root` plus `extra_roots`.
    ///
    /// Each extra root must exist and be a directory; anything else is skipped,
    /// because a root that cannot be canonicalised can never match a canonical
    /// path (the same fail-closed rule the checks use).
    ///
    /// The filesystem root `/` is refused: it would make every path "inside" a
    /// root, which is the one value that really does turn confinement off. A root
    /// that merely *contains* the workspace root is allowed - a workspace under
    /// `/tmp` is a normal scratch/CI layout, and there the temp dir being an
    /// ancestor is exactly what was asked for.
    pub fn with_extra_roots(root: impl AsRef<Path>, extra_roots: Vec<PathBuf>) -> Result<Self> {
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

        let mut extra: Vec<PathBuf> = Vec::new();
        for candidate in extra_roots {
            let Ok(canon_extra) = std::fs::canonicalize(&candidate) else {
                continue;
            };
            if !canon_extra.is_dir() {
                continue;
            }
            // `/` would make every absolute path legal, which is the whole disk.
            if canon_extra.parent().is_none() {
                tracing::warn!(
                    root = %canon.display(),
                    ignored = %canon_extra.display(),
                    "ignoring the filesystem root as an extra allowed root: it allows every path"
                );
                continue;
            }
            // Already the primary root: listing it again adds nothing.
            if canon_extra == canon {
                continue;
            }
            if extra.contains(&canon_extra) {
                continue;
            }
            extra.push(canon_extra);
        }
        Ok(Self { root: canon, extra })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The extra roots this workspace allows, largest-first is not guaranteed;
    /// the order is the order they were declared in (after canonicalisation).
    pub fn extra_roots(&self) -> &[PathBuf] {
        &self.extra
    }

    /// Every root a path may live under: the workspace root first, then the
    /// extra allowlist. Used by the checks below and by callers that need to
    /// re-verify a canonical path themselves (the upload route does).
    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.root.as_path()).chain(self.extra.iter().map(PathBuf::as_path))
    }

    /// Whether `abs` (already canonical when it matters) is inside any allowed
    /// root. Component-wise, so `/tmp-evil` is not inside `/tmp`.
    pub fn contains(&self, abs: &Path) -> bool {
        self.roots().any(|root| abs.starts_with(root))
    }

    /// The allowed root that would have contained `abs`, for error messages:
    /// the longest matching root when there is one. Falls back to the workspace
    /// root, which is what a caller almost always means to name.
    fn root_for(&self, abs: &Path) -> PathBuf {
        self.roots()
            .filter(|root| abs.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone())
    }

    /// Resolve a user/model supplied path against the root without touching the
    /// filesystem. Rejects absolute paths outside the allowed roots and `..`
    /// escapes above the root the path started in.
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.lexical_join(input)?;
        if !self.contains(&candidate) {
            return Err(Error::PathEscape {
                path: candidate,
                root: self.root.clone(),
            });
        }
        Ok(candidate)
    }

    /// Resolve a path that is expected to exist and verify the *canonical*
    /// location is inside one of the allowed roots (blocks symlink escapes).
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
        if !self.contains(&canon) {
            return Err(Error::PathEscape {
                path: canon,
                root: self.root_for(&lexical),
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
        if !self.contains(&canon_ancestor) {
            return Err(Error::PathEscape {
                path: canon_ancestor,
                root: self.root_for(&lexical),
            });
        }
        // If the target exists and is a symlink, refuse to write through it.
        if let Ok(meta) = std::fs::symlink_metadata(&lexical) {
            if meta.file_type().is_symlink() {
                let resolved = std::fs::canonicalize(&lexical).map_err(|e| Error::Io {
                    path: lexical.clone(),
                    source: e,
                })?;
                if !self.contains(&resolved) {
                    return Err(Error::PathEscape {
                        path: resolved,
                        root: self.root_for(&lexical),
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

    // ------------------------------------------------------------ extra roots ---

    /// A throwaway directory outside any workspace, standing in for `/tmp`.
    fn outside_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kc-core-extra-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_extra_root_is_usable_for_reads_and_writes() {
        let dir = tempdir("extra-ws");
        let shared = outside_dir("shared");
        std::fs::write(shared.join("scratch.txt"), "from tmp").unwrap();

        let ws = Workspace::with_extra_roots(&dir, vec![shared.clone()]).unwrap();
        assert_eq!(ws.extra_roots(), &[std::fs::canonicalize(&shared).unwrap()]);

        // Relative (resolved against the workspace root) still hits the root;
        // an absolute path into the extra root is now allowed.
        let abs = shared.join("scratch.txt");
        let read = ws.resolve_existing(abs.to_str().unwrap()).unwrap();
        assert_eq!(read, std::fs::canonicalize(&abs).unwrap());

        // A file that does not exist yet can be written into it.
        let new = shared.join("nested/deeper.txt");
        let write = ws.resolve_for_write(new.to_str().unwrap()).unwrap();
        assert!(write.starts_with(std::fs::canonicalize(&shared).unwrap()));
        assert!(write.ends_with("nested/deeper.txt"));
        // ... and `contains` agrees, which is what the upload route re-checks.
        assert!(ws.contains(&write));
        assert!(ws.contains(&read));

        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(shared).ok();
    }

    #[test]
    fn an_extra_root_does_not_widen_the_rest_of_the_filesystem() {
        let dir = tempdir("extra-scope");
        let shared = outside_dir("scope");
        let ws = Workspace::with_extra_roots(&dir, vec![shared.clone()]).unwrap();

        // `/etc` is still outside, and so is a sibling of the allowed root.
        assert!(matches!(
            ws.resolve("/etc/passwd").unwrap_err(),
            Error::PathEscape { .. }
        ));
        let sibling = outside_dir("sibling");
        let escape = sibling.join("file.txt");
        assert!(matches!(
            ws.resolve(escape.to_str().unwrap()).unwrap_err(),
            Error::PathEscape { .. }
        ));
        // `..` may not climb out of the allowed root either.
        let climb = shared.join("../etc/passwd");
        assert!(matches!(
            ws.resolve(climb.to_str().unwrap()).unwrap_err(),
            Error::PathEscape { .. }
        ));

        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(shared).ok();
        std::fs::remove_dir_all(sibling).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_an_extra_root_cannot_escape_it() {
        let dir = tempdir("extra-sym");
        let shared = outside_dir("sym-shared");
        let secret = outside_dir("sym-secret");
        std::fs::write(secret.join("shadow"), "root:x:0:0").unwrap();
        // `/tmp/link -> /some/other/place`, which is allowed but not reachable.
        std::os::unix::fs::symlink(&secret, shared.join("link")).unwrap();

        let ws = Workspace::with_extra_roots(&dir, vec![shared.clone()]).unwrap();
        let link = shared.join("link/shadow");
        assert!(matches!(
            ws.resolve_existing(link.to_str().unwrap()).unwrap_err(),
            Error::PathEscape { .. }
        ));
        let write = shared.join("link/planted.txt");
        assert!(matches!(
            ws.resolve_for_write(write.to_str().unwrap()).unwrap_err(),
            Error::PathEscape { .. }
        ));

        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(shared).ok();
        std::fs::remove_dir_all(secret).ok();
    }

    #[test]
    fn the_filesystem_root_is_ignored_but_a_containing_parent_is_kept() {
        let dir = tempdir("extra-ancestor");
        // `/` would allow everything, so it is the one entry that is refused.
        let ws = Workspace::with_extra_roots(&dir, vec![PathBuf::from("/")]).unwrap();
        assert!(ws.extra_roots().is_empty(), "{:?}", ws.extra_roots());
        assert!(matches!(
            ws.resolve("/etc/passwd").unwrap_err(),
            Error::PathEscape { .. }
        ));

        // A parent of the workspace is *not* refused: a workspace under `/tmp`
        // is a normal scratch/CI layout, and there the temp root must still work.
        let parent = std::fs::canonicalize(&dir)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let ws = Workspace::with_extra_roots(&dir, vec![parent.clone()]).unwrap();
        assert_eq!(ws.extra_roots(), std::slice::from_ref(&parent));
        // ... which means a sibling of the workspace is reachable now, exactly
        // like a `/tmp` workspace could reach `/tmp/other-scratch`.
        assert!(ws.resolve_existing(&parent.to_string_lossy()).is_ok());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_workspace_inside_an_extra_root_still_gets_that_root() {
        // The CI/scratch shape: workspace = <tmp>/ws, temp root = <tmp>.
        let tmp = std::env::temp_dir();
        let ws_dir = tmp.join(format!(
            "kc-core-inside-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&ws_dir).unwrap();
        let ws = Workspace::with_extra_roots(&ws_dir, vec![tmp.clone()]).unwrap();
        assert!(
            !ws.extra_roots().is_empty(),
            "the temp dir must not be dropped just because it contains the workspace"
        );
        // A file dropped next to the workspace (but outside it) is readable, and
        // one outside the temp dir is not.
        let dropped = tmp.join(format!(
            "kc-core-dropped-{}-{}.txt",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&dropped, "landed").unwrap();
        assert!(ws.resolve_existing(&dropped.to_string_lossy()).is_ok());
        assert!(matches!(
            ws.resolve("/etc/passwd").unwrap_err(),
            Error::PathEscape { .. }
        ));
        std::fs::remove_file(&dropped).ok();
        std::fs::remove_dir_all(ws_dir).ok();
    }

    #[test]
    fn a_missing_or_duplicate_extra_root_is_skipped() {
        let dir = tempdir("extra-missing");
        let missing = std::env::temp_dir().join(format!(
            "kc-core-does-not-exist-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let ws = Workspace::with_extra_roots(&dir, vec![missing.clone(), dir.clone(), dir.clone()])
            .unwrap();
        // The workspace root appears in the allowlist twice and is its own root,
        // so it must not be listed as an *extra* root; the missing entry is gone.
        assert!(ws.extra_roots().is_empty(), "{:?}", ws.extra_roots());
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
