#!/usr/bin/env python3
"""Stage the documentation as a mkdocs tree, rewriting the links that escape it.

`docs/` is written for the repository. Every link is relative, a handful of them
point outside the tree (`../AGENTS.md`, `../crates/kc-mcp`), and the homepage a
site expects is the root `README.md`, not a file under `docs/`. A static site has
no repository above it, so this script builds a staging directory that mkdocs can
import as-is:

* the tree is built from *git-tracked* files under `docs/`, so a developer's site
  is the one a clean clone produces (the design captures and the internal audit
  are gitignored, and would otherwise only exist on one machine);
* the pages a site needs at the top level are staged too: `README.md` becomes the
  homepage, and `CONTRIBUTING.md`, `SECURITY.md` (which replaces the redirect
  stub at `docs/SECURITY.md`) and `CHANGELOG.md` are linked from it;
* links are rewritten per page: a target that stays inside the staged tree is
  left relative, a target under `docs/` loses that prefix, and anything else
  becomes a `github.com` blob/tree URL - or a `raw.githubusercontent.com` URL
  when it is an image, so the page shows the picture rather than a file view.

Only real links are touched. Fenced code blocks and inline code spans are copied
verbatim, which keeps examples like ``](../X)`` in `docs/WEBSITE.md` readable.

Then mkdocs builds the staging directory, selected by `KARUCODE_SITE_DOCS` in
`mkdocs.yml`:

    python3 scripts/build_site.py
    KARUCODE_SITE_DOCS=.site-docs mkdocs build --strict
"""

from __future__ import annotations

import argparse
import posixpath
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Optional

DEFAULT_OUT = ".site-docs"
BRANCH = "main"

# Pages that live at the repository root and become pages of the site. `README.md`
# is the homepage; the stub at `docs/SECURITY.md` is replaced by the real file.
ROOT_PAGES = ("README.md", "CONTRIBUTING.md", "SECURITY.md", "CHANGELOG.md")

