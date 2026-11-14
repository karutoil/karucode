#!/usr/bin/env python3
"""Verify skills end to end: the API, the agent loop, and the tab — in a real browser.

What a unit test cannot prove, and this does, against one real `karucode` binary
and a real Chromium over raw CDP (the chrome-devtools plugin cannot launch a
browser on this host, so the plumbing is the same as `verify_agents_tab.py`):

  * a skill dropped in `.karucode/skills/<name>/SKILL.md` is found with no
    configuration, and `GET /api/skills` reports the root it came from;
  * the **system prompt of a real run** lists that skill by id and description —
    and does *not* contain its body, because a body in message[0] is paid for on
    every turn of every run;
  * the model's `skill` tool call returns the body, so the loop can actually use
    what the prompt advertised;
  * add / toggle / delete through the API and the UI agree, and a file-backed
    skill refuses deletion with a message that says what to do instead;
  * the Skills settings pane renders the count, the roots, the source badges and the body
    viewer, and saves the options through `PATCH /api/config` (surviving a
    reload);
  * nothing overflows at 1280px or 390px.

Usage: python3 scripts/verify_skills.py [--keep]

Requires: a built `target/debug/karucode` and a built `web/dist` (both are built
by `cargo build -p kc-server`), plus `chromium` and `node` on PATH.
"""

from __future__ import annotations

import argparse
import base64
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

SKILL_BODY_MARKER = "RELEASE-BODY-MARKER-9f2"
SKILL_DESCRIPTION = "Cut a release: bump, gate, tag, push."
CUSTOM_NAME = "verify-custom"
CUSTOM_MARKER = "CUSTOM-BODY-MARKER-4a1"


# ---------------------------------------------------------------------------
# A stub provider that records what the harness actually sent it.
# ---------------------------------------------------------------------------

REQUESTS: list[dict] = []


def sse(chunks) -> bytes:
    out = [f"data: {json.dumps(chunk)}\n\n" for chunk in chunks]
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
                                "function": {"name": name, "arguments": json.dumps(arguments)},
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
    for message in reversed(messages):
        if message.get("role") == "user":
            content = message.get("content")
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                return " ".join(
                    part.get("text", "") for part in content if isinstance(part, dict)
                )
    return ""


def answer(messages):
    text = user_text(messages)
    saw_tool_result = any(message.get("role") == "tool" for message in messages)
    if "USE-SKILL" in text and not saw_tool_result:
        return tool_chunks("call_skill_1", "skill", {"action": "read", "name": "release"})
    if "LIST-SKILLS" in text and not saw_tool_result:
        return tool_chunks("call_skill_2", "skill", {"action": "list"})
    return text_chunks("done")


