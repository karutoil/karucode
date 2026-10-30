#!/usr/bin/env python3
"""End-to-end smoke test for karucode.

Starts the real server binary and scripts/mock_openai.py (a deterministic
OpenAI-compatible streaming mock), then drives the full product surface:
sessions, streaming, tool calls, approvals, cancellation, subagents, settings,
plugins, MCP, path confinement, CSRF and the served UI.

Usage:
    python3 scripts/e2e_smoke.py --build
    python3 scripts/e2e_smoke.py --binary target/debug/karucode

Exit code 0 only if every check passes. Writes reports/e2e.json.
"""

import argparse
import http.client
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = sys.executable

PASS, FAIL, SKIP = [], [], []


def check(name, fn):
    try:
        detail = fn()
        PASS.append(name)
        print(f"  PASS  {name}" + (f"  [{detail}]" if detail else ""), flush=True)
        return True
    except AssertionError as exc:
        FAIL.append((name, str(exc)))
        print(f"  FAIL  {name}: {exc}", flush=True)
        return False
    except Exception as exc:  # noqa: BLE001
        FAIL.append((name, f"{type(exc).__name__}: {exc}"))
        print(f"  FAIL  {name}: {type(exc).__name__}: {exc}", flush=True)
        return False


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_http(url, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.05)
    raise TimeoutError(f"{url} did not become ready")


class Client:
    def __init__(self, port):
        self.port = port
        self.base = f"http://127.0.0.1:{port}"

    def raw(self, method, path, body=None, headers=None, timeout=30):
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

    def patch(self, path, body=None, **kw):
        return self.raw("PATCH", path, body, **kw)

    def delete(self, path, **kw):
        return self.raw("DELETE", path, **kw)

    def ok(self, method, path, body=None, headers=None, expect=200):
        status, payload = self.raw(method, path, body, headers)
        assert status == expect, f"{method} {path} -> {status}: {payload}"
        return payload


class EventStream:
    """Fetch-based SSE reader (same transport the browser uses)."""

    def __init__(self, port, token=None):
        self.port = port
        self.token = token
        self.events = []
        self.lock = threading.Lock()
        self.cond = threading.Condition(self.lock)
        self.stopped = False
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()
        time.sleep(0.2)

    def _run(self):
        while not self.stopped:
            try:
                conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=180)
                headers = {"Accept": "text/event-stream", "X-Karucode-Client": "e2e"}
                if self.token:
                    headers["Authorization"] = f"Bearer {self.token}"
                conn.request("GET", "/api/events", headers=headers)
                resp = conn.getresponse()
                if resp.status != 200:
                    time.sleep(0.5)
                    continue
                while not self.stopped:
                    line = resp.readline()
                    if not line:
                        break
                    line = line.decode("utf-8", "replace").strip()
                    if not line.startswith("data:"):
                        continue
                    try:
                        event = json.loads(line[len("data:"):].strip())
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

    def wait(self, predicate, timeout=45, label="event"):
        deadline = time.time() + timeout
        with self.cond:
            while True:
                for event in self.events:
                    if predicate(event):
                        return event
                remaining = deadline - time.time()
                if remaining <= 0:
                    tail = [e.get("type") for e in self.events[-15:]]
                    raise AssertionError(f"timeout waiting for {label}; recent events: {tail}")
                self.cond.wait(min(remaining, 0.5))

    def of_type(self, *types):
        with self.lock:
            return [e for e in self.events if e.get("type") in types]

    def for_run(self, run_id, *types):
        return [e for e in self.of_type(*types) if e.get("run_id") == run_id]

    def stop(self):
        self.stopped = True


