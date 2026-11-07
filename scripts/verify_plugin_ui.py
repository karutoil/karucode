#!/usr/bin/env python3
"""Verify plugin-contributed UI in a real Chromium against a real server.

What this proves that the unit tests cannot: that a plugin's declared controls are
actually drawn by the built bundle, that a click really posts to
`/api/plugins/<name>/ui/actions/<action>` and shows the plugin's own answer, that a
contributed command is an ordinary palette row, and that the hide requests change
the inspector tabs a person sees — then revert when the plugin is disabled.

Everything is disposable: a temp workspace, a temp data dir, a scripted provider
and a headless Chromium with its own profile. Nothing touches a real karucode data
directory or a real provider key.

Usage:
    python3 scripts/verify_plugin_ui.py [--binary target/debug/karucode] [--out DIR]
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.design_verify import Session, free_port, wait_for  # noqa: E402
from scripts.e2e_smoke import Client  # noqa: E402

# A plugin that uses every contribution kind. Python, because the whole point of
# the protocol is that a plugin is just a process: no SDK, no build step.
PLUGIN = r'''
import json, sys

MANIFEST = {
    "name": "kc-focus", "version": "1.0.0", "protocol": "1",
    "capabilities": ["tools", "ui"], "permissions": [], "hooks": [],
    "ui": [
        {"kind": "button", "id": "focus", "label": "Focus mode", "icon": "bolt",
         "title": "Run the focus check", "slot": "composer.toolbar",
         "action": "focus"},
        {"kind": "button", "id": "review", "label": "Review", "slot": "inspector.header",
         "action": "review"},
        {"kind": "command", "id": "ask", "label": "Focus: what changed?",
         "message": "What changed in this workspace?"},
        {"kind": "keybinding", "id": "focus-key", "label": "Focus shortcut",
         "keys": "mod+shift+f", "action": "focus"},
        {"kind": "hide", "target": "inspector.tab.agents"}
    ]
}

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    params = req.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "ui/action":
        action = params.get("action")
        if action == "focus":
            send({"jsonrpc": "2.0", "id": rid, "result": {
                "ok": True, "message": "Focused for " + str(params.get("session_id")),
            }})
        elif action == "review":
            send({"jsonrpc": "2.0", "id": rid, "result": {
                "ok": True, "message": "Nothing to review",
                # A URL the UI must refuse to open.
                "open_url": "javascript:alert(1)",
            }})
        else:
            send({"jsonrpc": "2.0", "id": rid,
                  "error": {"code": -32601, "message": "unknown action"}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
'''

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        PASS.append(name)
        print(f"  ok    {name}")
    else:
        FAIL.append(f"{name}{': ' + detail if detail else ''}")
        print(f"  FAIL  {name}{': ' + detail if detail else ''}")


def js(session: Session, script: str):
    return session.evaluate(script)


def wait_js(session: Session, script: str, timeout: float = 10.0, interval: float = 0.1):
    """Poll a JS expression until it returns something truthy."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            last = js(session, script)
        except Exception as error:  # a navigation in flight is not a failure
            last = f"error: {error}"
        if last:
            return last
        time.sleep(interval)
    raise AssertionError(f"timed out waiting for {script!r} (last: {last!r})")


