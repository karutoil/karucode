#!/usr/bin/env python3
"""Tests for scripts/check_readme.py.

The checker is the only thing standing between the README and a broken link, so
it is tested the same way as the code it guards: every rule it claims to enforce
is exercised against a document that violates exactly that rule.
"""

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts" / "check_readme.py"


def run(doc: str, files: dict | None = None, fix_toc: bool = False):
    """Check `doc` as README.md inside a temp root; return (code, output)."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "README.md").write_text(doc, encoding="utf-8")
        for name, content in (files or {}).items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        args = [sys.executable, str(CHECKER), "--root", str(root)]
        if fix_toc:
            args.append("--fix-toc")
        result = subprocess.run(args, capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr


CLEAN = """# demo

## Contents

| | |
| --- | --- |
| [Section](#section) | what it covers: keep this description long enough |

## Section

Some text with [a real link](other.md) and [an anchor](#section).

```text
┌────┐
│ ok │
└────┘
```
"""


class Clean(unittest.TestCase):
    def test_clean_document_passes(self):
        code, output = run(CLEAN, {"other.md": "# Other\n"})
        self.assertEqual(code, 0, output)
        self.assertIn("ok:", output)


class Links(unittest.TestCase):
    def test_missing_relative_file(self):
        code, output = run("# demo\n\n[t](nope.md)\n")
        self.assertEqual(code, 1)
        self.assertIn("does not exist", output)

    def test_missing_same_page_anchor(self):
        code, output = run("# demo\n\n[a](#nowhere)\n")
        self.assertEqual(code, 1)
        self.assertIn("matches no heading", output)

    def test_missing_anchor_in_other_file(self):
        code, output = run("# demo\n\n[a](other.md#nope)\n", {"other.md": "# Real\n"})
        self.assertEqual(code, 1)
        self.assertIn("matches no heading in other.md", output)

    def test_existing_anchor_in_other_file(self):
        code, output = run("# demo\n\n[a](other.md#real-heading)\n", {"other.md": "# Real heading\n"})
        self.assertEqual(code, 0, output)

    def test_external_urls_are_not_fetched(self):
        code, output = run("# demo\n\n[x](https://example.invalid/a/b/c)\n")
        self.assertEqual(code, 0, output)

    def test_html_img_src_is_checked(self):
        code, output = run('# demo\n<img src="branding/banner.png">\n')
        self.assertEqual(code, 1)
        self.assertIn("does not exist", output)


class Structure(unittest.TestCase):
    def test_duplicate_heading(self):
        code, output = run("# demo\n\n## Same\n\n## Same\n")
        self.assertEqual(code, 1)
        self.assertIn("duplicate heading", output)

    def test_unclosed_fence(self):
        code, output = run("# demo\n\n```bash\nls\n")
        self.assertEqual(code, 1)
        self.assertIn("unclosed code fence", output)

    def test_fence_language_required(self):
        code, output = run("# demo\n\n```\nls\n```\n")
        self.assertEqual(code, 1)
        self.assertIn("no language tag", output)

    def test_trailing_whitespace(self):
        code, output = run("# demo\n\nline   \n")
        self.assertEqual(code, 1)
        self.assertIn("trailing whitespace", output)

    def test_no_final_newline(self):
        code, output = run("# demo\n\nlast line")
        self.assertEqual(code, 1)
        self.assertIn("does not end with a newline", output)

    def test_ragged_ascii_diagram(self):
        doc = "# demo\n\n```text\n┌────┐\n│ ok │\n└────────┘\n```\n"
        code, output = run(doc)
        self.assertEqual(code, 1)
        self.assertIn("ragged", output)

    def test_aligned_ascii_diagram_passes(self):
        doc = "# demo\n\n```text\n┌────┐\n│ ok │\n└────┘\n```\n"
        code, output = run(doc)
        self.assertEqual(code, 0, output)

    def test_machine_specific_path(self):
        code, output = run("# demo\n\n/home/karutoil/karucode/x\n")
        self.assertEqual(code, 1)
        self.assertIn("absolute path", output)


class Contents(unittest.TestCase):
    DOC = """# demo

## Contents

| | |
| --- | --- |
| [One](#one) | first section, with a description long enough to count |
| [Two](#two) | second section, also with a description |

## One

## Two
"""

    def test_matching_contents_passes(self):
        code, output = run(self.DOC)
        self.assertEqual(code, 0, output)

    def test_missing_section(self):
        code, output = run(self.DOC.replace("| [Two](#two) | second section, also with a description |\n", ""))
        self.assertEqual(code, 1)
        self.assertIn("missing section", output)

    def test_invented_section(self):
        # The bogus row must be *inside* the table, otherwise the anchor check
        # fires first and this stops testing the Contents reconciliation.
        doc = self.DOC.replace(
            "| [Two](#two) | second section, also with a description |\n",
            "| [Two](#two) | second section, also with a description |\n"
            "| [Three](#three) | a section that does not exist |\n",
        )
        code, output = run(doc)
        self.assertEqual(code, 1)
        self.assertIn("do not exist", output)

    def test_wrong_order(self):
        swapped = self.DOC.replace(
            "| [One](#one) | first section, with a description long enough to count |\n"
            "| [Two](#two) | second section, also with a description |\n",
            "| [Two](#two) | second section, also with a description |\n"
            "| [One](#one) | first section, with a description long enough to count |\n",
        )
        code, output = run(swapped)
        self.assertEqual(code, 1)
        self.assertIn("order differs", output)

    def test_fix_toc_repairs_and_is_idempotent(self):
        broken = self.DOC.replace("| [Two](#two) | second section, also with a description |\n", "")
        code, _ = run(broken, fix_toc=True)
        self.assertEqual(code, 0)
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "README.md").write_text(broken, encoding="utf-8")
            for _ in range(2):
                subprocess.run(
                    [sys.executable, str(CHECKER), "--root", str(root), "--fix-toc"],
                    capture_output=True, text=True,
                )
                once = (root / "README.md").read_text(encoding="utf-8")
                subprocess.run(
                    [sys.executable, str(CHECKER), "--root", str(root), "--fix-toc"],
                    capture_output=True, text=True,
                )
                twice = (root / "README.md").read_text(encoding="utf-8")
                self.assertEqual(once, twice, "--fix-toc is not idempotent")
                break


class RealReadme(unittest.TestCase):
    def test_the_actual_readme_passes(self):
        result = subprocess.run(
            [sys.executable, str(CHECKER)], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