_FENCE = re.compile(r"^\s*(```|~~~)")
_INLINE_CODE = re.compile(r"(`+)([^`\n]+?)\1")
_MD_LINK = re.compile(r"(!?\[[^\]]*\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
_HTML_ATTR = re.compile(r"(\s(?:src|href)=\")([^\"]+)(\")")
_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")


def repo_url(repo_root: Path) -> str:
    """The public repository URL, read from mkdocs.yml so it has one source."""
    config = repo_root / "mkdocs.yml"
    if config.is_file():
        match = re.search(r"^repo_url:\s*(\S+)\s*$", config.read_text(encoding="utf-8"), re.M)
        if match:
            return match.group(1).rstrip("/")
    return "https://github.com/karutoil/karucode"


def tracked_docs(repo_root: Path) -> list[str]:
    """Repository-relative paths of the documentation files to stage.

    `git ls-files` is the point of the exercise: an ignored file is not part of a
    clean clone, so it must not be part of the site either. Without git (a source
    tarball, a test fixture) every file under `docs/` is used.
    """
    try:
        result = subprocess.run(
            ["git", "-C", str(repo_root), "ls-files", "-z", "--", "docs"],
            check=True,
            capture_output=True,
        )
        paths = [path for path in result.stdout.decode("utf-8").split("\0") if path]
        if paths:
            return sorted(paths)
    except (OSError, subprocess.CalledProcessError):
        pass
    return sorted(
        str(path.relative_to(repo_root))
        for path in (repo_root / "docs").rglob("*")
        if path.is_file()
    )


def github_url(url: str, repo_root: Path, path: str, kind: str) -> str:
    """A github.com URL for a repository path, raw for an image."""
    if kind == "image":
        rest = url.split("github.com/", 1)[-1]
        return f"https://raw.githubusercontent.com/{rest}/{BRANCH}/{path}"
    verb = "tree" if (repo_root / path).is_dir() else "blob"
    return f"{url}/{verb}/{BRANCH}/{path}"


def _split_suffix(target: str) -> tuple[str, str]:
    """Split `path?query#fragment` into the path and the part that must survive."""
    match = re.match(r"^([^?#]*)([?#].*)?$", target)
    if not match:
        return target, ""
    return match.group(1), match.group(2) or ""


def _resolve(
    target: str,
    *,
    base: str,
    root_relative: bool,
    repo_root: Path,
    site_root: Path,
    url: str,
    kind: str,
) -> Optional[str]:
    """The rewritten target, or None to leave the link exactly as written."""
    if not target or target.startswith(("#", "/")) or _SCHEME.match(target):
        return None
    path, suffix = _split_suffix(target)
    if not path:
        return None

    if root_relative:
        # README and friends are written against the repository root.
        norm = posixpath.normpath(path)
        if norm in ROOT_PAGES:
            return norm + suffix
        if norm == "docs" or norm.startswith("docs/"):
            inner = norm[len("docs/"):] if norm.startswith("docs/") else ""
            if inner:
                staged = site_root / inner
                index = staged / "README.md"
                if staged.is_dir() and index.is_file():
                    # A directory is not a page; its README is.
                    return f"{inner.rstrip('/')}/README.md" + suffix
                if staged.exists():
                    return inner + suffix
            # `docs/` itself has no page of its own; the tree URL is honest.
            return github_url(url, repo_root, norm, "link") + suffix
        if (repo_root / norm).exists():
            return github_url(url, repo_root, norm, kind) + suffix
        return None

    # Every other page is a file that lives under docs/, so resolve against it.
    norm = posixpath.normpath(posixpath.join(base, path))
    if not norm.startswith(".."):
        # Still inside the staged tree: mkdocs's own link checker owns it. A
        # directory, though, has no page - point at its README when it has one.
        staged = site_root / norm
        index = staged / "README.md"
        if staged.is_dir() and index.is_file():
            return posixpath.relpath(f"{norm}/README.md", base or ".") + suffix
        return None
    # One `..` above the staged tree is the repository root, because the staged
    # tree itself is `docs/`. Anything above that is not ours to point at.
    candidate = norm[3:] if norm.startswith("../") else norm
    if not candidate or candidate == "." or candidate.startswith(".."):
        return None
    if candidate in ROOT_PAGES:
        return posixpath.relpath(candidate, base or ".") + suffix
    if (repo_root / candidate).exists():
        return github_url(url, repo_root, candidate, kind) + suffix
    return None


def _rewrite_segment(
    segment: str,
    *,
    base: str,
    root_relative: bool,
    repo_root: Path,
    site_root: Path,
    url: str,
) -> str:
    def markdown(match: re.Match[str]) -> str:
        kind = "image" if match.group(1).startswith("!") else "link"
        rewritten = _resolve(
            match.group(2),
            base=base,
            root_relative=root_relative,
            repo_root=repo_root,
            site_root=site_root,
            url=url,
            kind=kind,
        )
        if rewritten is None:
            return match.group(0)
        return f"{match.group(1)}{rewritten}{match.group(3)}"

    def html(match: re.Match[str]) -> str:
        kind = "image" if "src=" in match.group(1) else "link"
        rewritten = _resolve(
            match.group(2),
            base=base,
            root_relative=root_relative,
            repo_root=repo_root,
            site_root=site_root,
            url=url,
            kind=kind,
        )
        if rewritten is None:
            return match.group(0)
        return f"{match.group(1)}{rewritten}{match.group(3)}"

    segment = _MD_LINK.sub(markdown, segment)
    return _HTML_ATTR.sub(html, segment)


def _protect_code(line: str) -> tuple[str, list[str]]:
    """Swap inline code spans for placeholders, so their text is never a link.

    A span can sit inside a link's text (``[`AGENTS.md`](../AGENTS.md)``); replacing
    it keeps the surrounding markdown link in one piece for the rewrite.
    """
    saved: list[str] = []

    def protect(match: re.Match[str]) -> str:
        saved.append(match.group(0))
        return f"\x00{len(saved) - 1}\x00"

    return _INLINE_CODE.sub(protect, line), saved


def _restore_code(line: str, saved: list[str]) -> str:
    for index, span in enumerate(saved):
        line = line.replace(f"\x00{index}\x00", span)
    return line


def rewrite_page(
    path: Path,
    *,
    src_uri: str,
    root_relative: bool,
    repo_root: Path,
    site_root: Path,
    url: str,
) -> None:
    """Rewrite one staged page in place, leaving fenced and inline code alone."""
    base = posixpath.dirname(src_uri)
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_fence = False
    for index, line in enumerate(lines):
        if _FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        # Inline code is not a link, and it may be the link's text.
        protected, saved = _protect_code(line)
        lines[index] = _restore_code(
            _rewrite_segment(
                protected,
                base=base,
                root_relative=root_relative,
                repo_root=repo_root,
                site_root=site_root,
                url=url,
            ),
            saved,
        )
    path.write_text("".join(lines), encoding="utf-8")


def build(repo_root: Path, out: Path) -> Path:
    """Stage the site tree under `out` and return it."""
    repo_root = repo_root.resolve()
    out = out.resolve()
    if repo_root != out and repo_root not in out.parents:
        raise SystemExit(f"refusing to write outside the repository: {out}")
    if out == repo_root:
        raise SystemExit("refusing to stage over the repository itself")

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    # The docs tree, minus anything git does not track.
    for rel in tracked_docs(repo_root):
        source = repo_root / rel
        if not source.is_file():
            continue
        dest = out / rel[len("docs/"):] if rel.startswith("docs/") else out / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)

    # The root pages a site needs at its top level.
    staged_roots: set[str] = set()
    for name in ROOT_PAGES:
        source = repo_root / name
        if source.is_file():
            shutil.copy2(source, out / name)
            staged_roots.add(name)

    url = repo_url(repo_root)
    for page in sorted(out.rglob("*.md")):
        rel = page.relative_to(out).as_posix()
        rewrite_page(
            page,
            src_uri=rel,
            root_relative=rel in staged_roots,
            repo_root=repo_root,
            site_root=out,
            url=url,
        )
    return out


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (default: the parent of scripts/)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help=f"staging directory (default: <repo-root>/{DEFAULT_OUT})",
    )
    args = parser.parse_args(argv)
    repo_root = args.repo_root.resolve()
    out = args.out.resolve() if args.out else repo_root / DEFAULT_OUT
    staged = build(repo_root, out)
    pages = len(list(staged.rglob("*.md")))
    print(f"ok: staged {pages} pages in {staged}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
