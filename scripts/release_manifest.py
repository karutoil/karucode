#!/usr/bin/env python3
"""Write the release manifest (`updates.json`) and `SHA256SUMS` for one build.

karucode versions itself by commit count, so a release is "build N" and the
manifest is one small document per release:

    {
      "schema": 1,
      "build": 108,
      "commit": "9c1de77a0b3c",
      "commit_date": "2026-05-31T09:12:40+00:00",
      "released_at": "2026-05-31T09:20:03Z",
      "url": "https://github.com/karutoil/karucode/releases/tag/v108",
      "notes": "...",
      "targets": {
        "x86_64-unknown-linux-gnu": {
          "url": "https://.../karucode-108-x86_64-unknown-linux-gnu",
          "sha256": "...",
          "size": 18432120,
          "tarball": "https://.../karucode-108-x86_64-unknown-linux-gnu.tar.gz"
        }
      }
    }

The updater in the binary reads that document (see the `updates` topic), so the
producer and the consumer have to agree: `crates/kc-server/src/update.rs` refuses
an unknown `schema`, a build of 0, a `sha256` that is not 64 hex characters and a
URL that is not http(s). This script writes exactly that shape, and its test
(`tests/test_release_manifest.py`) plus the Rust parser test pin both ends.

Artifacts are discovered by name in `--dist`:

    karucode-<build>-<target>            the raw binary, for the self-updater
    karucode-<build>-<target>.tar.gz     the archive, for packaging/install.sh
    example-hello-<build>-<target>       optional, ignored by the manifest

Usage:

    python3 scripts/release_manifest.py --dist dist --build 108 \\
        --commit 9c1de77a0b3c --commit-date 2026-05-31T09:12:40+00:00 \\
        --base-url https://github.com/karutoil/karucode/releases/download/v108 \\
        --release-url https://github.com/karutoil/karucode/releases/tag/v108 \\
        --notes-file dist/notes.md

Exit codes: 0 wrote the files, 2 no artifact was found (a release with no
downloadable binary is a mistake, not an empty manifest).
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import re
import sys

SCHEMA = 1
# Mirrors MAX_NOTES_CHARS in crates/kc-server/src/update.rs: the manifest is a
# release note, not a changelog dump.
MAX_NOTES_CHARS = 4000
TARGET_RE = re.compile(r"^[a-z0-9_]+(-[a-z0-9_]+){2,}$")


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def now_rfc3339() -> str:
    return _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def find_artifacts(dist: str, build: int) -> dict[str, dict[str, str]]:
    """Map target triple -> {binary path, tarball path?} for one build."""
    prefix = f"karucode-{build}-"
    found: dict[str, dict[str, str]] = {}
    for name in sorted(os.listdir(dist)):
        path = os.path.join(dist, name)
        if not os.path.isfile(path) or not name.startswith(prefix):
            continue
        rest = name[len(prefix):]
        if rest.endswith(".tar.gz"):
            target = rest[: -len(".tar.gz")]
            kind = "tarball"
        else:
            target = rest
            kind = "binary"
        if not TARGET_RE.match(target):
            # `karucode-108-SHA256SUMS` and friends are not artifacts.
            continue
        found.setdefault(target, {})[kind] = path
    # A target with only a tarball cannot be self-updated (the updater downloads
    # the raw binary), so it is not a target for this manifest.
    return {target: files for target, files in found.items() if "binary" in files}


def build_manifest(
    *,
    dist: str,
    build: int,
    commit: str,
    commit_date: str,
    base_url: str,
    release_url: str,
    notes: str,
    released_at: str | None = None,
) -> dict:
    if build <= 0:
        raise SystemExit("--build must be the commit count, a positive integer")
    artifacts = find_artifacts(dist, build)
    if not artifacts:
        raise SystemExit(
            f"no karucode-{build}-<target> binary in {dist}; nothing to publish"
        )
    base = base_url.rstrip("/")
    targets: dict[str, dict] = {}
    for target, files in sorted(artifacts.items()):
        binary = files["binary"]
        name = os.path.basename(binary)
        entry = {
            "url": f"{base}/{name}",
            "sha256": sha256_of(binary),
            "size": os.path.getsize(binary),
        }
        if "tarball" in files:
            entry["tarball"] = f"{base}/{os.path.basename(files['tarball'])}"
        targets[target] = entry
    return {
        "schema": SCHEMA,
        "build": build,
        "commit": commit,
        "commit_date": commit_date,
        "released_at": released_at or now_rfc3339(),
        "url": release_url,
        "notes": notes.strip()[:MAX_NOTES_CHARS],
        "targets": targets,
    }


def write_sha256sums(dist: str, build: int, path: str) -> list[str]:
    """`sha256sum`-compatible lines for every artifact of this build.

    `packaging/install.sh` verifies the tarball against a file of this shape, so it
    is written here rather than by a second shell incantation in CI: one place
    decides what a release contains.
    """
    lines = []
    prefix = f"karucode-{build}-"
    for name in sorted(os.listdir(dist)):
        full = os.path.join(dist, name)
        if not os.path.isfile(full) or not name.startswith(prefix):
            continue
        if name.endswith(".sha256") or name == "updates.json":
            continue
        lines.append(f"{sha256_of(full)}  {name}")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + ("\n" if lines else ""))
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dist", required=True, help="directory holding this build's artifacts")
    parser.add_argument("--build", required=True, type=int, help="the commit count (git rev-list --count HEAD)")
    parser.add_argument("--commit", required=True, help="short commit sha")
    parser.add_argument("--commit-date", required=True, help="commit date, RFC 3339")
    parser.add_argument("--base-url", required=True, help="download URL prefix for the artifacts")
    parser.add_argument("--release-url", default="", help="the human-readable release page")
    parser.add_argument("--notes-file", help="file whose text becomes the manifest's notes")
    parser.add_argument("--notes", default="", help="notes inline, when there is no file")
    parser.add_argument("--released-at", help="override the release timestamp (tests)")
    parser.add_argument("--out", default="updates.json", help="where to write the manifest")
    parser.add_argument("--sha256sums", help="also write a sha256sum-compatible file here")
    args = parser.parse_args(argv)

    notes = args.notes
    if args.notes_file:
        try:
            with open(args.notes_file, encoding="utf-8") as handle:
                notes = handle.read()
        except OSError as error:
            print(f"release_manifest: cannot read --notes-file: {error}", file=sys.stderr)
            return 2

    try:
        manifest = build_manifest(
            dist=args.dist,
            build=args.build,
            commit=args.commit,
            commit_date=args.commit_date,
            base_url=args.base_url,
            release_url=args.release_url,
            notes=notes,
            released_at=args.released_at,
        )
    except SystemExit as error:
        # A release with no artifact is a mistake; exit 2 and say so rather than
        # writing an empty manifest that every installed server would then read.
        print(f"release_manifest: {error}", file=sys.stderr)
        return 2
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, sort_keys=False)
        handle.write("\n")
    print(f"wrote {args.out}: build {manifest['build']} for {', '.join(manifest['targets'])}")
    for target, entry in manifest["targets"].items():
        print(f"  {target}: {entry['size']} bytes, sha256 {entry['sha256']}")
    if args.sha256sums:
        lines = write_sha256sums(args.dist, args.build, args.sha256sums)
        print(f"wrote {args.sha256sums}: {len(lines)} file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
