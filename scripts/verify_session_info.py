#!/usr/bin/env python3
"""Drive the inspector's **Session** tab in a real Chromium over raw CDP.

What the unit tests cannot prove, and this does:

  * the tab really replaces *Commands* in the strip, in a browser;
  * opening it issues `GET /api/sessions/:id/info` and the numbers on screen are
    the ones the server returned (not a client-side re-derivation);
  * a real run through a scripted provider moves the context meter, the token
    tiles and the working-time row — i.e. the runtime really persisted usage and
    the panel really picked it up;
  * a subagent appears as its own row with its own tokens;
  * a session whose model declares no `context_window` shows no percentage;
  * the panel fits the inspector without horizontal overflow at 1280px and on a
    phone-width viewport.

The three things this host forces, all learned the hard way:

  * Chromium needs `--no-sandbox`, an explicit `--user-data-dir`, and
    `setsid nohup` (a plain background job dies with the command's process group);
  * `KARUCODE_WEB_DIR` is mandatory, because the embedded `dist` is a build-time
    snapshot;
  * one persistent CDP WebSocket for the whole run: an
    `Emulation.setDeviceMetricsOverride` issued on a connection that then closes
    is not in force for the next one.

Usage: python3 scripts/verify_session_info.py [--keep]
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def format_tokens(value: int) -> str:
    """The same scale the UI uses, so an assertion compares like with like."""
    if value < 1000:
        return str(value)
    if value < 1_000_000:
        thousands = value / 1000
        return f"{round(thousands)}k" if thousands >= 100 else f"{thousands:.1f}k"
    millions = value / 1_000_000
    return f"{round(millions)}M" if millions >= 10 else f"{millions:.1f}M"


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
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read())


CDP_HELPER = r"""
/* A tiny CDP REPL: newline-delimited JSON on stdin, one JSON line per response. */
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
ws.addEventListener('error', (event) => {
  console.log(JSON.stringify({ error: 'ws ' + (event.message || 'error') }));
  process.exit(1);
});
ws.addEventListener('open', async () => {
  const readline = await import('node:readline');
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
    """One CDP connection for the whole run, driven over a line protocol."""

    def __init__(self, ws_url: str) -> None:
        self.helper = Path(tempfile.gettempdir()) / "kc-cdp-repl-session.mjs"
        self.helper.write_text(CDP_HELPER)
        self.proc = subprocess.Popen(
            ["node", str(self.helper), ws_url],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            start_new_session=True,
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
            raise SystemExit(
                f"CDP call failed ({plan.get('method') or 'evaluate'}): {payload['error']}"
            )
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


# Waits for the app to boot and returns once the inspector tab strip exists.
BOOT = """
const deadline = Date.now() + 30000;
while (Date.now() < deadline) {
  const list = document.querySelector('[role=tablist][aria-label="Inspector sections"]');
  if (list) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}
return 'timeout';
"""

OPEN_TAB = """
const click = (name) => {
  const tab = Array.from(document.querySelectorAll('[role=tab]')).find(
    (node) => (node.textContent || '').trim().startsWith(name)
  );
  if (!tab) return null;
  tab.click();
  return tab;
};
const tab = click('Session');
await new Promise((r) => setTimeout(r, 600));
return tab ? tab.getAttribute('aria-selected') : null;
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    if not (ROOT / "target/debug/karucode").exists():
        raise SystemExit("build first: cargo build -p kc-server")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("build the UI first: (cd web && npm run build)")

    session: Session | None = None
    tmp = Path(tempfile.mkdtemp(prefix="kc-sessioninfo-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()
    log_path = tmp / "provider.jsonl"

    provider_port = free_port()
    mock = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(log_path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )

    # A model that DOES declare a window, so the meter has a denominator; the
    # second session is pinned to one with no window to prove the "no
    # percentage" branch in a browser.
    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                "[model]",
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
                "",
                "[[providers]]",
                'id = "default"',
                'name = "Default"',
                'provider = "openai_compatible"',
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'api_key = "sk-test"',
                "",
                "[[providers.models]]",
                'id = "gpt-4o-mini"',
                'name = "GPT-4o mini"',
                "context_window = 128000",
                "cost_input = 3.0",
                "cost_output = 15.0",
                "",
                "[[providers.models]]",
                'id = "mystery"',
                'name = "Mystery"',
                "",
                "[active]",
                'provider = "default"',
                'model = "gpt-4o-mini"',
                "",
                "[subagents]",
                "max_concurrent = 2",
                "allow_recursive = false",
                "timeout_secs = 60",
                "",
                "[notifications]",
                "enabled = false",
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
            "--window-size=1440,960",
            "about:blank",
        ],
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    base = f"http://127.0.0.1:{port}"
    try:
        wait_for(f"{base}/api/health")
        wait_for(f"http://127.0.0.1:{provider_port}/v1/models")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")

        # Sessions must be created *in the project*: the rail is project-scoped,
        # so a session created without one never appears in it and the browser
        # would open with "No sessions yet".
        project = http_json(f"{base}/api/projects")["projects"][0]["id"]

        # ---- a session that does real work, including a subagent -----------
        first = http_json(
            f"{base}/api/sessions",
            "POST",
            {"title": "Session info demo", "project": project},
        )["id"]
        # `[tool:agent]` makes the mock spawn a subagent, so the panel has an
        # agent row with its own tokens to show.
        http_json(f"{base}/api/sessions/{first}/messages", "POST",
                  {"content": "[tool:agent] delegate the review"})
        # Two turns, so there is more than one run and a real working time.
        for _ in range(120):
            detail = http_json(f"{base}/api/sessions/{first}")
            if not detail["running"]:
                break
            time.sleep(0.25)
        http_json(f"{base}/api/sessions/{first}/messages", "POST",
                  {"content": "[text] summarise what you did"})
        for _ in range(120):
            detail = http_json(f"{base}/api/sessions/{first}")
            if not detail["running"]:
                break
            time.sleep(0.25)


        info = http_json(f"{base}/api/sessions/{first}/info")
        print(f"  server: {info['stats']['messages_total']} messages, "
              f"{info['runs']['total']} runs, {info['usage']['total']['calls']} calls, "
              f"{len(info['agents'])} agents", flush=True)

        # A second session pinned to a model with no declared window.
        second = http_json(
            f"{base}/api/sessions",
            "POST",
            {"title": "No window", "project": project},
        )["id"]
        http_json(f"{base}/api/sessions/{second}/model", "PATCH",
                  {"provider": "default", "model": "mystery"})
        http_json(f"{base}/api/sessions/{second}/messages", "POST", {"content": "[text] hi"})
        for _ in range(120):
            detail = http_json(f"{base}/api/sessions/{second}")
            if not detail["running"]:
                break
            time.sleep(0.25)

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{base}/"})
        check("the app boots", session.evaluate(BOOT) == "ready")

        # ---- the tab strip -------------------------------------------------
        tabs = session.evaluate(
            """
            return Array.from(
              document.querySelectorAll('[role=tablist][aria-label="Inspector sections"] [role=tab]')
            ).map((node) => (node.textContent || '').trim());
            """
        )
        check("the inspector no longer has a Commands tab",
              not any(t.startswith("Commands") for t in tabs), str(tabs))
        check("the inspector has a Session tab", any(t.startswith("Session") for t in tabs), str(tabs))
        check("the built-in tab count is unchanged (4)",
              len([t for t in tabs if t in ("Files", "Changes", "Session", "Agents")
                   or t.startswith(("Files", "Changes", "Session", "Agents"))]) == 4, str(tabs))

        # The app opens the most recently updated session, which is the
        # no-window one: select the demo session first, the way a user would.
        picked = session.evaluate(
            f"""
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {{
              const row = Array.from(document.querySelectorAll('button, [role=button]')).find(
                (node) => (node.textContent || '').includes('Session info demo')
              );
              if (row) {{ row.click(); return 'clicked'; }}
              await new Promise((r) => setTimeout(r, 150));
            }}
            return 'not-found';
            """
        )
        check("the demo session can be selected from the rail", picked == "clicked", str(picked))
        time.sleep(1.2)
        # The inspector starts collapsed; open it, or the panel is measured at
        # its 64px collapsed width and every layout check below is meaningless.
        opened = session.evaluate(
            """
            const deadline = Date.now() + 10000;
            while (Date.now() < deadline) {
              const button = Array.from(document.querySelectorAll('button')).find(
                (node) => /toggle workspace inspector/i.test(node.getAttribute('aria-label') || '')
              );
              if (button) {
                if (button.getAttribute('aria-expanded') !== 'true') button.click();
                return button.getAttribute('aria-expanded');
              }
              await new Promise((r) => setTimeout(r, 150));
            }
            return 'not-found';
            """
        )
        check("the inspector can be opened", opened == "true", str(opened))
        time.sleep(0.4)

        # ---- the sessions rail is untouched --------------------------------
        # `.session-meta` is the rail's timestamp/message line. A panel class of
        # the same name once restyled every row into a centred column, so this
        # measures the rail's own geometry rather than trusting the stylesheet.
        rail = session.evaluate(
            """
            const row = document.querySelector('.session-open .session-meta');
            if (!row) return null;
            const style = getComputedStyle(row);
            const box = row.getBoundingClientRect();
            return {
              direction: style.flexDirection,
              display: style.display,
              height: Math.round(box.height),
              // One line of ~11px text is about 16px tall; a stacked column is
              // two or three times that.
              text: (row.textContent || '').trim(),
              children: row.children.length,
            };
            """
        )
        check("the sessions rail renders at least one row", rail is not None)
        if rail:
            check("a session row's meta line is still a flex row",
                  rail["display"] == "flex" and rail["direction"] == "row",
                  f"{rail['display']}/{rail['direction']}")
            check("...and it is still one line tall",
                  rail["height"] <= 22, f"{rail['height']}px for {rail['text']!r}")
            check("...with its time and message count side by side",
                  rail["children"] >= 2
                  and any(token in rail["text"] for token in ("ago", "just now", "now")),
                  rail["text"])
        selected = session.evaluate(OPEN_TAB)
        check("clicking Session selects it", selected == "true", str(selected))

        # ---- the panel's numbers match the server --------------------------
        panel = session.evaluate(
            """
            // Clicking the tab mounts the panel and starts its read, so wait for
            // it to be populated rather than sampling the loading state.
            const deadline = Date.now() + 20000;
            let panel = null;
            while (Date.now() < deadline) {
              panel = document.getElementById('inspector-panel-session');
              if (panel && !panel.hidden && panel.querySelector('.si-window-percent')) break;
              await new Promise((r) => setTimeout(r, 100));
            }
            if (!panel || panel.hidden) return null;
            return {
              text: panel.innerText,
              percent: (panel.querySelector('.si-window-percent') || {}).textContent || '',
              legend: (panel.querySelector('.si-window-numbers') || {}).textContent || '',
              tone: (panel.querySelector('.si-window-meter') || {}).dataset?.tone || '',
              meterRole: (panel.querySelector('.si-window-meter') || {}).getAttribute?.('role') || '',
              agents: panel.querySelectorAll('.si-agent').length,
              agentNames: Array.from(panel.querySelectorAll('.si-agent-name')).map((n) => n.textContent),
              tools: Array.from(panel.querySelectorAll('.si-tool-name')).map((n) => n.textContent),
              runs: panel.querySelectorAll('.si-run').length,
              metrics: Array.from(panel.querySelectorAll('.metric')).map((node) => node.innerText.replace(/\\n/g, ' ')),
            };
            """
        )
        check("the Session panel renders", panel is not None)
        if panel:
            text = panel["text"]
            total = info["usage"]["total"]
            main = info["usage"]["main"]
            check("it shows the main agent's context heading",
              "main agent context" in text.lower())
            check("the window fill is a real percentage", panel["percent"].endswith("%"), panel["percent"])
            check("the meter is a live meter", panel["meterRole"] == "meter", panel["meterRole"])
            check("the window is the declared 128k",
                  "128k" in panel["legend"], panel["legend"])
            check("the fill is the main agent's newest prompt",
                  main["last_input_tokens"] == info["context"]["used"],
                  f"{main['last_input_tokens']} vs {info['context']['used']}")
            check("input tokens are shown", "Input" in text and format_tokens(main["input_tokens"]) in text)
            expected_ratio = round(100 * total["cache_hit_ratio"], 1)
            check("the cache hit rate comes from the server",
                  total["cache_hit_ratio"] is not None and f"{expected_ratio}%" in text,
                  f"want {expected_ratio}%, server {total['cache_hit_ratio']}")
            check("the delegated share is not reported as 0%",
                  "0.0% of tokens" not in text,
                  [line for line in text.splitlines() if "of tokens" in line])
            check("output tokens are shown", "Output" in text)
            check("cached tokens are shown", "Cached" in text and "Cached" in text)
            check("a cost estimate appears for a priced model",
                  "$" in text, text[text.find("Est. cost"):text.find("Est. cost") + 40] if "Est. cost" in text else "missing")
            check("work section is present", "Your prompts" in text and "Tool calls" in text)
            check("working time is a duration, not raw ms",
                  any(unit in text for unit in ("h ", "m ", "s")), "")
            check("the subagent appears as its own row",
                  panel["agents"] == len(info["agents"]) and panel["agents"] >= 1,
                  f"dom={panel['agents']} server={len(info['agents'])}")
            check("each subagent row has its own numbers",
                  len(panel["metrics"]) >= 8, str(len(panel["metrics"])))
            check("the run timeline is shown", panel["runs"] >= 1, str(panel["runs"]))
            if panel["tools"]:
                check("the tool breakdown names real tools",
                      any(name in ("read_file", "run_command", "spawn_agent") for name in panel["tools"]),
                      str(panel["tools"]))

        # Pictures, so a human can check it is not merely present but readable:
        # the inspector's own column, top and scrolled to the agents.
        import base64

        for name, scroll in (("session-info-top", 0), ("session-info-agents", 640)):
            session.evaluate(
                f"const p = document.getElementById('inspector-panel-session');"
                f" if (p) p.scrollTop = {scroll}; return p ? p.scrollHeight : 0;"
            )
            time.sleep(0.25)
            shot = session.cdp("Page.captureScreenshot", {"format": "png"})
            if isinstance(shot, dict) and shot.get("data"):
                out = Path(f"/tmp/kc-{name}.png")
                out.write_bytes(base64.b64decode(shot["data"]))
                print(f"  screenshot: {out}", flush=True)

        # ---- no horizontal overflow at 1280px ------------------------------
        session.cdp("Emulation.setDeviceMetricsOverride",
                    {"width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": False})
        time.sleep(0.4)
        overflow = session.evaluate(
            """
            const panel = document.getElementById('inspector-panel-session');
            if (!panel) return null;
            const before = panel.scrollLeft;
            panel.scrollLeft = 99999;
            const reached = panel.scrollLeft;
            panel.scrollLeft = before;
            return {
              scrollWidth: panel.scrollWidth,
              clientWidth: panel.clientWidth,
              reached,
              inner: panel.querySelector('.inspector-section')?.scrollWidth ?? 0,
            };
            """
        )
        check("the panel does not scroll sideways at 1280px",
              overflow is not None and overflow["reached"] <= 1,
              str(overflow))

        # ---- a session whose model declares no window ----------------------
        session.cdp("Emulation.clearDeviceMetricsOverride")
        session.cdp("Page.navigate", {"url": f"{base}/"})
        session.evaluate(BOOT)
        # Switch through the sessions rail, which is what a user does.
        switched = session.evaluate(
            f"""
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {{
              const row = Array.from(document.querySelectorAll('button, [role=button]')).find(
                (node) => (node.textContent || '').includes('No window')
              );
              if (row) {{ row.click(); return 'clicked'; }}
              await new Promise((r) => setTimeout(r, 150));
            }}
            return 'not-found';
            """
        )
        check("the second session can be opened from the rail", switched == "clicked", str(switched))
        if switched == "clicked":
            session.evaluate(OPEN_TAB)
            no_window = session.evaluate(
                """
                const deadline = Date.now() + 15000;
                while (Date.now() < deadline) {
                  const panel = document.getElementById('inspector-panel-session');
                  const percent = panel && panel.querySelector('.si-window-percent');
                  const meter = panel && panel.querySelector('.si-window-meter');
                  if (percent && percent.textContent === '—') {
                    return {
                      percent: percent.textContent,
                      role: meter ? meter.getAttribute('role') : null,
                      text: panel.innerText,
                    };
                  }
                  await new Promise((r) => setTimeout(r, 150));
                }
                const panel = document.getElementById('inspector-panel-session');
                return { percent: null, role: null, text: panel ? panel.innerText : '' };
                """
            )
            check("a model with no declared window shows no percentage",
                  no_window["percent"] == "—", str(no_window["percent"]))
            check("...and the meter is not a meter then", no_window["role"] is None, str(no_window["role"]))
            check("...and the panel says why",
                  "declares no context window" in (no_window["text"] or ""),
                  (no_window["text"] or "")[:120])

        # ---- a live run refreshes the panel -------------------------------
        before_calls = http_json(f"{base}/api/sessions/{first}/info")["usage"]["total"]["calls"]
        http_json(f"{base}/api/sessions/{first}/messages", "POST", {"content": "[text] one more"})
        for _ in range(120):
            if not http_json(f"{base}/api/sessions/{first}")["running"]:
                break
            time.sleep(0.25)
        after_calls = http_json(f"{base}/api/sessions/{first}/info")["usage"]["total"]["calls"]
        check("the server recorded the new call", after_calls > before_calls,
              f"{before_calls} -> {after_calls}")

        # ---- phone width ---------------------------------------------------
        session.cdp("Emulation.setDeviceMetricsOverride",
                    {"width": 390, "height": 844, "deviceScaleFactor": 2, "mobile": True})
        time.sleep(0.5)
        phone = session.evaluate(
            """
            const doc = document.documentElement;
            const panel = document.getElementById('inspector-panel-session');
            const wide = panel ? panel.scrollWidth - panel.clientWidth : 0;
            return {
              pageOverflow: doc.scrollWidth - doc.clientWidth,
              panelOverflow: wide,
              visible: Boolean(panel && !panel.hidden),
            };
            """
        )
        check("no page-level horizontal overflow at phone width",
              phone["pageOverflow"] <= 1, str(phone["pageOverflow"]))
        check("the panel does not break the phone layout",
              phone["panelOverflow"] <= 1, str(phone["panelOverflow"]))
        session.cdp("Emulation.clearDeviceMetricsOverride")

        print("", flush=True)
        print(f"{len(PASS)} passed, {len(FAIL)} failed", flush=True)
        if FAIL:
            print("failed:", ", ".join(FAIL), flush=True)
        return 1 if FAIL else 0
    finally:
        if session and not args.keep:
            session.close()
        if not args.keep:
            for proc in (chrome, server, mock):
                try:
                    proc.terminate()
                    proc.wait(timeout=10)
                except Exception:
                    proc.kill()
            server_log.close()
            chrome_log.close()


if __name__ == "__main__":
    sys.exit(main())
