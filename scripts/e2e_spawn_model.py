#!/usr/bin/env python3
"""End-to-end check for a subagent spawn's `model` argument.

Starts the real server against a scripted OpenAI-compatible endpoint that
serves TWO provider profiles, and asserts the rule the runtime promises:

* a requested model that any profile of this install declares is accepted,
  and the child actually runs on the profile that declares it - the request
  hits the *other* profile's endpoint carrying the requested id;
* an id no profile declares (`sonnet`, or the name of the agent being
  spawned) is refused as a tool error naming real ids, before any child
  starts: no agent row, no provider call with the bad id;
* the run still completes in both cases (a refusal is a tool error, not a
  failed run).

No credentials, no network. Usage:

    cargo build -p kc-server
    python3 scripts/e2e_spawn_model.py [--binary target/debug/karucode]
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

# Every chat request both endpoints saw: (path, model id).
SEEN = []
SEEN_LOCK = threading.Lock()


class Provider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        path = self.path
        with SEEN_LOCK:
            SEEN.append((path, body.get("model", "")))
        tool_messages = [m for m in body.get("messages", []) if m.get("role") == "tool"]
        wants_bad_model = any(
            (m.get("role") == "user" and "REFUSE" in (m.get("content") or ""))
            for m in body.get("messages", [])
        )
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()

        if path.startswith("/alt/"):
            # The child's loop: it only ever lands here if the spawn resolved
            # the requested model onto the profile that declares it.
            self._chunk({"role": "assistant", "content": "CHILD OK"})
            self._chunk({}, "stop")
        elif tool_messages:
            preview = (tool_messages[-1].get("content") or "")[:120].replace("\n", " ")
            self._chunk({"role": "assistant", "content": "FINAL:" + preview})
            self._chunk({}, "stop")
        elif wants_bad_model:
            self._spawn_call("sonnet")
        else:
            self._spawn_call("od/child-model")

    def _spawn_call(self, model):
        arguments = json.dumps({"task": "say CHILD OK", "model": model})
        self._chunk({"role": "assistant", "content": ""})
        self._chunk(
            {
                "tool_calls": [
                    {
                        "index": 0,
                        "id": "call_spawn_1",
                        "type": "function",
                        "function": {"name": "spawn_agent", "arguments": arguments},
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


CONFIG = """
active_provider = "main"
active_model = "e2e-main"

[[providers]]
id = "main"
name = "Main e2e"
provider = "openai_compatible"
base_url = "http://127.0.0.1:{provider_port}/v1"
api_key = "e2e-key"

  [[providers.models]]
  id = "e2e-main"
  name = "Main model"
  context_window = 128000
  tool_call = true

  [[providers.models]]
  id = "e2e-main-2"
  context_window = 128000
  tool_call = true

[[providers]]
id = "alt"
name = "Alt profile"
provider = "openai_compatible"
base_url = "http://127.0.0.1:{provider_port}/alt"
api_key = "e2e-alt-key"

  [[providers.models]]
  id = "od/child-model"
  name = "Child model on another profile"
  context_window = 128000
  tool_call = true
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(ROOT / "target/debug/karucode"))
    parser.add_argument("--provider-port", type=int, default=8931)
    parser.add_argument("--port", type=int, default=8932)
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()

    if not Path(args.binary).exists():
        print(f"missing binary {args.binary}; run `cargo build -p kc-server` first")
        return 2

    api = f"http://127.0.0.1:{args.port}"
    provider = ThreadingHTTPServer(("127.0.0.1", args.provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    workspace = tempfile.mkdtemp(prefix="kc-e2e-spawn-ws-")
    data_dir = tempfile.mkdtemp(prefix="kc-e2e-spawn-data-")
    config_path = Path(workspace) / "config.toml"
    config_path.write_text(CONFIG.format(provider_port=args.provider_port))

    env = dict(os.environ)
    env.update(
        {
            # The config file names both profiles; none of the env model
            # variables are set, so the reserved env profile never overrides.
            "KARUCODE_CONFIG": str(config_path),
            "KARUCODE_APPROVAL_MODE": "auto",
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

    def run_session(title, content):
        session = post("/api/sessions", {"title": title})
        session_id = session["id"]
        post(f"/api/sessions/{session_id}/messages", {"content": content})

        def finished():
            detail = get(f"/api/sessions/{session_id}")
            runs = detail.get("runs") or []
            return detail if runs and runs[-1]["status"] in ("completed", "failed", "cancelled") else None

        detail = wait_for(finished)
        if not detail:
            print(f"run for {title!r} did not finish in time")
            return None
        return detail

    try:
        if not wait_for(lambda: get("/api/health").get("status") == "ok"):
            print("server did not become healthy")
            return 1

        # --- case 1: a model declared on ANOTHER profile routes onto it -----
        detail = run_session("cross-profile", "spawn a subagent on the child model")
        if not detail:
            return 1
        routing_status = (detail.get("runs") or [{}])[-1].get("status")
        agents = detail.get("agents") or []
        with SEEN_LOCK:
            seen = list(SEEN)
        alt_hits = [model for path, model in seen if path.startswith("/alt/")]

        checks = {
            "the run completed": routing_status == "completed",
            "the child ran on the other profile's endpoint": len(alt_hits) == 1,
            "the child request carried the requested id": alt_hits[:1] == ["od/child-model"],
            "the agent row names the requested model": any(
                a.get("model") == "od/child-model" for a in agents
            ),
        }

        # --- case 2: an id no profile declares is refused before starting ---
        detail2 = run_session("refusal", "REFUSE: spawn a subagent on a made-up model")
        if not detail2:
            return 1
        refusal_status = (detail2.get("runs") or [{}])[-1].get("status")
        tool_text = "\n".join(
            m.get("content") or "" for m in detail2.get("messages", []) if m.get("role") == "tool"
        )
        with SEEN_LOCK:
            seen2 = list(SEEN)
        bad_calls = [model for _path, model in seen2 if model == "sonnet"]

        checks.update(
            {
                "the refused run still completed": refusal_status == "completed",
                "the refusal names the bad id": "unknown model `sonnet`" in tool_text,
                "the refusal names a real alternative": "od/child-model" in tool_text,
                "no child was started for the bad id": not (detail2.get("agents") or []),
                "no provider call carried the bad id": not bad_calls,
            }
        )

        for name, ok in checks.items():
            print(("PASS  " if ok else "FAIL  ") + name)
        if not all(checks.values()):
            print("\n--- tool results (refusal session) ---\n" + tool_text[:2000])
            print("\n--- provider calls ---\n" + "\n".join(f"{p} {m}" for p, m in seen2))
            return 1
        print("ok: spawn model validation verified end to end")
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