def type_into(session: Session, selector: str, text: str, timeout: float = 8.0) -> None:
    """Type into an element with real key input, retrying until the value sticks.

    Synthetic events are not good enough here: this is the one check that proves a
    contributed palette command is filtered like a built-in one, so it has to go
    through the same input path a person's fingers do. Headless Chrome can drop
    `Input.insertText` when the page is not focused, so focus, bring the page
    forward and retry — and fail loudly if the text never arrives.
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        js(session, f"""
            const el = document.querySelector({json.dumps(selector)});
            if (el) {{ el.focus(); el.select && el.select(); }}
            return el ? 1 : 0;
        """)
        session._send("Page.bringToFront")
        time.sleep(0.15)
        session._send("Input.insertText", {"text": text})
        if js(session, f"""
            const el = document.querySelector({json.dumps(selector)});
            return el && el.value === {json.dumps(text)} ? 1 : 0;
        """):
            return
        time.sleep(0.2)
    raise AssertionError(f"could not type {text!r} into {selector!r}")


def press(session: Session, key: str, code: str, vk: int, modifiers: int = 0) -> None:
    """Press one chord with real key events (CDP), not a synthetic DOM event.

    `modifiers` is the CDP bitmask: 1 alt, 2 ctrl, 4 meta, 8 shift. A synthetic
    `KeyboardEvent` would prove only that a handler exists; this goes through the
    browser's own input path, which is what a person's fingers use.
    """
    session._send("Page.bringToFront")
    time.sleep(0.1)
    for kind in ("rawKeyDown", "keyUp"):
        session._send(
            "Input.dispatchKeyEvent",
            {
                "type": kind,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": modifiers,
            },
        )
    time.sleep(0.15)


def open_settings(session: Session) -> None:
    """Open Settings the way a person does: command palette → "Open settings".

    The rail's own settings button only exists when the rail is expanded, and a
    verification script should not depend on that layout choice.
    """
    js(session, """document.querySelector('button[aria-label="Command palette"]').click(); return 1;""")
    wait_js(session, "return document.querySelector('.palette-input') ? 1 : 0", 10)
    type_into(session, ".palette-input", "settings")
    wait_js(session, """
        return [...document.querySelectorAll('.palette-item')]
          .some(n => n.textContent.toLowerCase().includes('settings')) ? 1 : 0;
    """, 10)
    js(session, """
        const row = [...document.querySelectorAll('.palette-item')]
          .find(n => n.textContent.toLowerCase().includes('settings'));
        if (row) row.click();
        return row ? 1 : 0;
    """)
    wait_js(session, "return document.querySelector('[data-testid=dialog]') ? 1 : 0", 10)


def open_plugin_controls(session: Session) -> None:
    """Open Settings → Plugins & MCP, where the person's overrides live."""
    open_settings(session)
    # Every settings panel stays mounted (`persist`), so "the element exists" is not
    # the same as "the tab is showing": wait for the panel to be visible, and retry
    # the click, because the first one can land while the dialog is still mounting.
    deadline = time.time() + 10
    while time.time() < deadline:
        js(session, """
            const tab = [...document.querySelectorAll('[role=tab]')]
              .find(n => n.textContent.includes('Plugins'));
            if (tab) tab.click();
            return tab ? 1 : 0;
        """)
        if js(session, """
            const el = document.querySelector('[data-testid=plugin-controls]');
            return el && el.offsetParent !== null ? 1 : 0;
        """):
            return
        time.sleep(0.2)
    detail = js(session, """
        return {
          tabs: [...document.querySelectorAll('[role=tab]')]
            .map(n => ({ text: n.textContent.trim(), selected: n.getAttribute('aria-selected') })),
          panel: !!document.querySelector('[data-testid=plugin-controls]'),
        };
    """)
    raise AssertionError(f"the Plugins & MCP settings tab never showed: {detail}")


