#!/usr/bin/env python3
"""Structure and link check for README.md and the markdown it points at.

The README is the front door of this project and the only file that is copied
into a release tarball, so a broken relative link there is a broken install note.
This script proves the claims the README makes about itself instead of trusting
that someone looked:

  * every relative link resolves to a file that exists;
  * every `#fragment` resolves to a heading in the target, using GitHub's slug
    rules (so `## Quick start` -> `#quick-start`);
  * the table of contents lists exactly the level-2 sections, in order;
  * code fences are balanced and language-tagged;
  * no heading is duplicated (a duplicate silently steals the first anchor);
  * no line has trailing whitespace, no file ends without a newline;
  * ASCII diagrams inside ```text blocks have equal display width per line, so a
    box drawn by hand actually lines up on every renderer.

Usage:
    python3 scripts/check_readme.py [--root .] [files...]

Exits 0 when everything holds, 1 with a `path:line: problem` list otherwise.
`--fix-toc` rewrites the Contents table from the current headings.
"""

import argparse
import pathlib
import re
import sys
import unicodedata

FENCE = re.compile(r"^(\s*)```(\S*)\s*$")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*$")
LINK = re.compile(r"(?<!!)\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HTML_SRC = re.compile(r"<img\s[^>]*src=\"([^\"]+)\"", re.IGNORECASE)
REF_DEF = re.compile(r"^\s*\[[^\]]+\]:\s*(\S+)")
SCHEME = re.compile(r"^[a-z][a-z0-9+.-]*:", re.IGNORECASE)


def display_width(text: str) -> int:
    """Terminal columns, so box-drawing characters count as one cell."""
    return sum(2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in text)


def github_slug(heading: str) -> str:
    """The anchor GitHub generates for a heading.

    Lowercase, punctuation dropped, spaces to hyphens. Inline code keeps its
    characters (`` `Host` `` -> `host`), which is why backticks are stripped
    rather than treated as punctuation.
    """
    text = heading.strip()
    text = re.sub(r"!\[([^\]]*)\]\([^)]*\)", r"\1", text)  # images -> alt text
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)  # links -> label
    text = text.replace("`", "")
    text = text.lower()
    text = re.sub(r"[^\w\- ]", "", text, flags=re.UNICODE)
    return text.replace(" ", "-")