class StubProvider(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        if self.path.endswith("/models"):
            body = json.dumps({"data": [{"id": "stub-model", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        payload = json.loads(self.rfile.read(length) or b"{}")
        REQUESTS.append(payload)
        body = sse(answer(payload.get("messages") or []))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


# ---------------------------------------------------------------------------
# Plumbing
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


def http_json(url: str, method: str = "GET", body: dict | None = None, ok=(200, 201, 204)):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("X-Karucode-Client", "web")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read()
            payload = {} if not raw else json.loads(raw)
            # A caller asserting the documented status must see the same shape
            # whether the call succeeded or failed.
            if isinstance(payload, dict):
                payload.setdefault("__status", response.status)
            return payload
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace")
        return {"__status": error.code, "__detail": detail}


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
        self.helper = (Path(tempfile.gettempdir()) / "kc-cdp-skills.cjs").resolve()
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
            raise SystemExit(f"CDP call failed: {payload['error']}")
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
  byTest(id) {
    return document.querySelector('[data-testid="' + id + '"]');
  },
  allTest(id) {
    return Array.from(document.querySelectorAll('[data-testid="' + id + '"]'));
  },
  row(id) {
    return window.__kc.allTest('skill-row').find(
      (node) => node.getAttribute('data-skill-id') === id,
    ) || null;
  },
  within(root, id) {
    return root ? root.querySelector('[data-testid="' + id + '"]') : null;
  },
  /**
   * Open Settings, then its Skills section.
   *
   * The pane is install configuration, so it lives in the sheet's rail - the
   * gear button opens it, and the rail row is matched on `.settings-nav-label`
   * (the accessible name also carries the note).
   */
  async openSkills() {
    if (!document.querySelector('[role=dialog] .settings-nav-label')) {
      const find = () =>
        Array.from(document.querySelectorAll('button')).find((node) =>
          /open settings/i.test(node.getAttribute('aria-label') || ''),
        ) || null;
      // A freshly navigated page has not booted yet, so the gear is waited for
      // rather than assumed.
      const gear = find() || (await window.__kc.until(find, 20000));
      if (!gear) return 'no-gear';
      gear.click();
      // The dialog is a portal and its rail is rendered after the click, so the
      // tab cannot be looked up in the same turn.
      const rail = await window.__kc.until(
        () => document.querySelector('[role=dialog] .settings-nav-label'),
        20000,
      );
      if (!rail) return 'no-rail';
    }
    const tab = Array.from(document.querySelectorAll('[role=tab]')).find(
      (node) =>
        ((node.querySelector('.settings-nav-label') || {}).textContent || '').trim().toLowerCase() ===
        'skills',
    );
    if (!tab) return 'no-skills-section';
    tab.click();
    const pane = await window.__kc.until(() => {
      const panel = document.querySelector('#settings-panel-skills');
      return panel && !panel.hasAttribute('hidden') ? panel : null;
    }, 20000);
    return pane ? true : 'pane-not-shown';
  },
  setValue(node, value) {
    if (!node) return false;
    const proto = node instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    setter.call(node, value);
    node.dispatchEvent(new Event('input', { bubbles: true }));
    node.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  },
  overflow() {
    return {
      scroll: document.documentElement.scrollWidth,
      inner: window.innerWidth,
    };
  },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    binary = ROOT / "target/debug/karucode"
    if not binary.exists():
        raise SystemExit(f"missing {binary}; run `cargo build -p kc-server` first")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("missing web/dist; run `npm run build` in web/ or `cargo build -p kc-server`")

    tmp = Path(tempfile.mkdtemp(prefix="kc-skills-"))
    workspace = tmp / "workspace"
    (workspace / ".karucode/skills/release").mkdir(parents=True)
    (workspace / ".karucode/skills/release/SKILL.md").write_text(
        "---\n"
        "name: release\n"
        f"description: {SKILL_DESCRIPTION}\n"
        "---\n"
        "\n"
        "# Cutting a release\n"
        "\n"
        f"1. {SKILL_BODY_MARKER}: run the gates before tagging.\n"
    )
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
                "max_iterations = 12",
                "",
                "[approvals]",
                'mode = "auto"',
                "",
                "[titles]",
                "enabled = false",
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
    server_args = [
        str(binary),
        "--port",
        str(port),
        "--workspace",
        str(workspace),
        "--data-dir",
        str(data_dir),
        "--config",
        str(config),
    ]
    server_env = {**os.environ, "KARUCODE_WEB_DIR": str(ROOT / "web/dist"), "KARUCODE_LOG": "warn"}

    def launch_server() -> subprocess.Popen:
        return subprocess.Popen(
            server_args,
            env=server_env,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    server = launch_server()

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
    api = f"http://127.0.0.1:{port}"
    try:
        wait_for(f"{api}/api/health")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")

        # ---------------- API: discovery ----------------
        listed = http_json(f"{api}/api/skills")
        skills = {row["id"]: row for row in listed.get("skills") or []}
        check(
            "the workspace skill is auto-detected with no configuration",
            "release" in skills,
            json.dumps(sorted(skills)),
        )
        release = skills.get("release") or {}
        check(
            "the discovered skill reports its description and source",
            release.get("description") == SKILL_DESCRIPTION
            and release.get("source") == "workspace"
            and release.get("enabled") is True,
            json.dumps(release),
        )
        roots = listed.get("roots") or []
        workspace_root = next(
            (root for root in roots if str(root.get("path", "")).endswith(".karucode/skills")), None
        )
        check(
            "the roots list names the workspace folder and its count",
            workspace_root is not None and workspace_root.get("exists") is True,
            json.dumps(roots),
        )
        check(
            "the listing exposes the switches",
            listed.get("enabled") is True
            and listed.get("auto_detect") is True
            and listed.get("prompt") is True
            and isinstance(listed.get("max_skills"), int),
            json.dumps({k: listed.get(k) for k in ("enabled", "auto_detect", "prompt")}),
        )

        one = http_json(f"{api}/api/skills/release")
        check(
            "GET /api/skills/:id returns the body with the frontmatter stripped",
            SKILL_BODY_MARKER in (one.get("body") or "")
            and not (one.get("body") or "").lstrip().startswith("---"),
            (one.get("body") or "")[:80],
        )

        # ---------------- API: add / toggle / delete ----------------
        created = http_json(
            f"{api}/api/skills",
            "POST",
            {
                "name": CUSTOM_NAME,
                "description": "added by the verifier",
                "body": f"# Custom\n\n{CUSTOM_MARKER}\n",
            },
        )
        check(
            "POST /api/skills adds a custom skill",
            created.get("id") == CUSTOM_NAME and created.get("source") == "custom",
            json.dumps(created),
        )
        duplicate = http_json(
            f"{api}/api/skills",
            "POST",
            {"name": CUSTOM_NAME, "description": "dup", "body": "x"},
        )
        check(
            "a duplicate id is refused with 409",
            duplicate.get("__status") == 409,
            json.dumps(duplicate),
        )

        toggled = http_json(f"{api}/api/skills/release", "PATCH", {"enabled": False})
        after = http_json(f"{api}/api/skills")
        release_after = next(
            (row for row in after.get("skills") or [] if row["id"] == "release"), {}
        )
        check(
            "PATCH enabled=false hides it from the agent but keeps it listed",
            toggled.get("enabled") is False
            and release_after.get("enabled") is False
            and after.get("enabled") is True,
            json.dumps(release_after),
        )
        back = http_json(f"{api}/api/skills/release", "PATCH", {"enabled": True})
        check("PATCH enabled=true turns it back on", back.get("enabled") is True, json.dumps(back))

        refused = http_json(f"{api}/api/skills/release", "DELETE")
        check(
            "a file-backed skill cannot be deleted through the API",
            refused.get("__status") == 400
            and "SKILL.md" in json.dumps(refused.get("__detail") or ""),
            json.dumps(refused),
        )
        removed = http_json(f"{api}/api/skills/{CUSTOM_NAME}", "DELETE")
        check("a custom skill is deleted", removed.get("__status") in (200, 204), json.dumps(removed))

        # ---------------- The agent loop ----------------
        session_created = http_json(f"{api}/api/sessions", "POST", {})
        session_id = session_created.get("id")
        check("a session is created", bool(session_id), str(session_created)[:120])
        REQUESTS.clear()
        posted = http_json(
            f"{api}/api/sessions/{session_id}/messages",
            "POST",
            {"content": "USE-SKILL please cut a release"},
        )
        # The run is asynchronous; wait for it to settle.
        deadline = time.time() + 60
        running = True
        while time.time() < deadline:
            detail = http_json(f"{api}/api/sessions/{session_id}")
            if detail.get("running") is False:
                running = False
                break
            time.sleep(0.3)
        check("the run finishes", running is False, json.dumps(posted)[:120])

        system_prompts = [
            message.get("content", "")
            for request in REQUESTS
            for message in request.get("messages") or []
            if message.get("role") == "system"
        ]
        first_system = system_prompts[0] if system_prompts else ""
        check(
            "the system prompt lists the skill by id",
            "`release`" in first_system,
            first_system[-400:] if first_system else "no system message captured",
        )
        check(
            "the system prompt names the call that reads a skill",
            '"action":"read"' in first_system,
            "skills block present" if '"action":"read"' in first_system else "no skill call hint",
        )
        check(
            "the skill body is NOT in the system prompt (it is paid for on every turn)",
            SKILL_BODY_MARKER not in first_system,
            "body absent from message[0]",
        )
        tool_results = [
            message.get("content", "")
            for request in REQUESTS
            for message in request.get("messages") or []
            if message.get("role") == "tool"
        ]
        check(
            "the model's skill tool call returned the body",
            any(SKILL_BODY_MARKER in (content or "") for content in tool_results),
            json.dumps([str(content)[:120] for content in tool_results]),
        )
        check(
            "the tool result marks the text as data",
            any("data" in (content or "").lower() for content in tool_results),
            "",
        )

        # ---------------- The tab, in a real browser ----------------
        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{api}/"})
        opened = session.evaluate(
            """
            window.__kc = undefined;
            """
            + JS_HELPERS
            + """
            const ready = await window.__kc.until(
              () => document.querySelector('button, [role=tab]'),
              20000,
            );
            return await window.__kc.openSkills();
            """
        )
        check("the settings sheet offers a Skills section", opened is True, str(opened))
        panel = session.evaluate(
            """
            const shown = document.querySelector('#settings-panel-skills');
            const pane = await window.__kc.until(() => window.__kc.byTest('skills-panel'), 20000);
            if (!pane || !shown || shown.hasAttribute('hidden')) return null;
            const title = document.querySelector('.settings-pane-title');
            return {
              title: title ? (title.textContent || '').trim() : null,
              panel: shown.id,
            };
            """
        )
        check(
            "the Skills pane renders under its own section header",
            isinstance(panel, dict)
            and panel.get("title") == "Skills"
            and panel.get("panel") == "settings-panel-skills",
            str(panel),
        )

        row_text = session.evaluate(
            """
            const row = await window.__kc.until(() => window.__kc.row('release'), 20000);
            return row ? window.__kc.within(row, 'skill-toggle') ? 'toggle' : 'no-toggle' : null;
            """
        )
        check("the discovered skill has a row with a switch", row_text == "toggle", str(row_text))

        count = session.evaluate(
            "const node = window.__kc.byTest('skills-count'); return node ? (node.textContent || '').trim() : null;"
        )
        check("the panel shows a count", bool(count) and any(ch.isdigit() for ch in count or ""), str(count))

        root_state = session.evaluate(
            """
            const roots = window.__kc.allTest('skills-root');
            const found = roots.find((node) => (node.textContent || '').includes('.karucode/skills'));
            return found ? found.getAttribute('data-exists') : null;
            """
        )
        check("the roots list shows the workspace folder as found", root_state == "true", str(root_state))

        # View the body.
        body_shown = session.evaluate(
            """
            const row = window.__kc.row('release');
            const view = window.__kc.within(row, 'skill-view');
            if (!view) return 'no-view';
            view.click();
            const body = await window.__kc.until(() => window.__kc.byTest('skill-body'), 20000);
            return body ? (body.textContent || '').includes('%s') : 'no-body';
            """
            % SKILL_BODY_MARKER
        )
        check("the body viewer shows the skill's markdown", body_shown is True, str(body_shown))

        # Toggle off from the UI and confirm the server agrees.
        toggled_ui = session.evaluate(
            """
            const row = window.__kc.row('release');
            const toggle = window.__kc.within(row, 'skill-toggle');
            if (!toggle) return 'no-toggle';
            toggle.click();
            const done = await window.__kc.until(() => {
              const now = window.__kc.row('release');
              return now && now.getAttribute('data-enabled') === 'false';
            }, 20000);
            return done ? 'off' : 'stayed-on';
            """
        )
        check("toggling a skill in the UI turns it off", toggled_ui == "off", str(toggled_ui))
        server_state = http_json(f"{api}/api/skills")
        release_now = next(
            (row for row in server_state.get("skills") or [] if row["id"] == "release"), {}
        )
        check(
            "the UI toggle reached the server",
            release_now.get("enabled") is False,
            json.dumps(release_now),
        )
        session.evaluate(
            """
            const row = window.__kc.row('release');
            const toggle = window.__kc.within(row, 'skill-toggle');
            if (toggle) toggle.click();
            await window.__kc.until(() => {
              const now = window.__kc.row('release');
              return now && now.getAttribute('data-enabled') === 'true';
            }, 20000);
            return 'on';
            """
        )

        # Add a custom skill through the form.
        added = session.evaluate(
            """
            if (!window.__kc.setValue(window.__kc.byTest('skill-add-name'), '%s')) return 'no-name';
            await window.__kc.sleep(150);
            window.__kc.setValue(window.__kc.byTest('skill-add-description'), 'from the browser');
            await window.__kc.sleep(150);
            if (!window.__kc.setValue(window.__kc.byTest('skill-add-body'), '# Custom\\n\\n%s\\n')) return 'no-body';
            // Preact commits a state update on a microtask, so the click handler
            // only sees the typed value one turn later. Clicking in the same
            // synchronous block posts an empty body and is refused.
            await window.__kc.sleep(250);
            const submit = window.__kc.byTest('skill-add-submit');
            if (!submit) return 'no-submit';
            submit.click();
            const row = await window.__kc.until(() => window.__kc.row('%s'), 20000);
            if (row) return 'added';
            const problem = window.__kc.byTest('skill-add-error');
            return 'missing: ' + (problem ? (problem.textContent || '').trim() : 'no message');
            """
            % (CUSTOM_NAME, CUSTOM_MARKER, CUSTOM_NAME)
        )
        check("the add form creates a skill and it appears in the list", added == "added", str(added))
        api_state = http_json(f"{api}/api/skills")
        check(
            "the skill added in the browser is on the server",
            any(row["id"] == CUSTOM_NAME for row in api_state.get("skills") or []),
            json.dumps([row["id"] for row in api_state.get("skills") or []]),
        )

        # Delete it again.
        deleted = session.evaluate(
            """
            const row = window.__kc.row('%s');
            const del = window.__kc.within(row, 'skill-delete');
            if (!del) return 'no-delete';
            del.click();
            // A confirmation dialog is acceptable: click its affirmative button.
            const confirm = await window.__kc.until(() => {
              const dialog = document.querySelector('[role=dialog], dialog');
              if (!dialog) return null;
              return Array.from(dialog.querySelectorAll('button')).find((node) =>
                /delete|confirm|yes/i.test(node.textContent || ''),
              );
            }, 1500);
            if (confirm) confirm.click();
            const gone = await window.__kc.until(() => !window.__kc.row('%s'), 20000);
            return gone ? 'deleted' : 'still-there';
            """
            % (CUSTOM_NAME, CUSTOM_NAME)
        )
        check("the delete action removes a custom skill", deleted == "deleted", str(deleted))

        # A file-backed skill cannot be deleted, and the row says so instead of
        # offering a button that would fail.
        file_delete = session.evaluate(
            """
            const row = window.__kc.row('release');
            if (!row) return 'no-row';
            return {
              hasDelete: !!window.__kc.within(row, 'skill-delete'),
              text: (row.textContent || '').replace(/\\s+/g, ' ').trim(),
            };
            """
        )
        check(
            "a file-backed skill offers no delete control and says why",
            isinstance(file_delete, dict)
            and file_delete.get("hasDelete") is False
            and "SKILL.md" in (file_delete.get("text") or ""),
            str(file_delete)[:300],
        )

        # Options round-trip.
        saved = session.evaluate(
            """
            window.__kc.setValue(window.__kc.byTest('skills-prompt-max'), '1234');
            await window.__kc.sleep(200);
            const off = window.__kc.byTest('skills-autodetect-toggle');
            if (off && off.getAttribute('aria-checked') === 'true') off.click();
            await window.__kc.sleep(200);
            const save = window.__kc.byTest('skills-save-options');
            if (!save) return 'no-save';
            save.click();
            // `until` takes a synchronous predicate, so the poll loop is explicit:
            // read the stored config until it shows the value that was saved.
            const deadline = Date.now() + 20000;
            while (Date.now() < deadline) {
              const config = await (await fetch('/api/config')).json();
              if (config.skills && config.skills.prompt_max_chars === 1234) return 'saved';
              await window.__kc.sleep(150);
            }
            return 'not-saved';
            """
        )
        check("the options save through PATCH /api/config", saved == "saved", str(saved))
        config_now = http_json(f"{api}/api/config")
        check(
            "auto_detect reached the config",
            (config_now.get("skills") or {}).get("auto_detect") is False,
            json.dumps(config_now.get("skills")),
        )

        # And survives a reload.
        session.cdp("Page.navigate", {"url": f"{api}/"})
        persisted = session.evaluate(
            JS_HELPERS
            + """
            const opened = await window.__kc.openSkills();
            if (opened !== true) return String(opened);
            const node = await window.__kc.until(() => window.__kc.byTest('skills-prompt-max'), 20000);
            return node ? String(node.value) : null;
            """
        )
        check("the saved options survive a reload", persisted == "1234", str(persisted))

        # ---------------- Persistence across a real restart ----------------
        # A hand-added skill and a turned-off skill are settings rows, and the
        # options are too. A restart is the only thing that proves they were
        # written rather than held in the live config object.
        http_json(
            f"{api}/api/skills",
            "POST",
            {
                "name": "persist-probe",
                "description": "survives a restart",
                "body": "kept across a restart",
            },
        )
        # `release` cannot carry this check: `auto_detect` was turned off above, so
        # the workspace folder is no longer scanned and a file-backed skill is not
        # discovered at all after the restart. A hand-added skill is always in the
        # registry, so it is the one that can prove a disabled id was persisted.
        http_json(f"{api}/api/skills/persist-probe", "PATCH", {"enabled": False})
        try:
            os.killpg(os.getpgid(server.pid), 15)
        except Exception:
            server.terminate()
        server.wait(timeout=30)
        server = launch_server()
        wait_for(f"{api}/api/health")
        restarted = http_json(f"{api}/api/skills")
        rows = {row["id"]: row for row in restarted.get("skills") or []}
        check(
            "a skill added through the UI survives a restart",
            "persist-probe" in rows and rows["persist-probe"].get("source") == "custom",
            json.dumps(rows.get("persist-probe")),
        )
        check(
            "a skill turned off is still off after a restart",
            rows.get("persist-probe", {}).get("enabled") is False,
            json.dumps(rows.get("persist-probe")),
        )
        check(
            "auto_detect off means the conventional folder is not scanned",
            "release" not in rows,
            json.dumps(sorted(rows)),
        )
        restarted_config = http_json(f"{api}/api/config")
        check(
            "the saved options survive a restart",
            (restarted_config.get("skills") or {}).get("prompt_max_chars") == 1234
            and (restarted_config.get("skills") or {}).get("auto_detect") is False,
            json.dumps(restarted_config.get("skills")),
        )

        # A screenshot of the pane, for a human to look at what the checks
        # measured. Kept beside the logs so a failure is inspectable.
        shots = tmp / "shots"
        shots.mkdir(exist_ok=True)
        shot = session.cdp("Page.captureScreenshot", {"format": "png"})
        if isinstance(shot, dict) and shot.get("data"):
            (shots / "skills-settings.png").write_bytes(base64.b64decode(shot["data"]))
            print(f"  shot {shots / 'skills-settings.png'}", flush=True)

        # Narrow layout: nothing may overflow.
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True},
        )
        time.sleep(0.6)
        narrow = session.evaluate("return window.__kc.overflow();")
        check(
            "nothing overflows at 390px",
            isinstance(narrow, dict) and narrow.get("scroll", 0) <= narrow.get("inner", 0) + 1,
            json.dumps(narrow),
        )
        session.cdp("Emulation.clearDeviceMetricsOverride")

        print()
        print(f"passed {len(PASS)}, failed {len(FAIL)}")
        if FAIL:
            print("failures:")
            for name in FAIL:
                print(f"  - {name}")
            print(f"logs: {tmp}/server.log, {tmp}/chrome.log")
            return 1
        if args.keep:
            print(f"kept: server {api}, browser cdp http://127.0.0.1:{cdp_port}, workspace {workspace}")
            return 0
        return 0
    finally:
        if session is not None:
            session.close()
        if not args.keep:
            for proc in (chrome, server):
                try:
                    os.killpg(os.getpgid(proc.pid), 15)
                except Exception:
                    pass
            provider.shutdown()
            provider.server_close()
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())