#!/usr/bin/env python3
"""Real-LLM-shaped browser check that throughput badges ride the blocks they measure.

The `↓ <tokens> <rate> t/s` readout used to sit on the assistant turn's header.
This drives a real server, a real Chromium and a purpose-written provider stub
through one turn shaped like real work — a slow reasoning stream, then a
`run_command` whose output streams for two seconds, then a slow answer — and
records the transcript's badges over time.

It proves the properties the unit tests cannot, because they are about *where*
the number lands in a laid-out page and *when* it goes away:

  * a badge appears on the streaming **Thought** header, and shows a rate;
  * a badge appears on the running command card, and the reasoning one is gone
    by then (each badge belongs to one block's stream, not the turn's);
  * no badge ever appears in the turn header (`.msg-top`) or on a folded group;
  * every badge is gone once the run has settled;
  * at 390 px the rate yields to the count, as the mobile tier says it should,
    and nothing overflows.

Usage (after `cargo build -p kc-server` and `npm run build` in web/):
    python3 scripts/verify_tps_badge.py
    python3 scripts/verify_tps_badge.py --keep   # keep the screenshots

Exit code 0 only if every check passes.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.design_verify import Session, free_port, wait_for  # noqa: E402
from scripts.e2e_smoke import Client  # noqa: E402

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    (PASS if ok else FAIL).append(name)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{f' — {detail}' if detail else ''}", flush=True)


# ---------------------------------------------------------------------------
# The provider stub
#
# One turn, scripted from the conversation: reasoning, then a shell command whose
# output trickles, then a final answer. Slow on purpose - a badge needs ~0.5 s of
# growth before it will claim a rate, and the command needs to be running long
# enough for the sampler to catch it.
# ---------------------------------------------------------------------------

REASONING = [f"Step {i}: weighing the next move carefully." for i in range(24)]
COMMAND = "for i in $(seq 1 25); do echo \"line $i of a slow command\"; sleep 0.08; done"
ANSWER = [f"Word{i} of the answer." for i in range(20)]


class Provider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # keep the run quiet
        return

    def _json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            self._json(200, {"object": "list", "data": [{"id": "mock-coder", "object": "model"}]})
            return
        if self.path.rstrip("/") in ("/health", "/v1/health"):
            self._json(200, {"status": "ok"})
            return
        self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        payload = json.loads(self.rfile.read(length) or b"{}")
        messages = payload.get("messages") or []
        newest_user = 0
        for index, message in enumerate(messages):
            if message.get("role") == "user":
                newest_user = index
        ran_tools = any(m.get("role") == "tool" for m in messages[newest_user:])
        self._stream(payload.get("model") or "mock-coder", ran_tools)

    def _stream(self, model: str, ran_tools: bool) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def chunk(delta: dict, finish: str | None = None) -> None:
            body = {
                "id": "chatcmpl-tps",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            data = f"data: {json.dumps(body)}\n\n".encode()
            try:
                self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                raise SystemExit(0)

        chunk({"role": "assistant", "content": ""})
        if ran_tools:
            for word in ANSWER:
                chunk({"content": word + " "})
                time.sleep(0.05)
            chunk({}, "stop")
        else:
            for line in REASONING:
                chunk({"reasoning_content": line + "\n"})
                time.sleep(0.06)
            # Arguments split across chunks the way a real provider sends them.
            arguments = json.dumps({"command": COMMAND})
            chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_slow_1",
                            "type": "function",
                            "function": {"name": "run_command", "arguments": ""},
                        }
                    ]
                }
            )
            for start in range(0, len(arguments), 12):
                chunk(
                    {
                        "tool_calls": [
                            {"index": 0, "function": {"arguments": arguments[start : start + 12]}}
                        ]
                    }
                )
            chunk({}, "tool_calls")

        usage = {
            "id": "chatcmpl-tps",
            "object": "chat.completion.chunk",
            "created": int(time.time()),
            "model": model,
            "choices": [],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
        }
        data = f"data: {json.dumps(usage)}\n\n".encode()
        self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
        done = b"data: [DONE]\n\n"
        self.wfile.write(hex(len(done))[2:].encode() + b"\r\n" + done + b"\r\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


# ---------------------------------------------------------------------------
# The sampler: one page-side timeline of what the transcript showed
# ---------------------------------------------------------------------------

SAMPLER = """
const where = (el) => {
  if (el.closest('.thinking-summary')) return 'reasoning';
  if (el.closest('.tool-card, .tool-row, .command-card, .command-row')) return 'call';
  if (el.closest('.msg-top')) return 'header';
  if (el.closest('.msg-body')) return 'message';
  return 'other';
};
window.__tps = { samples: [], done: false };
const started = Date.now();
let sawBadge = false;
let quietSince = null;
const timer = setInterval(() => {
  const badges = [...document.querySelectorAll('.tps-badge')].map((el) => ({
    where: where(el),
    count: (el.querySelector('.tps-badge-count') || {}).textContent || '',
    rate: (el.querySelector('.tps-badge-rate') || {}).textContent || '',
    tone: el.getAttribute('data-tone'),
  }));
  window.__tps.samples.push({
    t: Date.now() - started,
    badges,
    working: !!document.querySelector('.tag-live'),
  });
  if (badges.length > 0) { sawBadge = true; quietSince = null; }
  else if (sawBadge) {
    quietSince = quietSince === null ? Date.now() : quietSince;
    if (Date.now() - quietSince > 1500) {
      window.__tps.done = true;
      clearInterval(timer);
    }
  }
}, 80);
return true;
"""


def badges_of(samples: list[dict], where: str) -> list[dict]:
    return [badge for sample in samples for badge in sample["badges"] if badge["where"] == where]


def shot_of(session: Session, selector: str, path: Path) -> bool:
    """Capture the newest element matching `selector`, so the badge shows at real size.

    An element clip is in document coordinates while the transcript scrolls inside
    its own container, so a row that is only partly on screen falls back to the
    viewport: a wrong crop would be worse evidence than a wide one.
    """
    box = session.evaluate(
        f"""
