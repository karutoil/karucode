#!/usr/bin/env python3
"""Reproduce the transcript's scroll/layout flip-flop in a real Chromium.

Reported from real use: on a conversation longer than the screen, scrolling down
"jumps it up", the bottom cannot be reached, "Jump to latest" does not land, and
rows flash. Intermittent, "as if some sizing layout event is flip-flopping" -
which points at the windowed transcript's measure -> padding -> correction loop,
not at the scroll handlers alone.

The unit tests cannot see this: jsdom has no layout, so every height is 0 and
every estimate is right. So this drives the built bundle in Chromium over raw
CDP against a real kc-server, samples the scroller *every frame* while a real
`Input.synthesizeScrollGesture` moves it, and reports:

  * scrollTop backsteps during a downward gesture (a jump up under the wheel);
  * whether the bottom is reachable at all;
  * whether "Jump to latest" lands and stays landed;
  * layout churn - rows mounted/unmounted, padding rewritten, scrollHeight
    rewritten - per frame, which is what "sizing layout event flip-flopping"
    describes;
  * the same during a live run that streams reasoning, with the theme's thinking
    mode set to `expanded` (the setting the report blamed).

Run: cargo build -p kc-server --bin karucode && (cd web && npm run build)
     python3 scripts/verify_transcript_scroll.py [--keep] [--quiet]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import ROOT, Session, free_port, http_json, wait_for

PASS: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{('  -> ' + detail) if detail else ''}", flush=True)
    if ok:
        PASS.append(name)
    return ok


# ---------------------------------------------------------------------------
# Stub provider: calm by default, a long reasoning stream on demand
# ---------------------------------------------------------------------------

STATE = {"tall_reasoning": False}


def _chunk(delta: dict, finish: str | None = None) -> dict:
    return {
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }


def sse_line(payload: dict) -> bytes:
    return f"data: {json.dumps(payload)}\n\n".encode()


class Provider(BaseHTTPRequestHandler):
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
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        try:
            request = json.loads(raw or b"{}")
        except ValueError:
            request = {}
        messages = request.get("messages") or []
        newest = ""
        for message in reversed(messages):
            if message.get("role") == "user":
                newest = message.get("content") or ""
                break

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def write(payload: dict) -> None:
            try:
                self.wfile.write(sse_line(payload))
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                raise SystemExit(0)

        if STATE["tall_reasoning"] and "THINK-STREAM" in newest:
            # ~300 reasoning deltas: a thought that grows a screenful a second,
            # which is the shape of a real reasoning stream on a slow model.
            for index in range(300):
                write(_chunk({"reasoning_content": f"thought {index} " + "step " * 8}))
                time.sleep(0.01)
            for index in range(160):
                write(_chunk({"content": f"answer line {index} " + "word " * 6 + "\n\n"}))
                time.sleep(0.012)
            write(_chunk({}, "stop"))
        else:
            write(_chunk({"content": "Quick answer. Done."}))
            write(_chunk({}, "stop"))
        try:
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


# ---------------------------------------------------------------------------
# Seeded history: long enough to window, tall enough to scroll
# ---------------------------------------------------------------------------

SEED_ROWS = 60
NEWEST = "SEED-NEWEST-MARKER"


CODE_BLOCK = "\n".join(
    f"    line_{index} = compute(alpha, beta) + {index}" for index in range(24)
)

TOOL_OUTPUT = "\n".join(
    f"test result line {index}: ok. 200 passed; 0 failed" for index in range(40)
)

TOOL_CALL = [
    {
        "id": "call_seed_{index}",
        "type": "function",
        "function": {"name": "run_command", "arguments": json.dumps({"command": "cargo test -p kc-store"})},
    }
]


def seed_message(conn: sqlite3.Connection, values: dict) -> None:
    """Insert one message row, naming the columns the transcript reads."""
    columns = {
        "session_id": None,
        "run_id": None,
        "agent_id": None,
        "role": "assistant",
        "content": "",
        "tool_calls": None,
        "tool_call_id": None,
        "name": None,
        "images": "[]",
        "reasoning_content": None,
        "created_at": 0,
    }
    columns.update(values)
    if columns["tool_calls"] is not None:
        columns["tool_calls"] = json.dumps(columns["tool_calls"])
    keys = ",".join(columns)
    marks = ",".join("?" for _ in columns)
    conn.execute(f"INSERT INTO messages ({keys}) VALUES ({marks})", list(columns.values()))


def seed_transcript(db: Path, session_id: str) -> None:
    """A transcript shaped like a real one: prose, lists, fences, tool turns.

    The window's estimate disagreement is what it has to correct for, so a seed
    of identical short paragraphs would hide the bug. Every shape the estimator
    models badly gets a row here: a single long paragraph (its line estimate is
    far off), a fenced block (monospace, taller than the prose model), a bullet
    list, and a tool turn whose output card is nothing like 46px.
    """
    now = int(time.time() * 1000) - 600_000
    with sqlite3.connect(db, timeout=10) as conn:
        for index in range(SEED_ROWS):
            role = "user" if index % 2 == 0 else "assistant"
            marker = f"{NEWEST} " if index == SEED_ROWS - 1 else ""
            at = now + index * 1000
            if role == "user":
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "user",
                        "content": marker + f"question {index}: " + "why is the transcript jumpy when I scroll? " * 3,
                        "created_at": at,
                    },
                )
            elif index % 5 == 1:
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "assistant",
                        "content": marker + " ".join(
                            f"one long paragraph {index} with no line breaks at all " for _ in range(60)
                        ),
                        "reasoning_content": "thinking about the shape of this answer",
                        "created_at": at,
                    },
                )
            elif index % 5 == 2:
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "assistant",
                        "content": marker + f"here is the patch\n\n```rust\n{CODE_BLOCK}\n```\n\nand the note after it",
                        "created_at": at,
                    },
                )
            elif index % 5 == 3:
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "assistant",
                        "content": marker + "\n".join(f"* bullet {line} of the list" for line in range(9)),
                        "created_at": at,
                    },
                )
            else:
                # A tool turn: the call row, then its long result.
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "assistant",
                        "content": marker,
                        "tool_calls": TOOL_CALL,
                        "created_at": at,
                    },
                )
                seed_message(
                    conn,
                    {
                        "session_id": session_id,
                        "role": "tool",
                        "tool_call_id": "call_seed_0",
                        "name": "run_command",
                        "content": TOOL_OUTPUT,
                        "created_at": at + 1,
                    },
                )
        conn.commit()


# ---------------------------------------------------------------------------
# In-page instrumentation
# ---------------------------------------------------------------------------

PROBE = r"""
window.__probe = {
  running: false,
  samples: [],
  writes: [],
  tracing: false,
  /**
   * Shadow `scrollTop` on the scroller so every write reports who made it.
   *
   * The symptom is the view moving *against* the gesture, and a frame sample can
   * only show that it moved: the writer is either the transcript's measurement
   * correction, the sticky-bottom follow, or the history loader. A stack per
   * write names which one, and that is the difference between a diagnosis and a
   * guess.
   */
  trace() {
    if (this.tracing) return 'already';
    const proto = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
    this.writes = [];
    // On the prototype, not on the node: the transcript scroller is created when
    // a session opens, so a shadow installed on the landing view's element (or a
    // lookup that runs after the first paint) misses every write of the open —
    // which is exactly the moment under test. The class check keeps the log to
    // the scroller that matters.
    Object.defineProperty(Element.prototype, 'scrollTop', {
      configurable: true,
      get() {
        return proto.get.call(this);
      },
      set(value) {
        const before = proto.get.call(this);
        const delta = Number(value) - before;
        const watched = this.classList && this.classList.contains('chat-scroll');
        if (watched && Math.abs(delta) > 0.5) {
          const stack = (new Error().stack || '').split('\n').slice(1, 7).join(' | ');
          window.__probe.writes.push({
            t: Math.round(performance.now()),
            from: Math.round(before * 100) / 100,
            to: Math.round(Number(value) * 100) / 100,
            delta: Math.round(delta * 100) / 100,
            stack,
          });
        }
        proto.set.call(this, value);
      },
    });
    // `scrollTo` bypasses the `scrollTop` accessor entirely, so it needs its own
    // trace: a smooth scroll it starts is a scroll nobody wrote a property for.
    const protoScrollTo = Element.prototype.scrollTo;
    this.writes.length = this.writes.length;
    Element.prototype.scrollTo = function (...args) {
      if (this.classList && this.classList.contains('chat-scroll')) {
        const stack = (new Error().stack || '').split('\n').slice(1, 6).join(' | ');
        window.__probe.writes.push({
          t: Math.round(performance.now()),
          from: Math.round(this.scrollTop),
          to: 'scrollTo:' + JSON.stringify(args[0] ?? null).slice(0, 80),
          delta: 0,
          stack,
        });
      }
      return protoScrollTo.apply(this, args);
    };
    this.tracing = true;
    return 'tracing';
  },
  /** Install the trace before any session is opened; it is prototype-wide. */
  traceWhenReady() {
    return this.trace();
  },
  /** The scroller may not exist yet (the landing view has none): install on sight. */
  waitForScroller() {
    const found = document.querySelector('.chat-scroll');
    if (found) return found;
    this.pending = true;
    const poll = setInterval(() => {
      const node = document.querySelector('.chat-scroll');
      if (!node) return;
      clearInterval(poll);
      this.trace();
    }, 10);
    return null;
  },
  /** Writes since the last dump; `start()` deliberately does not clear them. */
  dump() {
    const writes = this.writes;
    this.writes = [];
    return { writes, geometry: this.geometry() };
  },
  geometry() {
    const node = document.querySelector('.chat-scroll');
    const transcript = document.querySelector('.transcript');
    if (!node) return { missing: true };
    return {
      t: Math.round(performance.now()),
      distance: Math.round(node.scrollHeight - node.scrollTop - node.clientHeight),
      top: Math.round(node.scrollTop),
      h: node.scrollHeight,
      ch: node.clientHeight,
      rows: transcript ? transcript.children.length : -1,
      padTop: transcript ? parseFloat(getComputedStyle(transcript).paddingTop) || 0 : -1,
      padBottom: transcript ? parseFloat(getComputedStyle(transcript).paddingBottom) || 0 : -1,
      jump: !!document.querySelector('.jump-latest'),
    };
  },
  start() {
    this.samples = [];
    this.running = true;
    const tick = () => {
      if (!this.running) return;
      const node = document.querySelector('.chat-scroll');
      const transcript = document.querySelector('.transcript');
      if (node && transcript) {
        const box = node.getBoundingClientRect();
        const rows = transcript.children;
        let firstRow = '';
        let firstTop = null;
        for (const row of rows) {
          const rect = row.getBoundingClientRect();
          if (rect.bottom <= box.top) continue;
          firstRow = row.getAttribute('data-entry-key') || '';
          firstTop = Math.round((rect.top - box.top) * 100) / 100;
          break;
        }
        this.samples.push({
          t: Math.round(performance.now()),
          top: Math.round(node.scrollTop * 100) / 100,
          h: node.scrollHeight,
          ch: node.clientHeight,
          rows: rows.length,
          first: firstRow,
          firstTop,
          padTop: parseFloat(getComputedStyle(transcript).paddingTop) || 0,
          padBottom: parseFloat(getComputedStyle(transcript).paddingBottom) || 0,
          jump: !!document.querySelector('.jump-latest'),
          panel: (document.querySelector('.chat-panel') || {}).getAttribute
            ? document.querySelector('.chat-panel').getAttribute('data-reading-history')
            : null,
          fresh: document.querySelectorAll('.msg.is-fresh').length,
          composer: ((document.querySelector('.chat-panel') || {}).style || {}).getPropertyValue
            ? document.querySelector('.chat-panel').style.getPropertyValue('--composer-h')
            : '',
        });
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return 'started';
  },
  stop() {
    this.running = false;
    return { samples: this.samples, writes: this.writes };
  },
  distance() {
    const node = document.querySelector('.chat-scroll');
    return Math.round(node.scrollHeight - node.scrollTop - node.clientHeight);
  },
};
"""


def analyse(recording: dict, direction: str = "down") -> dict:
    """Summarise one gesture: backsteps, churn and the writes that caused them.

    `Input.synthesizeScrollGesture` counts positive `yDistance` as *up*, so a
    downward gesture moves `scrollTop` up. A backstep is therefore a decrease
    while scrolling down and an increase while scrolling up — the view going
    against the hand on the wheel, which is the reported symptom.
    """
    samples = recording["samples"] if isinstance(recording, dict) else recording
    writes = recording.get("writes", []) if isinstance(recording, dict) else []
    # Only the writes inside this recording: the probe keeps a running log so an
    # event that happens before the first frame (the open) is not lost, and a
    # gesture must not be accused of writes made by an earlier phase.
    if samples:
        first = samples[0]["t"]
        writes = [write for write in writes if write["t"] >= first]
    if not samples:
        return {"frames": 0, "backsteps": [], "against": [], "worstBackstep": 0, "churn": {}}
    sign = 1 if direction == "down" else -1
    backsteps = []
    churn = {"rows": 0, "padTop": 0, "height": 0, "jumpFlips": 0, "panelFlips": 0}
    for previous, current in zip(samples, samples[1:]):
        delta = (current["top"] - previous["top"]) * sign
        if delta < -6:
            backsteps.append(
                {
                    "t": current["t"],
                    "delta": round(delta, 1),
                    "h": [previous["h"], current["h"]],
                    "pad": [previous["padTop"], current["padTop"]],
                    "rows": [previous["rows"], current["rows"]],
                    "first": [previous["first"], current["first"]],
                    "jump": [previous["jump"], current["jump"]],
                }
            )
        if current["rows"] != previous["rows"]:
            churn["rows"] += 1
        if abs(current["padTop"] - previous["padTop"]) > 0.5:
            churn["padTop"] += 1
        if abs(current["h"] - previous["h"]) > 0.5:
            churn["height"] += 1
        if current["jump"] != previous["jump"]:
            churn["jumpFlips"] += 1
        if current["panel"] != previous["panel"]:
            churn["panelFlips"] += 1
    # Programmatic writes that fought the gesture, with the stack that made them.
    against = [write for write in writes if write["delta"] * sign < -6]
    tops = [sample["top"] for sample in samples]
    return {
        "frames": len(samples),
        "span": round(samples[-1]["t"] - samples[0]["t"]),
        "from": tops[0],
        "to": tops[-1],
        "min": min(tops),
        "max": max(tops),
        "backsteps": backsteps,
        "against": against,
        "worstBackstep": min((step["delta"] for step in backsteps), default=0),
        "churn": churn,
        "endDistance": round(samples[-1]["h"] - samples[-1]["top"] - samples[-1]["ch"]),
    }


def dump(summary: dict, limit: int = 6) -> None:
    """Print the evidence for one failing gesture: geometry + who wrote the scroll."""
    for step in summary["backsteps"][:limit]:
        print(f"       backstep {step['delta']:+.0f}px @t{step['t']} "
              f"h {step['h']} pad {step['pad']} rows {step['rows']} first {step['first']}", flush=True)
    for write in summary["against"][:limit]:
        print(f"       write {write['delta']:+.0f}px ({write['from']}→{write['to']}) {write['stack'][:300]}", flush=True)


POINT = r"""
// A wheel is delivered to whatever is under the pointer, and the transcript
// contains scrollers of its own (a reasoning block, a command's output). A
// gesture that lands in one of those scrolls *that* box and looks, in a probe
// of `.chat-scroll`, exactly like a page that refused to move. So pick a point
// that hit-tests to plain transcript.
(function () {
  const node = document.querySelector('.chat-scroll');
  const box = node.getBoundingClientRect();
  const x = Math.round(box.left + box.width / 2);
  const bad = 'pre, code, .thinking-body, .tool-card, .command-card, .queued-panel, textarea, .jump-latest';
  for (let offset = 0.25; offset < 0.85; offset += 0.06) {
    const y = Math.round(box.top + box.height * offset);
    const hit = document.elementFromPoint(x, y);
    if (!hit || !node.contains(hit)) continue;
    if (hit.closest(bad)) continue;
    window.__point = { x, y };
    return { x, y, hit: hit.className || hit.tagName };
  }
  window.__point = { x, y: Math.round(box.top + box.height / 2) };
  return { ...window.__point, hit: 'fallback' };
})();
"""


def transcript_point(session: Session) -> dict:
    """Ask the page for a wheel target that is the transcript itself."""
    return session.evaluate(POINT)


def gesture(session: Session, direction: str, distance: int, x: int | None = None, y: int | None = None, speed: int = 1200) -> None:
    """A real smooth wheel scroll, which is what the report used.

    CDP counts positive `yDistance` as up, so "down" (toward the newest content)
    asks for a negative distance.
    """
    sign = -1 if direction == "down" else 1
    point = session.evaluate("return window.__point || {x: 640, y: 420};")
    session.cdp(
        "Input.synthesizeScrollGesture",
        {
            "x": x if x is not None else point["x"],
            "y": y if y is not None else point["y"],
            "yDistance": sign * distance,
            "speed": speed,
            "gestureSourceType": "mouse",
        },
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    binary = ROOT / "target/debug/karucode"
    if not binary.exists():
        raise SystemExit("build first: cargo build -p kc-server --bin karucode")

    tmp = Path(tempfile.mkdtemp(prefix="kc-scroll-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()

    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), Provider)
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
                "[approvals]",
                'mode = "auto"',
                "",
                "[limits]",
                "max_iterations = 4",
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
                'context_window = 200000',
                "reasoning = true",
                "",
                "[active]",
                'provider = "default"',
                'model = "stub-model"',
                "",
            ]
        )
    )

    port = free_port()
    cdp_port = free_port()
    profile = tmp / "chrome-profile"
    profile.mkdir()

    print(f"workdir {tmp}", flush=True)
    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(binary),
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
        stdout=server_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    chrome_env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("DBUS_SESSION_BUS_ADDRESS", "QT_ACCESSIBILITY")
    }
    chrome_log = open(tmp / "chrome.log", "wb")
    chrome = subprocess.Popen(
        [
            "chromium",
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            f"--remote-debugging-port={cdp_port}",
            f"--user-data-dir={profile}",
            "--window-size=1280,900",
            "about:blank",
        ],
        env=chrome_env,
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    session: Session | None = None
    try:
        wait_for(f"http://127.0.0.1:{port}/api/health")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        api = f"http://127.0.0.1:{port}"

        projects = http_json(f"{api}/api/projects")["projects"]
        project_id = projects[0]["id"]
        created = http_json(f"{api}/api/sessions", "POST", {"title": "Scroll target", "project": project_id})
        session_id = created.get("id") or (created.get("session") or {}).get("id")
        db = data_dir / "karucode.db"
        seed_transcript(db, session_id)

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(target for target in targets if target["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{api}/"})
        session.evaluate(PROBE)

        # The app restores the session it was last on, so opening a long one can
        # happen during the page load itself — before any `evaluate` here can run.
        # Install the probe as a *new-document* script and reload, so the trace and
        # the frame sampler exist from the first script tag of the open.
        session.cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            {"source": PROBE + "\nwindow.__probe.traceWhenReady(); window.__probe.start();"},
        )
        session.cdp("Page.reload")
        session.evaluate(
            """
            const deadline = Date.now() + 30000;
            while (Date.now() < deadline) {
              const ready = [...document.querySelectorAll('button')].some(
                (node) => (node.textContent || '').trim() === 'Settings');
              if (ready && window.__probe && window.__probe.tracing) return 'reloaded';
              await new Promise((r) => setTimeout(r, 100));
            }
            return 'reload-timeout';
            """
        )

        # The report blamed the thinking-streaming setting, so make it explicit:
        # a long thought renders open while it arrives. Set it through the real
        # control (Settings -> Appearance -> Reasoning blocks), then reload so the
        # stored theme is what the transcript renders with.
        boot = session.evaluate(
            """
            const deadline = Date.now() + 30000;
            while (Date.now() < deadline) {
              const ready = [...document.querySelectorAll('button')].some(
                (node) => (node.textContent || '').trim() === 'Settings');
              if (ready) return 'ready';
              await new Promise((r) => setTimeout(r, 100));
            }
            return 'timeout:' + location.href + ':' + document.readyState + ':' +
              (document.body.innerText || '').slice(0, 200);
            """
        )
        check("the app boots", boot == "ready", str(boot)[:600])

        theme_set = session.evaluate(
            """
            const find = async (selector, ms) => {
              const end = Date.now() + ms;
              while (Date.now() < end) {
                const node = typeof selector === 'function' ? selector() : document.querySelector(selector);
                if (node) return node;
                await new Promise((r) => setTimeout(r, 100));
              }
              return null;
            };
            const trigger = await find(
              () => [...document.querySelectorAll('button')].find(
                (node) => (node.getAttribute('aria-label') || '') === 'Open settings'
                  || (node.textContent || '').trim() === 'Settings'),
              10000,
            );
            if (!trigger) {
              return 'no-settings-button:' + JSON.stringify({
              href: location.href,
              ready: document.readyState,
              rail: !!document.querySelector('.project-rail'),
              body: (document.body.innerText || '').slice(0, 200),
              labels: [...document.querySelectorAll('button[aria-label]')].map(
                (node) => node.getAttribute('aria-label')),
            });
            }
            trigger.click();
            // The Appearance panel owns the reasoning select; open it explicitly
            // so the script does not depend on which tab the sheet remembered.
            const nav = await find('#settings-nav-tab-general', 15000);
            if (nav) {
              nav.click();
              await new Promise((r) => setTimeout(r, 300));
            }
            const select = await find('#settings-thinking', 15000);
            if (!select) return 'no-thinking-select';
            select.value = 'expanded';
            select.dispatchEvent(new Event('input', { bubbles: true }));
            select.dispatchEvent(new Event('change', { bubbles: true }));
            await new Promise((r) => setTimeout(r, 300));
            const close = [...document.querySelectorAll('button')].find((node) =>
              /close settings/i.test(node.getAttribute('aria-label') || ''));
            if (close) close.click();
            await new Promise((r) => setTimeout(r, 300));
            return document.documentElement.dataset.thinking;
            """
        )
        check("the reasoning blocks setting takes", theme_set == "expanded", str(theme_set)[:400])

        trace_state = session.evaluate("return window.__probe.traceWhenReady();")
        if trace_state != 'tracing':
            print(f"       WARNING trace not installed: {trace_state}", flush=True)

        def stage(label: str) -> None:
            """Where the view stands, and every write that got it there since the
            last stage. The open is one of several events (restore-on-load, the
            reasoning setting re-laying every row, the click), and only one of
            them can be the one that leaves a gap."""
            recording = session.evaluate("return window.__probe.dump();")
            geometry = recording["geometry"]
            print(
                f"       [{label}] {json.dumps(geometry)} "
                f"{len(recording['writes'])} writes",
                flush=True,
            )
            for write in recording["writes"][-16:]:
                stack = str(write.get("stack") or "").replace("\n", " ")
                print(
                    f"         write t{write['t']} {write['delta']:+.1f}px "
                    f"({write['from']}\u2192{write['to']}) {stack[:190]}",
                    flush=True,
                )

        stage('after theme')
        stage('restored')
        session.evaluate("return window.__probe.start();")
        opened = session.evaluate(
            """
            const okButton = await (async () => {
              const deadline = Date.now() + 20000;
              while (Date.now() < deadline) {
                const button = [...document.querySelectorAll('button')].find((node) =>
                  (node.textContent || '').includes('Scroll target'));
                if (button) return button;
                await new Promise((r) => setTimeout(r, 100));
              }
              return null;
            })();
            if (!okButton) return 'no-session-row';
            // Record the open from the inside: a scroll event log answers "who
            // moved the view" even when the main thread is blocked long enough
            // that no animation frame is sampled during the transition.
            window.__openLog = [];
            window.__roLog = [];
            // Does anything observe the growth? The sticky-bottom follows through
            // a ResizeObserver on `.chat-inner`; if that never fires, the document
            // can grow under the viewport and nothing will ever catch up.
            const startRo = setInterval(() => {
              const inner = document.querySelector('.chat-inner');
              if (!inner) return;
              clearInterval(startRo);
              const ro = new ResizeObserver(() => {
                const node = document.querySelector('.chat-scroll');
                window.__roLog.push({
                  t: Math.round(performance.now()),
                  inner: Math.round(inner.getBoundingClientRect().height),
                  padB: getComputedStyle(inner).paddingBottom,
                  top: node ? Math.round(node.scrollTop) : -1,
                  h: node ? node.scrollHeight : -1,
                });
              });
              ro.observe(inner);
              window.__openRo = ro;
            }, 20);
            const watch = () => {
              const node = document.querySelector('.chat-scroll');
              if (!node) return;
              window.__openLog.push({
                t: Math.round(performance.now()),
                top: Math.round(node.scrollTop),
                h: node.scrollHeight,
                ch: node.clientHeight,
                rows: document.querySelectorAll('.transcript > *').length,
              });
            };
            okButton.click();
            const watcher = setInterval(watch, 30);
            setTimeout(() => clearInterval(watcher), 4000);
            const deadline = Date.now() + 20000;
            while (Date.now() < deadline) {
              if ((document.body.innerText || '').includes('SEED-NEWEST-MARKER')) {
                // Let the open settle: the pin (layout effect), the composer's
                // reserved band (passive effect) and the window's first
                // measurement all land within a few frames of the marker.
                await new Promise((r) => setTimeout(r, 1500));
                return 'opened';
              }
              await new Promise((r) => setTimeout(r, 100));
            }
            return 'no-marker';
            """
        )
        check("the long session opens", opened == "opened", str(opened))
        stage('clicked open')

        # The frames either side of the open, and every write to the scroll in
        # them. This is the one moment the pin, the composer's reserved band and
        # the window's first measurement land together, so a leftover gap is born
        # here. Nothing may be pushed to the tail afterwards, because the reader
        # has not gestured yet.
        settle = session.evaluate("return window.__probe.stop();")
        print(f"       open: {len(settle['samples'])} frames, {len(settle['writes'])} writes", flush=True)
        ro_log = session.evaluate("return window.__roLog || [];")
        print(f"       RO fired {len(ro_log)} times", flush=True)
        for entry in ro_log[:20]:
            print(
                f"       RO t{entry['t']} inner {entry['inner']} padB {entry['padB']} "
                f"top {entry['top']} h {entry['h']}",
                flush=True,
            )
        open_log = session.evaluate("return window.__openLog || [];")
        for entry in open_log[:26]:
            print(
                f"       log t{entry['t']} top {entry['top']} h {entry['h']} ch {entry['ch']} rows {entry['rows']}",
                flush=True,
            )
        for write in settle["writes"]:
            print(
                f"       open write {write['delta']:+.1f}px ({write['from']}→{write['to']}) "
                f"{write['stack'][:200]}",
                flush=True,
            )
        for sample in (settle["samples"][:14] + settle["samples"][::4][-6:]):
            print(
                f"       open t{sample['t']} top {sample['top']} h {sample['h']} ch {sample['ch']} "
                f"padT {sample['padTop']} padB {sample['padBottom']} rows {sample['rows']} "
                f"jump {sample['jump']}",
                flush=True,
            )

        thinking = session.evaluate("return document.documentElement.dataset.thinking;")
        check("thinking renders open", thinking == "expanded", str(thinking))

        geometry = session.evaluate(
            """
            const node = document.querySelector('.chat-scroll');
            return {
              distance: Math.round(node.scrollHeight - node.scrollTop - node.clientHeight),
              height: node.scrollHeight,
              viewport: node.clientHeight,
              rows: document.querySelectorAll('.transcript > *').length,
              total: {SEED_ROWS},
            };
            """.replace("{SEED_ROWS}", str(SEED_ROWS))
        )
        print(f"       geometry {geometry}", flush=True)
        check("opening lands on the newest content", geometry["distance"] <= 2, json.dumps(geometry))
        check("the transcript is windowed", 0 < geometry["rows"] < SEED_ROWS, str(geometry["rows"]))

        # ---- 1. a downward gesture in the middle of history -----------------
        session.evaluate(
            """
            const node = document.querySelector('.chat-scroll');
            node.scrollTop = Math.round(node.scrollHeight * 0.35);
            await new Promise((r) => setTimeout(r, 500));
            return Math.round(node.scrollTop);
            """
        )
        session.evaluate("return window.__probe.trace();")
        session.evaluate("return window.__probe.start();")
        gesture(session, "down", 900)
        session.evaluate("await new Promise((r) => setTimeout(r, 700)); return 1;")
        down = analyse(session.evaluate("return window.__probe.stop();"), "down")
        if not args.quiet:
            print(f"       down-gesture {json.dumps({k: down[k] for k in ('frames','from','to','min','max','churn')})}", flush=True)
            dump(down)
        check(
            "a downward gesture never throws the view back up",
            not down["backsteps"],
            f"{len(down['backsteps'])} backsteps, worst {down['worstBackstep']}px",
        )

        # ---- 1b. an upward gesture: the reader going back into history ------
        session.evaluate(
            """
            const node = document.querySelector('.chat-scroll');
            node.scrollTop = Math.round(node.scrollHeight * 0.6);
            await new Promise((r) => setTimeout(r, 500));
            return Math.round(node.scrollTop);
            """
        )
        session.evaluate("return window.__probe.start();")
        gesture(session, "up", 900)
        session.evaluate("await new Promise((r) => setTimeout(r, 700)); return 1;")
        up = analyse(session.evaluate("return window.__probe.stop();"), "up")
        if not args.quiet:
            print(f"       up-gesture {json.dumps({k: up[k] for k in ('frames','from','to','min','max','churn')})}", flush=True)
            dump(up)
        check(
            "an upward gesture is never dragged back down",
            not up["backsteps"],
            f"{len(up['backsteps'])} backsteps, worst {up['worstBackstep']}px",
        )

        # ---- 2. can the reader reach the bottom at all? --------------------
        reaches = []
        for attempt in range(8):
            session.evaluate("return window.__probe.start();")
            gesture(session, "down", 1200)
            session.evaluate("await new Promise((r) => setTimeout(r, 800)); return 1;")
            recording = session.evaluate("return window.__probe.stop();")
            summerised = analyse(recording, "down")
            reaches.append(
                {
                    "attempt": attempt,
                    "distance": session.evaluate("return window.__probe.distance();"),
                    "backsteps": len(summerised.get("backsteps") or []),
                    "worst": summerised.get("worstBackstep"),
                    "churn": summerised.get("churn"),
                }
            )
            if summerised["backsteps"] and attempt == 0:
                dump(summerised)
        print(f"       bottom attempts {json.dumps(reaches)}", flush=True)
        check(
            "repeated downward gestures reach the bottom",
            reaches[-1]["distance"] <= 2,
            json.dumps(reaches[-1]),
        )
        check(
            "no gesture toward the bottom is fought by the page",
            all(entry["backsteps"] == 0 for entry in reaches),
            json.dumps([entry["backsteps"] for entry in reaches]),
        )

        # ---- 3. Jump to latest ---------------------------------------------
        session.evaluate(
            """
            const node = document.querySelector('.chat-scroll');
            node.scrollTop = 0;
            await new Promise((r) => setTimeout(r, 500));
            return Math.round(node.scrollTop);
            """
        )
        jumped = session.evaluate(
            """
            const button = await (async () => {
              const deadline = Date.now() + 5000;
              while (Date.now() < deadline) {
                const node = document.querySelector('.jump-latest');
                if (node) return node;
                await new Promise((r) => setTimeout(r, 100));
              }
              return null;
            })();
            if (!button) return 'no-button';
            await window.__probe.start();
            button.click();
            await new Promise((r) => setTimeout(r, 1600));
            const recording = await window.__probe.stop();
            const node = document.querySelector('.chat-scroll');
            return {
              distance: Math.round(node.scrollHeight - node.scrollTop - node.clientHeight),
              recording,
            };
            """
        )
        if isinstance(jumped, dict):
            jump_summary = analyse(jumped["recording"], "down")
            print(
                f"       jump: distance {jumped['distance']}, "
                f"backsteps {len(jump_summary['backsteps'])}, churn {jump_summary['churn']}",
                flush=True,
            )
            check("Jump to latest lands on the newest content", jumped["distance"] <= 2, str(jumped["distance"]))
            check(
                "Jump to latest stays landed",
                not jump_summary["backsteps"],
                f"worst {jump_summary['worstBackstep']}px",
            )
        else:
            check("Jump to latest lands on the newest content", False, str(jumped))

        # ---- 4. a live run that streams reasoning ---------------------------
        STATE["tall_reasoning"] = True
        session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'THINK-STREAM please answer';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await new Promise((r) => setTimeout(r, 150));
            const send = [...document.querySelectorAll('button')].find((node) =>
              /send message/i.test(node.getAttribute('aria-label') || ''));
            if (!send) return 'no-send';
            send.click();
            return 'sent';
            """
        )
        session.evaluate("return window.__probe.start();")
        session.evaluate("await new Promise((r) => setTimeout(r, 7000)); return 1;")
        live_recording = session.evaluate("return window.__probe.stop();")
        live = analyse(live_recording, "down")
        live_samples = live_recording["samples"]
        flips = live.get("churn", {}).get("jumpFlips", 0)
        fresh_max = max((sample["fresh"] for sample in live_samples), default=0)
        streaming = session.evaluate(
            """
            const node = document.querySelector('.chat-scroll');
            return {
              distance: Math.round(node.scrollHeight - node.scrollTop - node.clientHeight),
              thinking: document.querySelectorAll('.thinking').length,
              fresh: document.querySelectorAll('.msg.is-fresh').length,
              jump: !!document.querySelector('.jump-latest'),
            };
            """
        )
        print(
            f"       live run: {json.dumps({k: live[k] for k in ('frames', 'from', 'to', 'worstBackstep', 'churn')})}"
            f" freshMax {fresh_max} state {json.dumps(streaming)}",
            flush=True,
        )
        dump(live, limit=8)
        check(
            "a streaming answer keeps the reader at the bottom",
            streaming["distance"] <= 4,
            str(streaming["distance"]),
        )
        check(
            "following a stream never throws the view up",
            not live["backsteps"],
            f"{len(live['backsteps'])} backsteps, worst {live['worstBackstep']}px",
        )
        check(
            "Jump to latest does not flicker while following",
            flips <= 2,
            f"{flips} flips",
        )

        # ---- 5. gestures while the answer streams --------------------------
        # This is the case the report describes: the run is producing text, the
        # reader scrolls, and the page fights the hand on the wheel.
        STATE["tall_reasoning"] = True
        session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'THINK-STREAM and a second answer';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await new Promise((r) => setTimeout(r, 150));
            const send = [...document.querySelectorAll('button')].find((node) =>
              /send message/i.test(node.getAttribute('aria-label') || ''));
            if (send) send.click();
            return 'sent';
            """
        )
        session.evaluate("await new Promise((r) => setTimeout(r, 400)); return 1;")
        print(f"       wheel target {transcript_point(session)}", flush=True)
        mid_stream = []
        for direction in ("up", "down", "up", "down"):
            session.evaluate("return window.__probe.start();")
            gesture(session, direction, 700)
            session.evaluate("await new Promise((r) => setTimeout(r, 450)); return 1;")
            summary = analyse(session.evaluate("return window.__probe.stop();"), direction)
            mid_stream.append(
                {
                    "direction": direction,
                    "from": summary.get("from"),
                    "to": summary.get("to"),
                    "distance": session.evaluate("return window.__probe.distance();"),
                    "backsteps": len(summary.get("backsteps") or []),
                    "worst": summary.get("worstBackstep"),
                    "against": [write["delta"] for write in summary.get("against") or []],
                    "jump": session.evaluate("return !!document.querySelector('.jump-latest');"),
                    "reading": session.evaluate(
                        "return (document.querySelector('.chat-panel') || {dataset:{}}).getAttribute('data-reading-history');"
                    ),
                }
            )
            if summary.get("against"):
                dump(summary, limit=3)
        print(f"       mid-stream {json.dumps(mid_stream)}", flush=True)
        fought = [entry["against"] for entry in mid_stream]
        check(
            "a stream never moves the view against the wheel",
            not any(fought),
            f"{sum(len(entry) for entry in fought)} writes fought the gesture: "
            + json.dumps([entry[:4] for entry in fought]),
        )

        # ---- 6. Jump to latest in the middle of a stream -------------------
        STATE["tall_reasoning"] = True
        session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'THINK-STREAM one more';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await new Promise((r) => setTimeout(r, 150));
            const send = [...document.querySelectorAll('button')].find((node) =>
              /send message/i.test(node.getAttribute('aria-label') || ''));
            if (send) send.click();
            return 'sent';
            """
        )
        session.evaluate("await new Promise((r) => setTimeout(r, 600)); return 1;")
        transcript_point(session)
        gesture(session, "up", 2500)
        session.evaluate("await new Promise((r) => setTimeout(r, 500)); return 1;")
        landed = session.evaluate(
            """
            const button = document.querySelector('.jump-latest');
            if (!button) return 'no-button';
            await window.__probe.start();
            button.click();
            await new Promise((r) => setTimeout(r, 1200));
            const recording = await window.__probe.stop();
            const node = document.querySelector('.chat-scroll');
            return {
              distance: Math.round(node.scrollHeight - node.scrollTop - node.clientHeight),
              recording,
            };
            """
        )
        if isinstance(landed, dict):
            landed_summary = analyse(landed["recording"], "down")
            print(
                f"       mid-stream jump: distance {landed['distance']} "
                f"backsteps {len(landed_summary['backsteps'])} churn {landed_summary['churn']}",
                flush=True,
            )
            dump(landed_summary, limit=4)
            check(
                "Jump to latest lands while the answer streams",
                landed["distance"] <= 4,
                str(landed["distance"]),
            )
        else:
            check("Jump to latest lands while the answer streams", False, str(landed))

        # ---- 7. a steer shows up in the transcript --------------------------
        # Reported: "text sent as steering doesn't show in the history". The
        # runtime already persists it as an ordinary user message (so a reload
        # shows it), and the live frame carries the text — the transcript is what
        # has to draw it. Send one over the API while the run above still
        # streams, then read the rendered rows.
        STATE["tall_reasoning"] = True
        session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'THINK-STREAM while I steer';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await new Promise((r) => setTimeout(r, 150));
            const send = [...document.querySelectorAll('button')].find((node) =>
              /send message/i.test(node.getAttribute('aria-label') || ''));
            if (send) send.click();
            return 'sent';
            """
        )
        session.evaluate("await new Promise((r) => setTimeout(r, 1200)); return 1;")
        steer_text = "STEER-ROW: also check the fixtures"
        queued = http_json(
            f"{api}/api/sessions/{session_id}/messages?mode=steer",
            "POST",
            {"content": steer_text},
        )
        print(f"       steer accepted: {json.dumps(queued)[:200]}", flush=True)
        steered = session.evaluate(
            f"""
            const needle = {json.dumps(steer_text)};
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {{
              const rows = [...document.querySelectorAll('.transcript .msg')];
              const hit = rows.find((row) => (row.innerText || '').includes(needle));
              if (hit) {{
                return {{
                  found: true,
                  // The user row's own class (`.msg-user`, styles/chat.css) —
                  // not a text match, so this proves it was drawn as a user turn.
                  isUser: hit.classList.contains('msg-user'),
                  text: (hit.innerText || '').slice(0, 120),
                }};
              }}
              await new Promise((r) => setTimeout(r, 100));
            }}
            return {{ found: false }};
            """
        )
        print(f"       steer row: {json.dumps(steered)}", flush=True)
        check(
            "a steered message appears in the transcript",
            isinstance(steered, dict) and steered.get("found") is True,
            json.dumps(steered)[:300],
        )
        check(
            "the steered message is drawn as a user turn",
            isinstance(steered, dict) and steered.get("isUser") is True,
            json.dumps(steered)[:300],
        )

        print(f"\n{len(PASS)} checks passed", flush=True)
        if args.keep:
            print(f"server {api} chrome cdp {cdp_port}", flush=True)
            input("press enter to stop\n")
        return 0
    finally:
        if not args.keep:
            if session:
                session.close()
            chrome.terminate()
            server.terminate()
            try:
                chrome.wait(timeout=5)
                server.wait(timeout=5)
            except Exception:
                chrome.kill()
                server.kill()
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())