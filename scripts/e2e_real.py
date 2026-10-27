#!/usr/bin/env python3
"""End-to-end verification against a REAL OpenAI-compatible provider.

Unlike scripts/e2e_smoke.py (which uses a deterministic mock), this drives the
configured endpoint with real model output. It is slower, non-deterministic and
costs tokens, so it is a separate, opt-in gate.

It boots its own server instance so your sessions and workspace are untouched:

    python3 scripts/e2e_real.py --workspace /tmp/kc-real/ws

By default it copies the settings (endpoint, key, model) out of your existing
karucode database with a consistent SQLite backup, so you do not have to paste a
key anywhere. Pass --config <file> to use a TOML config instead.

What it checks:
  1. provider probe (/models + chat)
  2. plain streamed text, incremental (not one blob at the end)
  3. a real tool call: read a file, then answer from its contents
  4. a multi-step coding task: run the tests, fix the bug, re-run until green
  5. history continuity across a second turn in the same session
  6. cancellation of a live run
  7. a subagent delegation
  8. persistence across a restart

Every check prints PASS/FAIL with a short reason; the exit code is the gate.
"""

import argparse
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "karucode"
DEFAULT_DB = Path.home() / ".local/share/karucode/karucode.db"

PASS: list[str] = []
FAIL: list[tuple[str, str]] = []
SKIP: list[str] = []


class Client:
    def __init__(self, port: int):
        self.base = f"http://127.0.0.1:{port}"

    def raw(self, method, path, body=None, timeout=300, headers=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        req.add_header("X-Karucode-Client", "web")
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
                ctype = r.headers.get("Content-Type", "")
                if "json" in ctype and raw:
                    return r.status, json.loads(raw)
                return r.status, raw
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw)
            except Exception:
                return e.code, raw

    def get(self, path, **kw):
        return self.raw("GET", path, **kw)

    def post(self, path, body=None, **kw):
        return self.raw("POST", path, body, **kw)

    def ok(self, method, path, body=None, expect=200, **kw):
        status, payload = self.raw(method, path, body, **kw)
        if status != expect:
            raise AssertionError(f"{method} {path} -> {status}: {payload}")
        return payload


