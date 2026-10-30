#!/usr/bin/env python3
"""Render a markdown file to HTML the way GitHub does, for visual checking.

GitHub renders README.md with GFM: tables, task lists, autolinks, strikethrough
and HTML blocks. This is not a pixel-perfect clone, but it gets the things a
human actually notices in a diff-and-browse review: table alignment, code block
wrapping, heading rhythm, and whether the hand-drawn diagram survives.

Usage:
    python3 scripts/render_md.py README.md --out /tmp/readme.html \
        [--css /path/to/github-markdown.css]

Needs `markdown-it-py` (pip) or falls back to `python3 -m markdown` / `cmark-gfm`
if either is installed. Deliberately not a build dependency of the project.
"""

import argparse
import pathlib
import shutil
import subprocess
import sys
import tempfile

TEMPLATE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>{title}</title>
<style>{css}</style>
<style>
  body {{ margin: 0; background: #fff; }}
  .markdown-body {{ box-sizing: border-box; min-width: 200px;
    max-width: 980px; margin: 0 auto; padding: 45px; }}
  .markdown-body pre {{ overflow: auto; }}
  .markdown-body pre code {{ font-size: 85%; line-height: 1.45; }}
  .markdown-body img {{ max-width: 100%; }}
</style></head>
<body><article class="markdown-body">
{body}
</article></body></html>
"""


def render(markdown: str) -> str:
    """GFM-ish HTML for `markdown`, using whatever renderer is installed."""
    try:
        from markdown_it import MarkdownIt
    except ImportError:
        MarkdownIt = None
    if MarkdownIt is not None:
        try:
            # `gfm-like` needs linkify-it-py; table and strikethrough do not.
            return MarkdownIt("gfm-like", {"html": True, "linkify": True}).render(markdown)
        except ImportError:
            md = MarkdownIt("commonmark", {"html": True})
            md.enable(["table", "strikethrough"])
            return md.render(markdown)
    if shutil.which("cmark-gfm"):
        return subprocess.run(
            ["cmark-gfm", "--unsafe", "-e", "table", "-e", "strikethrough", "-e", "autolink"],
            input=markdown, capture_output=True, text=True, check=True,
        ).stdout
    raise SystemExit(
        "no markdown renderer found; install one:\n"
        "    python3 -m pip install --user markdown-it-py linkify-it-py"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("source")
    parser.add_argument("--out", required=True)
    parser.add_argument("--css", default=None, help="GitHub markdown CSS file")
    parser.add_argument("--dark", action="store_true")
    args = parser.parse_args()

    source = pathlib.Path(args.source)
    markdown = source.read_text(encoding="utf-8")
    body = render(markdown)

    css = ""
    if args.css:
        css = pathlib.Path(args.css).read_text(encoding="utf-8")
    elif args.dark:
        css = ".markdown-body{background:#0d1117;color:#e6edf3}"

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(
        TEMPLATE.format(title=source.name, css=css, body=body), encoding="utf-8"
    )
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
