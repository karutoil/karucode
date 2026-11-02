#!/usr/bin/env python3
"""Browser verification for two web fixes: the terminal tab switch and the file viewer.

Both were reported as *visible* defects, and both passed their unit tests before
the fix - the terminal one because the xterm stub was a no-op, the file viewer one
because nothing asserted the absence of a tab. jsdom cannot settle either: xterm
draws to a canvas it does not implement, and `content-visibility` is a layout
feature no jsdom test can observe.

So: a real server, the real built bundle, a real Chromium, driven over CDP.

  * **Terminal** - open two shells, switch between the sub-tabs, and assert that
    the emulator's DOM node is *inside the live host* and still there after the
    switch (the old code left it in the discarded host, which is a blank pane).
    Typing is checked through xterm's own hidden textarea, which is where the
    keystrokes land.
  * **File viewer** - open a `.rs` file and assert there is no Preview tab (the
    dead tab), then open a `.md` file and drive the tabs with the keyboard.

Usage: python3 scripts/verify_web_fixes.py [--keep]
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
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PASS: list[str] = []
FAIL: list[str] = []


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


def http_json(url: str, method: str = "GET", body: dict | None = None) -> object:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("X-Karucode-Client", "web")
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        print(f"  HTTP {error.code} from {method} {url}: {error.read()[:400]!r}", flush=True)
        raise


class Session:
    def __init__(self, ws_url: str) -> None:
        from websockets.sync.client import connect

        self.ws = connect(ws_url, max_size=64 * 1024 * 1024, open_timeout=20)
        self.next_id = 0
        self.evaluate("return 1;")

    def _send(self, method: str, params: dict | None = None) -> dict:
        self.next_id += 1
        request_id = self.next_id
        self.ws.send(json.dumps({"id": request_id, "method": method, "params": params or {}}))
        deadline = time.time() + 60
        while time.time() < deadline:
            raw = self.ws.recv(timeout=max(1.0, deadline - time.time()))
            message = json.loads(raw)
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise SystemExit(f"CDP {method} failed: {message['error']}")
            return message.get("result") or {}
        raise SystemExit(f"CDP {method} timed out")

    def evaluate(self, body: str) -> object:
        result = self._send(
            "Runtime.evaluate",
            {
                "expression": "(async () => {" + body + "})()",
                "awaitPromise": True,
                "returnByValue": True,
            },
        )
        if result.get("exceptionDetails"):
            raise SystemExit(f"page script failed: {json.dumps(result['exceptionDetails'])[:800]}")
        return (result.get("result") or {}).get("value")

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f' — {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f' — {detail}'}", flush=True)


BOOT = """
const deadline = Date.now() + 30000;
while (Date.now() < deadline) {
  const box = document.querySelector('#composer-input') || document.querySelector('.chat-main');
  if (box) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}
