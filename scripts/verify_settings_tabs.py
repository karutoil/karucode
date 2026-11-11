#!/usr/bin/env python3
"""Drive the two-pane settings sheet in a real Chromium over raw CDP.

The chrome-devtools plugin cannot launch Chromium on this host (no usable
sandbox), and plain background jobs die with the command's process group, so this
script does the three things that actually work here:

  * starts Chromium with an explicit `--user-data-dir` and `--no-sandbox`, behind
    `setsid nohup`;
  * starts a private kc-server on its own port/data-dir, pointed at the freshly
    built `web/dist` with `KARUCODE_WEB_DIR`;
  * speaks CDP over the WebSocket directly (Node 22 `fetch` + `WebSocket`).

It asserts the behaviour the unit tests cannot: that the sheet really is two
panes in a browser (rail + content, with the pane header and the footer outside
the scroller), that exactly one panel is in layout, that the Advanced blocks are
collapsed and open on click, that the search filters the rail, lands on the
section it names, opens the fold that holds it and focuses it, that Escape clears
the field before it closes the sheet, that a rejected field brings its section
into view, and that the phone tier stacks the panes with nothing overflowing.

Usage: python3 scripts/verify_settings_tabs.py [--keep]
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
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 30.0) -> None:
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
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.loads(response.read())


CDP_HELPER = r"""
/* A tiny CDP REPL: newline-delimited JSON on stdin, one JSON line per response.
 *
 * One persistent WebSocket for the whole run, because a CDP connection *is* the
 * session: `Emulation.setDeviceMetricsOverride` issued on a connection that then
 * closed is not in force for the next one, which silently turns a phone check
 * into a second desktop check.
 */
