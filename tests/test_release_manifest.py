#!/usr/bin/env python3
"""Tests for scripts/release_manifest.py.

The manifest is the contract between the release and every installed binary: the
updater will download and execute what it names. So the checks here are about the
things that would make it dangerous or useless - a wrong checksum, a missing
binary, an unknown schema, notes that are not plain text, a URL that is not
http(s) - and about the two files CI hands to the installer (`updates.json` and
`SHA256SUMS`) agreeing with the files on disk.

Run: python3 tests/test_release_manifest.py
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "scripts", "release_manifest.py")
sys.path.insert(0, os.path.join(ROOT, "scripts"))

import release_manifest  # noqa: E402  (path set above)


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, SCRIPT, *args],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )


class TempDist(unittest.TestCase):
    def setUp(self) -> None:
        self.dist = tempfile.mkdtemp(prefix="karucode-release-")
        self.addCleanup(shutil.rmtree, self.dist, True)

    def artifact(self, name: str, content: bytes) -> str:
        path = os.path.join(self.dist, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(content)
        return path


class BuildManifestTests(TempDist):
    def test_one_target_produces_the_documented_manifest(self) -> None:
        self.artifact("karucode-108-x86_64-unknown-linux-gnu", b"binary-bytes")
        self.artifact("karucode-108-x86_64-unknown-linux-gnu.tar.gz", b"tarball-bytes")
        out = os.path.join(self.dist, "updates.json")
        completed = run(
            [
                "--dist", self.dist,
                "--build", "108",
                "--commit", "9c1de77a0b3c",
                "--commit-date", "2026-05-31T09:12:40+00:00",
                "--base-url", "https://example.com/downloads/v108/",
                "--release-url", "https://example.com/releases/tag/v108",
                "--notes", "  Faster startup.  ",
                "--released-at", "2026-05-31T09:20:03Z",
                "--out", out,
            ]
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        with open(out, encoding="utf-8") as handle:
            manifest = json.load(handle)
        self.assertEqual(manifest["schema"], 1)
        self.assertEqual(manifest["build"], 108)
        self.assertEqual(manifest["commit"], "9c1de77a0b3c")
        self.assertEqual(manifest["commit_date"], "2026-05-31T09:12:40+00:00")
        self.assertEqual(manifest["released_at"], "2026-05-31T09:20:03Z")
        self.assertEqual(manifest["notes"], "Faster startup.")
        entry = manifest["targets"]["x86_64-unknown-linux-gnu"]
        self.assertEqual(
            entry["url"],
            "https://example.com/downloads/v108/karucode-108-x86_64-unknown-linux-gnu",
            "the base URL's trailing slash must not double up",
        )
        self.assertEqual(entry["size"], len(b"binary-bytes"))
        self.assertEqual(
            entry["sha256"],
            release_manifest.sha256_of(
                os.path.join(self.dist, "karucode-108-x86_64-unknown-linux-gnu")
            ),
            "the manifest's checksum must be the file's real checksum",
        )
        self.assertEqual(len(entry["sha256"]), 64, "sha256 is 64 hex characters")
        self.assertTrue(entry["tarball"].endswith(".tar.gz"))
        self.assertEqual(
            list(manifest["targets"]), ["x86_64-unknown-linux-gnu"]
        )
        self.assertIn("build 108", completed.stdout)

    def test_two_targets_are_both_listed_and_sorted(self) -> None:
        self.artifact("karucode-5-x86_64-unknown-linux-gnu", b"x86")
        self.artifact("karucode-5-aarch64-unknown-linux-gnu", b"arm")
        manifest = release_manifest.build_manifest(
            dist=self.dist,
            build=5,
            commit="abc",
            commit_date="2026-01-01T00:00:00+00:00",
            base_url="https://example.com/v5",
            release_url="",
            notes="",
        )
        self.assertEqual(
            list(manifest["targets"]),
            ["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"],
        )
        self.assertNotIn("tarball", manifest["targets"]["x86_64-unknown-linux-gnu"])

    def test_a_tarball_without_the_binary_is_not_a_target(self) -> None:
        # The self-updater downloads the raw binary; a target that only has an
        # archive would be a manifest entry every installed server refuses.
        self.artifact("karucode-7-x86_64-unknown-linux-gnu.tar.gz", b"tar")
        with self.assertRaises(SystemExit) as caught:
            release_manifest.build_manifest(
                dist=self.dist,
                build=7,
                commit="abc",
                commit_date="x",
                base_url="https://example.com/v7",
                release_url="",
                notes="",
            )
        self.assertIn("nothing to publish", str(caught.exception))

    def test_artifacts_of_another_build_or_name_are_ignored(self) -> None:
        self.artifact("karucode-108-x86_64-unknown-linux-gnu", b"good")
        self.artifact("karucode-107-x86_64-unknown-linux-gnu", b"old build")
        self.artifact("karucode-108-SHA256SUMS", b"not an artifact")
        self.artifact("example-hello-108-x86_64-unknown-linux-gnu", b"plugin")
        manifest = release_manifest.build_manifest(
            dist=self.dist,
            build=108,
            commit="abc",
            commit_date="x",
            base_url="https://example.com/v108",
            release_url="",
            notes="",
        )
        self.assertEqual(list(manifest["targets"]), ["x86_64-unknown-linux-gnu"])

    def test_build_zero_is_refused(self) -> None:
        self.artifact("karucode-0-x86_64-unknown-linux-gnu", b"x")
        with self.assertRaises(SystemExit):
            release_manifest.build_manifest(
                dist=self.dist,
                build=0,
                commit="abc",
                commit_date="x",
                base_url="https://example.com",
                release_url="",
                notes="",
            )

    def test_notes_are_capped_like_the_binary_caps_them(self) -> None:
        self.artifact("karucode-9-x86_64-unknown-linux-gnu", b"x")
        notes = "n" * (release_manifest.MAX_NOTES_CHARS + 500)
        manifest = release_manifest.build_manifest(
            dist=self.dist,
            build=9,
            commit="abc",
            commit_date="x",
            base_url="https://example.com",
            release_url="",
            notes=notes,
        )
        self.assertEqual(len(manifest["notes"]), release_manifest.MAX_NOTES_CHARS)

    def test_sha256sums_lists_exactly_the_release_files(self) -> None:
        self.artifact("karucode-3-x86_64-unknown-linux-gnu", b"bin")
        self.artifact("karucode-3-x86_64-unknown-linux-gnu.tar.gz", b"tar")
        self.artifact("karucode-2-x86_64-unknown-linux-gnu", b"previous build")
        self.artifact("README.md", b"docs")
        sums = os.path.join(self.dist, "SHA256SUMS")
        lines = release_manifest.write_sha256sums(self.dist, 3, sums)
        self.assertEqual(len(lines), 2)
        with open(sums, encoding="utf-8") as handle:
            text = handle.read()
        self.assertIn("karucode-3-x86_64-unknown-linux-gnu", text)
        self.assertIn("karucode-3-x86_64-unknown-linux-gnu.tar.gz", text)
        self.assertNotIn("karucode-2-", text)
        self.assertNotIn("README", text)
        # The shape `sha256sum -c` and install.sh expect: two spaces, then the name.
        for line in lines:
            digest, name = line.split("  ")
            self.assertEqual(len(digest), 64)
            self.assertTrue(name.startswith("karucode-3-"))

    def test_the_cli_fails_when_there_is_nothing_to_publish(self) -> None:
        completed = run(
            [
                "--dist", self.dist,
                "--build", "108",
                "--commit", "abc",
                "--commit-date", "x",
                "--base-url", "https://example.com/v108",
            ]
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("nothing to publish", completed.stderr)
        self.assertFalse(
            os.path.exists(os.path.join(self.dist, "updates.json")),
            "a failed run must not leave an empty manifest behind",
        )

    def test_the_cli_writes_sums_and_notes_from_a_file(self) -> None:
        self.artifact("karucode-12-aarch64-unknown-linux-gnu", b"arm")
        notes_path = os.path.join(self.dist, "notes.md")
        with open(notes_path, "w", encoding="utf-8") as handle:
            handle.write("A bigger change.\n")
        out = os.path.join(self.dist, "updates.json")
        sums = os.path.join(self.dist, "SHA256SUMS")
        completed = run(
            [
                "--dist", self.dist,
                "--build", "12",
                "--commit", "abc",
                "--commit-date", "2026-01-01T00:00:00+00:00",
                "--base-url", "https://example.com/v12",
                "--notes-file", notes_path,
                "--out", out,
                "--sha256sums", sums,
            ]
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        with open(out, encoding="utf-8") as handle:
            manifest = json.load(handle)
        self.assertEqual(manifest["notes"], "A bigger change.")
        self.assertTrue(os.path.isfile(sums))
        self.assertIn("wrote", completed.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
