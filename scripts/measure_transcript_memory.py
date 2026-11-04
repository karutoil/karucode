#!/usr/bin/env python3
"""Measure what a very long transcript costs the server, before and after.

A session's detail request used to load and return every stored message, so a
120 MB transcript meant a 120 MB response and a much larger transient peak. The
page is now bounded by stored payload bytes (1 MiB), which this measures the
only way that counts: bytes on the wire, and the server's own resident memory
while it answers.

Build first: `cargo build -p kc-server --bin karucode`.
Run: python3 scripts/measure_transcript_memory.py [--messages N] [--size BYTES] [--binary PATH]

It prints a JSON object so two builds can be compared mechanically:

    python3 scripts/measure_transcript_memory.py --binary target/debug/karucode
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import ROOT, free_port, http_json, wait_for


class StubProvider(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        self.send_response(500)
        self.send_header("Content-Length", "0")
        self.end_headers()


def rss_kib(pid: int) -> int:
    with open(f"/proc/{pid}/smaps_rollup") as handle:
        for line in handle:
            if line.startswith("Rss:"):
                return int(line.split()[1])
    return 0


def seed(db: Path, session_id: str, count: int, size: int) -> None:
    now = int(time.time() * 1000)
    body = "x" * size
    with sqlite3.connect(db, timeout=30) as conn:
        conn.executemany(
            "INSERT INTO messages (session_id, run_id, agent_id, role, content, created_at)"
            " VALUES (?, NULL, NULL, ?, ?, ?)",
            [(session_id, "user" if index % 2 == 0 else "assistant", body, now + index) for index in range(count)],
        )
        conn.commit()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--messages", type=int, default=3000)
    parser.add_argument("--size", type=int, default=40_000)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="kc-transcript-memory-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    data_dir = tmp / "data"
    data_dir.mkdir()

    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), StubProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                "[model]",
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'provider = "openai_compatible"',
                'model = "stub-model"',
                'api_key = "sk-test"',
                "",
                "[[providers]]",
                'id = "default"',
                'name = "Default"',
                'provider = "openai_compatible"',
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'api_key = "sk-test"',
                "",
                "[[providers.models]]",
                'id = "stub-model"',
                'name = "Stub"',
                "",
                "[active]",
                'provider = "default"',
                'model = "stub-model"',
                "",
            ]
        )
    )

    port = free_port()
    log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(args.binary),
            "--port",
            str(port),
            "--workspace",
            str(workspace),
            "--data-dir",
            str(data_dir),
            "--config",
            str(config),
        ],
        env={**os.environ, "KARUCODE_WEB_DIR": str(ROOT / "web/dist"), "KARUCODE_LOG": "warn"},
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    api = f"http://127.0.0.1:{port}"
    try:
        wait_for(f"{api}/api/health")
        project_id = http_json(f"{api}/api/projects")["projects"][0]["id"]
        created = http_json(
            f"{api}/api/sessions",
            "POST",
            {"title": "Memory target", "project": project_id},
        )
        session_id = created.get("id") or (created.get("session") or {}).get("id")
        seed(data_dir / "karucode.db", session_id, args.messages, args.size)
        stored_mb = args.messages * args.size / 1_000_000

        # Warm the connection pools so the delta is the request, not startup.
        http_json(f"{api}/api/sessions")

        before = rss_kib(server.pid)
        peak = {"rss": before}
        stop = threading.Event()

        def sample() -> None:
            while not stop.is_set():
                peak["rss"] = max(peak["rss"], rss_kib(server.pid))
                time.sleep(0.01)

        sampler = threading.Thread(target=sample, daemon=True)
        sampler.start()
        started = time.time()
        with urllib.request.urlopen(f"{api}/api/sessions/{session_id}", timeout=120) as response:
            payload = response.read()
        elapsed = time.time() - started
        stop.set()
        sampler.join()
        after = rss_kib(server.pid)
        detail = json.loads(payload)
        print(
            json.dumps(
                {
                    "binary": str(args.binary),
                    "stored_transcript_mb": round(stored_mb, 1),
                    "response_bytes": len(payload),
                    "messages_returned": len(detail.get("messages") or []),
                    "messages_truncated": detail.get("messages_truncated"),
                    "rss_before_mib": round(before / 1024, 1),
                    "rss_after_mib": round(after / 1024, 1),
                    "rss_peak_mib": round(peak["rss"] / 1024, 1),
                    "rss_peak_delta_mib": round((peak["rss"] - before) / 1024, 1),
                    "seconds": round(elapsed, 2),
                },
                indent=2,
            )
        )
        return 0
    finally:
        for process in (server,):
            try:
                os.killpg(os.getpgid(process.pid), signal.SIGTERM)
            except ProcessLookupError:
                pass
        provider.shutdown()
        log.close()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