const wsUrl = process.argv[2];
const readline = process.argv[3] ? (await import('node:readline')) : null;
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
ws.addEventListener('open', () => {
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
        self.helper = Path(tempfile.gettempdir()) / "kc-cdp-repl.mjs"
        self.helper.write_text(CDP_HELPER)
        self.proc = subprocess.Popen(
            ["node", str(self.helper), ws_url, "repl"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        # The Node side connects asynchronously; one cheap round trip proves it.
        self.evaluate("return 1;")

    def _send(self, plan: dict, timeout_ms: int = 30_000) -> object:
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

    def evaluate(self, body: str, timeout_ms: int = 30_000) -> object:
        """Run `body` as an async function in the page and return its value."""
        return self._send({"kind": "evaluate", "body": body}, timeout_ms)

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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    session: Session | None = None
    tmp = Path(tempfile.mkdtemp(prefix="kc-settings-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()
    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                "[model]",
                'base_url = "http://127.0.0.1:9/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
                "",
                "[[providers]]",
                'id = "default"',
                'name = "Default"',
                'provider = "openai_compatible"',
                'base_url = "http://127.0.0.1:9/v1"',
                'api_key = "sk-test"',
                "",
                "[[providers.models]]",
                'id = "gpt-4o-mini"',
                'name = "GPT-4o mini"',
                "",
                "[active]",
                'provider = "default"',
                'model = "gpt-4o-mini"',
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
            "--window-size=1280,900",
            "about:blank",
        ],
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    try:
        wait_for(f"http://127.0.0.1:{port}/api/health", timeout=45)
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version", timeout=45)

        settings = http_json(f"http://127.0.0.1:{port}/api/config")
        notifications_on = settings["notifications"]["run_finished"]

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])

        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"http://127.0.0.1:{port}/"})
        # Wait for the app to boot, then open the settings sheet from the rail.
        opened = session.evaluate(
            """
            const tablist = () =>
              document.querySelector('[role=tablist][aria-label="Settings sections"]');
            const deadline = Date.now() + 30000;
            while (Date.now() < deadline) {
              if (tablist()) return 'opened';
              const gear = Array.from(document.querySelectorAll('button')).find(
                (button) => /open settings/i.test(button.getAttribute('aria-label') || ''),
              );
              if (gear) {
                gear.click();
                await new Promise((resolve) => setTimeout(resolve, 400));
                if (tablist()) return 'opened';
              }
              await new Promise((resolve) => setTimeout(resolve, 200));
            }
            // Report why, so a failure is diagnosable rather than "not-opened".
            return {
              state: 'not-opened',
              dialog: !!document.querySelector('[role=dialog]'),
              dialogText: (document.querySelector('[role=dialog]') || {}).textContent?.slice(0, 200) || '',
              hasGear: Array.from(document.querySelectorAll('button')).some(
                (button) => /open settings/i.test(button.getAttribute('aria-label') || ''),
              ),
            };
            """,
        )
        check(
            "the settings sheet opens with a section rail",
            opened == "opened",
            "opened" if opened == "opened" else str(opened),
        )
        if opened != "opened":
            raise SystemExit(1)

        rail = session.evaluate(
            """
            const list = document.querySelector('[role=tablist][aria-label="Settings sections"]');
            const tabs = Array.from(list.querySelectorAll('[role=tab]'));
            return {
              orientation: list.getAttribute('aria-orientation'),
              search: !!document.querySelector('.settings-search input'),
              labels: tabs.map((tab) => (tab.querySelector('.settings-nav-label') || {}).textContent || ''),
              notes: tabs.map((tab) => (tab.querySelector('.settings-nav-note') || {}).textContent || ''),
              selected: tabs.map((tab) => tab.getAttribute('aria-selected')),
              tabIndex: tabs.map((tab) => tab.tabIndex),
              controls: tabs.map((tab) => tab.getAttribute('aria-controls')),
            };
            """,
        )
        check(
            "the sheet opens with a vertical rail of seven sections",
            rail["labels"]
            == [
                "Appearance",
                "Models & providers",
                "Agent",
                "Access & sign-in",
                "Notifications",
                "Plugins & MCP",
                "Updates",
            ]
            and rail["orientation"] == "vertical",
            ", ".join(rail["labels"]),
        )
        check(
            "every rail row says what it holds",
            all(note.strip() for note in rail["notes"]),
            " / ".join(rail["notes"]),
        )
        check(
            "the rail carries a search field",
            rail["search"] is True,
            "search present" if rail["search"] else "no search field",
        )
        check(
            "one section is selected, and it is the only one in the tab order",
            rail["selected"].count("true") == 1
            and rail["tabIndex"][0] == 0
            and set(rail["tabIndex"][1:]) == {-1},
            f'selected={rail["selected"].count("true")} tabIndex={rail["tabIndex"]}',
        )

        visible = session.evaluate(
            """
            const panels = Array.from(document.querySelectorAll('.settings-panels > .tab-panel'));
            return {
              count: panels.length,
              visible: panels.filter((panel) => !panel.hasAttribute('hidden')).map((p) => p.id),
              display: panels.map((panel) => getComputedStyle(panel).display),
            };
            """,
        )
        check(
            "exactly one panel is visible at a time",
            visible["count"] == 7 and len(visible["visible"]) == 1,
            f'{len(visible["visible"])} of {visible["count"]} visible',
        )
        check(
            "the hidden panels are really out of layout",
            visible["display"].count("none") == 6,
            str(visible["display"]),
        )

        # Walking the rail: each section shows its own content and nothing else.
        walked = session.evaluate(
            """
            const wanted = {
              Appearance: 'settings-theme',
              'Models & providers': 'settings-providers',
              Agent: 'settings-approvals',
              'Access & sign-in': 'settings-access-signin',
              Notifications: 'settings-notifications',
              'Plugins & MCP': 'settings-plugins',
              Updates: 'settings-update-build',
            };
            const results = [];
            for (const [label, anchor] of Object.entries(wanted)) {
              const tab = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
                (candidate) =>
                  ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() === label,
              );
              tab.click();
              await new Promise((resolve) => setTimeout(resolve, 250));
              const panel = document.querySelector('.settings-panels > .tab-panel:not([hidden])');
              results.push({
                label,
                title: (document.querySelector('.settings-pane-title') || {}).textContent || '',
                anchorFound: !!document.getElementById(anchor),
                anchorInPanel: !!(panel && panel.querySelector('#' + anchor)),
                panel: panel ? panel.id : null,
              });
            }
            return results;
            """,
        )
        check(
            "every section shows the panel it owns and only that one",
            all(item["anchorFound"] and item["anchorInPanel"] for item in walked),
            "; ".join(f'{i["label"]}->{i["panel"]}' for i in walked),
        )
        check(
            "the pane header names the section on screen",
            all(item["title"] == item["label"] for item in walked),
            ", ".join(f'{i["label"]}: {i["title"]}' for i in walked),
        )

        # The Advanced blocks start collapsed, and opening one relaid out its body.
        advanced = session.evaluate(
            """
            const out = {};
            for (const panelId of ['general', 'models', 'agent', 'notifications']) {
              const panel = document.getElementById('settings-panel-' + panelId);
              const details = panel && panel.querySelector('details.settings-advanced');
              out[panelId] = details ? details.open : 'missing';
            }
            return out;
            """,
        )
        check(
            "every Advanced block starts collapsed",
            all(value is False for value in advanced.values()),
            json.dumps(advanced),
        )

        toggled = session.evaluate(
            """
            const tab = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
              (candidate) =>
                ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() ===
                'Models & providers',
            );
            tab.click();
            await new Promise((resolve) => setTimeout(resolve, 250));
            const details = document.querySelector('#settings-panel-models details.settings-advanced');
            const summary = details.querySelector('summary');
            summary.click();
            await new Promise((resolve) => setTimeout(resolve, 250));
            return {
              openAfter: details.open,
              bodyHeight: details.querySelector('.settings-advanced-body').getBoundingClientRect().height,
              summaryText: summary.textContent.trim(),
            };
            """,
        )
        check(
            "clicking Advanced opens the block and lays out its body",
            toggled["openAfter"] is True and toggled["bodyHeight"] > 40,
            f'body {round(toggled["bodyHeight"])}px',
        )
        check(
            "the summary says what is folded away, not just 'Advanced'",
            "Sampling" in toggled["summaryText"],
            toggled["summaryText"][:60],
        )

        inset = session.evaluate(
            """
            const details = document.querySelector('#settings-panel-models details.settings-advanced');
            const box = details.getBoundingClientRect();
            const label = details.querySelector('.field-label').getBoundingClientRect();
            return { left: Math.round(label.left - box.left), right: Math.round(box.right - label.right) };
            """,
        )
        # A label flush against the disclosure's border reads as broken; this is
        # the defect a screenshot caught and jsdom cannot.
        check(
            "the folded content is inset from the disclosure's border",
            inset["left"] >= 10 and inset["right"] >= 10,
            f'{inset["left"]}px / {inset["right"]}px',
        )

        # The rail, the pane header and the footer must not scroll with a panel.
        #
        # The Agent tab's Advanced block is opened first, for the same reason a
        # reader opens it: every subagent field lives behind that fold, so a
        # collapsed panel can fit the viewport exactly and then nothing scrolls.
        pinned = session.evaluate(
            """
            const agent = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
              (candidate) =>
                ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() === 'Agent',
            );
            agent.click();
            await new Promise((resolve) => setTimeout(resolve, 300));
            const panel = document.querySelector('.settings-panels > .tab-panel:not([hidden])');
            const details = panel.querySelector('details.settings-advanced');
            if (details && !details.open) {
              details.querySelector('summary').click();
              await new Promise((resolve) => setTimeout(resolve, 300));
            }
            const rail = document.querySelector('.settings-nav');
            const head = document.querySelector('.settings-pane-head');
            const footer = document.querySelector('.settings-footer');
            const before = {
              rail: rail.getBoundingClientRect().top,
              head: head.getBoundingClientRect().top,
              footer: footer.getBoundingClientRect().top,
              scrollable: panel.scrollHeight > panel.clientHeight,
            };
            panel.scrollTop = 400;
            await new Promise((resolve) => setTimeout(resolve, 150));
            return {
              before,
              after: {
                rail: rail.getBoundingClientRect().top,
                head: head.getBoundingClientRect().top,
                footer: footer.getBoundingClientRect().top,
                panelScroll: panel.scrollTop,
              },
            };
            """,
        )
        check(
            "the panel is the thing that scrolls",
            pinned["before"]["scrollable"] is True and pinned["after"]["panelScroll"] > 0,
            f'scrolled {pinned["after"]["panelScroll"]}px',
        )
        check(
            "the rail, the pane header and the footer stay put while it does",
            abs(pinned["after"]["rail"] - pinned["before"]["rail"]) < 1
            and abs(pinned["after"]["head"] - pinned["before"]["head"]) < 1
            and abs(pinned["after"]["footer"] - pinned["before"]["footer"]) < 1,
            f'head {pinned["before"]["head"]:.0f}->{pinned["after"]["head"]:.0f}, '
            f'footer {pinned["before"]["footer"]:.0f}->{pinned["after"]["footer"]:.0f}',
        )

        # Search: filter the rail, then land on the first match.
        searched = session.evaluate(
            """
            const input = document.querySelector('.settings-search input');
            const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
            const type = (value) => {
              setter.call(input, value);
              input.dispatchEvent(new Event('input', { bubbles: true }));
            };
            type('max concurrent');
            await new Promise((resolve) => setTimeout(resolve, 300));
            const afterType = Array.from(document.querySelectorAll('.settings-nav-label')).map(
              (node) => node.textContent,
            );
            const note = (document.querySelector('.settings-nav-note') || {}).textContent || '';
            const hint = (document.querySelector('.settings-nav-hint') || {}).textContent || '';
            input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
            await new Promise((resolve) => setTimeout(resolve, 600));
            const panel = document.querySelector('.settings-panels > .tab-panel:not([hidden])');
            const details = panel.querySelector('details.settings-advanced');
            const found = panel.querySelector('.settings-section.is-found');
            const tabs = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]'));
            return {
              afterType,
              note,
              hint,
              tabs: tabs.map((tab) => [
                tab.getAttribute('aria-selected'),
                (tab.querySelector('.settings-nav-label') || {}).textContent || tab.textContent,
              ]),
              selected: (tabs.find((tab) => tab.getAttribute('aria-selected') === 'true') || {})
                .textContent || '',
              advancedOpen: details ? details.open : false,
              foundHeading: found ? (found.querySelector('h3') || {}).textContent : null,
              focused: document.activeElement ? document.activeElement.className : null,
            };
            """,
        )
        check(
            "typing filters the rail to the section that owns the match",
            searched["afterType"] == ["Agent"],
            str(searched["afterType"]),
        )
        check(
            "the rail row names what inside it matched",
            "Subagents" in searched["note"],
            f'note "{searched["note"].strip()}"',
        )
        check(
            "the search says how many sections match",
            "1 section match" in searched["hint"],
            searched["hint"].strip()[:60],
        )
        check(
            "Enter opens the matching section and its disclosure",
            "Agent" in searched["selected"]
            and searched["advancedOpen"] is True
            and searched["foundHeading"] == "Subagents",
            f'selected={searched["selected"]} open={searched["advancedOpen"]} '
            f'section={searched["foundHeading"]} tabs={searched["tabs"]}',
        )
        check(
            "and focuses the section it landed on",
            "settings-section" in (searched["focused"] or ""),
            str(searched["focused"]),
        )

        escaped = session.evaluate(
            """
            const input = document.querySelector('.settings-search input');
            input.focus();
            input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            await new Promise((resolve) => setTimeout(resolve, 250));
            const cleared = input.value === '';
            const stillOpen = !!document.querySelector('[role=dialog]');
            const labels = document.querySelectorAll('.settings-nav-label').length;
            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            await new Promise((resolve) => setTimeout(resolve, 500));
            return {
              cleared,
              stillOpen,
              labels,
              closedAfterSecond: !document.querySelector('[role=dialog]'),
            };
          """,
        )
        check(
            "Escape clears the search without closing the sheet",
            escaped["cleared"] is True
            and escaped["stillOpen"] is True
            and escaped["labels"] == 7,
            f'cleared={escaped["cleared"]} open={escaped["stillOpen"]} rows={escaped["labels"]}',
        )
        check(
            "a second Escape closes the sheet",
            escaped["closedAfterSecond"] is True,
            "closed" if escaped["closedAfterSecond"] else "still open",
        )

        # Reopen for the rest of the checks.
        reopened = session.evaluate(
            """
            const gear = Array.from(document.querySelectorAll('button')).find(
              (button) => /open settings/i.test(button.getAttribute('aria-label') || ''),
            );
            gear.click();
            await new Promise((resolve) => setTimeout(resolve, 700));
            return !!document.querySelector('[role=tablist][aria-label="Settings sections"]');
            """,
        )
        if not reopened:
            check("the sheet reopens after Escape", False, "no tablist")
            raise SystemExit(1)

        # A rejected field pulls the sheet to its section and opens its disclosure.
        jumped = session.evaluate(
            """
            const byLabel = (label) =>
              Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
                (candidate) =>
                  ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() === label,
              );
            byLabel('Appearance').click();
            await new Promise((resolve) => setTimeout(resolve, 200));
            const save = Array.from(document.querySelectorAll('button')).find(
              (button) => (button.textContent || '').trim() === 'Save changes',
            );
            // Plant a value the server bounds reject, in the Agent tab's fold.
            byLabel('Agent').click();
            await new Promise((resolve) => setTimeout(resolve, 200));
            const details = document.querySelector('#settings-panel-agent details.settings-advanced');
            details.querySelector('summary').click();
            await new Promise((resolve) => setTimeout(resolve, 200));
            const field = document.getElementById('subagent-iterations');
            const setter = Object.getOwnPropertyDescriptor(
              window.HTMLInputElement.prototype, 'value',
            ).set;
            setter.call(field, '10001');
            field.dispatchEvent(new Event('input', { bubbles: true }));
            await new Promise((resolve) => setTimeout(resolve, 100));
            details.querySelector('summary').click();
            await new Promise((resolve) => setTimeout(resolve, 100));
            byLabel('Appearance').click();
            await new Promise((resolve) => setTimeout(resolve, 200));
            save.click();
            await new Promise((resolve) => setTimeout(resolve, 900));
            return {
              selected: byLabel('Agent').getAttribute('aria-selected'),
              title: (document.querySelector('.settings-pane-title') || {}).textContent || '',
              advancedOpen: document.querySelector('#settings-panel-agent details.settings-advanced').open,
              message: (document.querySelector('.settings-error-summary') || {}).textContent || '',
              fieldError: (document.getElementById('subagent-iterations-error') || {}).textContent || '',
              focused: document.activeElement ? document.activeElement.id : null,
            };
            """,
        )
        check(
            "a rejected field switches the sheet to its section",
            jumped["selected"] == "true" and jumped["title"] == "Agent",
            f'Agent selected={jumped["selected"]} title={jumped["title"]}',
        )
        check(
            "and opens the disclosure the field lives behind",
            jumped["advancedOpen"] is True,
            f'open={jumped["advancedOpen"]}',
        )
        check(
            "the footer says where the problem is",
            "Agent" in jumped["message"] and "1 setting" in jumped["message"],
            jumped["message"].strip(),
        )
        check(
            "the failing input is focused, with its message",
            jumped["focused"] == "subagent-iterations"
            and "1–10000" in (jumped["fieldError"] or ""),
            f'focus={jumped["focused"]} error="{jumped["fieldError"].strip()}"',
        )

        # Restore the planted value so the check does not leave a broken form.
        session.evaluate(
            """
            const field = document.getElementById('subagent-iterations');
            const setter = Object.getOwnPropertyDescriptor(
              window.HTMLInputElement.prototype, 'value',
            ).set;
            setter.call(field, '0');
            field.dispatchEvent(new Event('input', { bubbles: true }));
            return 'restored';
            """,
        )

        # Notifications still round-trip: the section move must not have detached them.
        flipped = session.evaluate(
            f"""
            const tab = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
              (candidate) =>
                ((candidate.querySelector('.settings-nav-label') || {{}}).textContent || '').trim() ===
                'Notifications',
            );
            tab.click();
            await new Promise((resolve) => setTimeout(resolve, 300));
            const el = Array.from(document.querySelectorAll('[role=switch]')).find(
              (s) => (s.getAttribute('aria-label') || '').includes('A run finished'),
            );
            if (!el) return {{ error: 'missing switch' }};
            el.click();
            await new Promise((resolve) => setTimeout(resolve, 900));
            const response = await fetch('/api/config', {{ headers: {{ 'X-Karucode-Client': 'web' }} }});
            const config = await response.json();
            return {{ run_finished: config.notifications.run_finished }};
            """,
        )
        check(
            "a notification switch still PATCHes from its section",
            flipped.get("run_finished") is (not notifications_on),
            f'run_finished={flipped.get("run_finished")}',
        )
        session.evaluate(
            f"""
            await fetch('/api/config', {{
              method: 'PATCH',
              headers: {{ 'Content-Type': 'application/json', 'X-Karucode-Client': 'web' }},
              body: JSON.stringify({{ notifications: {{ run_finished: {str(notifications_on).lower()} }} }}),
            }});
            return 'restored';
            """,
        )

        # Layout sanity at 1280px: the rail is a rail, and nothing overlaps.
        wide = session.evaluate(
            """
            const sheet = document.querySelector('.dialog-settings');
            const nav = document.querySelector('.settings-nav').getBoundingClientRect();
            const content = document.querySelector('.settings-content').getBoundingClientRect();
            const items = Array.from(document.querySelectorAll('.settings-nav-item'));
            const overlapping = items.some((item, index) => {
              if (index === 0) return false;
              const prev = items[index - 1].getBoundingClientRect();
              const box = item.getBoundingClientRect();
              return box.top < prev.bottom - 1;
            });
            return {
              overlapping,
              navWidth: Math.round(nav.width),
              contentWidth: Math.round(content.width),
              sheetWidth: Math.round(sheet.getBoundingClientRect().width),
              widths: items.map((item) => Math.round(item.getBoundingClientRect().width)),
              searchHeight: Math.round(
                document.querySelector('.settings-search').getBoundingClientRect().height,
              ),
            };
            """,
        )
        check(
            "the rail and the content split the sheet, and no two rows overlap",
            wide["overlapping"] is False
            and wide["navWidth"] >= 200
            and wide["contentWidth"] > wide["navWidth"],
            f'rail {wide["navWidth"]}px / content {wide["contentWidth"]}px '
            f'of {wide["sheetWidth"]}px',
        )
        check(
            "every rail row fills the rail",
            len(set(wide["widths"])) == 1,
            str(wide["widths"]),
        )

        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {
                "width": 390,
                "height": 844,
                "deviceScaleFactor": 2,
                "mobile": True,
                # Without an explicit viewport the page keeps a 980px layout width
                # and the 780px media query never fires, which would make the
                # phone check pass without testing anything.
                "screenWidth": 390,
                "screenHeight": 844,
                "viewport": {"x": 0, "y": 0, "width": 390, "height": 600, "scale": 1},
            },
        )
        narrow = session.evaluate(
            """
            await new Promise((resolve) => setTimeout(resolve, 500));
            const shell = document.querySelector('.settings-shell');
            const list = document.querySelector('.settings-nav-list');
            const items = Array.from(document.querySelectorAll('.settings-nav-item'));
            const overlapping = items.some((item, index) => {
              if (index === 0) return false;
              const prev = items[index - 1].getBoundingClientRect();
              const box = item.getBoundingClientRect();
              return box.left < prev.right - 1;
            });
            const footer = document.querySelector('.settings-footer').getBoundingClientRect();
            const head = document.querySelector('.settings-pane-head').getBoundingClientRect();
            return {
              shellDirection: getComputedStyle(shell).flexDirection,
              overlapping,
              scrollable: list.scrollWidth > list.clientWidth + 1,
              noteHidden: getComputedStyle(document.querySelector('.settings-nav-note')).display,
              footerVisible: footer.bottom <= window.innerHeight + 1 && footer.top >= 0,
              headVisible: head.top >= 0 && head.bottom <= window.innerHeight,
              sheetWidth: document.querySelector('.dialog-settings').getBoundingClientRect().width,
              viewport: window.innerWidth,
            };
            """,
        )
        check(
            "on a phone the sheet stacks into one column",
            narrow["shellDirection"] == "column"
            and narrow["sheetWidth"] <= 391
            and narrow["overlapping"] is False,
            f'viewport={narrow["viewport"]} sheet={round(narrow["sheetWidth"])} '
            f'direction={narrow["shellDirection"]} scrollable={narrow["scrollable"]}',
        )
        check(
            "the phone rail scrolls sideways and drops its notes",
            narrow["scrollable"] is True and narrow["noteHidden"] == "none",
            f'scrollable={narrow["scrollable"]} note={narrow["noteHidden"]}',
        )
        check(
            "the pane header and the Save button are both reachable",
            narrow["headVisible"] is True and narrow["footerVisible"] is True,
            f'head={narrow["headVisible"]} footer={narrow["footerVisible"]}',
        )
        overflow = session.evaluate(
            """
            const sheet = document.querySelector('.dialog-settings');
            const sheetRight = sheet.getBoundingClientRect().right;
            // A child of a horizontal scroller is allowed to hang past the edge:
            // that is what scrolling it means, and the strip is the phone rail's
            // whole answer to seven labels in 390px.
            const insideScroller = (el) => {
              for (let node = el.parentElement; node && node !== sheet; node = node.parentElement) {
                if (getComputedStyle(node).overflowX !== 'visible') return true;
              }
              return false;
            };
            const wide = Array.from(sheet.querySelectorAll('*')).filter((el) => {
              const box = el.getBoundingClientRect();
              if (box.width <= 0 || box.right <= sheetRight + 1) return false;
              return !insideScroller(el);
            });
            return {
              pageScrollWidth: document.documentElement.scrollWidth,
              viewport: window.innerWidth,
              sheetWidth: Math.round(sheetRight - sheet.getBoundingClientRect().left),
              overflowing: wide.length,
              firstOverflowing: wide.slice(0, 3).map((el) => el.className || el.tagName),
            };
            """,
        )
        # Nothing may stick out of the sheet at 390px: the settings form is
        # full-width inputs and long labels, which is exactly where a fixed width
        # or a wide grid column escapes the drawer.
        check(
            "nothing overflows the sheet horizontally at phone width",
            overflow["pageScrollWidth"] <= overflow["viewport"] + 1
            and overflow["overflowing"] == 0,
            f'page {overflow["pageScrollWidth"]}px / viewport {overflow["viewport"]}px, '
            f'{overflow["overflowing"]} node(s) past the sheet '
            f'{overflow["firstOverflowing"]}',
        )

        # A phone screenshot too: the narrow tier is the one a desktop screenshot
        # cannot show, and it is where the rail has to earn its keep.
        import base64

        shots_dir = tmp / "shots"
        shots_dir.mkdir(exist_ok=True)
        # Device scale 1 on purpose: the layout is measured in CSS pixels, and a
        # 2x capture doubles the exported image without changing what it proves.
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {
                "width": 390,
                "height": 844,
                "deviceScaleFactor": 1,
                "mobile": True,
                "screenWidth": 390,
                "screenHeight": 844,
            },
        )
        session.evaluate(
            """
            const tab = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
              (candidate) =>
                ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() === 'Agent',
            );
            tab.click();
            await new Promise((resolve) => setTimeout(resolve, 500));
            return 'ready';
            """,
        )
        (shots_dir / "8-phone-agent.png").write_bytes(
            base64.b64decode(session.cdp("Page.captureScreenshot", {"format": "png"})["data"])
        )

        # A screenshot of every section, for a human to look at.
        session.cdp("Emulation.clearDeviceMetricsOverride")
        shots = shots_dir
        for index, label in enumerate(
            [
                "Appearance",
                "Models & providers",
                "Agent",
                "Access & sign-in",
                "Notifications",
                "Plugins & MCP",
                "Updates",
            ]
        ):
            session.evaluate(
                f"""
                const tab = Array.from((document.querySelector('[role=dialog]') || document).querySelectorAll('[role=tab]')).find(
                  (candidate) =>
                    ((candidate.querySelector('.settings-nav-label') || {{}}).textContent || '').trim() ===
                    {json.dumps(label)},
                );
                tab.click();
                await new Promise((resolve) => setTimeout(resolve, 450));
                return 'ready';
                """,
            )
            capture = session.cdp("Page.captureScreenshot", {"format": "png"})["data"]
            slug = label.replace(" & ", "-and-").replace(" ", "-").lower()
            (shots / f"{index + 1}-{slug}.png").write_bytes(base64.b64decode(capture))
        print(f"\nscreenshots: {shots}", flush=True)

        print(f"\n{len(PASS)} passed, {len(FAIL)} failed", flush=True)
        if FAIL:
            print("failed:", *FAIL, sep="\n  - ", flush=True)
        return 0 if not FAIL else 1
    finally:
        if session is not None:
            session.close()
        if args.keep:
            print(f"\nkept: server pid {server.pid} on :{port}, chrome pid {chrome.pid}")
            print(f"logs: {tmp}")
        else:
            for proc in (chrome, server):
                try:
                    proc.terminate()
                except Exception:
                    pass
            server_log.close()
            chrome_log.close()
            if not args.keep:
                time.sleep(0.5)
                shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
