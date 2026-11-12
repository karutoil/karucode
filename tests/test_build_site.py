#!/usr/bin/env python3
"""Tests for scripts/build_site.py.

The stager is what makes "the docs import into a website as-is" true, and every
rewrite it performs is a claim about where a reader will land. Each test pins one
of those claims against a synthetic repository, including the two things that
must not be touched: fenced code and inline code.
"""

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import build_site  # noqa: E402


CONFIG = """site_name: demo
repo_url: https://github.com/acme/demo
"""

README = """# Demo

[![build](https://example.invalid/badge.svg)](docs/API.md)

<img src="branding/banner.svg" alt="banner">
<a href="LICENSE">license</a>

* [API](docs/API.md)
* [`API.md`](docs/API.md)
* [internal](docs/internal/)
* [docs](docs/)
* [license](LICENSE)
* [contributing](CONTRIBUTING.md)
* [security](SECURITY.md)
* [plugins](plugins/demos/README.md)
![banner](branding/banner.svg)
"""

HANDBOOK = """# Handbook

[crate](../crates/kc-core)
[agents](../AGENTS.md)
[security](../SECURITY.md)
[api](API.md)
[internal](internal/)

```text
](../AGENTS.md)
```

Inline `../AGENTS.md` stays as written.
"""


def make_repo(root: pathlib.Path) -> None:
    files = {
        "mkdocs.yml": CONFIG,
        "README.md": README,
        "CONTRIBUTING.md": "# Contributing\n",
        "SECURITY.md": "# Real security model\n",
        "CHANGELOG.md": "# Changelog\n",
        "AGENTS.md": "# Agent contract\n",
        "LICENSE": "MIT\n",
        "crates/kc-core/config.rs": "// config\n",
        "branding/banner.svg": "<svg/>\n",
        "plugins/demos/README.md": "# Demos\n",
        "docs/API.md": "# API\n",
        "docs/SECURITY.md": "# Security (moved)\n[SECURITY.md](../SECURITY.md)\n",
        "docs/HANDBOOK.md": HANDBOOK,
        "docs/INTERNAL-DOCS.md": "# Internal\n[internal](internal/)\n",
        "docs/internal/README.md": "# Internal index\n[api](../API.md)\n[top](../../README.md)\n",
    }
    for name, content in files.items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")


class Staging(unittest.TestCase):
    def stage(self) -> tuple[pathlib.Path, pathlib.Path]:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = pathlib.Path(directory.name) / "repo"
        root.mkdir()
        make_repo(root)
        out = build_site.build(root, root / ".site")
        return root, out

    def test_root_readme_loses_the_docs_prefix_and_links_elsewhere_to_github(self):
        _, out = self.stage()
        page = (out / "README.md").read_text(encoding="utf-8")
        self.assertIn("[API](API.md)", page)
        self.assertIn("[`API.md`](API.md)", page)
        self.assertIn(
            "[license](https://github.com/acme/demo/blob/main/LICENSE)", page
        )
        self.assertIn("[contributing](CONTRIBUTING.md)", page)
        self.assertIn(
            "[plugins](https://github.com/acme/demo/blob/main/plugins/demos/README.md)",
            page,
        )
        self.assertIn("[docs](https://github.com/acme/demo/tree/main/docs)", page)
        self.assertIn("[internal](internal/README.md)", page)

    def test_images_become_raw_urls_so_the_page_shows_the_picture(self):
        _, out = self.stage()
        page = (out / "README.md").read_text(encoding="utf-8")
        raw = "https://raw.githubusercontent.com/acme/demo/main/branding/banner.svg"
        self.assertIn(f'<img src="{raw}" alt="banner">', page)
        self.assertIn(f"![banner]({raw})", page)
        # The badge link wrapped around the image is a page, not an image.
        self.assertIn("](API.md)", page)

    def test_handbook_links_that_escape_the_tree_point_at_the_repository(self):
        _, out = self.stage()
        page = (out / "HANDBOOK.md").read_text(encoding="utf-8")
        self.assertIn(
            "[crate](https://github.com/acme/demo/tree/main/crates/kc-core)", page
        )
        self.assertIn(
            "[agents](https://github.com/acme/demo/blob/main/AGENTS.md)", page
        )
        # A root page the site also stages is a page here, not a GitHub link.
        self.assertIn("[security](SECURITY.md)", page)
        # A link that already resolves inside the tree is left alone.
        self.assertIn("[api](API.md)", page)
        self.assertIn("[internal](internal/README.md)", page)

    def test_fenced_and_inline_code_are_never_rewritten(self):
        _, out = self.stage()
        page = (out / "HANDBOOK.md").read_text(encoding="utf-8")
        self.assertIn("](../AGENTS.md)", page)
        self.assertIn("Inline `../AGENTS.md` stays as written.", page)

    def test_internal_pages_keep_relative_links_and_reach_the_root(self):
        _, out = self.stage()
        page = (out / "internal" / "README.md").read_text(encoding="utf-8")
        self.assertIn("[api](../API.md)", page)
        self.assertIn("[top](../README.md)", page)

    def test_the_security_stub_is_replaced_by_the_root_page(self):
        root, out = self.stage()
        self.assertEqual(
            (out / "SECURITY.md").read_text(encoding="utf-8"),
            (root / "SECURITY.md").read_text(encoding="utf-8"),
        )
        self.assertIn("[security](SECURITY.md)", (out / "README.md").read_text(encoding="utf-8"))

    def test_a_target_that_does_not_exist_is_left_alone(self):
        root, out = self.stage()
        (root / "docs" / "ghost.md").write_text(
            "# Ghost\n[gone](../missing.md)\n", encoding="utf-8"
        )
        out = build_site.build(root, root / ".site")
        page = (out / "ghost.md").read_text(encoding="utf-8")
        self.assertIn("[gone](../missing.md)", page)

    def test_only_git_tracked_docs_are_staged(self):
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(root)], check=False))
        make_repo(root)
        (root / ".gitignore").write_text("docs/local.md\n", encoding="utf-8")
        (root / "docs" / "local.md").write_text("# Local\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", "-A"], check=True)
        out = build_site.build(root, root / ".site")
        self.assertTrue((out / "API.md").is_file())
        self.assertFalse((out / "local.md").exists())

    def test_refuses_to_write_outside_the_repository(self):
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(lambda: subprocess.run(["rm", "-rf", str(root)], check=False))
        make_repo(root)
        with self.assertRaises(SystemExit):
            build_site.build(root, root.parent / "elsewhere")


if __name__ == "__main__":
    unittest.main()