def close_dialog(session: Session) -> None:
    js(session, """
        const closes = [...document.querySelectorAll('[aria-label="Close dialog"]')];
        if (closes.length) closes[closes.length - 1].click();
        return 1;
    """)
    wait_js(session, "return document.querySelector('[data-testid=dialog]') ? 0 : 1", 10)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run `cargo build -p kc-server`")

    out = args.out or Path(tempfile.mkdtemp(prefix="plugin-ui-verify-"))
    workspace = out / "project"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# Plugin UI fixture\n")
    home = out / "home"
    home.mkdir(exist_ok=True)
    data_dir = out / "data"
    plugin_dir = data_dir / "plugins" / "kc-focus"
    plugin_dir.mkdir(parents=True, exist_ok=True)

    provider_log = out / "provider.jsonl"
    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(provider_log)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    config = out / "config.toml"
    config.write_text(
        'host = "127.0.0.1"\n'
        'log_level = "warn"\n'
        '[approvals]\n'
        'mode = "auto"\n'
    )
    server_port = free_port()
    env = {
        **os.environ,
        "HOME": str(home),
        "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(data_dir),
        # The embedded bundle is a build-time snapshot; serve the one just built.
        "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder",
        "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    server_log = (out / "server.log").open("w")
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(server_port),
         "--workspace", str(workspace)],
        cwd=workspace, env=env, stdout=server_log, stderr=subprocess.STDOUT,
    )
    base = f"http://127.0.0.1:{server_port}"
    chrome = None
    try:
        wait_for(f"{base}/api/health")

        # Install the plugin through the host's own discovery path, then rescan.
        (plugin_dir / "plugin.toml").write_text(
            'name = "kc-focus"\n'
            f'command = ["{sys.executable}", "{plugin_dir / "focus.py"}"]\n'
            'enabled = true\n'
            'permissions = []\n'
        )
        (plugin_dir / "focus.py").write_text(PLUGIN)
        client = Client(server_port)
        status, report = client.post("/api/plugins/reload")
        check("the plugin hot-loads without a restart", status == 200 and
              "kc-focus" in json.dumps(report.get("report", {})), str(report)[:400])

        status, config_body = client.get("/api/config")
        ui = config_body.get("plugin_ui", {})
        kinds = [(c["kind"], c["id"]) for c in ui.get("contributions", [])]
        check("config publishes the button, the command and the shortcut",
              kinds == [("button", "focus"), ("button", "review"), ("command", "ask"),
                        ("keybinding", "focus-key")], str(kinds))
        check("config publishes the hide with its owner",
              ui.get("hidden") == [{"target": "inspector.tab.agents", "plugins": ["kc-focus"]}],
              json.dumps(ui.get("hidden")))

        # A session, so the composer, the rail row and the session header render.
        status, projects = client.get("/api/projects")
        project_id = (projects.get("projects") or [{}])[0].get("id")
        status, created = client.post(
            "/api/sessions", {"title": "plugin ui", "project": project_id}
        )
        session_id = created.get("id")
        check("a session exists in the open project",
              isinstance(session_id, str) and isinstance(project_id, str),
              str(created)[:200])

        # ---- browser ----
        chrome_profile = out / "profile"
        cdp_port = free_port()
        chrome_log = (out / "chrome.log").open("wb")
        chrome = subprocess.Popen(
            [
                "chromium", "--headless=new", "--no-sandbox", "--disable-gpu",
                "--disable-dev-shm-usage", "--hide-scrollbars",
                f"--remote-debugging-port={cdp_port}",
                f"--user-data-dir={chrome_profile}",
                f"--window-size=1440,900",
                base,
            ],
            stdout=chrome_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        ws_url = None
        deadline = time.time() + 30
        while time.time() < deadline and not ws_url:
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=1) as r:
                    for page in json.loads(r.read()):
                        if page.get("type") == "page" and page.get("webSocketDebuggerUrl"):
                            ws_url = page["webSocketDebuggerUrl"]
                            break
            except Exception:
                time.sleep(0.2)
        if not ws_url:
            raise AssertionError("Chromium did not open its DevTools endpoint")
        session = Session(ws_url)

        # Open the session the way a person does: click its row in the rail.
        wait_js(session, f"return document.querySelector('[data-session-id=\"{session_id}\"]') ? 1 : 0", 25)
        js(session, f"""document.querySelector('[data-session-id="{session_id}"] .session-open').click(); return 1;""")
        wait_js(session, f"""return document.querySelector('[data-session-id="{session_id}"].is-active, [data-session-id="{session_id}"] [aria-current="true"]') ? 1 : 0""", 15)
        wait_js(session, "return document.querySelector('.compose-bar-left') ? 1 : 0", 25)

        # The composer button is drawn, labelled and attributed.
        wait_js(session, "return document.querySelector('[data-plugin=\"kc-focus\"][data-contribution=\"focus\"]') ? 1 : 0")
        info = js(session, """
            const b = document.querySelector('[data-plugin="kc-focus"][data-contribution="focus"]');
            return { text: b.textContent.trim(), title: b.title, dot: !!b.querySelector('.plugin-slot-dot'),
                     inComposer: !!b.closest('.compose-bar-left') };
        """)
        check("the contributed button is in the composer action row", info["inComposer"], json.dumps(info))
        check("it carries its label", "Focus mode" in info["text"], json.dumps(info))
        check("it is attributed to its plugin", "kc-focus" in info["title"], json.dumps(info))
        check("it shows the plugin dot", info["dot"], json.dumps(info))

        # The inspector header button too, and the hidden tab.
        header = js(session, """
            const b = document.querySelector('[data-plugin="kc-focus"][data-contribution="review"]');
            const tabs = [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim());
            return { header: !!b && !!b.closest('.inspector-head-actions'), tabs };
        """)
        check("the inspector-header button is mounted beside Context", header["header"], json.dumps(header))
        check("the hidden inspector tab is gone", not any("Agents" in t for t in header["tabs"]),
              json.dumps(header))
        check("the remaining built-in tabs are still there",
              any("Files" in t for t in header["tabs"]) and any("Git" in t for t in header["tabs"]),
              json.dumps(header))

        # Keep a picture of the contributed controls in the real layout.
        enabled_shot = out / "plugin-ui-enabled.png"
        session.screenshot(enabled_shot)
        print(f"  ...   screenshot (plugin enabled): {enabled_shot}")

        # A control that exists but is laid out at zero size is not a control.
        box = js(session, """
            const b = document.querySelector('[data-plugin="kc-focus"][data-contribution="focus"]');
            const r = b.getBoundingClientRect();
            const s = getComputedStyle(b);
            return { w: Math.round(r.width), h: Math.round(r.height), display: s.display,
                     visibility: s.visibility, opacity: Number(s.opacity) };
        """)
        check("the contributed button is laid out and visible",
              box["w"] > 40 and box["h"] > 10 and box["display"] != "none"
              and box["visibility"] != "hidden" and box["opacity"] > 0.5, json.dumps(box))
        ellipsis = js(session, """
            const l = document.querySelector('[data-plugin="kc-focus"][data-contribution="focus"] .plugin-slot-label');
            const b = l.closest('button');
            const row = b.closest('.compose-bar-left');
            return { visible: l.textContent, name: b.getAttribute('aria-label'),
                     tooltip: b.title, clipped: l.scrollWidth > l.clientWidth + 1,
                     buttonRight: Math.round(b.getBoundingClientRect().right),
                     rowRight: Math.round(row.getBoundingClientRect().right) };
        """)
        print(f"  ...   label geometry: {ellipsis}")
        # A narrow row may ellipsise the label (the model pins beside it are cut the
        # same way); what must never happen is losing the name.
        check("its accessible name is the full label", ellipsis["name"] == "Focus mode",
              json.dumps(ellipsis))
        check("its tooltip names the plugin", "kc-focus" in (ellipsis["tooltip"] or ""),
              json.dumps(ellipsis))
        check("it stays inside the composer row",
              ellipsis["buttonRight"] <= ellipsis["rowRight"] + 1, json.dumps(ellipsis))
        crop = js(session, """
            const b = document.querySelector('[data-plugin="kc-focus"][data-contribution="focus"]');
            const r = b.getBoundingClientRect();
            const row = b.closest('.compose-bar-left') || b.parentElement;
            const rr = row.getBoundingClientRect();
            return { x: Math.max(0, rr.x - 4), y: Math.max(0, rr.y - 6),
                     width: rr.width + 8, height: rr.height + 12, scale: 2,
                     button: Math.round(r.width), label: Math.round(
                       b.querySelector('.plugin-slot-label').getBoundingClientRect().width) };
        """)
        print(f"  ...   composer row: {crop}")
        result = session._send("Page.captureScreenshot", {"format": "png", "clip": {**crop, "scale": 2}})
        (out / "plugin-ui-button.png").write_bytes(__import__("base64").b64decode(result["data"]))
        print(f"  ...   screenshot (the button): {out / 'plugin-ui-button.png'}")

        head = js(session, """
            const actions = document.querySelector('.inspector-head-actions');
            const r = actions.getBoundingClientRect();
            return { x: Math.max(0, r.x - 6), y: Math.max(0, r.y - 6),
                     width: Math.max(120, r.width + 12), height: r.height + 12, scale: 2 };
        """)
        result = session._send("Page.captureScreenshot", {"format": "png", "clip": head})
        (out / "plugin-ui-inspector.png").write_bytes(__import__("base64").b64decode(result["data"]))
        print(f"  ...   screenshot (inspector header): {out / 'plugin-ui-inspector.png'}")

        # Click it: one request, the plugin's own answer as a toast.
        js(session, """document.querySelector('[data-plugin="kc-focus"][data-contribution="focus"]').click(); return 1;""")
        toast = wait_js(session, """
            const t = [...document.querySelectorAll('.toast .toast-message')]
              .map(n => n.textContent.trim()).find(m => m.startsWith('Focused for'));
            return t || '';
        """, 15)
        check("clicking runs ui/action and shows the plugin's response", f"Focused for {session_id}" == toast,
              toast)

        # A URL that is not http(s) never opens a tab.
        js(session, """document.querySelector('[data-plugin="kc-focus"][data-contribution="review"]').click(); return 1;""")
        wait_js(session, """
            return [...document.querySelectorAll('.toast .toast-message')]
              .some(n => n.textContent.includes('Nothing to review')) ? 1 : 0;
        """, 15)
        pages = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=2).read())
        check("a javascript: open_url never becomes a tab",
              len([p for p in pages if p.get("type") == "page"]) == 1, str(len(pages)))

        # The contributed command is a real palette row.
        js(session, """document.querySelector('button[aria-label="Command palette"]').click(); return 1;""")
        wait_js(session, "return document.querySelector('.palette-input') ? 1 : 0", 10)
        row = js(session, """
            return [...document.querySelectorAll('.palette-item')]
              .filter(n => n.getAttribute('data-plugin') || n.textContent.includes('Focus:'))
              .map(n => n.textContent.trim());
        """)
        check("the contributed command is a palette row", any("what changed" in r for r in row), json.dumps(row))
        group = js(session, """
            return [...document.querySelectorAll('.palette-group-label')].map(n => n.textContent.trim());
        """)
        check("its rows are grouped under Plugins, naming the plugin", "Plugins" in group, json.dumps(group))
        # Typing filters it: use real key input, not a synthetic event, so this is
        # the same path a person's fingers take.
        type_into(session, ".palette-input", "Focus")
        wait_js(session, """
            const rows = [...document.querySelectorAll('.palette-item')].map(n => n.textContent);
            return rows.length && rows.some(r => r.includes('what changed')) &&
                   !rows.some(r => r.includes('New session')) ? 1 : 0;
        """, 10)
        filtered = js(session, """
            return [...document.querySelectorAll('.palette-item')].map(n => n.textContent.trim());
        """)
        check("typing filters down to the contributed command",
              any("what changed" in r for r in filtered) and
              not any("New session" in r for r in filtered), json.dumps(filtered))
        session.evaluate("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true})); return 1;")

        # ---- a contributed shortcut, pressed for real ----
        js(session, "document.activeElement && document.activeElement.blur && document.activeElement.blur(); return 1;")
        press(session, "F", "KeyF", 70, 2 | 8)  # Ctrl+Shift+F (mod is Ctrl on Linux)
        shortcut_toast = wait_js(session, """
            return [...document.querySelectorAll('.toast .toast-message')]
              .map(n => n.textContent.trim())
              .filter(m => m.startsWith('Focused for')).pop() || '';
        """, 15)
        check("a contributed shortcut runs the same action a button does",
              shortcut_toast == f"Focused for {session_id}", shortcut_toast)

        # …but never while the caret is in a field.
        js(session, """document.querySelector('#composer-input').focus(); return 1;""")
        before = js(session, "return document.querySelector('.toast-region')?.textContent ?? '';")
        press(session, "F", "KeyF", 70, 2 | 8)
        time.sleep(0.4)
        after = js(session, "return document.querySelector('.toast-region')?.textContent ?? '';")
        check("the same chord does nothing while typing", before == after, f"{before!r} -> {after!r}")
        js(session, "document.activeElement && document.activeElement.blur && document.activeElement.blur(); return 1;")

        # ---- the person's own overrides, in Settings ----
        open_plugin_controls(session)
        listed = js(session, """
            return [...document.querySelectorAll('[data-testid=plugin-controls] [role=switch]')]
              .map(n => ({ name: n.getAttribute('aria-label'), on: n.getAttribute('aria-checked') }));
        """)
        check("settings lists every contributed control and every hide",
              any(r["name"] == "Focus mode" for r in listed) and
              any(r["name"] == "Show the Agents tab again" for r in listed), json.dumps(listed))
        check("both start in the state the plugin asked for",
              all(r["on"] == "true" for r in listed if r["name"] == "Focus mode") and
              all(r["on"] == "false" for r in listed if r["name"] == "Show the Agents tab again"),
              json.dumps(listed))
        controls_shot = out / "plugin-ui-settings.png"
        session.screenshot(controls_shot)
        print(f"  ...   screenshot (controls in the interface): {controls_shot}")

        js(session, """
            const toggle = (name) => {
              const s = [...document.querySelectorAll('[data-testid=plugin-controls] [role=switch]')]
                .find(n => n.getAttribute('aria-label') === name);
              if (s) s.click();
              return s ? 1 : 0;
            };
            return toggle('Focus mode') & toggle('Show the Agents tab again');
        """)
        flipped = js(session, """
            return [...document.querySelectorAll('[data-testid=plugin-controls] [role=switch]')]
              .map(n => ({ name: n.getAttribute('aria-label'), on: n.getAttribute('aria-checked') }));
        """)
        check("the switches take effect immediately",
              all(r["on"] == "false" for r in flipped if r["name"] == "Focus mode") and
              all(r["on"] == "true" for r in flipped if r["name"] == "Show the Agents tab again"),
              json.dumps(flipped))

        close_dialog(session)
        overridden = js(session, """
            return {
              switchedOff: document.querySelectorAll('[data-plugin="kc-focus"][data-contribution="focus"]').length,
              sibling: document.querySelectorAll('[data-plugin="kc-focus"][data-contribution="review"]').length,
              tabs: [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim()),
            };
        """)
        check("a switched-off control disappears without a reload",
              overridden["switchedOff"] == 0, json.dumps(overridden))
        check("the plugin's other control is untouched", overridden["sibling"] == 1,
              json.dumps(overridden))
        check("a hidden built-in comes back when the person overrides the hide",
              any("Agents" in t for t in overridden["tabs"]), json.dumps(overridden))

        # Reset, so the rest of this script sees the plugins' own declarations again.
        open_plugin_controls(session)
        js(session, """
            const reset = [...document.querySelectorAll('[data-testid=plugin-controls] button')]
              .find(n => n.textContent.includes('Reset to what the plugins ask for'));
            if (reset) reset.click();
            return reset ? 1 : 0;
        """)
        close_dialog(session)
        restored = js(session, """
            return {
              buttons: document.querySelectorAll('[data-plugin="kc-focus"]').length,
              tabs: [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim()),
            };
        """)
        check("reset brings the declarations back", restored["buttons"] >= 1,
              json.dumps(restored))
        check("reset hides the built-in again",
              not any("Agents" in t for t in restored["tabs"]), json.dumps(restored))

        # Disabling the plugin takes its controls away and gives the tab back.
        status, _ = client.post("/api/plugins/kc-focus/enable", {"enabled": False})
        check("the plugin can be disabled", status == 200)
        status, after = client.get("/api/config")
        check("a disabled plugin contributes nothing",
              after.get("plugin_ui", {}).get("contributions") == [] and
              after.get("plugin_ui", {}).get("hidden") == [], json.dumps(after.get("plugin_ui"))[:300])

        session.evaluate("location.reload(); return 1;")
        wait_js(session, "return document.querySelector('.compose-bar-left') ? 1 : 0", 25)
        gone = js(session, """
            return {
              buttons: document.querySelectorAll('[data-plugin="kc-focus"]').length,
              tabs: [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim()),
            };
        """)
        check("its buttons disappear", gone["buttons"] == 0, json.dumps(gone))
        check("the hidden tab comes back", any("Agents" in t for t in gone["tabs"]), json.dumps(gone))

        shot = out / "plugin-ui.png"
        session.screenshot(shot)
        print(f"  ...   screenshot: {shot}")

    finally:
        if chrome is not None:
            try:
                chrome.terminate()
                chrome.wait(timeout=10)
            except Exception:
                chrome.kill()
        server.terminate()
        try:
            server.wait(timeout=10)
        except Exception:
            server.kill()
        provider.terminate()

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    for failure in FAIL:
        print(f"  - {failure}")
    print(f"artifacts: {out}")
    return 0 if not FAIL else 1


if __name__ == "__main__":
    raise SystemExit(main())
