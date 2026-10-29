#!/usr/bin/env python3
"""End-to-end check for karucode's self-documentation.

Starts the real server against a scripted OpenAI-compatible endpoint, asks a
question about karucode itself, and asserts that:

* the model can see and call `karucode_docs`;
* a tool call split across two SSE chunks is reassembled;
* the documentation comes back through the tool result (with provenance);
* the model receives that result;
* no approval was requested (the tool is read-only).

Two provider calls, no credentials, no network. Usage:

    cargo build -p kc-server
    python3 scripts/e2e_docs.py [--binary target/debug/karucode]
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Answers the two provider calls. The first streams a tool call for
# `karucode_docs` in two fragments (argument reassembly); the second, when the
# tool result is in the conversation, echoes its beginning back as the answer.
class Provider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        tool_messages = [m for m in body.get("messages", []) if m.get("role") == "tool"]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()

        if tool_messages:
            preview = (tool_messages[-1].get("content") or "")[:160].replace("\n", " ")
            self._chunk({"role": "assistant", "content": "FINAL:" + preview})
            self._chunk({}, "stop")
        else:
            self._chunk({"role": "assistant", "content": ""})
            self._chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_docs_1",
                            "type": "function",
                            "function": {
                                "name": "karucode_docs",
                                "arguments": '{"action":"read",',
                            },
                        }
                    ]
                }
            )
            self._chunk(
                {
                    "tool_calls": [
                        {"index": 0, "function": {"arguments": '"doc":"mcp"}'}}
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
    parser.add_argument("--provider-port", type=int, default=8911)
    parser.add_argument("--port", type=int, default=8912)
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    if not Path(args.binary).exists():
        print(f"missing binary {args.binary}; run `cargo build -p kc-server` first")
        return 2

    api = f"http://127.0.0.1:{args.port}"
    provider = ThreadingHTTPServer(("127.0.0.1", args.provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    workspace = tempfile.mkdtemp(prefix="kc-e2e-docs-ws-")
    data_dir = tempfile.mkdtemp(prefix="kc-e2e-docs-data-")
    env = dict(os.environ)
    env.update(
        {
            "KARUCODE_BASE_URL": f"http://127.0.0.1:{args.provider_port}/v1",
            "KARUCODE_API_KEY": "e2e-key",
            "KARUCODE_MODEL": "e2e-model",
            "KARUCODE_APPROVAL_MODE": "ask",
            "KARUCODE_LOG": "warn",
            # Keep startup's catalog fetch off the network.
            "KARUCODE_MODELS_DEV_URL": f"http://127.0.0.1:{args.provider_port}/models.json",
        }
    )
    server = subprocess.Popen(
        [args.binary, "--workspace", workspace, "--data-dir", data_dir, "--port", str(args.port)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    def get(path):
        with urllib.request.urlopen(api + path, timeout=10) as response:
            return json.loads(response.read() or b"{}")

    def post(path, body):
        request = urllib.request.Request(
            api + path, data=json.dumps(body).encode(), method="POST"
        )
        request.add_header("Content-Type", "application/json")
        request.add_header("X-Karucode-Client", "web")
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.loads(response.read() or b"{}")

    def wait_for(predicate):
        deadline = time.time() + args.timeout
        while time.time() < deadline:
            try:
                value = predicate()
            except Exception:
                value = None
            if value:
                return value
            time.sleep(0.4)
        return None

    try:
        if not wait_for(lambda: get("/api/health").get("status") == "ok"):
            print("server did not become healthy")
            return 1

        session = post("/api/sessions", {"title": "docs e2e"})
        session_id = session["id"]
        post(f"/api/sessions/{session_id}/messages", {"content": "How do I configure an MCP server?"})

        def finished():
            detail = get(f"/api/sessions/{session_id}")
            runs = detail.get("runs") or []
            return detail if runs and runs[-1]["status"] in ("completed", "failed", "cancelled") else None

        detail = wait_for(finished)
        if not detail:
            print("run did not finish in time")
            return 1

        messages = detail["messages"]
        called = [c["name"] for m in messages for c in (m.get("tool_calls") or [])]
        tool_text = "\n".join(m.get("content") or "" for m in messages if m.get("role") == "tool")
        final_text = "".join(
            m.get("content") or "" for m in messages if m.get("role") == "assistant"
        )

        checks = {
            "the model called karucode_docs": "karucode_docs" in called,
            "the MCP doc came back": "mcp_servers" in tool_text,
            "the doc carries its source": "Source of truth" in tool_text,
            "split tool arguments were reassembled": "Connect MCP servers" in tool_text,
            "the result reached the model": "FINAL:" in final_text
            and "Connect MCP servers" in final_text,
            "no approval was requested": not detail.get("approvals"),
        }
        for name, ok in checks.items():
            print(("PASS  " if ok else "FAIL  ") + name)
        if not all(checks.values()):
            print("\n--- tool result ---\n" + tool_text[:2000])
            return 1
        print("ok: karucode_docs verified end to end")
        return 0
    finally:
        server.send_signal(signal.SIGTERM)
        try:
            server.wait(timeout=15)
        except subprocess.TimeoutExpired:
            server.kill()
        provider.shutdown()


if __name__ == "__main__":
    sys.exit(main())