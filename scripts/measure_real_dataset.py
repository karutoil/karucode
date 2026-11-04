#!/usr/bin/env python3
"""Measure a build against a *copy* of a real deployment's database.

The synthetic soak proves a change works on a fresh instance; this proves it on
the shape a real instance has: a 350 MB database, a hundred-odd sessions, one of
them thousands of messages long, and the operator's real plugin set. The
original data directory is never touched - SQLite is copied in WAL-safe order
into a temp dir and the server is pointed at the copy.

Usage:
    python3 scripts/measure_real_dataset.py \
        --data-dir ~/.local/share/karucode \
        --binary target/release/karucode [--plugin NAME=/abs/binary ...]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

from measure_steady_memory import Provider, free_port, http_json, snapshot, wait_for


def copy_database(source: Path, target: Path) -> None:
    """Copy with SQLite's own backup API: WAL contents included, no torn page."""
    with sqlite3.connect(f"file:{source}?mode=ro", uri=True, timeout=30) as src:
        with sqlite3.connect(target) as dst:
            src.backup(dst)


def largest_session(db: Path) -> tuple[str, int]:
    with sqlite3.connect(db) as conn:
        row = conn.execute(
            "SELECT session_id, COUNT(*) AS n FROM messages GROUP BY session_id"
            " ORDER BY n DESC LIMIT 1"
        ).fetchone()
        return (row[0], row[1]) if row else ("", 0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path("target/release/karucode"))
    parser.add_argument("--web-dir", type=Path, default=Path("web/dist"))
    parser.add_argument("--plugin", action="append", default=[], metavar="NAME=/abs/binary")
    parser.add_argument("--settle", type=float, default=60.0)
    parser.add_argument("--label", default="")
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="kc-real-"))
    data = tmp / "data"
    data.mkdir()
    workspace = tmp / "workspace"
    workspace.mkdir()
    db = data / "karucode.db"
    started = time.time()
    copy_database(args.data_dir / "karucode.db", db)
    print(f"copied {db.stat().st_size / 1e6:.0f} MB in {time.time() - started:.1f}s")

    # Take the largest session only *after* copying, so a busy source cannot
    # change the answer halfway through.
    session_id, message_count = largest_session(db)

    if args.plugin:
        for item in args.plugin:
            name, _, binary = item.partition("=")
            folder = data / "plugins" / name
            folder.mkdir(parents=True)
            (folder / "plugin.toml").write_text(
                f'name = "{name}"\ncommand = ["{binary}"]\nenabled = true\n'
                'permissions = ["shell", "files.read", "files.write", "network"]\n'
            )
    else:
        shutil.copytree(args.data_dir / "plugins", data / "plugins", dirs_exist_ok=True)

    port = free_port()
    log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [str(args.binary), "--port", str(port), "--workspace", str(workspace),
         "--data-dir", str(data)],
        env={**os.environ, "KARUCODE_WEB_DIR": str(args.web_dir), "KARUCODE_LOG": "warn"},
        stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
    )
    api = f"http://127.0.0.1:{port}"
    try:
        wait_for(f"{api}/api/health", timeout=180)
        time.sleep(2)
        stages = {"startup": snapshot(server.pid)}
        stages["sessions_list"] = None
        http_json(f"{api}/api/sessions")
        time.sleep(1)
        stages["sessions_list"] = snapshot(server.pid)
        if session_id:
            detail = http_json(f"{api}/api/sessions/{session_id}")
            print(f"opened {session_id}: {len(detail.get('messages') or [])} of {message_count} messages")
            time.sleep(2)
            stages["biggest_session_open"] = snapshot(server.pid)
            time.sleep(args.settle)
            stages["idle"] = snapshot(server.pid)
        print(json.dumps({
            "label": args.label or str(args.binary),
            "database_mb": round(db.stat().st_size / 1e6, 1),
            "biggest_session_messages": message_count,
            "stages": stages,
        }, indent=2))
        return 0
    finally:
        try:
            os.killpg(os.getpgid(server.pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
        log.close()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
