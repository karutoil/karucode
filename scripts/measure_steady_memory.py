#!/usr/bin/env python3
"""Measure a karucode server's steady-state memory, the way a deployment sees it.

The point is a number you can compare across builds and allocator settings, not a
number that flatters one change. It starts the real binary against a throwaway
workspace, data dir and stub provider, runs one scripted turn (a tool call plus an
answer) and samples:

  * RSS and PSS from /proc/<pid>/smaps_rollup;
  * how much of it is private anonymous memory (the heap we can influence) versus
    file-backed code pages (the binary itself);
  * the number and size of large anonymous mappings - glibc's per-thread malloc
    arenas show up here as ~7 MiB islands, and their count is the whole story for
    "why is an idle Rust server 100 MiB?";
  * thread count, which is what creates those arenas in the first place.

Usage:
    cargo build --release -p kc-server --bin karucode
    python3 scripts/measure_steady_memory.py --binary target/release/karucode
    python3 scripts/measure_steady_memory.py --env MALLOC_ARENA_MAX=2

Everything is printed as one JSON object so two runs can be diffed mechanically.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def sse(chunks) -> bytes:
    out = [f"data: {json.dumps(chunk)}\n\n" for chunk in chunks]
    out.append("data: [DONE]\n\n")
    return "".join(out).encode()


def text_chunks(text: str):
    return [
        {"id": "c", "object": "chat.completion.chunk",
         "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}]},
        {"id": "c", "object": "chat.completion.chunk",
         "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
    ]


def tool_chunks(call_id: str, name: str, arguments: dict):
    return [
        {"id": "c", "object": "chat.completion.chunk",
         "choices": [{"index": 0, "delta": {"tool_calls": [{
             "index": 0, "id": call_id, "type": "function",
             "function": {"name": name, "arguments": json.dumps(arguments)}}]},
             "finish_reason": None}]},
        {"id": "c", "object": "chat.completion.chunk",
         "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]


class Provider(BaseHTTPRequestHandler):
    """One tool call, then an answer: enough to exercise the whole run path."""

    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            body = json.dumps({"data": [{"id": "stub-model", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        messages = payload.get("messages") or []
        used_tool = any(message.get("role") == "tool" for message in messages)
        chunks = text_chunks("Done. The workspace listing is above.") if used_tool else tool_chunks(
            "call-1", "run_command", {"command": "ls -la", "description": "list the workspace"}
        )
        body = sse(chunks)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 60.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.1)
    raise SystemExit(f"timed out waiting for {url}")


def http_json(url: str, method: str = "GET", body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("X-Karucode-Client", "web")
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as error:
        raise SystemExit(f"{method} {url} -> {error.code}: {error.read().decode('utf-8', 'replace')[:400]}")


SAMPLE = re.compile(r"^([0-9a-f]+)-([0-9a-f]+) [rwxps-]{4} ")


def snapshot(pid: int) -> dict:
    """RSS/PSS split, plus the anonymous mappings a malloc arena hides in."""
    with open(f"/proc/{pid}/smaps_rollup") as handle:
        rollup = {}
        for line in handle:
            if line.startswith(("Rss:", "Pss:", "Anonymous:", "Private_Dirty:", "Private_Clean:", "Shared_Clean:")):
                key, value = line.split(":", 1)
                rollup[key] = int(value.split()[0])
    big_anon = []
    current = None
    for line in open(f"/proc/{pid}/smaps"):
        match = SAMPLE.match(line)
        if match:
            current = (int(match.group(2), 16) - int(match.group(1), 16)) // 1024
        elif line.startswith("Rss:") and current is not None:
            rss = int(line.split()[1])
            if rss >= 1024:
                big_anon.append(rss)
            current = None
    with open(f"/proc/{pid}/status") as handle:
        threads = next(int(line.split()[1]) for line in handle if line.startswith("Threads:"))
    return {
        "rss_mib": round(rollup.get("Rss", 0) / 1024, 1),
        "pss_mib": round(rollup.get("Pss", 0) / 1024, 1),
        "anon_mib": round(rollup.get("Anonymous", 0) / 1024, 1),
        "private_clean_mib": round(rollup.get("Private_Clean", 0) / 1024, 1),
        "big_anon_mappings": len(big_anon),
        "big_anon_mib": round(sum(big_anon) / 1024, 1),
        "threads": threads,
    }


class Sampler(threading.Thread):
    def __init__(self, pid: int) -> None:
        super().__init__(daemon=True)
        self.pid = pid
        self.peak = 0
        self.stop = threading.Event()

    def run(self) -> None:
        while not self.stop.is_set():
            try:
                with open(f"/proc/{self.pid}/smaps_rollup") as handle:
                    for line in handle:
                        if line.startswith("Rss:"):
                            self.peak = max(self.peak, int(line.split()[1]))
                            break
            except FileNotFoundError:
                return
            time.sleep(0.02)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/karucode")
    parser.add_argument("--web-dir", type=Path, default=ROOT / "web/dist")
    parser.add_argument("--env", action="append", default=[], metavar="KEY=VALUE")
    parser.add_argument("--settle", type=float, default=20.0, help="seconds to idle before the last sample")
    parser.add_argument("--runs", type=int, default=1,
                        help="scripted turns to run concurrently; this is what spreads allocation across threads")
    parser.add_argument("--plugin", action="append", default=[], metavar="NAME=/abs/binary",
                        help="register a real plugin manifest in the temp data dir")
    parser.add_argument("--plugin-permissions", default="shell,files.read,network",
                        help="permissions granted to --plugin manifests")
    parser.add_argument("--label", default="")
    args = parser.parse_args()

    env = dict(os.environ)
    env.update({"KARUCODE_WEB_DIR": str(args.web_dir), "KARUCODE_LOG": "warn"})
    for item in args.env:
        key, _, value = item.partition("=")
        env[key] = value

    tmp = Path(tempfile.mkdtemp(prefix="kc-steady-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()
    # Real plugin manifests, exactly as the host discovers them in a deployment.
    for item in args.plugin:
        name, _, binary = item.partition("=")
        folder = data_dir / "plugins" / name
        folder.mkdir(parents=True)
        granted = [p.strip() for p in args.plugin_permissions.split(",") if p.strip()]
        (folder / "plugin.toml").write_text(
            f'name = "{name}"\n'
            f'command = ["{binary}"]\n'
            "enabled = true\n"
            f"permissions = {json.dumps(granted)}\n"
        )
    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    config = tmp / "config.toml"
    config.write_text("\n".join([
        "[model]",
        f'base_url = "http://127.0.0.1:{provider_port}/v1"',
        'provider = "openai_compatible"',
        'model = "stub-model"',
        'api_key = "sk-test"',
        "",
        "[approvals]",
        'mode = "auto"',
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
    ]))

    port = free_port()
    log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [str(args.binary), "--port", str(port), "--workspace", str(workspace),
         "--data-dir", str(data_dir), "--config", str(config)],
        env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
    )
    api = f"http://127.0.0.1:{port}"
    stages: dict[str, dict] = {}
    try:
        wait_for(f"{api}/api/health")
        time.sleep(1.5)
        stages["startup"] = snapshot(server.pid)

        project_id = http_json(f"{api}/api/projects")["projects"][0]["id"]
        session = http_json(f"{api}/api/sessions", "POST",
                            {"title": "Steady state", "project": project_id})
        session_id = session.get("id") or (session.get("session") or {}).get("id")
        time.sleep(1.0)
        stages["session_open"] = snapshot(server.pid)

        sampler = Sampler(server.pid)
        sampler.start()
        session_ids = [session_id]
        for index in range(1, max(1, args.runs)):
            extra = http_json(f"{api}/api/sessions", "POST",
                              {"title": f"Soak {index}", "project": project_id})
            session_ids.append(extra.get("id") or (extra.get("session") or {}).get("id"))
        # Start every run before waiting for any: concurrent turns are what puts
        # allocations on many threads at once, which is exactly what grows a
        # per-thread malloc arena.
        for one in session_ids:
            http_json(f"{api}/api/sessions/{one}/messages", "POST",
                      {"content": "list the workspace"})
        deadline = time.time() + 180
        pending = list(session_ids)
        while pending and time.time() < deadline:
            time.sleep(0.3)
            still = []
            for one in pending:
                if http_json(f"{api}/api/sessions/{one}").get("running"):
                    still.append(one)
                else:
                    # Read the whole transcript back, the way a browser reload does.
                    http_json(f"{api}/api/sessions/{one}")
            pending = still
        if pending:
            raise SystemExit("the scripted runs never finished")
        sampler.stop.set()
        sampler.join()
        stages["after_run"] = snapshot(server.pid)
        time.sleep(args.settle)
        stages["idle"] = snapshot(server.pid)
        print(json.dumps({
            "label": args.label or str(args.binary),
            "env": args.env,
            "peak_rss_during_run_mib": round(sampler.peak / 1024, 1),
            "stages": stages,
        }, indent=2))
        return 0
    finally:
        try:
            os.killpg(os.getpgid(server.pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
        provider.shutdown()
        log.close()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