def build_config(tmp, plugin_bin):
    cfg = ['host = "127.0.0.1"', 'log_level = "warn"']
    if plugin_bin:
        cfg += [
            "[[plugins]]",
            'name = "example-hello"',
            f'command = ["{plugin_bin}"]',
            "enabled = true",
            'permissions = ["files.read"]',
        ]
    # A plugin whose tool raises a `notify` frame: the only way a plugin can reach a
    # human, and the path that has to work end to end through the host's validation
    # and the server's bridge onto the event stream.
    notify_plugin = ROOT / "tests/fixtures/notify_plugin.sh"
    if notify_plugin.exists():
        cfg += [
            "[[plugins]]",
            'name = "notifier-fixture"',
            f'command = ["sh", "{notify_plugin}"]',
            "enabled = true",
            'permissions = ["*"]',
        ]
    cfg += [
        "[[mcp_servers]]",
        'name = "mock"',
        'transport = "stdio"',
        f'command = ["{PY}", "{ROOT / "scripts/fixtures/mock_mcp_server.py"}"]',
        "enabled = true",
        "require_approval = true",
    ]
    path = tmp / "config.toml"
    path.write_text("\n".join(cfg) + "\n")
    return path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default=str(ROOT / "target/debug/karucode"))
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--web-dir", default=None)
    parser.add_argument("--keep", action="store_true", help="keep the temp dir for inspection")
    args = parser.parse_args()

    if args.build:
        print("building server + example plugin...", flush=True)
        subprocess.run(["cargo", "build", "-p", "kc-server", "-p", "example-hello"], cwd=ROOT, check=True)

    binary = Path(args.binary).resolve()
    if not binary.exists():
        print(f"binary not found: {binary} (use --build)", file=sys.stderr)
        return 2

    plugin_bin = next(
        (
            candidate
            for candidate in (
                ROOT / "target/debug/example-hello",
                ROOT / "target/release/example-hello",
            )
            if candidate.exists()
        ),
        None,
    )
    if plugin_bin is None:
        SKIP.append("plugin checks: no example-hello binary (cargo build -p example-hello)")
    tmp = Path(tempfile.mkdtemp(prefix="kc-e2e-"))
    try:
        workspace = tmp / "ws"
        (workspace / "src").mkdir(parents=True)
        (workspace / "README.md").write_text("# e2e workspace\n\nHello from the end-to-end test.\n")
        (workspace / "src" / "main.rs").write_text("fn main() {\n    println!(\"hi\");\n}\n")
        subprocess.run(["git", "init", "-q"], cwd=workspace, check=False)
        subprocess.run(["git", "add", "-A"], cwd=workspace, check=False)
        subprocess.run(
            ["git", "-c", "user.email=e2e@local", "-c", "user.name=e2e", "commit", "-qm", "init"],
            cwd=workspace,
            check=False,
        )

        mock_port = free_port()
        mock_log = tmp / "mock.jsonl"
        mock = subprocess.Popen(
            [PY, str(ROOT / "scripts/mock_openai.py"), "--port", str(mock_port), "--log", str(mock_log)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.STDOUT,
        )
        wait_http(f"http://127.0.0.1:{mock_port}/health", timeout=15)

        config = build_config(tmp, plugin_bin)
        port = free_port()
        env = os.environ.copy()
        env.update(
            {
                "KARUCODE_HOST": "127.0.0.1",
                "KARUCODE_PORT": str(port),
                "KARUCODE_WORKSPACE": str(workspace),
                "KARUCODE_DATA_DIR": str(tmp / "data"),
                "KARUCODE_CONFIG": str(config),
                "KARUCODE_BASE_URL": f"http://127.0.0.1:{mock_port}/v1",
                "KARUCODE_MODEL": "mock-coder",
                "KARUCODE_APPROVAL_MODE": "ask",
                "KARUCODE_LOG": "warn",
                "RUST_LOG": "warn",
                # The suite must not sit through the production retry budgets: a
                # scripted `[fail]` would otherwise be retried ~50 times with a
                # 30s ceiling before it surfaces as a failed run.
                "KARUCODE_RETRY_MAX_RETRIES": "2",
                "KARUCODE_RETRY_BASE_DELAY_MS": "50",
                "KARUCODE_RETRY_MAX_DELAY_MS": "200",
                "KARUCODE_RETRY_MAX_STREAM_RESTARTS": "3",
            }
        )
        if args.web_dir:
            env["KARUCODE_WEB_DIR"] = args.web_dir
        elif (ROOT / "web" / "dist").exists():
            env["KARUCODE_WEB_DIR"] = str(ROOT / "web" / "dist")

        server_log = tmp / "server.log"
        server = subprocess.Popen(
            [str(binary)],
            cwd=str(workspace),
            env=env,
            stdout=open(server_log, "w"),
            stderr=subprocess.STDOUT,
        )
        client = Client(port)
        try:
            wait_http(client.base + "/api/health", timeout=30)
        except Exception:
            print(server_log.read_text() if server_log.exists() else "", file=sys.stderr)
            server.kill()
            mock.terminate()
            return 2

        events = EventStream(port)
        created_sessions = []
        healthy_since = time.time()

        def sse_for_session(session_id):
            def pred(event):
                return event.get("session_id") == session_id
            return pred

        print("\n== health / config ==", flush=True)
        check(
            "health reports ok and local bind",
            lambda: (
                (lambda h: (
                    _assert(h["status"] == "ok", "status"),
                    _assert(h["auth_required"] is False, "auth_required"),
                ))(client.ok("GET", "/api/health")),
                "ok",
            )[-1],
        )
        cfg = client.ok("GET", "/api/config")
        check("config exposes model without api key", lambda: (
            _assert(cfg["model"]["base_url"].endswith("/v1"), "base_url"),
            _assert("api_key" not in cfg["model"], "api key leaked into config"),
            _assert(cfg["workspace"].endswith("ws"), "workspace"),
            "model=" + cfg["model"]["model"],
        )[-1])
        check("plugins and MCP discovered", lambda: (
            _assert(any(p["name"] == "example-hello" and p["status"] == "ready" for p in cfg["plugins"]),
                    f"plugins={cfg['plugins']}"),
            _assert(any(s["name"] == "mock" and s["status"] == "connected" for s in cfg["mcp_servers"]),
                    f"mcp={cfg['mcp_servers']}"),
            "plugin+mcp ready",
        )[-1])

        print("\n== security ==", flush=True)
        status, _ = client.raw("POST", "/api/sessions", {"title": "csrf"}, headers={"X-Karucode-Client": ""})
        check("mutation without client header is rejected", lambda: (
            _assert(status == 403, f"expected 403 got {status}"), "403"
        )[-1])
        status, _ = client.raw(
            "POST", "/api/sessions", {"title": "origin"},
            headers={"Origin": "http://evil.example"},
        )
        check("cross-origin mutation is rejected", lambda: (
            _assert(status == 403, f"expected 403 got {status}"), "403"
        )[-1])
        # Escape must be *refused*, not merely fail. `../..` from a workspace under
        # the temp dir now lands inside the allowed temp root (allow_temp_dir is on
        # by default), so that spelling answers 404 "no such file" - which is the
        # correct answer, and not a containment failure. Walk far enough up to be
        # outside every allowed root so this asserts the containment check itself.
        status, body = client.get("/api/workspace/file?path=../../../../../../etc/passwd")
        check("path traversal is rejected", lambda: (
            _assert(status in (400, 403), f"expected 400/403 got {status}"),
            f"{status} {body.get('error', {}).get('code', '')}".strip(),
        )[-1])

        # Regression guard: the server once wrapped its whole serve future in a
        # 10s timeout and shut itself down mid-suite. Nothing may kill it.
        wait = 13.0 - (time.time() - healthy_since)
        if wait > 0:
            time.sleep(wait)
        health_late = client.ok("GET", "/api/health")
        check("server stays up past the shutdown-drain window", lambda: (
            _assert(health_late["uptime_secs"] >= 12, "uptime reset or server restarted"),
            f"{health_late['uptime_secs']}s",
        )[-1])

        print("\n== sessions / workspace ==", flush=True)
        session = client.ok("POST", "/api/sessions", {"title": "e2e session"})
        sid = session["id"]
        created_sessions.append(sid)
        check("session created", lambda: (_assert(sid.startswith("s_"), sid), sid)[-1])
        tree = client.ok("GET", "/api/workspace/tree?path=.&depth=2")
        names = [e["name"] for e in tree["entries"]]
        check("workspace tree lists files", lambda: (
            _assert("README.md" in names, names), _assert("src" in names, names), "2 entries"
        )[-1])
        f = client.ok("GET", "/api/workspace/file?path=README.md")
        check("file read returns content", lambda: (
            _assert("e2e workspace" in f["content"], f["content"][:80]), "ok"
        )[-1])
        git = client.ok("GET", "/api/git/status")
        check("git status works in a repo", lambda: (
            _assert(git["is_repo"] is True, git), _assert(git.get("branch"), "no branch"), git.get("branch") or ""
        )[-1])

        def run_prompt(prompt, timeout=60):
            out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": prompt}, expect=202)
            rid = out["run_id"]
            events.wait(
                lambda e: e.get("type") == "run_finished" and e.get("run_id") == rid,
                timeout=timeout,
                label=f"run_finished for {prompt[:20]}",
            )
            return rid

        print("\n== agent loop ==", flush=True)
        rid = run_prompt("[tool:read_file] read the readme")
        check("text streams and tool executes", lambda: (
            _assert(len(events.for_run(rid, "text_delta")) > 0, "no text_delta"),
            _assert(any(e["name"] == "read_file" for e in events.for_run(rid, "tool_call_started")), "tool not called"),
            _assert(all(e["ok"] for e in events.for_run(rid, "tool_call_finished")), "tool failed"),
            f"{len(events.for_run(rid, 'text_delta'))} deltas",
        )[-1])

        # A completed run must announce itself as a notification, with the kind and
        # the session title the UI keys its cue, badge and toast off.
        def notification_for_run():
            # The notification is emitted *after* `run_finished` (deliberately: a
            # client that reacts to it must find the terminal state already
            # persisted), so `run_prompt` returning is not enough — wait for it.
            events.wait(
                lambda e: e.get("type") == "notification" and e.get("run_id") == rid,
                timeout=30,
                label="notification for the run",
            )
            frames = events.for_run(rid, "notification")
            _assert(len(frames) == 1, f"expected exactly one notification, got {len(frames)}")
            payload = frames[0]["notification"]
            _assert(payload["kind"] == "run_finished", payload["kind"])
            _assert(payload["source"] == "runtime", payload["source"])
            _assert(payload["session_id"] == sid, payload["session_id"])
            _assert(payload["id"] == f"run:{rid}", payload["id"])
            _assert(payload["session_title"], "the notification must carry the session title")
            # Ordering: the terminal state must be on the wire first, so a client
            # that reacts by refetching finds the truth.
            types = [e.get("type") for e in events.of_type("run_finished", "notification")]
            _assert(
                types.index("run_finished") < types.index("notification"),
                f"run_finished must precede its notification: {types}",
            )
            return f"kind={payload['kind']} title={payload['session_title']!r}"

        check("a finished run emits one notification", notification_for_run)

        print("\n== notifications can be turned off ==", flush=True)
        # The master switch is enforced by the server, so the proof is that no frame
        # arrives at all rather than that a client chose to ignore one.
        client.ok("PATCH", "/api/config", {"notifications": {"enabled": False}})
        try:
            silent_rid = run_prompt("[text] second prompt while notifications are off")
            # Give any (incorrect) frame time to arrive before asserting absence.
            time.sleep(0.5)
            check("a disabled configuration emits no notification", lambda: (
                _assert(
                    len(events.for_run(silent_rid, "notification")) == 0,
                    f"got {len(events.for_run(silent_rid, 'notification'))} frame(s)",
                ),
                "0 frames",
            )[-1])
        finally:
            client.ok("PATCH", "/api/config", {"notifications": {"enabled": True}})

        check("the config reports every notification switch", lambda: (
            _assert(
                set(client.ok("GET", "/api/config")["notifications"])
                >= {
                    "enabled", "run_finished", "needs_input", "error", "info",
                    "plugin", "min_run_secs", "say_body", "max_min_run_secs", "kinds",
                },
                client.ok("GET", "/api/config")["notifications"],
            ),
            "all switches present",
        )[-1])

        print("\n== a plugin can raise a notification ==", flush=True)
        if any(p["name"] == "notifier-fixture" for p in client.ok("GET", "/api/plugins")["plugins"]):
            # A session of its own: these checks run two plugin tools (each behind an
            # approval), and a session holds one run at a time — the later steps still
            # use the original session.
            plugin_session = client.ok("POST", "/api/sessions", {"title": "plugin notify"})["id"]
            # The only channel by which a plugin can reach a human: a `notify` frame,
            # which the host validates and the server bridges onto the event stream.
            #
            # A plugin tool is always class `external`, so it needs approval. The
            # prompt is submitted first and the approval answered below, because the
            # run only finishes after the gate opens.
            client.ok("POST", f"/api/sessions/{plugin_session}/messages",
                      {"content": "[tool:notify] ask the plugin to notify"}, expect=202)

            def approve_plugin_tool():
                events.wait(lambda e: e.get("type") == "approval_requested", timeout=60,
                            label="approval for the plugin tool")
                pending = client.ok("GET", "/api/approvals")["approvals"]
                _assert(pending, "no pending approval to answer")
                client.ok("POST", f"/api/approvals/{pending[0]['id']}", {"decision": "allow_once"})
                return pending[0]["title"]

            check("the plugin tool needs approval", approve_plugin_tool)
            # The run finishes now; wait for its terminal frame and the notification.
            events.wait(
                lambda e: e.get("type") == "notification"
                and e["notification"].get("source") == "notifier-fixture",
                timeout=60,
                label="plugin notification",
            )

            def plugin_notification():
                frames = [
                    e for e in events.of_type("notification")
                    if e["notification"].get("source") == "notifier-fixture"
                ]
                _assert(len(frames) >= 1, "the plugin notification never arrived")
                payload = frames[-1]["notification"]
                _assert(payload["source"] == "notifier-fixture", payload["source"])
                _assert(payload["kind"] == "needs_input", payload["kind"])
                _assert(payload["title"], "a plugin notification must carry its title")
                _assert(payload["sticky"] is True, "the fixture asked to stay on screen")
                return f"source={payload['source']} kind={payload['kind']}"

            check("a plugin-raised notification reaches the event stream", plugin_notification)

            # The switch is enforced in the host, so a plugin that keeps sending gets
            # nothing through.
            client.ok("PATCH", "/api/config", {"notifications": {"plugin": False}})
            try:
                # The session accepts one run at a time. `events.wait` returns at once
                # for an event it has already seen, so poll the session itself until
                # the previous run has actually settled.
                deadline = time.time() + 60
                while time.time() < deadline:
                    detail = client.ok("GET", f"/api/sessions/{plugin_session}")
                    if detail.get("running") is False:
                        break
                    time.sleep(0.1)
                else:
                    raise AssertionError("the plugin run never settled")
                before = len(
                    [e for e in events.of_type("notification")
                     if e["notification"].get("source") == "notifier-fixture"]
                )
                client.ok("POST", f"/api/sessions/{plugin_session}/messages",
                          {"content": "[tool:notify] try again with plugins muted"}, expect=202)
                events.wait(lambda e: e.get("type") == "approval_requested", timeout=60,
                            label="second approval")
                pending = client.ok("GET", "/api/approvals")["approvals"]
                if pending:
                    client.ok("POST", f"/api/approvals/{pending[0]['id']}", {"decision": "allow_once"})
                # Leave the session idle: the steps after this one submit again, and a
                # session accepts one run at a time.
                deadline = time.time() + 60
                while time.time() < deadline:
                    detail = client.ok("GET", f"/api/sessions/{plugin_session}")
                    if detail.get("running") is False:
                        break
                    time.sleep(0.1)
                time.sleep(1.0)
                check("disabling plugin notifications refuses the frame", lambda: (
                    _assert(
                        len([e for e in events.of_type("notification")
                             if e["notification"].get("source") == "notifier-fixture"]) == before,
                        "a plugin notification got through while plugin = false",
                    ),
                    "0 further plugin frames",
                )[-1])
            finally:
                client.ok("PATCH", "/api/config", {"notifications": {"plugin": True}})
        else:
            SKIP.append("plugin notification (fixture plugin not discovered)")
            print("  SKIP  a plugin can raise a notification  [fixture not discovered]", flush=True)



        detail = client.ok("GET", f"/api/sessions/{sid}")
        check("run completed and history persisted", lambda: (
            _assert(any(r["id"] == rid and r["status"] == "completed" for r in detail["runs"]),
                    [r["status"] for r in detail["runs"]]),
            _assert(any(m["role"] == "tool" for m in detail["messages"]), "no tool message"),
            _assert(any(m["role"] == "assistant" for m in detail["messages"]), "no assistant message"),
            f"{len(detail['messages'])} messages",
        )[-1])

        print("\n== approvals / shell ==", flush=True)
        out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": "[tool:run] run the echo"}, expect=202)
        run_id = out["run_id"]
        approval = events.wait(
            lambda e: e.get("type") == "approval_requested" and e.get("run_id") == run_id,
            timeout=30,
            label="approval request",
        )
        aid = approval["approval"]["id"]
        check("shell command asks for approval", lambda: (
            _assert(approval["approval"]["kind"] == "shell_command", approval["approval"]),
            approval["approval"]["title"][:40],
        )[-1])
        client.ok("POST", f"/api/approvals/{aid}", {"decision": "allow_once"})
        events.wait(
            lambda e: e.get("type") == "run_finished" and e.get("run_id") == run_id,
            timeout=60,
            label="shell run finish",
        )
        check("approved command runs and streams output", lambda: (
            _assert(events.for_run(run_id, "command_started"), "no command_started"),
            _assert("karucode-e2e" in "".join(e.get("chunk", "") for e in events.for_run(run_id, "command_output")), "no output"),
            _assert(any(e.get("exit_code") == 0 for e in events.for_run(run_id, "command_finished")), "exit code"),
            "exit 0",
        )[-1])

        out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": "[tool:run] denied"}, expect=202)
        run_id = out["run_id"]
        approval = events.wait(
            lambda e: e.get("type") == "approval_requested" and e.get("run_id") == run_id,
            timeout=30, label="deny approval",
        )
        client.ok("POST", f"/api/approvals/{approval['approval']['id']}", {"decision": "deny"})
        fin = events.wait(
            lambda e: e.get("type") == "run_finished" and e.get("run_id") == run_id,
            timeout=60, label="denied run finish",
        )
        check("denied command is not executed", lambda: (
            _assert(not events.for_run(run_id, "command_started"), "command ran despite deny"),
            _assert(fin["status"] in ("completed", "failed"), fin["status"]),
            fin["status"],
        )[-1])

        print("\n== multiple tools / errors / cancel ==", flush=True)
        rid = run_prompt("[tool:multi] look around")
        check("parallel tool calls in one turn both execute", lambda: (
            _assert(len(events.for_run(rid, "tool_call_finished")) >= 2, [e.get("name") for e in events.for_run(rid, "tool_call_finished")]),
            "2 tools",
        )[-1])
        # Regression: DeepSeek-V4 writes its DSML tool-call markup into the text
        # channel and an endpoint that does not parse it forwards the markup as
        # prose, so the call never ran and the model looked broken. The markup
        # must be recovered into a real call - and must not reach the answer.
        rid = run_prompt("[tool:dsml] look at the workspace")
        check("a tool call written as DSML text is executed", lambda: (
            _assert(
                [e.get("name") for e in events.for_run(rid, "tool_call_finished")].count("list_dir") == 1,
                [e.get("name") for e in events.for_run(rid, "tool_call_finished")],
            ),
            _assert(
                all(e["ok"] for e in events.for_run(rid, "tool_call_finished")),
                [(e["name"], e["ok"]) for e in events.for_run(rid, "tool_call_finished")],
            ),
            "list_dir ran from text markup",
        )[-1])
        detail = client.ok("GET", f"/api/sessions/{sid}")
        check("DSML markup never reaches the transcript", lambda: (
            _assert(
                not any("DSML" in (m.get("content") or "") for m in detail["messages"]),
                [m.get("content", "")[:200] for m in detail["messages"] if "DSML" in (m.get("content") or "")],
            ),
            "no markup in the transcript",
        )[-1])

        rid = run_prompt("[tool:bad] call a missing tool")
        check("unknown tool fails visibly, run continues", lambda: (
            _assert(any(not e["ok"] for e in events.for_run(rid, "tool_call_finished")), "unknown tool reported ok"),
            "error surfaced",
        )[-1])
        rid = run_prompt("[fail] explode")
        check("provider failure is visible", lambda: (
            _assert(any(e.get("type") == "run_finished" and e.get("status") == "failed" and e.get("run_id") == rid
                        for e in events.events), "no failed run"),
            "failed status",
        )[-1])

        out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": "[slow] take your time"}, expect=202)
        slow_run = out["run_id"]
        events.wait(lambda e: e.get("type") == "run_started" and e.get("run_id") == slow_run, timeout=20, label="slow start")
        client.ok("POST", f"/api/sessions/{sid}/cancel")
        fin = events.wait(lambda e: e.get("type") == "run_finished" and e.get("run_id") == slow_run, timeout=20, label="cancel")
        check("run cancels promptly", lambda: (
            _assert(fin["status"] == "cancelled", fin["status"]), "cancelled"
        )[-1])

        print("\n== mid-stream restart ==", flush=True)
        # Regression: the gateway cut the body after it had started streaming
        # (`error decoding response body`). The partial attempt must be thrown
        # away and the turn restarted, not failed.
        out = client.ok(
            "POST",
            f"/api/sessions/{sid}/messages",
            {"content": "[die] survive a stream that dies mid-answer"},
            expect=202,
        )
        dead_run = out["run_id"]
        retry = events.wait(
            lambda e: e.get("type") == "stream_retry" and e.get("run_id") == dead_run,
            timeout=60,
            label="stream_retry",
        )
        fin = events.wait(
            lambda e: e.get("type") == "run_finished" and e.get("run_id") == dead_run,
            timeout=60,
            label="restarted run finish",
        )
        check("a stream that dies mid-answer is restarted, not fatal", lambda: (
            _assert(retry["attempt"] == 1, retry),
            _assert(retry["max_attempts"] >= 2, retry),
            _assert(bool(retry.get("reason")), "retry carried no reason"),
            _assert(fin["status"] == "completed", fin["status"]),
            f"attempt {retry['attempt']}/{retry['max_attempts']}",
        )[-1])
        time.sleep(0.2)
        detail = client.ok("GET", f"/api/sessions/{sid}")
        check("only the restart's answer is persisted", lambda: (
            _assert(
                any(m["role"] == "assistant" and "Recovered" in (m.get("content") or "")
                    and m.get("run_id") == dead_run for m in detail["messages"]),
                "the recovered answer is missing",
            ),
            _assert(
                not any("half an answer" in (m.get("content") or "") for m in detail["messages"]),
                "the abandoned attempt leaked into history",
            ),
            "one answer",
        )[-1])

        print("\n== subagents ==", flush=True)
        rid = run_prompt("[tool:agent] delegate this", timeout=90)
        time.sleep(0.3)
        detail = client.ok("GET", f"/api/sessions/{sid}")
        check("subagent runs and reports", lambda: (
            _assert(len(detail["agents"]) >= 1, detail["agents"]),
            _assert(any(a["status"] == "completed" for a in detail["agents"]),
                    [(a["name"], a["status"]) for a in detail["agents"]]),
            _assert(any(e.get("type") == "agent_finished" for e in events.events), "no agent_finished event"),
            f"{len(detail['agents'])} agent(s)",
        )[-1])

        print("\n== plugins / MCP tools ==", flush=True)
        if plugin_bin is None:
            print("  SKIP  plugin checks (no example-hello binary)", flush=True)
        else:
            out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": "[tool:plugin] say hello"}, expect=202)
            prun = out["run_id"]
            # Plugin tools are external code: the approval policy must gate them.
            ap = events.wait(
                lambda e: e.get("type") == "approval_requested" and e.get("run_id") == prun,
                timeout=20, label="plugin approval",
            )
            check("plugin tool requires approval", lambda: (
                _assert(ap["approval"]["kind"] == "plugin", ap["approval"]),
                ap["approval"]["title"][:40],
            )[-1])
            client.ok("POST", f"/api/approvals/{ap['approval']['id']}", {"decision": "allow_session"})
            events.wait(lambda e: e.get("type") == "run_finished" and e.get("run_id") == prun, timeout=60, label="plugin run")
            check("plugin tool is callable", lambda: (
                _assert(any(e.get("name") == "plugin__example-hello__hello" and e.get("ok")
                            for e in events.for_run(prun, "tool_call_finished")),
                        [e.get("name") for e in events.for_run(prun, "tool_call_finished")]),
                "plugin tool ok",
            )[-1])

        out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": "[tool:mcp] echo"}, expect=202)
        mrun = out["run_id"]
        ap = events.wait(
            lambda e: e.get("type") == "approval_requested" and e.get("run_id") == mrun,
            timeout=20, label="mcp approval",
        )
        check("MCP tool requires approval", lambda: (
            _assert(ap["approval"]["kind"] == "mcp", ap["approval"]),
            ap["approval"]["title"][:40],
        )[-1])
        client.ok("POST", f"/api/approvals/{ap['approval']['id']}", {"decision": "allow_session"})
        events.wait(lambda e: e.get("type") == "run_finished" and e.get("run_id") == mrun, timeout=60, label="mcp run")
        check("MCP tool is callable and namespaced", lambda: (
            _assert(any(e.get("name") == "mcp__mock__echo" and e.get("ok")
                        for e in events.for_run(mrun, "tool_call_finished")),
                    [e.get("name") for e in events.for_run(mrun, "tool_call_finished")]),
            "mcp tool ok",
        )[-1])

        # read_only mode must deny external tools without prompting, whether or
        # not the tool opts out of the prompt.
        target = "[tool:plugin] say hello" if plugin_bin else "[tool:mcp] echo"
        client.ok("PATCH", "/api/config", {"approvals": {"mode": "read_only"}})
        out = client.ok("POST", f"/api/sessions/{sid}/messages", {"content": target}, expect=202)
        rrun = out["run_id"]
        events.wait(lambda e: e.get("type") == "run_finished" and e.get("run_id") == rrun, timeout=60, label="read_only run")
        check("read_only mode denies external tools", lambda: (
            _assert(not events.for_run(rrun, "approval_requested"), "read_only must not prompt"),
            _assert(any(not e["ok"] and "denied" in (e.get("output") or "").lower()
                        for e in events.for_run(rrun, "tool_call_finished")), "tool was not denied"),
            "denied by policy",
        )[-1])
        client.ok("PATCH", "/api/config", {"approvals": {"mode": "ask"}})

        print("\n== hardening ==", flush=True)
        # DNS rebinding: a rebound host must not be served even with a matching
        # Origin (the classic attack on a loopback dev server).
        status, body = client.raw(
            "PATCH", "/api/config", {"approvals": {"mode": "auto"}},
            headers={"Host": "evil.example:{}".format(port), "Origin": "http://evil.example"},
        )
        check("rebound Host is refused", lambda: (
            _assert(status == 403, f"expected 403 got {status}: {body}"), "403"
        )[-1])
        mode_after = client.ok("GET", "/api/config")["approvals"]["mode"]
        check("rebinding attempt did not change the approval mode", lambda: (
            _assert(mode_after == "ask", f"mode became {mode_after}"), mode_after
        )[-1])
        # Settings bounds: an OOM-sized read limit must be rejected, not stored.
        status, body = client.raw(
            "PATCH", "/api/config", {"limits": {"max_file_read_bytes": 5_000_000_000}}
        )
        check("out-of-range settings are rejected", lambda: (
            _assert(status == 400, f"expected 400 got {status}: {body}"), "400"
        )[-1])

        print("\n== settings ==", flush=True)
        patched = client.ok("PATCH", "/api/config", {"model": {"temperature": 0.7, "api_key": "sk-e2e-secret"}})
        check("settings patch persists and hides the key", lambda: (
            _assert(patched["model"]["temperature"] == 0.7, patched["model"]),
            _assert(patched["model"]["api_key_set"] is True, "api_key_set"),
            _assert("sk-e2e-secret" not in json.dumps(patched), "API KEY LEAKED"),
            "key stored, redacted",
        )[-1])
        probe = client.ok("POST", "/api/provider/test")
        check("provider test reaches the endpoint", lambda: (
            _assert(probe["ok"] is True, probe), f"{probe.get('latency_ms')} ms"
        )[-1])
        stats = client.ok("GET", "/api/stats")
        check("stats endpoint reports RSS", lambda: (
            _assert(stats["rss_kb"] > 0, stats), f"{round(stats['rss_kb']/1024,1)} MiB"
        )[-1])

        print("\n== session management / UI ==", flush=True)
        renamed = client.patch(f"/api/sessions/{sid}", {"title": "renamed by e2e"})
        check("session rename", lambda: (
            _assert(renamed[1]["title"] == "renamed by e2e", renamed), "renamed"
        )[-1])
        extra = client.ok("POST", "/api/sessions", {"title": "delete me"})
        client.delete(f"/api/sessions/{extra['id']}")
        listing = client.ok("GET", "/api/sessions")
        check("session delete", lambda: (
            _assert(all(s["id"] != extra["id"] for s in listing["sessions"]), "still listed"), "gone"
        )[-1])
        status, body = client.raw("GET", "/")
        check("UI is served by the same process", lambda: (
            _assert(status == 200, status),
            _assert(b"<div id=\"app\"" in body or b"<script" in body, body[:120]),
            "html ok",
        )[-1])

        # Persistence across restart is checked separately by the restart helper.
        print("\n== restart / interrupted state ==", flush=True)
        client.post(f"/api/sessions/{sid}/messages", {"content": "[slow] interrupted run"})
        time.sleep(1.0)
        # A graceful signal cancels active runs (correct behaviour). The
        # "interrupted" state is for a hard crash / power loss, so kill -9 the
        # process to exercise WAL recovery plus run recovery on startup.
        server.kill()
        server.wait(timeout=15)
        server2 = subprocess.Popen(
            [str(binary)], cwd=str(workspace), env=env,
            stdout=open(tmp / "server2.log", "w"), stderr=subprocess.STDOUT,
        )
        try:
            client2 = Client(port)
            wait_http(client2.base + "/api/health", timeout=30)
            detail = client2.ok("GET", f"/api/sessions/{sid}")
            check("history survives restart and interrupted runs are marked", lambda: (
                _assert(len(detail["messages"]) > 3, len(detail["messages"])),
                _assert(any(r["status"] == "interrupted" for r in detail["runs"]),
                        [r["status"] for r in detail["runs"]]),
                "interrupted visible",
            )[-1])
        finally:
            server2.send_signal(signal.SIGINT)
            try:
                server2.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server2.kill()
        events.stop()

        report = {
            "passed": PASS,
            "failed": [{"name": n, "error": e} for n, e in FAIL],
            "skipped": SKIP,
            "events": len(events.events),
            "server_log": str(server_log),
            "mock_requests": sum(1 for _ in mock_log.read_text().splitlines()) if mock_log.exists() else 0,
        }
        (ROOT / "reports").mkdir(exist_ok=True)
        (ROOT / "reports" / "e2e.json").write_text(json.dumps(report, indent=2) + "\n")

        print(f"\n{len(PASS)} passed, {len(FAIL)} failed, {len(SKIP)} skipped", flush=True)
        if FAIL:
            print("\nFailures:", file=sys.stderr)
            for name, err in FAIL:
                print(f"  - {name}: {err}", file=sys.stderr)
            print(f"\nserver log: {server_log}", file=sys.stderr)
            if server_log.exists():
                print(server_log.read_text()[-4000:], file=sys.stderr)
        return 0 if not FAIL else 1
    finally:
        try:
            mock.terminate()
            mock.wait(timeout=5)
        except Exception:
            pass
        try:
            server.send_signal(signal.SIGINT)
            server.wait(timeout=5)
        except Exception:
            pass
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)
        else:
            print(f"kept temp dir: {tmp}")


def _assert(cond, detail):
    if not cond:
        raise AssertionError(str(detail))


if __name__ == "__main__":
    raise SystemExit(main())