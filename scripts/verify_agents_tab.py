#!/usr/bin/env python3
"""Drive the Agents tab in a real Chromium over raw CDP.

The chrome-devtools plugin cannot launch Chromium on this host, so this does what
actually works here (see `scripts/verify_settings_tabs.py`, whose CDP plumbing
this reuses):

  * starts Chromium with an explicit `--user-data-dir` and `--no-sandbox`, behind
    `setsid` (`start_new_session`);
  * starts a private kc-server on its own port/data-dir against a stub provider,
    pointed at the built `web/dist` with `KARUCODE_WEB_DIR`;
  * speaks CDP over one persistent WebSocket.

What it proves, which the unit tests cannot:

  * a background subagent really runs while its parent's turn is over, and the
    row shows the `background` badge;
  * the **Pause** button on a live agent parks it and the roster says `paused`
    (a live status, not a settled one);
  * **Resume** releases it and the run finishes;
  * **Stop** on a live agent ends it and the row says `cancelled`;
  * `report_status` reaches the row as a note plus a progress bar;
  * a settled agent has no control buttons at all (no dead controls);
  * nothing overflows at 1280px or 390px.

Usage: python3 scripts/verify_agents_tab.py [--keep]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
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


# ---------------------------------------------------------------------------
# A stub provider: scripted, keyed by the prompt the harness sends.
# ---------------------------------------------------------------------------
#
# The browser drives the run, so the scripts have to live in the "model". Each
# entry maps a marker in the newest user turn to what the model answers. Chat
# completions are SSE, because that is what the runtime consumes.

def sse(chunks) -> bytes:
    out = []
    for index, chunk in enumerate(chunks):
        # The terminating `data: [DONE]` is what ends the stream cleanly; without
        # it the runtime waits for a usage block that never arrives.
        out.append(f"data: {json.dumps(chunk)}\n\n")
    out.append("data: [DONE]\n\n")
    return "".join(out).encode()


def text_chunks(text: str):
    return [
        {
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        },
    ]


def tool_chunks(call_id: str, name: str, arguments: dict):
    return [
        {
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": json.dumps(arguments),
                                },
                            }
                        ]
                    },
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
    ]


def user_text(messages) -> str:
    """The newest user turn.

    Deliberately not the concatenation of every user turn: the marker
    ("KICK-OFF") would still be in the transcript on the parent's *second* call,
    so a marker match over all of history makes the model repeat its first move
    forever - which is exactly what an agent loop looks like when it is stuck.
    """
    for message in reversed(messages):
        if message.get("role") == "user":
            content = message.get("content")
            return content if isinstance(content, str) else ""
    return ""


def turns_so_far(messages) -> int:
    """How many times the model has already answered *this* prompt.

    Counted from the newest user turn, not from the start of the conversation: a
    marker stays in the transcript forever, so a rule keyed on the whole history
    makes the model repeat its first move on every turn - a re-spawn loop that
    ends in `max_background` refusals.
    """
    count = 0
    for message in reversed(messages):
        if message.get("role") == "user":
            break
        if message.get("role") == "assistant":
            count += 1
    return count


def answer(messages):
    """Pick what the scripted model does for this request.

    A subagent's script is turn-based rather than marker-based: a child that
    answered its marker on every call would loop forever, which looks exactly like
    a hung agent. Turn 0 parks it inside a long shell command (so there is an
    iteration boundary for the browser to pause or a stop to land at) and turn 1
    finishes it.
    """
    text = user_text(messages)
    # A subagent's system prompt *begins* with its role; the main agent's merely
    # mentions "focused subagent" later, in the tool list, so a substring search
    # would classify the parent as a child.
    system = (messages[0].get("content") or "").lstrip()
    is_child = system.startswith("You are a subagent") or system.startswith(
        "You are a focused subagent"
    )

    if is_child:
        turn = turns_so_far(messages)
        if "PAUSE-ME" in text:
            if turn == 0:
                return (
                    "tools",
                    tool_chunks("c_pause", "run_command", {"command": "sleep 30"}),
                )
            return ("text", text_chunks("the paused child resumed and finished"))
        if "STOP-ME" in text:
            if turn == 0:
                return (
                    "tools",
                    tool_chunks("c_stop", "run_command", {"command": "sleep 30"}),
                )
            return ("text", text_chunks("the stopped child should never say this"))
        if "REPORT-ME" in text:
            if turn == 0:
                return (
                    "tools",
                    tool_chunks(
                        "c_note",
                        "report_status",
                        {"note": "halfway through crates/", "progress": 0.5},
                    ),
                )
            if turn == 1:
                return (
                    "tools",
                    tool_chunks("c_sleep", "run_command", {"command": "sleep 30"}),
                )
            return ("text", text_chunks("the reporting child finished"))
        return ("text", text_chunks("background child finished"))

    # The parent is turn-based for the same reason the child is: its marker stays
    # in the transcript, so a marker-only rule would re-spawn on every turn until
    # `max_background` refused it. The *newest* user turn decides which script this
    # is, and the assistant count after it is the turn number.
    #
    # A spawn prompt waits on a following turn (a read-only `wait_for_agents`), so
    # the run stays alive while the background child is paused, renamed, resumed or
    # killed. That matters: a run's end cancels the children it started (the
    # documented rule - "a background agent outlives the turn that started it but
    # not the run"), so a mock that ended the run on its very next turn would make
    # every control assertion below race a legitimate cancellation.
    parent_turn = turns_so_far(messages)
    # Which spawn script this run belongs to. `user_text` is only the *newest* user
    # turn, so the marker is gone by turn 1; the conversation still holds it.
    spawn_marker = next(
        (
            marker
            for marker in ("KICK-OFF", "STOP-TARGET", "REPORT-TARGET")
            if any(
                marker in (message.get("content") or "")
                for message in messages
                if message.get("role") == "user"
            )
        ),
        None,
    )
    if spawn_marker and parent_turn == 1:
        return ("tools", tool_chunks("c_wait", "wait_for_agents", {"timeout_secs": 20}))
    # Turn 2 retires the child: the run has done its job by then, and every control
    # assertion sits on turns 0-1. Without this the parent stays parked in
    # `wait_for_agents` for the whole `wait_timeout_secs` and the next prompt is a
    # 409 ("a run is already active"), even though the child is long done.
    if spawn_marker and parent_turn == 2:
        return ("tools", tool_chunks("c_stop_child", "kill_agents", {}))
    if parent_turn > 0:
        return ("text", text_chunks("ok"))

    if "KICK-OFF" in text:
        return (
            "tools",
            tool_chunks(
                "c_spawn",
                "spawn_agent",
                {
                    "task": "PAUSE-ME: work until someone lets you go",
                    "name": "pause-target",
                    "role": "tester",
                    "background": True,
                },
            ),
        )
    if "STOP-TARGET" in text:
        return (
            "tools",
            tool_chunks(
                "c_spawn2",
                "spawn_agent",
                {
                    "task": "STOP-ME: work until someone stops you",
                    "name": "stop-target",
                    "role": "tester",
                    "background": True,
                },
            ),
        )
    if "REPORT-TARGET" in text:
        return (
            "tools",
            tool_chunks(
                "c_spawn3",
                "spawn_agent",
                {
                    "task": "REPORT-ME: say what you are doing",
                    "name": "report-target",
                    "role": "tester",
                    "background": True,
                },
            ),
        )
    return ("text", text_chunks("ok"))


class StubProvider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
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
        payload = json.loads(self.rfile.read(length) or b"{}")
        kind, chunks = answer(payload.get("messages") or [])
        body = sse(chunks)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


# ---------------------------------------------------------------------------
# Plumbing (see scripts/verify_settings_tabs.py for the same three tricks)
# ---------------------------------------------------------------------------


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 45.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise SystemExit(f"timed out waiting for {url}")


def http_json(url: str, method: str = "GET", body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("X-Karucode-Client", "web")
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        # The body explains a 409/400; urllib drops it, which turns "a run is in
        # flight" into a bare traceback.
        detail = error.read().decode("utf-8", "replace")
        raise SystemExit(f"{method} {url} -> {error.code}: {detail}")


CDP_HELPER = r"""
const wsUrl = process.argv[2];
const ws = new WebSocket(wsUrl);
let nextId = 0;
const pending = new Map();
ws.addEventListener('message', (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id !== undefined && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) reject(new Error(JSON.stringify(msg.error)));
    else resolve(msg.result);
  }
});
function send(method, params) {
  const requestId = ++nextId;
  return new Promise((resolve, reject) => {
    pending.set(requestId, { resolve, reject });
    ws.send(JSON.stringify({ id: requestId, method, params: params || {} }));
  });
}
ws.addEventListener('error', () => {
  console.log(JSON.stringify({ error: 'ws error' }));
  process.exit(1);
});
ws.addEventListener('open', () => {
  const readline = require('node:readline');
  const rl = readline.createInterface({ input: process.stdin });
  rl.on('line', async (line) => {
    if (!line.trim()) return;
    let plan;
    try {
      plan = JSON.parse(line);
    } catch (error) {
      console.log(JSON.stringify({ error: 'bad json' }));
      return;
    }
    try {
      let value = null;
      if (plan.kind === 'cdp') {
        const result = await send(plan.method, plan.params || {});
        value = result === undefined ? null : result;
      } else {
        const result = await send('Runtime.evaluate', {
          expression: '(async () => {' + plan.body + '})()',
          awaitPromise: true,
          returnByValue: true,
        });
        if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
        value = result.result.value === undefined ? null : result.result.value;
      }
      console.log(JSON.stringify({ ok: value }));
    } catch (error) {
      console.log(JSON.stringify({ error: String(error && error.message ? error.message : error) }));
    }
  });
  rl.on('close', () => process.exit(0));
});
"""


class Session:
    def __init__(self, ws_url: str) -> None:
        self.helper = Path(tempfile.gettempdir()) / "kc-cdp-agents.mjs"
        # `require` needs CommonJS; keep the helper a .cjs file.
        self.helper = self.helper.with_suffix(".cjs")
        self.helper.write_text(CDP_HELPER)
        self.proc = subprocess.Popen(
            ["node", str(self.helper), ws_url],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.evaluate("return 1;")

    def _send(self, plan: dict) -> object:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(plan) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            stderr = self.proc.stderr.read() if self.proc.stderr else ""
            raise SystemExit(f"CDP session died:\n{stderr}")
        payload = json.loads(line)
        if "error" in payload:
            raise SystemExit(f"CDP call failed ({plan.get('method') or 'evaluate'}): {payload['error']}")
        return payload["ok"]

    def evaluate(self, body: str) -> object:
        return self._send({"kind": "evaluate", "body": body})

    def cdp(self, method: str, params: dict | None = None) -> object:
        return self._send({"kind": "cdp", "method": method, "params": params or {}})

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f' — {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f' — {detail}' if detail else ''}", flush=True)


# ---------------------------------------------------------------------------
# Page helpers, evaluated in the browser
# ---------------------------------------------------------------------------

JS_HELPERS = r"""
window.__kc = {
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  async until(fn, ms = 20000) {
    const deadline = Date.now() + ms;
    while (Date.now() < deadline) {
      const value = fn();
      if (value) return value;
      await window.__kc.sleep(100);
    }
    return null;
  },
  openAgents() {
    const tab = Array.from(document.querySelectorAll('[role=tab]')).find((node) =>
      /agents/i.test(node.textContent || ''),
    );
    if (tab) { tab.click(); return true; }
    return false;
  },
  cards() {
    return Array.from(document.querySelectorAll('.agent-card'));
  },
  cardText(card) {
    return (card.textContent || '').replace(/\s+/g, ' ').trim();
  },
  findCard(name) {
    return window.__kc.cards().find((card) => window.__kc.cardText(card).includes(name)) || null;
  },
  button(card, label) {
    if (!card) return null;
    return Array.from(card.querySelectorAll('button')).find(
      (node) => (node.getAttribute('aria-label') || '').startsWith(label),
    ) || null;
  },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="kc-agents-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
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
                "[limits]",
                # A short run budget keeps a parked child from outliving the test.
                "max_iterations = 20",
                "",
                "[subagents]",
                "max_concurrent = 4",
                "max_background = 4",
                "allow_background = true",
                "timeout_secs = 120",
                "wait_timeout_secs = 30",
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
            ]
        )
    )

    port = free_port()
    cdp_port = free_port()
    profile = tmp / "chrome-profile"
    profile.mkdir()

    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(ROOT / "target/debug/karucode"),
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
        # A headless browser that inherits the session bus blocks ~25s on the
        # accessibility bus before serving anything.
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

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{api}/"})
        session.evaluate(JS_HELPERS)

        # The session is created through the UI, not the API: the client keys its
        # session list (and therefore the registry of spawned agents, which needs
        # the title and workspace) by its own project scope. A session made behind
        # its back is invisible to the reducer until a reload.
        created = session.evaluate(
            """
            const button = await window.__kc.until(
              () =>
                Array.from(document.querySelectorAll('button')).find((node) =>
                  /^new session/i.test((node.textContent || '').trim()),
                ),
              20000,
            );
            if (!button) return null;
            button.click();
            // The composer only appears once a session is open, so its presence is
            // the signal that the UI really switched.
            const ready = await window.__kc.until(
              () => document.querySelector('textarea, [contenteditable=true]'),
              20000,
            );
            return ready ? 'open' : 'no-composer';
            """
        )
        check("the UI creates and opens a session", created == "open", str(created))

        # The session it just made: newest first in the API the UI reads.
        sessions = http_json(f"{api}/api/sessions")
        rows = sessions.get("sessions") or sessions
        session_id = rows[0]["id"]
        check("the session is the one the UI is on", bool(session_id), str(session_id))

        def prompt(text: str) -> None:
            """Send a prompt, waiting for the previous run to finish first.

            A session runs one thing at a time, so posting while the previous run is
            still alive is a 409 - and a run is now legitimately alive until its
            background children settle (the run's end is what cancels them). Waiting
            for idle makes each `prompt()` mean "start a run", which is what every
            caller below assumes.
            """
            deadline = time.time() + 60.0
            while time.time() < deadline:
                body = http_json(f"{api}/api/sessions/{session_id}")
                if not body.get("running"):
                    break
                time.sleep(0.2)
            else:  # pragma: no cover - only on a hung runtime
                raise SystemExit("the session never went idle before the next prompt")
            http_json(
                f"{api}/api/sessions/{session_id}/messages",
                "POST",
                {"content": text},
            )

        def agent_named(name: str):
            body = http_json(f"{api}/api/agents?session={session_id}")
            return next((a for a in body["agents"] if a["name"] == name), None)

        def wait_agent(name: str, statuses: tuple[str, ...], timeout: float = 30.0):
            deadline = time.time() + timeout
            while time.time() < deadline:
                agent = agent_named(name)
                if agent and agent["status"] in statuses:
                    return agent
                time.sleep(0.2)
            return None

        # --- open the Agents tab ------------------------------------------------
        opened = session.evaluate(
            """
            await window.__kc.until(() => document.querySelector('[role=tab]'));
            return window.__kc.openAgents();
            """
        )
        check("the Agents tab opens", bool(opened), str(opened))
        # Scope the roster to the session under test, so a leftover agent from
        # another session cannot satisfy an assertion.
        session.evaluate(
            """
            const button = Array.from(document.querySelectorAll('.agent-scope-button')).find(
              (node) => /this session/i.test(node.textContent || ''),
            );
            if (button && button.getAttribute('aria-pressed') !== 'true') button.click();
            await window.__kc.sleep(300);
            return true;
            """
        )

        # --- a background agent appears and carries its badge --------------------
        prompt("KICK-OFF")
        background = wait_agent("pause-target", ("running", "paused"))
        check(
            "a background subagent shows up as live",
            background is not None,
            str(background and background["status"]),
        )
        check(
            "the row records that the spawn detached",
            bool(background and background.get("background")),
            str(background and background.get("background")),
        )

        row = session.evaluate(
            """
            await window.__kc.openAgents();
            const card = await window.__kc.until(() => window.__kc.findCard('pause-target'));
            return card ? window.__kc.cardText(card) : null;
            """
        )
        check(
            "the roster renders the agent with a background badge",
            bool(row) and "background" in (row or ""),
            str(row)[:120],
        )
        check(
            "a live agent has Pause and Stop controls",
            bool(
                session.evaluate(
                    """
                    const card = await window.__kc.until(() => window.__kc.findCard('pause-target'));
                    return !!(window.__kc.button(card, 'Pause') && window.__kc.button(card, 'Stop'));
                    """
                )
            ),
        )

        # --- pause --------------------------------------------------------------
        session.evaluate(
            """
            const card = await window.__kc.until(() => window.__kc.findCard('pause-target'));
            window.__kc.button(card, 'Pause').click();
            return true;
            """
        )
        paused = wait_agent("pause-target", ("paused",))
        check("Pause parks the agent", paused is not None, str(paused and paused["status"]))
        check(
            "a paused agent is still live (it holds a slot)",
            paused is not None and not paused.get("finished_at"),
            str(paused and paused.get("finished_at")),
        )

        paused_row = session.evaluate(
            """
            await window.__kc.until(() => {
              const card = window.__kc.findCard('pause-target');
              return card && /paused/i.test(window.__kc.cardText(card));
            });
            const card = window.__kc.findCard('pause-target');
            return {
              text: window.__kc.cardText(card),
              hasResume: !!window.__kc.button(card, 'Resume'),
              status: card.getAttribute('data-status'),
            };
            """
        )
        check(
            "the roster chip says paused",
            bool(paused_row) and paused_row.get("status") == "paused",
            str(paused_row),
        )
        check(
            "a paused agent offers Resume instead of Pause",
            bool(paused_row) and paused_row.get("hasResume") is True,
            str(paused_row and paused_row.get("hasResume")),
        )

        # --- resume -------------------------------------------------------------
        session.evaluate(
            """
            const card = window.__kc.findCard('pause-target');
            window.__kc.button(card, 'Resume').click();
            return true;
            """
        )
        resumed = wait_agent("pause-target", ("running", "completed", "failed"), timeout=30)
        check(
            "Resume releases the agent",
            resumed is not None,
            str(resumed and resumed["status"]),
        )

        # --- stop ---------------------------------------------------------------
        prompt("STOP-TARGET")
        stop_agent = wait_agent("stop-target", ("running",), timeout=30)
        session.evaluate(
            """
            const card = await window.__kc.until(() => window.__kc.findCard('stop-target'));
            window.__kc.button(card, 'Stop').click();
            return true;
            """
        )
        stopped = wait_agent("stop-target", ("cancelled",), timeout=30)
        check("Stop ends the agent", stopped is not None, str(stopped and stopped["status"]))

        # A settled agent must not offer controls at all: a dead button that 404s
        # is worse than no button.
        settled = session.evaluate(
            """
            await window.__kc.until(() => {
              const card = window.__kc.findCard('stop-target');
              return card && !window.__kc.button(card, 'Pause') && !window.__kc.button(card, 'Stop');
            }, 8000);
            const card = window.__kc.findCard('stop-target');
            return {
              hasPause: !!window.__kc.button(card, 'Pause'),
              hasStop: !!window.__kc.button(card, 'Stop'),
              text: window.__kc.cardText(card).slice(0, 120),
            };
            """
        )
        check(
            "a settled agent has no control buttons",
            bool(settled) and not settled.get("hasPause") and not settled.get("hasStop"),
            str(settled),
        )

        # --- report_status -----------------------------------------------------
        prompt("REPORT-TARGET")
        reported = wait_agent("report-target", ("running", "paused", "completed"), timeout=30)
        check(
            "report_status reaches the agent's row",
            bool(reported) and "halfway through crates/" in (reported.get("note") or ""),
            str(reported and reported.get("note")),
        )
        check(
            "the claimed progress is stored as a fraction",
            bool(reported) and reported.get("progress") == 0.5,
            str(reported and reported.get("progress")),
        )
        note_row = session.evaluate(
            """
            await window.__kc.until(() => {
              const card = window.__kc.findCard('report-target');
              return card && /halfway through/.test(window.__kc.cardText(card));
            }, 10000);
            const card = window.__kc.findCard('report-target');
            return {
              text: window.__kc.cardText(card).slice(0, 160),
              hasBar: !!card.querySelector('.agent-progress-fill'),
              width: (card.querySelector('.agent-progress-fill') || {}).style?.width || '',
            };
            """
        )
        check(
            "the note and its progress bar render on the card",
            bool(note_row) and note_row.get("hasBar") is True,
            str(note_row),
        )
        check(
            "the progress bar reflects the claimed fraction",
            bool(note_row) and note_row.get("width") == "50%",
            str(note_row and note_row.get("width")),
        )

        # --- cleanup: stop whatever is still live -------------------------------
        for name in ("pause-target", "report-target"):
            agent = agent_named(name)
            if agent and agent["status"] in ("running", "paused"):
                try:
                    http_json(f"{api}/api/agents/{agent['id']}/cancel", "POST", {})
                except Exception:
                    pass

        # --- layout at two widths ----------------------------------------------
        for width, height, label in ((1280, 900, "1280px"), (390, 844, "390px")):
            session.cdp(
                "Emulation.setDeviceMetricsOverride",
                {
                    "width": width,
                    "height": height,
                    "deviceScaleFactor": 1,
                    "mobile": width < 600,
                },
            )
            overflow = session.evaluate(
                """
                await window.__kc.sleep(400);
                await window.__kc.openAgents();
                await window.__kc.sleep(300);
                const doc = document.documentElement;
                return {
                  scroll: doc.scrollWidth - doc.clientWidth,
                  cards: document.querySelectorAll('.agent-card').length,
                };
                """
            )
            check(
                f"the Agents tab does not overflow at {label}",
                bool(overflow) and overflow.get("scroll", 1) <= 1,
                str(overflow),
            )
        session.cdp("Emulation.clearDeviceMetricsOverride")

        # --- a screenshot, for the record --------------------------------------
        shot = session.cdp("Page.captureScreenshot", {"format": "png"})
        out = ROOT / "reports" / "agents-tab.png"
        out.parent.mkdir(exist_ok=True)
        import base64

        out.write_bytes(base64.b64decode(shot["data"]))
        check("a screenshot of the tab was captured", out.exists(), str(out))

    finally:
        if not args.keep:
            if session is not None:
                session.close()
            for process in (chrome, server):
                try:
                    process.terminate()
                    process.wait(timeout=5)
                except Exception:
                    process.kill()
            provider.shutdown()
            server_log.close()
            chrome_log.close()
            shutil.rmtree(tmp, ignore_errors=True)
            if not args.keep:
                pass

    total = len(PASS) + len(FAIL)
    print(f"\n{len(PASS)}/{total} checks passed", flush=True)
    if FAIL:
        print("failed: " + ", ".join(FAIL), flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())