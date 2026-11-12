#!/usr/bin/env python3
"""Generate the site navigation for the docs website from docs/internal/index.json.

`docs/internal/index.json` is written by `cargo run -p kc-docs -- --write` and is
the machine-readable index of every generated topic (slug, path, title, summary,
tags, identifiers). This script turns it into the `nav:` block mkdocs.yml
consumes, so the site navigation can never drift from the generated docs.

Usage:
    python3 scripts/gen_docs_nav.py               # print the nav YAML
    python3 scripts/gen_docs_nav.py --snippet     # only the Reference section,
                                                  # for pasting into mkdocs.yml
    python3 scripts/gen_docs_nav.py --check       # exit 1 if mkdocs.yml's
                                                  # Reference section is stale

Nothing is written unless you redirect the output; mkdocs.yml stays hand-owned.
Run this after `kc-docs -- --write` whenever a topic is added or removed.
"""

import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
INDEX = ROOT / "docs" / "internal" / "index.json"
MKDOCS = ROOT / "mkdocs.yml"

# Files that live in docs/internal/ but are not topics in index.json (design
# docs, audits). They are hand-added in mkdocs.yml, not generated here.
HAND_HELD = {
    "DESIGN-SYSTEM.md",
    "DESIGN-AUDIT.md",
    "README.md",
}


def reference_section() -> list[str]:
    """The `Reference` nav entries, one topic per line, in index order."""
    data = json.loads(INDEX.read_text(encoding="utf-8"))
    lines = []
    for topic in data["topics"]:
        title = topic["title"]
        path = f"internal/{topic['path']}"
        lines.append(f"      - {title}: {path}")
    return lines


def full_nav() -> str:
    lines = [
        "nav:",
        "  - Home: README.md",
        "  - Handbooks:",
        "      - HTTP API: API.md",
        "      - Architecture: ARCHITECTURE.md",
        "      - MCP client: MCP.md",
        "      - Plugins: PLUGINS.md",
        "      - Security model: SECURITY.md",
        "      - Internal docs system: INTERNAL-DOCS.md",
        "      - Contributing (agents): CONTRIBUTING-AGENTS.md",
        "  - Reference:",
    ]
    lines.extend(reference_section())
    return "\n".join(lines)


def check() -> int:
    """Compare the generated Reference section against mkdocs.yml's."""
    if not MKDOCS.exists():
        print(f"{MKDOCS.name} not found; nothing to check", file=sys.stderr)
        return 1
    generated = reference_section()
    text = MKDOCS.read_text(encoding="utf-8")
    stale = [line for line in generated if line not in text]
    if stale:
        for line in stale:
            print(f"mkdocs.yml is missing nav entry: {line}", file=sys.stderr)
        print(
            "run: python3 scripts/gen_docs_nav.py and update mkdocs.yml",
            file=sys.stderr,
        )
        return 1
    print(f"ok: mkdocs.yml Reference section matches {INDEX.name}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--snippet",
        action="store_true",
        help="print only the Reference section",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 when mkdocs.yml's Reference section is stale",
    )
    args = parser.parse_args()
    if args.check:
        return check()
    if args.snippet:
        print("\n".join(reference_section()))
    else:
        print(full_nav())
    return 0


if __name__ == "__main__":
    sys.exit(main())