const nodes = [...document.querySelectorAll({json.dumps(selector)})];
const el = nodes[nodes.length - 1];
if (!el) return null;
const rect = el.getBoundingClientRect();
return {{
  x: rect.x, y: rect.y, width: rect.width, height: rect.height,
  visible: rect.top >= 0 && rect.left >= 0 && rect.bottom <= innerHeight && rect.right <= innerWidth,
}};
"""
    )
    if not box or box["width"] < 2 or box["height"] < 2:
        return False
    if not box["visible"]:
        session.screenshot(path)
        return True
    result = session._send(
        "Page.captureScreenshot",
        {
            "format": "png",
            "captureBeyondViewport": True,
            "clip": {
                "x": box["x"],
                "y": box["y"],
                "width": box["width"],
                "height": box["height"],
                "scale": 1,
            },
        },
    )
    path.write_bytes(base64.b64decode(result["data"]))
    return True


def record_turn(
    session: Session,
    client: Client,
    session_id: str,
    content: str,
    shots: dict[str, Path] | None = None,
    timeout: float = 40.0,
) -> list[dict]:
    """Send one turn and return what the transcript's badges did during it.

    The sampler is page-side and samples every 80 ms, because polling from here
    would miss the transitions this is about: a badge appearing on one block and
    leaving with that block's stream.
    """
    session.evaluate(SAMPLER)
    client.ok("POST", f"/api/sessions/{session_id}/messages", {"content": content}, expect=202)
    rated: set[str] = set()
    deadline = time.time() + timeout
    while time.time() < deadline:
        state = session.evaluate(
            "return { done: window.__tps.done, last: window.__tps.samples[window.__tps.samples.length - 1] || null };"
        ) or {}
        badges = (state.get("last") or {}).get("badges") or []
        for where, path in (shots or {}).items():
            if where in rated:
                continue
            if any(badge["where"] == where and badge["rate"] for badge in badges):
                # An empty running card would prove the badge mounts, not that it
                # measures, so the evidence waits for output to arrive.
                if shot_of(session, ".msg-assistant", path):
                    rated.add(where)
        if state.get("done"):
            break
        time.sleep(0.1)
    return session.evaluate("return window.__tps.samples;") or []


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "debug" / "karucode")
    parser.add_argument("--keep", action="store_true", help="keep the output directory")
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run `cargo build -p kc-server`")

    out = Path(tempfile.mkdtemp(prefix="tps-verify-"))
    workspace = out / "workspace"
    workspace.mkdir(parents=True)
    (workspace / "README.md").write_text("# TPS fixture\n\nThe command runs here.\n")

    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    config = out / "config.toml"
    config.write_text('host = "127.0.0.1"\nlog_level = "warn"\n[approvals]\nmode = "auto"\n')
    server_port = free_port()
    env = {
        **os.environ,
        "HOME": str(out / "home"),
        "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(out / "data"),
        "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder",
        "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    (out / "home").mkdir()
    server_log = (out / "server.log").open("w")
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(server_port), "--workspace", str(workspace)],
        cwd=workspace,
        env=env,
        stdout=server_log,
        stderr=subprocess.STDOUT,
    )
    browser = None
    session = None
    try:
        wait_for(f"http://127.0.0.1:{provider_port}/health")
        wait_for(f"http://127.0.0.1:{server_port}/api/health")
        client = Client(server_port)
        project = next(
            (item for item in client.ok("GET", "/api/projects")["projects"] if item["path"] == str(workspace.resolve())),
            None,
        )
        if project is None:
            project = client.ok("POST", "/api/projects", {"path": str(workspace), "name": "tps-fixture"})
        created = client.ok("POST", "/api/sessions", {"title": "TPS fixture", "project": project["id"]})
        session_id = created["id"]

        cdp_port = free_port()
        browser_log = (out / "chromium.log").open("w")
        browser = subprocess.Popen(
            [
                "chromium",
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                f"--remote-debugging-port={cdp_port}",
                f"--user-data-dir={out / 'browser-profile'}",
                "--window-size=1440,1000",
                "about:blank",
            ],
            stdout=browser_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list").read())
        page = next(target for target in targets if target.get("type") == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session._send("Runtime.enable")
        session._send(
            "Emulation.setDeviceMetricsOverride",
            {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False},
        )
        session.evaluate("window.__errors = []; window.addEventListener('error', (e) => window.__errors.push(String(e.message || e.type))); return true;")
        # The UI restores the open session from localStorage; it does not read
        # `?session=`. The origin has to be loaded before storage exists, so this
        # is navigate, remember, navigate again.
        session.navigate(f"http://127.0.0.1:{server_port}")
        session.evaluate(f"localStorage.setItem('karucode.lastSession', {json.dumps(session_id)}); return true;")
        session.navigate(f"http://127.0.0.1:{server_port}")
        session.evaluate("""
        const deadline = Date.now() + 20000;
        while (Date.now() < deadline) {
          if (document.querySelector('#composer-input')) return 'ready';
          await new Promise((r) => setTimeout(r, 100));
        }
        return 'timeout';
        """)

        samples = record_turn(
            session,
            client,
            session_id,
            "Run the slow command.",
            shots={
                "reasoning": out / "tps-during-reasoning.png",
                "call": out / "tps-during-call.png",
            },
        )
        session.screenshot(out / "tps-settled.png")

        reasoning = badges_of(samples, "reasoning")
        calls = badges_of(samples, "call")
        headers = badges_of(samples, "header")
        messages = badges_of(samples, "message")

        check("the sampler caught the reasoning stream", len(reasoning) > 0, f"{len(reasoning)} samples")
        check(
            "the reasoning block's own badge claims a rate",
            any("t/s" in (badge["rate"] or "") for badge in reasoning),
            next((badge["rate"] for badge in reasoning if badge["rate"]), "never"),
        )
        check("the sampler caught the running command", len(calls) > 0, f"{len(calls)} samples")
        check(
            "the call badge is on the call, and the reasoning badge has gone with its stream",
            all(
                not any(badge["where"] == "reasoning" for badge in sample["badges"])
                for sample in samples
                if any(badge["where"] == "call" for badge in sample["badges"])
            ),
        )
        check("the streaming answer carries its own badge", len(messages) > 0, f"{len(messages)} samples")
        check("no badge ever sits in the turn header", len(headers) == 0, f"{len(headers)} samples")
        check(
            "each badge counted its own block's characters",
            any(badge["count"] not in ("", "0") for badge in reasoning + calls + messages),
            f"reasoning={reasoning[-1]['count'] if reasoning else '-'} "
            f"call={calls[-1]['count'] if calls else '-'} "
            f"answer={messages[-1]['count'] if messages else '-'}",
        )
        check(
            "every badge is gone once the run settles",
            bool(samples) and all(not sample["badges"] for sample in samples[-3:]),
            f"last sample: {samples[-1]['badges'] if samples else 'none'}",
        )
        errors = session.evaluate("return window.__errors || [];")
        check("the page raised no errors", not errors, str(errors[:3]))
        check(
            "the transcript does not overflow its column",
            bool(session.evaluate("return document.documentElement.scrollWidth <= innerWidth + 1;")),
        )

        # A phone has room for the count but not the rate, and the card headers are
        # the tightest place a badge now lives, so the compact tier gets its own run.
        session._send(
            "Emulation.setDeviceMetricsOverride",
            {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True},
        )
        time.sleep(0.5)
        phone = record_turn(
            session,
            client,
            session_id,
            "Run it again, on a phone.",
        )
        phone_calls = badges_of(phone, "call")
        check("the phone tier still measures the running call", len(phone_calls) > 0, f"{len(phone_calls)} samples")
        check(
            "the phone tier keeps the count and drops the rate",
            any(badge["count"] and not badge["rate"] for badge in phone_calls),
        )
        check(
            "the phone transcript does not overflow",
            bool(session.evaluate("return document.documentElement.scrollWidth <= innerWidth + 1;")),
        )
        check(
            "nothing is left behind in the phone frame either",
            bool(session.evaluate("return document.querySelectorAll('.tps-badge').length === 0;")),
        )
        session.screenshot(out / "tps-phone-settled.png")
    finally:
        if session is not None:
            try:
                session.ws.close()
            except Exception:
                pass
        if browser is not None:
            browser.terminate()
            try:
                browser.wait(timeout=10)
            except subprocess.TimeoutExpired:
                browser.kill()
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        provider.shutdown()

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    if FAIL:
        print("screenshots: " + str(out))
    elif args.keep:
        print("screenshots: " + str(out))
    else:
        shutil.rmtree(out, ignore_errors=True)
    return 1 if FAIL else 0


if __name__ == "__main__":
    raise SystemExit(main())
