#!/usr/bin/env python3
"""Deterministic OpenAI-compatible mock server for karucode end-to-end tests.

No third-party dependencies. Serves:

  GET  /v1/models
  POST /v1/chat/completions   (streaming and non-streaming)

Behaviour is scripted from the conversation so the agent loop can be exercised
without a real provider:

  * first model turn (no tool message yet) -> one tool call from the prompt hint
  * after a tool result                    -> final text summary

Prompt hints (in the user message):
  [tool:list_dir]        call list_dir on "."
  [tool:read_file]       call read_file on README.md
  [tool:run]             call run_command "echo karucode-e2e"
  [tool:multi]           call list_dir + git_status in one turn
  [tool:bad]             call a tool that does not exist
  [tool:crash]           call run_command "sleep 5" (cancellation/timeout)
  [tool:agent]           spawn a subagent through the delegate tool
  [tool:plugin]          call the example-hello plugin tool
  [tool:mcp]             call an MCP tool from the mock MCP server
  [text]                 reply with plain text only (no tools)
  [fail]                 return HTTP 500
  [slow]                 sleep 30s before answering (cancellation tests)

Every request is appended to the log file given by --log (JSONL).
"""

import argparse
import json
import os
import sys
import threading
import time


class ClientGone(Exception):
    """Raised when the client closed the stream mid-response."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG_LOCK = threading.Lock()


def now_ms() -> int:
    return int(time.time() * 1000)


def log_request(path: str, body, extra: dict | None = None) -> None:
    if not ARGS.log:
        return
    record = {"ts": now_ms(), "path": path, "body": body}
    if extra:
        record.update(extra)
    with LOG_LOCK:
        with open(ARGS.log, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(record, ensure_ascii=False) + "\n")


def textual_content(message) -> str:
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            part.get("text", "")
            for part in content
            if isinstance(part, dict) and part.get("type") == "text"
        )
    return ""


def decide(messages):
    """Return ('text', str) or ('tools', [tool_call...]) ."""
    last_user = ""
    last_user_index = 0
    for index, message in enumerate(messages):
        if message.get("role") == "user":
            last_user = textual_content(message)
            last_user_index = index
    # Only tool results produced *after* the newest user message mean this turn
    # already ran its tools; results from earlier turns must not suppress a new
    # tool call.
    has_tool_result = any(
        m.get("role") == "tool" for m in messages[last_user_index:]
    )

    if "[fail]" in last_user:
        return ("fail", None)

    if has_tool_result or "[text]" in last_user:
        return (
            "text",
            "Done. The mock provider finished the task. "
            f"Conversation had {len(messages)} messages.",
        )

    def call(call_id, name, args):
        return {
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)},
        }

    if "[tool:multi]" in last_user:
        return (
            "tools",
            [
                call("call_multi_1", "list_dir", {"path": "."}),
                call("call_multi_2", "git_status", {}),
            ],
        )
    if "[tool:run]" in last_user:
        return ("tools", [call("call_run_1", "run_command", {"command": "echo karucode-e2e"})])
    if "[tool:read_file]" in last_user:
        return ("tools", [call("call_read_1", "read_file", {"path": "README.md", "limit": 20})])
    if "[tool:bad]" in last_user:
        return ("tools", [call("call_bad_1", "definitely_not_a_tool", {})])
    if "[tool:crash]" in last_user:
        return ("tools", [call("call_crash_1", "run_command", {"command": "sleep 5"})])
    if "[tool:agent]" in last_user:
        return (
            "tools",
            [
                call(
                    "call_agent_1",
                    "spawn_agent",
                    {
                        "task": "[text] Subagent reporting: the delegated work is complete.",
                        "name": "e2e-helper",
                        "role": "tester",
                        "owned_paths": ["docs/**"],
                    },
                )
            ],
        )
    if "[tool:plugin]" in last_user:
        return (
            "tools",
            [call("call_plugin_1", "plugin__example-hello__hello", {"name": "e2e"})],
        )
    if "[tool:mcp]" in last_user:
        return ("tools", [call("call_mcp_1", "mcp__mock__echo", {"text": "hi"})])

    # Default: exercise the loop with a real read of the workspace listing.
    return ("tools", [call("call_default_1", "list_dir", {"path": "."})])


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # keep test output clean
        if ARGS.verbose:
            sys.stderr.write("mock: " + (fmt % args) + "\n")

    def _json(self, status: int, payload) -> None:
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _authorized(self) -> bool:
        if not ARGS.require_key:
            return True
        header = self.headers.get("Authorization", "")
        return header == f"Bearer {ARGS.require_key}"

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            if not self._authorized():
                self._json(401, {"error": {"message": "invalid api key", "type": "auth"}})
                return
            self._json(200, {"object": "list", "data": [{"id": "mock-coder", "object": "model"}]})
            return
        if self.path.rstrip("/") in ("/health", "/v1/health"):
            self._json(200, {"status": "ok"})
            return
        self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self._json(400, {"error": {"message": "invalid json"}})
            return

        if self.path.rstrip("/") not in ("/v1/chat/completions", "/chat/completions"):
            self._json(404, {"error": {"message": "not found"}})
            return

        log_request(
            self.path,
            body,
            extra={
                "authorization_present": bool(self.headers.get("Authorization")),
                "stream": bool(body.get("stream")),
            },
        )

        if not self._authorized():
            self._json(401, {"error": {"message": "invalid api key", "type": "auth"}})
            return

        messages = body.get("messages") or []
        mode, payload = decide(messages)

        if mode == "fail":
            self._json(500, {"error": {"message": "mock upstream failure", "type": "server_error"}})
            return

        model = body.get("model") or "mock-coder"
        last_user = ""
        for message in messages:
            if message.get("role") == "user":
                last_user = textual_content(message)
        if "[slow]" in last_user:
            time.sleep(30)

        if body.get("stream"):
            self._stream(model, mode, payload, messages)
        else:
            self._complete(model, mode, payload, messages)

    def _complete(self, model, mode, payload, messages):
        message = {"role": "assistant", "content": None}
        finish = "stop"
        if mode == "tools":
            message["content"] = "Working on it."
            message["tool_calls"] = payload
            finish = "tool_calls"
        else:
            message["content"] = payload
        self._json(
            200,
            {
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "message": message, "finish_reason": finish}],
                "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
            },
        )

    def _stream(self, model, mode, payload, messages):
        try:
            self._stream_inner(model, mode, payload)
        except ClientGone:
            # The agent cancelled the run and hung up; expected in tests.
            pass

    def _stream_inner(self, model, mode, payload):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        # Chunked encoding keeps the stream open without a Content-Length.
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def send(obj):
            data = f"data: {json.dumps(obj)}\n\n".encode()
            try:
                self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                raise ClientGone()

        def base(delta, finish=None):
            return {
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }

        send(base({"role": "assistant", "content": ""}))

        if mode == "tools":
            # Text first, then tool calls split across chunks on purpose.
            for token in ["Working", " on", " it."]:
                send(base({"content": token}))
                time.sleep(0.01)
            for index, call in enumerate(payload):
                send(
                    base(
                        {
                            "tool_calls": [
                                {
                                    "index": index,
                                    "id": call["id"],
                                    "type": "function",
                                    "function": {"name": call["function"]["name"], "arguments": ""},
                                }
                            ]
                        }
                    )
                )
                args = call["function"]["arguments"]
                for i in range(0, len(args), 7):
                    send(
                        base(
                            {
                                "tool_calls": [
                                    {"index": index, "function": {"arguments": args[i : i + 7]}}
                                ]
                            }
                        )
                    )
            send(base({}, "tool_calls"))
        else:
            for token in payload.split(" "):
                send(base({"content": token + " "}))
                time.sleep(0.005)
            send(base({}, "stop"))

        send(
            {
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [],
                "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
            }
        )
        data = b"data: [DONE]\n\n"
        self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


ARGS = argparse.Namespace(log=None, require_key=None, verbose=False)


def main() -> int:
    global ARGS
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=4010)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--log", default=None, help="append request logs as JSONL")
    parser.add_argument("--require-key", default=None, help="reject requests without this bearer token")
    parser.add_argument("--verbose", action="store_true")
    ARGS = parser.parse_args()

    server = ThreadingHTTPServer((ARGS.host, ARGS.port), Handler)
    server.daemon_threads = True
    print(json.dumps({"mock_openai": f"http://{ARGS.host}:{ARGS.port}/v1", "pid": os.getpid()}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())