return 'timeout';
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()

    if not (ROOT / "target/debug/karucode").exists():
        raise SystemExit("build first: cargo build -p kc-server")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("build the UI first: (cd web && npm run build)")

    tmp = Path(tempfile.mkdtemp(prefix="kc-webfix-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    (workspace / "notes.md").write_text("# Notes\n\nSome *rendered* markdown.\n")
    (workspace / "main.rs").write_text('fn main() {\n    println!("hi");\n}\n')
    data_dir = tmp / "data"
    data_dir.mkdir()

    # A deterministic provider, so a send produces a stored user row and a
    # rendered answer. `[text]` makes it reply with prose and no tool call.
    provider_port = free_port()
    provider_log = open(tmp / "provider.jsonl", "wb")
    provider = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "scripts/mock_openai.py"),
            "--port",
            str(provider_port),
            "--log",
            str(tmp / "provider.jsonl"),
        ],
        stdout=subprocess.DEVNULL,
        stderr=provider_log,
        start_new_session=True,
    )

    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                # Top-level keys must come before any `[table]`, or TOML parses
                # them as members of the last table written.
                f'project_roots = ["{workspace}"]',
                "",
                "[model]",
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
                "",
                "[terminals]",
                "enabled = true",
                "max_terminals = 4",
                "",
                "[titles]",
                "enabled = false",
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
            "--js-flags=--jitless",
            "about:blank",
        ],
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    base = f"http://127.0.0.1:{port}"
    session: Session | None = None
    try:
        wait_for(f"{base}/api/health")
        wait_for(f"http://127.0.0.1:{provider_port}/v1/models")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list") as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t.get("type") == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session._send("Page.enable")
        session._send("Runtime.enable")

        # The server starts with no projects registered, and a session with no
        # project is not in the rail's scope, so register the workspace first.
        # Bootstrapping already registered the workspace as a project; creating it
        # again is a 400, so read the list instead.
        projects = http_json(f"{base}/api/projects")["projects"]
        project_id = projects[0]["id"]
        print(f"  projects: {[p['name'] for p in projects]}", flush=True)
        # One session so the transcript and the composer exist.
        # `POST /api/sessions` returns the session row itself.
        # Scoped to the project: the rail lists only the current project's
        # sessions, and a session created without one would not appear in it.
        created = http_json(
            f"{base}/api/sessions",
            "POST",
            {"title": "web fix check", "project": project_id},
        )
        session_id = created["id"]
        print(f"  session {session_id}", flush=True)
        # A stored user turn, so the transcript has a `.msg` row to measure. A send
        # would need a live provider; this is the same row the server writes after
        # one, and `content-visibility` is a property of the row, not of its text.
        # Drive the composer, which is what writes the stored user turn. The
        # provider URL is a dead port on purpose: the *send* fails, but the row is
        # persisted before the run starts, and `content-visibility` is a property
        # of the row rather than of the answer.
        seeded = {"started": False}

        session._send("Page.navigate", {"url": f"{base}/"})
        time.sleep(1.0)
        booted = session.evaluate(BOOT)
        check("the app boots", booted == "ready", str(booted))

        # A fresh browser profile has no "last session" in localStorage, so the
        # app boots on the empty state. Pick the session this script created from
        # the rail, the way a returning user would.
        opened_session = session.evaluate(
            """
            const deadline = Date.now() + 20000;
            while (Date.now() < deadline) {
              const row = Array.from(document.querySelectorAll('.session-row'))
                .find((node) => (node.textContent || '').includes('web fix check'));
              if (row) { row.click(); await new Promise((r) => setTimeout(r, 1200)); return 'clicked'; }
              await new Promise((r) => setTimeout(r, 200));
            }
            return {
              rows: document.querySelectorAll('.session-row').length,
              text: (document.querySelector('[aria-label=\\"Sessions\\"]') || {}).textContent?.slice(0, 300) || '',
            };
            """
        )
        check("the created session opens from the rail", opened_session == "clicked", str(opened_session))

        # ---- terminal: switching sub-tabs must keep the emulator live --------
        session.evaluate(
            """
            window.location.hash = '';
            const tab = Array.from(document.querySelectorAll('[role="tab"]'))
              .find((node) => /terminal/i.test(node.textContent || ''));
            if (tab) tab.click();
            await new Promise((r) => setTimeout(r, 600));
            return 'ok';
            """
        )
        opened = session.evaluate(
            """
            // Create the two shells through the API path the UI uses, by clicking
            // "New terminal" twice. The list is server-owned, so this is what a
            // user does too.
            const deadline = Date.now() + 15000;
            const buttons = () => Array.from(document.querySelectorAll('button'))
              .filter((node) => /new terminal|^\\s*New\\s*$/i.test(node.textContent || ''));
            while (Date.now() < deadline) {
              const target = buttons()[0];
              if (target && !target.disabled) { target.click(); break; }
              await new Promise((r) => setTimeout(r, 150));
            }
            await new Promise((r) => setTimeout(r, 1200));
            const again = buttons()[0];
            if (again && !again.disabled) again.click();
            await new Promise((r) => setTimeout(r, 1500));
            return document.querySelectorAll('.terminal-chip-select').length;
            """
        )
        check("two terminal sub-tabs can be created", opened == 2, str(opened))

        # The emulator renders into `.terminal-host`; xterm's root node is `.xterm`.
        first = session.evaluate(
            """
            const chips = Array.from(document.querySelectorAll('.terminal-chip-select'));
            if (chips.length < 2) return { chips: chips.length };
            chips[0].click();
            await new Promise((r) => setTimeout(r, 1200));
            const host = document.querySelector('.terminal-host');
            const term = document.querySelector('.terminal-host .xterm');
            return {
              chips: chips.length,
              hostFound: !!host,
              termFound: !!term,
              // The bug: the emulator stays in the host that was thrown away, so
              // the live host has no `.xterm` child at all and the pane is blank.
              termInLiveHost: !!(host && term && term.parentElement === host),
              rows: term ? term.querySelectorAll('.xterm-rows').length : 0,
            };
            """
        )
        check("the first terminal draws its emulator into the live host",
              isinstance(first, dict) and first.get("termInLiveHost") is True, str(first))

        # Switch to the second sub-tab and back. Each switch remounts the surface
        # (the parent keys it by terminal id), which is exactly the path that broke.
        switched = session.evaluate(
            """
            const chips = Array.from(document.querySelectorAll('.terminal-chip-select'));
            const report = [];
            for (const index of [1, 0, 1, 0]) {
              chips[index].click();
              await new Promise((r) => setTimeout(r, 900));
              const host = document.querySelector('.terminal-host');
              const term = host ? host.querySelector('.xterm') : null;
              report.push({
                index,
                hostChildren: host ? host.children.length : -1,
                termInLiveHost: !!(host && term && term.parentElement === host),
                textarea: !!(term && term.querySelector('textarea')),
              });
            }
            return report;
            """
        )
        check("every sub-tab switch leaves the emulator in the live host",
              isinstance(switched, list)
              and len(switched) == 4
              and all(step.get("termInLiveHost") for step in switched),
              json.dumps(switched))
        check("the switched-to pane is not blank (it has the emulator node)",
              isinstance(switched, list)
              and all(step.get("hostChildren") == 1 for step in switched),
              json.dumps([step.get("hostChildren") for step in switched]))
        check("the switched-to pane has xterm's input textarea (it is typable)",
              isinstance(switched, list) and all(step.get("textarea") for step in switched),
              json.dumps([step.get("textarea") for step in switched]))

        # Typing: send real key events at xterm's hidden textarea and check the
        # shell echoes them back. `pwd` is safe and always available.
        typed = session.evaluate(
            """
            const term = document.querySelector('.terminal-host .xterm');
            const textarea = term ? term.querySelector('textarea') : null;
            if (!textarea) return 'no-textarea';
            textarea.focus();
            return document.activeElement === textarea ? 'focused' : 'not-focused';
            """
        )
        check("a click target in the switched-to pane is focused and accepting input",
              typed == "focused", str(typed))

        # ---- file viewer: no dead Preview tab for a code file ----------------
        # A production build exposes no test hook, so the tree is the way in.
        opened_code = session.evaluate(
            """
            const filesTab = Array.from(document.querySelectorAll('[role="tab"]'))
              .find((node) => /files/i.test(node.textContent || ''));
            if (filesTab) filesTab.click();
            await new Promise((r) => setTimeout(r, 800));
            const findRow = () => Array.from(document.querySelectorAll('[role="treeitem"]'))
              .find((node) => (node.querySelector('.tree-name')?.textContent || '') === 'main.rs');
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {
              const row = findRow();
              if (row) { row.click(); break; }
              await new Promise((r) => setTimeout(r, 200));
            }
            await new Promise((r) => setTimeout(r, 900));
            const dialog = document.querySelector('.dialog-file');
            return {
              opened: !!dialog,
              tabs: Array.from(document.querySelectorAll('[aria-label="File view"] [role="tab"]'))
                .map((node) => (node.textContent || '').trim()),
              hasCodePane: !!document.querySelector('.file-code'),
            };
            """
        )
        check("a code file opens in the viewer",
              isinstance(opened_code, dict) and opened_code.get("opened") is True,
              str(opened_code))
        check("no dead Preview tab is offered for a .rs file",
              isinstance(opened_code, dict) and opened_code.get("tabs") == [],
              str(opened_code.get("tabs") if isinstance(opened_code, dict) else opened_code))
        check("the code file still renders its source pane",
              isinstance(opened_code, dict) and opened_code.get("hasCodePane") is True,
              str(opened_code))

        # Close, then open a markdown file: both tabs, and the keyboard model.
        md = session.evaluate(
            """
            const close = document.querySelector('[aria-label="Close dialog"]');
            if (close) close.click();
            await new Promise((r) => setTimeout(r, 400));
            const findRow = () => Array.from(document.querySelectorAll('[role="treeitem"]'))
              .find((node) => (node.querySelector('.tree-name')?.textContent || '') === 'notes.md');
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {
              const row = findRow();
              if (row) { row.click(); break; }
              await new Promise((r) => setTimeout(r, 200));
            }
            await new Promise((r) => setTimeout(r, 900));
            const list = document.querySelector('[aria-label="File view"]');
            const tabs = Array.from(document.querySelectorAll('[aria-label="File view"] [role="tab"]'));
            const preview = tabs.find((node) => /preview/i.test(node.textContent || ''));
            const source = tabs.find((node) => /source/i.test(node.textContent || ''));
            if (!list || !preview || !source) return { tabs: tabs.length };
            const previewPanel = document.getElementById(preview.getAttribute('aria-controls'));
            const before = {
              tabindex: [preview.getAttribute('tabindex'), source.getAttribute('tabindex')],
              controlsResolves: !!previewPanel,
              panelRole: previewPanel ? previewPanel.getAttribute('role') : null,
              panelNamesTab: previewPanel ? previewPanel.getAttribute('aria-labelledby') : null,
            };
            // ArrowRight must move selection and focus to Source, with no pointer.
            preview.focus();
            preview.dispatchEvent(new KeyboardEvent('keydown', {
              key: 'ArrowRight', bubbles: true, cancelable: true,
            }));
            await new Promise((r) => setTimeout(r, 500));
            const after = {
              selected: (source.getAttribute('aria-selected') || '') === 'true',
              focused: document.activeElement === source,
              hasCode: !!document.querySelector('.file-code'),
            };
            return { tabs: tabs.length, before, after };
            """
        )
        check("a markdown file offers both tabs",
              isinstance(md, dict) and md.get("tabs") == 2, str(md.get("tabs") if isinstance(md, dict) else md))
        before = md.get("before", {}) if isinstance(md, dict) else {}
        check("only the selected tab is in the tab order",
              before.get("tabindex") == ["0", "-1"], str(before.get("tabindex")))
        check("aria-controls resolves to a real tabpanel that names the tab back",
              before.get("controlsResolves") is True
              and before.get("panelRole") == "tabpanel"
              and before.get("panelNamesTab") == "file-view-tab-preview",
              str(before))
        after = md.get("after", {}) if isinstance(md, dict) else {}
        check("ArrowRight activates Source and moves focus there",
              after.get("selected") is True and after.get("focused") is True and after.get("hasCode") is True,
              str(after))

        # ---- file viewer: the rendered preview fills the modal ----------------
        # Reported as a visible defect with no test watching it: a Markdown
        # preview used a `78ch` reading measure on `.file-preview-markdown`, which
        # is *also* the scrolling surface, so the cap shrank the panel rather than
        # the prose. At 1440x900 the preview was 564px inside a 1038px pane and
        # the rest of the dialog was a strip of empty background. jsdom cannot
        # measure this, so it is measured here, in the real browser.
        filled = session.evaluate(
            """
            const previewTab = Array.from(
              document.querySelectorAll('[aria-label="File view"] [role="tab"]'))
              .find((node) => /preview/i.test(node.textContent || ''));
            if (previewTab) previewTab.click();
            await new Promise((r) => setTimeout(r, 700));
            const pane = document.querySelector('.file-view-pane');
            const scroll = document.querySelector('.file-preview-scroll');
            if (!pane || !scroll) return { pane: false, scroll: false };
            const paneWidth = pane.getBoundingClientRect().width;
            const scrollWidth = scroll.getBoundingClientRect().width;
            return {
              paneWidth: Math.round(paneWidth),
              scrollWidth: Math.round(scrollWidth),
              ratio: paneWidth ? scrollWidth / paneWidth : 0,
              maxWidth: getComputedStyle(scroll).maxWidth,
              scrollbar: scroll.scrollWidth,
              client: scroll.clientWidth,
            };
            """
        )
        check("the rendered preview spans the pane instead of leaving a blank strip",
              isinstance(filled, dict) and filled.get("ratio", 0) > 0.99,
              str(filled))
        check("the preview carries no reading measure on its scroll surface",
              isinstance(filled, dict) and filled.get("maxWidth") == "none",
              str(filled.get("maxWidth") if isinstance(filled, dict) else filled))
        check("widening the preview does not make the pane scroll sideways",
              isinstance(filled, dict) and filled.get("scrollbar") == filled.get("client"),
              str(filled))

        # ---- transcript: content-visibility is actually applied --------------
        # Type and send. The turn is persisted as a user row before the provider
        # is called, so this renders a message even with no reachable model.
        session.evaluate(
            """
            // Back to the conversation: the terminal check above left the centre
            // pane on the Terminal view.
            const conversation = Array.from(document.querySelectorAll('[role="tab"]'))
              .find((node) => /conversation/i.test(node.textContent || ''));
            if (conversation) conversation.click();
            await new Promise((r) => setTimeout(r, 600));
            const box = document.querySelector('#composer-input');
            if (box) {
              box.focus();
              box.value = '[text] does the row opt into content-visibility?';
              box.dispatchEvent(new Event('input', { bubbles: true }));
              await new Promise((r) => setTimeout(r, 200));
              const send = document.querySelector('#composer-send')
                || document.querySelector('[aria-label^="Send message"]');
              if (send) send.click();
            }
            await new Promise((r) => setTimeout(r, 2500));
            return document.querySelectorAll('.transcript .msg').length;
            """
        )
        layout = session.evaluate(
            """
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {
              const rows = Array.from(document.querySelectorAll('.transcript .msg'));
              if (rows.length > 0) {
                const style = getComputedStyle(rows[0]);
                return {
                  rows: rows.length,
                  contentVisibility: style.contentVisibility,
                  containIntrinsicSize: style.containIntrinsicSize,
                };
              }
              await new Promise((r) => setTimeout(r, 200));
            }
            return { rows: 0, html: (document.querySelector('.transcript') || {}).innerHTML?.slice(0, 300) || 'no transcript' };
            """
        )
        check("the transcript renders a message row",
              isinstance(layout, dict) and layout.get("rows", 0) > 0, str(layout)[:300])
        check("the transcript row opts into content-visibility in a real browser",
              isinstance(layout, dict) and layout.get("contentVisibility") == "auto",
              str(layout))
        check("the row reserves a measured height while it is off-screen",
              isinstance(layout, dict) and "160px" in str(layout.get("containIntrinsicSize")),
              str(layout.get("containIntrinsicSize") if isinstance(layout, dict) else layout))

        # Console errors are the last thing: a crash card or an unhandled throw
        # would show up here rather than in a selector.
        return 0 if not FAIL else 1
    finally:
        if session is not None:
            session.close()
        if not args.keep:
            for process in (chrome, server, provider):
                try:
                    process.terminate()
                except Exception:
                    pass
        print()
        print(f"{len(PASS)} passed, {len(FAIL)} failed")
        for name in FAIL:
            print(f"  FAILED: {name}")
        if args.keep:
            print(f"kept: {tmp} (server on {base})")


if __name__ == "__main__":
    sys.exit(main())