def headings_of(text: str):
    """(line_number, level, raw_title, slug) for every heading outside fences."""
    out = []
    in_fence = False
    for number, line in enumerate(text.split("\n"), start=1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        match = HEADING.match(line)
        if match:
            out.append((number, len(match.group(1)), match.group(2), github_slug(match.group(2))))
    return out


def strip_fences(text: str):
    """(line_number, line) for lines outside fenced code blocks."""
    in_fence = False
    for number, line in enumerate(text.split("\n"), start=1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            yield number, line


def check(root: pathlib.Path, path: pathlib.Path, problems: list) -> None:
    rel = path.relative_to(root)
    text = path.read_text(encoding="utf-8")

    def bad(line: int, message: str) -> None:
        problems.append(f"{rel}:{line}: {message}")

    if not text.endswith("\n"):
        bad(text.count("\n") + 1, "file does not end with a newline")
    if "\t" in text:
        bad(text.count("\n") + 1, "contains a tab (use spaces)")

    # --- fences ------------------------------------------------------------
    fences = [n for n, line in enumerate(text.split("\n"), start=1) if FENCE.match(line)]
    if len(fences) % 2:
        bad(fences[-1] if fences else 1, "unclosed code fence")
    in_fence, fence_lang = False, None
    for number, line in enumerate(text.split("\n"), start=1):
        match = FENCE.match(line)
        if match:
            if not in_fence:
                fence_lang = match.group(2)
                if path.name == "README.md" and not fence_lang:
                    bad(number, "code fence has no language tag (use ```text for output)")
            in_fence = not in_fence
            continue
        if not in_fence and line != line.rstrip():
            bad(number, "trailing whitespace")

    # --- headings ----------------------------------------------------------
    heads = headings_of(text)
    slugs = {}
    for number, level, title, slug in heads:
        if slug in slugs:
            bad(number, f"duplicate heading {title!r} (also line {slugs[slug]}); the second anchor is unreachable")
        slugs.setdefault(slug, number)

    # --- links -------------------------------------------------------------
    known_cache = {}

    def anchors_of(target: pathlib.Path):
        if target not in known_cache:
            known_cache[target] = {slug for _, _, _, slug in headings_of(target.read_text(encoding="utf-8"))}
        return known_cache[target]

    for number, line in strip_fences(text):
        targets = [m.group(2) for m in LINK.finditer(line)]
        targets += HTML_SRC.findall(line)
        ref = REF_DEF.match(line)
        if ref:
            targets.append(ref.group(1))
        for target in targets:
            if SCHEME.match(target) or target.startswith("//"):
                continue  # external URL; not this script's job to fetch
            file_part, _, fragment = target.partition("#")
            if not file_part:
                # Same-document anchor.
                if fragment not in slugs:
                    bad(number, f"anchor {target!r} matches no heading in this file")
                continue
            resolved = (path.parent / file_part).resolve()
            if not resolved.exists():
                bad(number, f"link target {target!r} does not exist ({resolved.relative_to(root.resolve()) if resolved.is_relative_to(root.resolve()) else resolved})")
                continue
            if fragment and resolved.suffix == ".md" and fragment not in anchors_of(resolved):
                bad(number, f"anchor {target!r} matches no heading in {file_part}")

    # --- ASCII diagrams ----------------------------------------------------
    # A hand-drawn box only lines up if every line that closes on a right-hand
    # border ends in the same column, so measure those lines and nothing else.
    # (The old README had borders 216 and 252 characters wide around 93
    # characters of text, which is invisible in a diff and obvious on screen.)
    right_edge = "│┐┘┤├┌└"
    in_fence, lang, block_start, widths = False, None, 0, []
    for number, line in enumerate(text.split("\n"), start=1):
        match = FENCE.match(line)
        if match:
            if not in_fence:
                in_fence, lang, block_start, widths = True, match.group(2), number, []
            else:
                if widths:
                    distinct = sorted({w for _, w in widths})
                    if len(distinct) > 1:
                        bad(block_start, f"ASCII diagram is ragged: border lines end at columns {distinct}")
                in_fence = False
            continue
        if in_fence and lang in ("text", "console"):
            stripped = line.rstrip()
            if stripped and stripped[-1] in right_edge and any(c in stripped for c in "─│"):
                widths.append((number, display_width(stripped)))

    # --- no stray absolute paths in prose ----------------------------------
    for number, line in enumerate(text.split("\n"), start=1):
        if "/home/karutoil" in line:
            bad(number, "contains a machine-specific absolute path")


def check_toc(root: pathlib.Path, path: pathlib.Path, problems: list) -> None:
    """The Contents table must list exactly the level-2 headings."""
    text = path.read_text(encoding="utf-8")
    heads = [h for h in headings_of(text) if h[1] == 2 and h[2] != "Contents"]
    lines = text.split("\n")
    start = next((i for i, line in enumerate(lines) if line.strip() == "## Contents"), None)
    if start is None:
        return
    listed, order = [], []
    for line in lines[start + 1:]:
        if line.startswith("## "):
            break
        for m in re.finditer(r"\[([^\]]+)\]\(#([^)]+)\)", line):
            listed.append(m.group(2))
            order.append(m.group(1))
    # The table may be laid out row-major (one section per row, with a
    # description) or column-major (a compact two-column link grid), so accept
    # either reading of the cells — but the *set* and the *order* must be right.
    expected = [slug for _, _, _, slug in heads]
    rows = [line for line in lines[start + 1:] if line.startswith("|")]
    cells = [[m.group(2) for m in re.finditer(r"\[([^\]]+)\]\(#([^)]+)\)", line)] for line in rows[1:]]
    cells = [c for c in cells if c]
    row_major = [slug for row in cells for slug in row]
    column_major = [
        slug for column in range(max((len(r) for r in cells), default=0))
        for row in cells if len(row) > column for slug in [row[column]]
    ]
    rel = path.relative_to(root)
    missing = [s for s in expected if s not in listed]
    extra = [s for s in listed if s not in expected]
    if missing:
        problems.append(f"{rel}:{start + 1}: Contents is missing section(s): {', '.join(missing)}")
    if extra:
        problems.append(f"{rel}:{start + 1}: Contents lists section(s) that do not exist: {', '.join(extra)}")
    if not missing and not extra and row_major != expected and column_major != expected:
        problems.append(f"{rel}:{start + 1}: Contents order differs from the document order")


def fix_toc(path: pathlib.Path) -> bool:
    text = path.read_text(encoding="utf-8")
    lines = text.split("\n")
    start = next((i for i, line in enumerate(lines) if line.strip() == "## Contents"), None)
    if start is None:
        return False
    end = next(i for i in range(start + 1, len(lines)) if lines[i].startswith("## "))
    heads = [h for h in headings_of(text) if h[1] == 2 and h[2] != "Contents"]
    # Two-column table keeps a long list short and scannable.
    rows = []
    half = (len(heads) + 1) // 2
    for i in range(half):
        left = heads[i]
        right = heads[i + half] if i + half < len(heads) else None
        left_cell = f"[{left[2]}](#{left[3]})"
        right_cell = f"[{right[2]}](#{right[3]})" if right else ""
        rows.append(f"| {left_cell} | {right_cell} |")
    table = ["| | |", "| --- | --- |"] + rows
    lines[start + 1:end] = ["", *table, ""]
    path.write_text("\n".join(lines), encoding="utf-8")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--fix-toc", action="store_true", help="rewrite the Contents table")
    parser.add_argument("files", nargs="*", help="markdown files (default: README.md)")
    args = parser.parse_args()

    root = pathlib.Path(args.root).resolve()
    files = [root / f for f in args.files] or [root / "README.md"]
    problems: list = []
    for path in files:
        if not path.is_file():
            problems.append(f"{path}: not found")
            continue
        if args.fix_toc and path.name == "README.md":
            fix_toc(path)
        check(root, path, problems)
        if path.name == "README.md":
            check_toc(root, path, problems)

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s) in {', '.join(str(f.relative_to(root)) for f in files if f.is_file())}", file=sys.stderr)
        return 1
    print(f"ok: {', '.join(str(f.relative_to(root)) for f in files if f.is_file())} — links, anchors, fences and structure are consistent")
    return 0


if __name__ == "__main__":
    sys.exit(main())
