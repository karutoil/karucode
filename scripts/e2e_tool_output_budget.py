#!/usr/bin/env python3
"""End-to-end check for the per-plugin tool-output budget.

Runs the real `karucode` binary against a scripted OpenAI-compatible endpoint and
a real (Python) plugin whose one tool returns 60,000 bytes, and asserts the whole
rule the feature exists for:

1. with no grant, the result the model receives is cut at the global default
   (24,000 bytes);
2. `karucode plugin tool-output request …` (the CLI an agent runs) files the ask and
   grants **nothing** - the next result is still cut at the default;
3. the operator's answer, posted the way the browser posts it, takes effect on
   the very next tool call with no reload: the same result now arrives whole;
4. the answer is on the record (who, when, why) and a second request is refused;
5. clearing it puts the plugin back on the global default.

Everything is local: a temp data dir, no credentials, no network. Usage:

    cargo build -p kc-server
    python3 scripts/e2e_tool_output_budget.py [--binary target/debug/karucode]
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

GLOBAL_BYTES = 24_000
TOOL_BYTES = 60_000
GRANT_BYTES = 200_000

PLUGIN_NAME = "blob"
TOOL_NAME = f"plugin__{PLUGIN_NAME}__dump"

PLUGIN = r'''#!/usr/bin/env python3
"""A plugin whose one tool returns a large payload."""
import json
import sys

MANIFEST = {
    "name": "blob",
    "version": "0.1.0",
    "description": "Returns a body of bytes",
    "protocol": "1",
    "capabilities": ["tools"],
    "permissions": [],
    "hooks": [],
}


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        request = json.loads(line)
        method = request.get("method")
        request_id = request.get("id")
        params = request.get("params") or {}
        if method == "initialize":
            send({"jsonrpc": "2.0", "id": request_id, "result": MANIFEST})
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": [{
                "name": "dump",
                "description": "Return `bytes` characters of filler.",
                "parameters": {"type": "object",
                               "properties": {"bytes": {"type": "integer"}}},
            }]}})
        elif method == "tools/call":
            size = int((params.get("arguments") or {}).get("bytes", 60000))
            send({"jsonrpc": "2.0", "id": request_id,
                  "result": {"ok": True, "content": "B" * size}})
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": request_id, "result": None})
            return 0
        elif request_id is not None:
            send({"jsonrpc": "2.0", "id": request_id, "result": None})
    return 0


sys.exit(main())
'''


class Provider(BaseHTTPRequestHandler):
    """Call the plugin's tool once, then report how many bytes came back."""

    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.path.endswith("/models"):
            body = json.dumps({"data": [{"id": "e2e-model", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        tool_messages = [m for m in body.get("messages", []) if m.get("role") == "tool"]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        if tool_messages:
            content = tool_messages[-1].get("content") or ""
            self._chunk({"role": "assistant", "content": f"BYTES:{len(content)}"})
            self._chunk({}, "stop")
        else:
            self._chunk({"role": "assistant", "content": ""})
            self._chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_blob_1",
                            "type": "function",
                            "function": {
                                "name": TOOL_NAME,
                                "arguments": json.dumps({"bytes": TOOL_BYTES}),
                            },
                        }
                    ]
                }
            )
            self._chunk({}, "tool_calls")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _chunk(self, delta, finish=None):
        payload = {
            "id": "chatcmpl-e2e",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "e2e",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }
        self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(ROOT / "target/debug/karucode"))
    parser.add_argument("--provider-port", type=int, default=8921)
    parser.add_argument("--port", type=int, default=8922)
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    binary = Path(args.binary)
    if not binary.exists():
        print(f"missing binary {binary}; run `cargo build -p kc-server` first")
        return 2

    api = f"http://127.0.0.1:{args.port}"
    provider = ThreadingHTTPServer(("127.0.0.1", args.provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    root = Path(tempfile.mkdtemp(prefix="kc-e2e-budget-"))
    workspace = root / "ws"
    data_dir = root / "data"
    workspace.mkdir()
    plugin_dir = root / "plugin"
    plugin_dir.mkdir()
    plugin_path = plugin_dir / "blob.py"
    plugin_path.write_text(PLUGIN)
    # One source only: the host fails a name that appears in both the config file
    # and `<data_dir>/plugins/`, and that would be a different test.
    # A provider profile, not the flat env override: `context_window` is only
    # carried through a model entry, and the window decides how much of a result
    # a *turn* may carry. Without a declared window the harness assumes a small
    # one, and the turn's share - not the per-result ceiling under test - is what
    # would cut the result.
    config = root / "config.toml"
    config.write_text(
        "[[providers]]\n"
        'id = "e2e"\n'
        f'base_url = "http://127.0.0.1:{args.provider_port}/v1"\n'
        'api_key = "e2e-key"\n'
        'models = [{ id = "e2e-model", context_window = 200000 }]\n'
        'active_provider = "e2e"\n'
        'active_model = "e2e-model"\n'
        "\n"
        f'[[plugins]]\nname = "{PLUGIN_NAME}"\n'
        f'command = ["python3", "{plugin_path}"]\nenabled = true\n'
    )

    env = dict(os.environ)
    env.update(
        {
            "KARUCODE_CONFIG": str(config),
            # Plugin tools are `external`: `auto` lets the call run so the test is
            # about truncation, not about waiting on an approval card.
            "KARUCODE_APPROVAL_MODE": "auto",
            "KARUCODE_LOG": "warn",
            "KARUCODE_MODELS_DEV_URL": f"http://127.0.0.1:{args.provider_port}/models.json",
        }
    )
    server = subprocess.Popen(
        [str(binary), "--workspace", str(workspace), "--data-dir", str(data_dir),
         "--port", str(args.port)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    def get(path):
        with urllib.request.urlopen(api + path, timeout=10) as response:
            return json.loads(response.read() or b"{}")

    def post(path, body):
        request = urllib.request.Request(api + path, data=json.dumps(body).encode(), method="POST")
        request.add_header("Content-Type", "application/json")
        request.add_header("X-Karucode-Client", "web")
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.loads(response.read() or b"{}")

    def delete(path):
        request = urllib.request.Request(api + path, method="DELETE")
        request.add_header("X-Karucode-Client", "web")
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.loads(response.read() or b"{}")

    def wait_for(predicate, timeout=None):
        deadline = time.time() + (timeout or args.timeout)
        while time.time() < deadline:
            try:
                value = predicate()
            except Exception:
                value = None
            if value:
                return value
            time.sleep(0.4)
        return None

    def run_once(title):
        """One session: the model calls the tool, and reports the bytes it saw.

        Two numbers come back, and they answer different questions:

        * `stored` is the length of the persisted tool message - the bytes the
          host kept for the model. This is the per-result ceiling under test.
        * `seen` is what the model reported receiving. It is additionally bounded
          by the conversation window (a turn gets at most half of it), so it is
          evidence the result reached the model, not a measurement of the ceiling.
        """
        session = post("/api/sessions", {"title": title})
        session_id = session["id"]
        post(f"/api/sessions/{session_id}/messages", {"content": "dump the blob"})

        def finished():
            detail = get(f"/api/sessions/{session_id}")
            runs = detail.get("runs") or []
            return detail if runs and runs[-1]["status"] in ("completed", "failed", "cancelled") else None

        detail = wait_for(finished)
        if not detail:
            return None, "the run did not finish in time"
        if (detail.get("runs") or [{}])[-1]["status"] != "completed":
            return None, f"the run ended {(detail.get('runs') or [{}])[-1]['status']}"
        texts = [m.get("content") or "" for m in detail["messages"] if m.get("role") == "assistant"]
        seen = next((t for t in texts if t.startswith("BYTES:")), None)
        if seen is None:
            return None, "the model never reported a byte count"
        stored = [len(m.get("content") or "") for m in detail["messages"] if m.get("role") == "tool"]
        if not stored:
            return None, "no tool result was persisted"
        return {"seen": int(seen.split(":", 1)[1]), "stored": stored[-1]}, None

    checks = {}
    try:
        if not wait_for(lambda: get("/api/health").get("status") == "ok"):
            print("server did not become healthy")
            return 1
        if not wait_for(lambda: any(p["name"] == PLUGIN_NAME and p["status"] == "ready"
                                    for p in get("/api/plugins")["plugins"])):
            print("the plugin did not become ready")
            return 1

        # 1. No grant: the global default cuts the result.
        first, error = run_once("no grant")
        if error:
            print(f"FAIL  first run: {error}")
            return 1
        checks["an ungranted plugin is cut at the global default"] = (
            GLOBAL_BYTES - 2_000 < first["stored"] <= GLOBAL_BYTES
        )
        print(f"      first result: {first['stored']} bytes stored, "
              f"{first['seen']} seen (global {GLOBAL_BYTES})")

        # 2. The agent asks, through the real CLI, and the operator answers.
        cli = [
            str(binary), "plugin", "tool-output", "request", PLUGIN_NAME,
            "--bytes", str(GRANT_BYTES),
            "--reason", "the whole feature bundle is the answer",
            "--wait", "60",
            "--url", api,
            "--config", str(config),
        ]
        cli_result = {}

        def ask():
            finished = subprocess.run(cli, env=env, capture_output=True, text=True, timeout=120)
            cli_result["code"] = finished.returncode
            cli_result["out"] = finished.stdout + finished.stderr

        asker = threading.Thread(target=ask)
        asker.start()
        pending = wait_for(
            lambda: get(f"/api/plugins/{PLUGIN_NAME}/tool-output").get("pending"), timeout=30
        )
        checks["the request is filed and waits for an answer"] = bool(pending)
        if pending:
            print(f"      pending ask: {pending['bytes']} bytes - {pending['reason']}")

        # Nothing changes while it is unanswered.
        during, _ = run_once("pending grant")
        checks["an unanswered request grants nothing"] = (
            during is not None and during["stored"] <= GLOBAL_BYTES
        )

        view = post(
            f"/api/plugins/{PLUGIN_NAME}/tool-output/decision",
            {"decision": "grant", "channel": "web", "reason": "agreed"},
        )
        checks["the answer is recorded with who and when"] = (
            view["decision"]["decided_by"] == "operator"
            and view["decision"]["channel"] == "web"
            and bool(view["decision"]["decided_at"])
        )
        checks["the effective budget is the granted one"] = view["effective_bytes"] == GRANT_BYTES
        asker.join(timeout=120)
        checks["the CLI reports the grant"] = (
            cli_result.get("code") == 0 and "Granted" in cli_result.get("out", "")
        )

        # 3. The very next tool call is no longer cut at the default.
        second, error = run_once("granted")
        if error:
            print(f"FAIL  second run: {error}")
            return 1
        checks["a granted plugin keeps the whole result"] = second["stored"] == TOOL_BYTES
        print(f"      second result: {second['stored']} bytes stored, "
              f"{second['seen']} seen (granted {GRANT_BYTES})")

        # 4. Listed, and a second ask is refused.
        listed = next(p for p in get("/api/plugins")["plugins"] if p["name"] == PLUGIN_NAME)
        checks["the listing shows the effective budget"] = (
            listed["tool_output"]["effective_bytes"] == GRANT_BYTES
            and listed["tool_output"]["granted"] is True
        )
        try:
            post(
                f"/api/plugins/{PLUGIN_NAME}/tool-output/request",
                {"bytes": GRANT_BYTES, "reason": "again"},
            )
            checks["a second ask is refused"] = False
        except urllib.error.HTTPError as error:
            checks["a second ask is refused"] = error.code == 409

        # 5. Clearing puts it back on the global default.
        cleared = delete(f"/api/plugins/{PLUGIN_NAME}/tool-output")
        checks["clearing restores the global default"] = (
            cleared["effective_bytes"] == GLOBAL_BYTES and cleared.get("decision") is None
        )
        third, error = run_once("cleared")
        if error:
            print(f"FAIL  third run: {error}")
            return 1
        checks["the cleared plugin is cut again"] = third["stored"] <= GLOBAL_BYTES

        for name, ok in checks.items():
            print(("PASS  " if ok else "FAIL  ") + name)
        if not all(checks.values()):
            return 1
        print("ok: per-plugin tool-output budget verified end to end")
        return 0
    finally:
        server.terminate()
        try:
            output, _ = server.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            output = ""
        if output and (os.environ.get("KARUCODE_LOG") or "").lower() == "debug":
            print(output)


if __name__ == "__main__":
    sys.exit(main())