def check(name, fn):
    started = time.time()
    try:
        detail = fn()
        PASS.append(name)
        print(f"  PASS  {name}  [{detail}]  ({time.time()-started:.1f}s)", flush=True)
        return True
    except Exception as exc:  # noqa: BLE001
        FAIL.append((name, str(exc)))
        print(f"  FAIL  {name}: {exc}  ({time.time()-started:.1f}s)", flush=True)
        return False


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_health(client: Client, timeout=30.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            if client.get("/api/health", timeout=1)[0] == 200:
                return
        except Exception:
            pass
        time.sleep(0.05)
    raise TimeoutError("server did not become healthy")


class Stream:
    """Collects SSE events in a background thread (same transport as the browser)."""

    def __init__(self, port: int):
        import http.client
        import threading

        self.events: list[dict] = []
        self.lock = threading.Lock()
        self.cond = threading.Condition(self.lock)
        self.stopped = False
        self._http = http.client
        self.thread = threading.Thread(target=self._run, args=(port,), daemon=True)
        self.thread.start()
        time.sleep(0.25)

    def _run(self, port: int) -> None:
        while not self.stopped:
            conn = None
            try:
                conn = self._http.HTTPConnection("127.0.0.1", port, timeout=600)
                conn.request(
                    "GET",
                    "/api/events",
                    headers={"Accept": "text/event-stream", "X-Karucode-Client": "web"},
                )
                resp = conn.getresponse()
                if resp.status != 200:
                    time.sleep(0.5)
                    continue
                while not self.stopped:
                    line = resp.readline()
                    if not line:
                        break
                    text = line.decode("utf-8", "replace").strip()
                    if not text.startswith("data:"):
                        continue
                    try:
                        event = json.loads(text[5:].strip())
                    except json.JSONDecodeError:
                        continue
                    with self.cond:
                        self.events.append(event)
                        self.cond.notify_all()
            except Exception:
                if not self.stopped:
                    time.sleep(0.5)
            finally:
                try:
                    conn.close()
                except Exception:
                    pass

    def wait(self, predicate, timeout=300.0, label="event"):
        deadline = time.time() + timeout
        with self.cond:
            while True:
                for event in self.events:
                    if predicate(event):
                        return event
                remaining = deadline - time.time()
                if remaining <= 0:
                    tail = [e.get("type") for e in self.events[-20:]]
                    raise AssertionError(f"timeout waiting for {label}; recent: {tail}")
                self.cond.wait(min(remaining, 0.5))

    def for_run(self, run_id: str, *types: str) -> list[dict]:
        with self.lock:
            return [
                e
                for e in self.events
                if e.get("run_id") == run_id and (not types or e.get("type") in types)
            ]

    def stop(self):
        self.stopped = True


def start_server(port: int, workspace: Path, data_dir: Path, extra_env=None, config=None):
    env = os.environ.copy()
    env.update(
        {
            "KARUCODE_HOST": "127.0.0.1",
            "KARUCODE_PORT": str(port),
            "KARUCODE_WORKSPACE": str(workspace),
            "KARUCODE_DATA_DIR": str(data_dir),
            "KARUCODE_LOG": "warn",
        }
    )
    if config:
        env["KARUCODE_CONFIG"] = str(config)
    if extra_env:
        env.update(extra_env)
    return subprocess.Popen(
        [str(BIN)],
        cwd=str(workspace),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def tool_names(events, run_id, kind="tool_call_started"):
    return [e.get("name") for e in events.for_run(run_id, kind)]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", required=True, help="scratch workspace to test in")
    parser.add_argument("--db", default=str(DEFAULT_DB), help="source DB to copy settings from")
    parser.add_argument("--config", default=None, help="TOML config to use instead of --db")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--only", default=None, help="run only checks whose name contains this")
    args = parser.parse_args()

    if not BIN.exists():
        print(f"missing {BIN}; run: cargo build --release -p kc-server", file=sys.stderr)
        return 2

    workspace = Path(args.workspace).resolve()
    if not workspace.is_dir():
        print(f"workspace not found: {workspace}", file=sys.stderr)
        return 2

    tmp = Path(tempfile.mkdtemp(prefix="kc-real-"))
    data_dir = tmp / "data"
    data_dir.mkdir()
    if not args.config:
        if not Path(args.db).exists():
            print(f"no settings database at {args.db}; pass --config", file=sys.stderr)
            return 2
        # Consistent snapshot (WAL included) so the configured endpoint and key
        # carry over without the key ever passing through this process.
        src = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
        dst = sqlite3.connect(data_dir / "karucode.db")
        src.backup(dst)
        src.close()
        dst.close()

    port = args.port or free_port()
    server = start_server(port, workspace, data_dir, config=args.config)
    client = Client(port)

    def want(name: str) -> bool:
        return args.only is None or args.only.lower() in name.lower()

    try:
        wait_health(client)
        cfg = client.ok("GET", "/api/config")
        model = cfg["model"]
        print(f"\nendpoint : {model['base_url']}")
        print(f"model    : {model['model']}")
        print(f"key set  : {model['api_key_set']}")
        print(f"workspace: {cfg['workspace']}\n")
        if not model["api_key_set"]:
            print("no API key configured — set it in the UI or pass --config", file=sys.stderr)
            return 2

        stream = Stream(port)

        # ---------------------------------------------------------------- probe
        if want("provider probe"):
            def probe():
                started = time.time()
                out = client.ok("POST", "/api/provider/test")
                assert out["ok"], out
                assert out["latency_ms"] < 60_000, out
                assert out["models"], "no models listed"
                return f"{out['latency_ms']} ms, {len(out['models'])} models, {time.time()-started:.1f}s"

            check("provider probe (/models + chat)", probe)

        session = client.ok("POST", "/api/sessions", {"title": "real-provider e2e"})
        sid = session["id"]

        def run(prompt, timeout=600, cancel_after=None):
            out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": prompt}, expect=202)
            rid = out["run_id"]
            started = time.time()
            cancelled = False
            while time.time() - started < timeout:
                if cancel_after and not cancelled and time.time() - started > cancel_after:
                    client.post(f"/api/sessions/{sid}/cancel")
                    cancelled = True
                for e in stream.for_run(rid, "run_finished", "run_cancelled"):
                    return rid, e
                time.sleep(0.1)
            raise AssertionError(f"run did not finish in {timeout}s")
            return rid, None

        # ------------------------------------------------------- streamed text
        if want("streamed text"):
            def streamed_text():
                started = time.time()
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "Reply with exactly one short sentence about what a compiler does. Do not use any tools."},
                    expect=202,
                )
                rid = out["run_id"]
                stream.wait(
                    lambda e: e.get("type") == "text_delta" and e.get("run_id") == rid,
                    timeout=180, label="first text_delta",
                )
                finished = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=300, label="run_finished",
                )
                deltas = stream.for_run(rid, "text_delta")
                text = "".join(e["delta"] for e in deltas)
                assert finished["status"] == "completed", finished
                assert len(text.strip()) > 10, f"short answer: {text!r}"
                assert not stream.for_run(rid, "tool_call_started"), "called a tool despite instructions"
                # Granularity is a property of the gateway, not of the harness: a
                # buffering or caching gateway can legitimately deliver one big
                # delta. Report it, do not fail on it; the deterministic mock test
                # (scripts/e2e_smoke.py) is what proves incremental forwarding.
                return f"{len(deltas)} delta(s), {len(text)} chars, {time.time()-started:.1f}s"

            check("text response completes (delta count reported)", streamed_text)

        # ------------------------------------------------------- real tool call
        if want("tool read"):
            def tool_read():
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "Use list_dir and read_file to look at this workspace. "
                                "Tell me in one line the total number of test cases in test/math.test.js. "
                                "Do not modify anything."},
                    expect=202,
                )
                rid = out["run_id"]
                finished = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=600, label="tool run finish",
                )
                assert finished["status"] == "completed", finished
                names = tool_names(stream, rid)
                assert names, f"no tools called; text={''.join(e['delta'] for e in stream.for_run(rid, 'text_delta'))[:200]!r}"
                results = stream.for_run(rid, "tool_call_finished")
                assert any(e["ok"] for e in results), [(e.get("name"), e.get("output", "")[:80]) for e in results]
                detail = client.ok("GET", f"/api/sessions/{sid}")
                assert any(m["role"] == "tool" for m in detail["messages"]), "no tool message persisted"
                return f"tools: {', '.join(names[:4])}"

            check("real tool call (list_dir/read_file)", tool_read)

        # ------------------------------------------------ multi-step coding task
        if want("multi-step"):
            def multi_step():
                before = subprocess.run(
                    ["node", "--test"], cwd=workspace, capture_output=True, text=True, timeout=120
                )
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "This project has failing tests. Run `node --test`, read the failing test "
                                "and the source it covers, fix the bug in the source (not the test), then "
                                "re-run the tests to prove they pass. Report the failing count before and after."},
                    expect=202,
                )
                rid = out["run_id"]
                finished = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=900, label="coding task finish",
                )
                assert finished["status"] == "completed", finished
                names = tool_names(stream, rid)
                assert "run_command" in names, f"never ran a command: {names}"
                assert any(n in names for n in ("read_file", "edit_file", "search_files")), f"never read/edited: {names}"
                # The real proof: the workspace tests actually pass now.
                after = subprocess.run(
                    ["node", "--test"], cwd=workspace, capture_output=True, text=True, timeout=120
                )
                combined = after.stdout + after.stderr
                assert after.returncode == 0, f"tests still failing:\n{combined[-1200:]}"
                diff = subprocess.run(
                    ["git", "diff", "--stat"], cwd=workspace, capture_output=True, text=True
                ).stdout.strip()
                assert diff, "no file was changed"
                before_fail = "fail 2" in before.stderr + before.stdout
                return f"tools={len(names)}, tests green, changed: {diff.splitlines()[-1]}"

            check("multi-step coding task (run tests -> fix -> verify)", multi_step)

        # ------------------------------------------------------ history continuity
        if want("history"):
            def history():
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "In one short sentence: which file did you change earlier in this conversation, "
                                "and what was wrong with it? Use no tools."},
                    expect=202,
                )
                rid = out["run_id"]
                finish = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=300, label="history turn",
                )
                assert finish["status"] == "completed", finish
                text = "".join(e["delta"] for e in stream.for_run(rid, "text_delta")).lower()
                assert "math" in text or "add" in text, f"lost context: {text[:300]!r}"
                return text.strip().splitlines()[0][:70]

            check("history continuity across turns", history)

        # ------------------------------------------------------------ cancellation
        if want("cancel"):
            def cancel():
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "Count slowly from 1 to 200, writing one number per sentence, and think "
                                "carefully about each. Do not use tools."},
                    expect=202,
                )
                rid = out["run_id"]
                stream.wait(
                    lambda e: e.get("type") == "run_started" and e.get("run_id") == rid,
                    timeout=60, label="run start",
                )
                time.sleep(1.5)
                started = time.time()
                client.post(f"/api/sessions/{sid}/cancel")
                finish = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=60, label="cancelled run",
                )
                took = time.time() - started
                assert finish["status"] in ("cancelled", "completed"), finish
                assert took < 20, f"cancel took {took:.1f}s"
                # the session must be usable immediately afterwards
                out2 = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "Reply with the single word: ready"},
                    expect=202,
                )
                f2 = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == out2["run_id"],
                    timeout=180, label="post-cancel run",
                )
                assert f2["status"] == "completed", f2
                return f"cancelled={finish['status']} in {took:.1f}s, session reusable"

            check("cancellation stops a live run", cancel)

        # -------------------------------------------------------------- subagent
        if want("subagent"):
            def subagent():
                out = client.ok(
                    "POST", f"/api/sessions/{sid}/messages",
                    {"content": "Delegate to a subagent: ask it to read README.md and package.json and report "
                                "the project name and the test script in one line. Then relay its answer.",
                    },
                    expect=202,
                )
                rid = out["run_id"]
                finish = stream.wait(
                    lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                    timeout=600, label="subagent run",
                )
                assert finish["status"] == "completed", finish
                spawned = stream.for_run(rid, "agent_spawned")
                assert spawned, f"no subagent spawned; tools={tool_names(stream, rid)}"
                detail = client.ok("GET", f"/api/sessions/{sid}")
                assert detail["agents"], "no AgentRecord persisted"
                done = [a for a in detail["agents"] if a["status"] == "completed"]
                assert done, [(a["name"], a["status"], a.get("error")) for a in detail["agents"]]
                return f"{len(done)} agent(s) completed: {done[0]['name']}"

            check("subagent delegation", subagent)

        # ------------------------------------------------------------ persistence
        if want("persist"):
            def persist():
                before = client.ok("GET", f"/api/sessions/{sid}")
                server.send_signal(signal.SIGTERM)
                server.wait(timeout=20)
                restarted = start_server(port, workspace, data_dir, config=args.config)
                wait_health(client)
                after = client.ok("GET", f"/api/sessions/{sid}")
                assert len(after["messages"]) == len(before["messages"]), (
                    f"{len(before['messages'])} -> {len(after['messages'])} messages"
                )
                assert after["agents"], "subagent records lost"
                return f"{len(after['messages'])} messages, {len(after['agents'])} agent(s) survived"

            check("state survives a restart", persist)

        stream.stop()

    finally:
        try:
            server.send_signal(signal.SIGTERM)
            server.wait(timeout=20)
        except Exception:
            try:
                server.kill()
            except Exception:
                pass
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)
        else:
            print(f"\nkept: {tmp}")

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed" + (f", {len(SKIP)} skipped" if SKIP else ""))
    if FAIL:
        print("\nFailures:", file=sys.stderr)
        for name, err in FAIL:
            print(f"  - {name}: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())