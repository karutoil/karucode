#!/usr/bin/env python3
"""Drive the two demo plugins in a real Chromium against a real server.

`plugins/demos/` exists to show what a plugin can do to the interface, so the only
honest verification is a browser: a screenshot of the panel, the chrome that went
away, the shortcut that fired, and the click that changed the plugin's own state.

Usage:
    python3 scripts/verify_plugin_demos.py [--binary target/debug/karucode]
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


def wait_js(session: Session, script: str, timeout: float = 15.0, interval: float = 0.1):
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


def press(session: Session, key: str, code: str, vk: int, modifiers: int = 0) -> None:
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


def open_panel(session: Session, title: str) -> None:
    """Click the inspector tab whose label starts with `title`."""
    deadline = time.time() + 15
    while time.time() < deadline:
        clicked = js(session, f"""
            const tab = [...document.querySelectorAll('[role=tab]')]
              .find(n => n.textContent.trim().startsWith({json.dumps(title)}));
            if (tab) tab.click();
            return tab ? 1 : 0;
        """)
        if clicked and js(session, """
            const el = document.querySelector('[role=tabpanel] .plugin-view-frame');
            return el && el.offsetParent !== null ? 1 : 0;
        """):
            return
        time.sleep(0.2)
    raise AssertionError(f"the {title} panel never showed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run `cargo build -p kc-server`")

    out = args.out or Path(tempfile.mkdtemp(prefix="plugin-demos-"))
    workspace = out / "project"
    # A fixture with real shape: prose to count, source to weigh, one big file so
    # the bar chart has a clear tallest bar.
    (workspace / "docs" / "deep").mkdir(parents=True, exist_ok=True)
    (workspace / "src").mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# Demo workspace\n\n" + "word " * 120 + "\n")
    (workspace / "docs" / "guide.md").write_text("# Guide\n\n" + "sentence " * 400 + "\n")
    (workspace / "docs" / "deep" / "notes.md").write_text("notes " * 60 + "\n")
    (workspace / "src" / "main.rs").write_text("fn main() {}\n")
    (workspace / "src" / "big.rs").write_text("// line\n" * 4_000)
    (workspace / "src" / "blob.bin").write_bytes(bytes(range(256)) * 400)
    # The number the Zen panel should show, computed the same way the plugin does
    # (`text.split()`), so this check cannot drift from the fixture.
    prose = sorted(workspace.rglob("*.md"))
    expected_words = sum(len(path.read_text().split()) for path in prose)
    expected_word_text = f"{expected_words:,}"

    home = out / "home"
    home.mkdir(exist_ok=True)
    data_dir = out / "data"
    data_dir.mkdir(exist_ok=True)

    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(out / "provider.jsonl")],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    zen = ROOT / "plugins" / "demos" / "zen" / "zen.py"
    atlas = ROOT / "plugins" / "demos" / "atlas" / "atlas.py"
    config = out / "config.toml"
    config.write_text(
        'host = "127.0.0.1"\n'
        'log_level = "warn"\n'
        '[approvals]\n'
        'mode = "auto"\n'
        '[[plugins]]\n'
        'name = "zen"\n'
        f'command = ["{sys.executable}", "{zen}"]\n'
        'enabled = true\n'
        'permissions = ["files.read"]\n'
        '[[plugins]]\n'
        'name = "atlas"\n'
        f'command = ["{sys.executable}", "{atlas}"]\n'
        'enabled = true\n'
        'permissions = ["files.read", "files.write"]\n'
    )

    server_port = free_port()
    env = {
        **os.environ,
        "HOME": str(home),
        "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(data_dir),
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

        status, body = Client(server_port).get("/api/config")
        plugins = {group["plugin"] for group in body.get("plugin_panels", [])}
        check("both demo plugins loaded", plugins == {"zen", "atlas"}, json.dumps(sorted(plugins)))
        ui = body.get("plugin_ui", {})
        contributions = [(c["plugin"], c["kind"], c["id"]) for c in ui.get("contributions", [])]
        check("zen contributes a button, a shortcut and a command",
              ("zen", "button", "zen") in contributions
              and ("zen", "keybinding", "zen-key") in contributions
              and ("zen", "command", "zen-ask") in contributions, json.dumps(contributions))
        check("atlas contributes a button, a shortcut and two commands",
              ("atlas", "button", "snapshot") in contributions
              and ("atlas", "keybinding", "snapshot-key") in contributions
              and ("atlas", "command", "atlas-snapshot") in contributions
              and ("atlas", "command", "atlas-ask") in contributions, json.dumps(contributions))
        hidden = {entry["target"]: entry["plugins"] for entry in ui.get("hidden", [])}
        check("zen asks for three built-in controls",
              set(hidden) == {"inspector.tab.agents", "inspector.context", "composer.attach"},
              json.dumps(hidden))

        # A session to look at, in the open project.
        status, projects = Client(server_port).get("/api/projects")
        project_id = (projects.get("projects") or [{}])[0].get("id")
        status, created = Client(server_port).post(
            "/api/sessions", {"title": "demo", "project": project_id}
        )
        session_id = created.get("id")

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
                "--window-size=1440,900",
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

        # Open the session from the rail, so the composer and header mount.
        wait_js(session, f"return document.querySelector('[data-session-id=\"{session_id}\"]') ? 1 : 0", 25)
        js(session, f"""document.querySelector('[data-session-id="{session_id}"] .session-open').click(); return 1;""")
        wait_js(session, "return document.querySelector('.compose-bar-left') ? 1 : 0", 25)

        # ---- zen: the chrome really is different ----
        chrome_state = js(session, """
            return {
              tabs: [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim()),
              context: !!document.querySelector('.inspector-context-button'),
              attach: !!document.querySelector('.compose-plus'),
              zenButton: !!document.querySelector('[data-plugin="zen"][data-contribution="zen"]'),
              header: !!document.querySelector('[data-plugin="zen"][data-contribution="zen"]')
                       && !!document.querySelector('[data-plugin="zen"][data-contribution="zen"]').closest('.header-buttons'),
            };
        """)
        check("zen hides the Agents tab", not any("Agents" in t for t in chrome_state["tabs"]),
              json.dumps(chrome_state))
        check("zen hides the Context button", chrome_state["context"] is False, json.dumps(chrome_state))
        check("zen hides the composer's paperclip", chrome_state["attach"] is False,
              json.dumps(chrome_state))
        check("zen puts its own button in the session header",
              chrome_state["zenButton"] and chrome_state["header"], json.dumps(chrome_state))

        # ---- zen: a live panel drawn by the view vocabulary ----
        open_panel(session, "Zen")
        shapes = js(session, """
            return {
              clock: !!document.querySelector('.plugin-view-clock'),
              stat: document.querySelector('.plugin-view-stat .plugin-view-value')?.textContent ?? '',
              bars: document.querySelectorAll('.plugin-view-bar').length,
              rows: document.querySelectorAll('.plugin-view-row').length,
              json: !!document.querySelector('.plugin-panel-json'),
              clockText: document.querySelector('.plugin-view-clock-face')?.textContent ?? '',
            };
        """)
        check("the Zen panel is drawn as views, not raw JSON", shapes["json"] is False, json.dumps(shapes))
        check("it draws a clock, a stat, a chart and a list",
              shapes["clock"] and shapes["stat"] and shapes["bars"] >= 1 and shapes["rows"] >= 1,
              json.dumps(shapes))
        check("the word count comes from the workspace", shapes["stat"] == expected_word_text,
              f"panel {shapes['stat']} vs fixture {expected_word_text}")
        first_clock = shapes["clockText"]
        session.screenshot(out / "demo-zen.png")
        print(f"  ...   screenshot: {out / 'demo-zen.png'}")
        # The panel polls every second: the clock has to move on its own.
        time.sleep(2.2)
        later_clock = js(session, "return document.querySelector('.plugin-view-clock-face')?.textContent ?? '';")
        check("the clock ticks without a click", later_clock != first_clock, f"{first_clock} -> {later_clock}")

        # ---- zen: one action, three ways in ----
        js(session, "document.activeElement && document.activeElement.blur && document.activeElement.blur(); return 1;")
        press(session, "Z", "KeyZ", 90, 2 | 8)  # Ctrl+Shift+Z
        toast = wait_js(session, """
            return [...document.querySelectorAll('.toast .toast-message')]
              .map(n => n.textContent.trim())
              .find(m => m.startsWith('Zen: ')) || '';
        """, 15)
        check("Ctrl/Cmd+Shift+Z runs the zen action",
              toast == f"Zen: {expected_word_text} words in {len(prose)} prose files.", toast)

        # ---- atlas: a second plugin, two panels, real state ----
        open_panel(session, "Atlas")
        atlas_shapes = js(session, """
            return {
              stat: !!document.querySelector('.plugin-view-stat'),
              bars: document.querySelectorAll('.plugin-view-bar').length,
              rows: document.querySelectorAll('.plugin-view-row').length,
              progress: !!document.querySelector('.plugin-view-progress'),
              json: !!document.querySelector('.plugin-panel-json'),
            };
        """)
        check("the Atlas panel draws a stat, bars, a list and progress",
              atlas_shapes["stat"] and atlas_shapes["bars"] >= 2 and atlas_shapes["rows"] >= 1
              and atlas_shapes["progress"] and atlas_shapes["json"] is False, json.dumps(atlas_shapes))
        session.screenshot(out / "demo-atlas.png")
        print(f"  ...   screenshot: {out / 'demo-atlas.png'}")

        snapshot_dir = workspace / ".karucode" / "atlas"
        check("nothing is written before you press Snapshot", not snapshot_dir.exists())

        js(session, """document.querySelector('[data-plugin="atlas"][data-contribution="snapshot"]').click(); return 1;""")
        snapshot_toast = wait_js(session, """
            return [...document.querySelectorAll('.toast .toast-message')]
              .map(n => n.textContent.trim())
              .find(m => m.startsWith('Snapshot snapshot-')) || '';
        """, 15)
        files = sorted(snapshot_dir.glob("*.json")) if snapshot_dir.exists() else []
        check("the button wrote a snapshot the plugin owns", len(files) == 1, str(files))
        check("the answer names the file it wrote", snapshot_toast.startswith("Snapshot snapshot-"),
              snapshot_toast)
        # The action asked for two panels to be re-read. The one on screen is Atlas,
        # whose "Snapshots taken" progress now reads 1 of 10 (10%).
        progress = wait_js(session, """
            const bar = document.querySelector('.plugin-view-progress [role=progressbar]');
            return bar ? Number(bar.getAttribute('aria-valuenow')) : 0;
        """, 10)
        check("the panel it refreshed picked up the new snapshot", progress == 10, str(progress))

        open_panel(session, "Snapshots")
        history = js(session, """
            return [...document.querySelectorAll('.plugin-view-row-text')].map(n => n.textContent.trim());
        """)
        check("the history tab lists the snapshot it wrote",
              any(name.endswith(".json") for name in history), json.dumps(history))

        # ---- atlas: the same action from the keyboard ----
        js(session, "document.activeElement && document.activeElement.blur && document.activeElement.blur(); return 1;")
        press(session, "S", "KeyS", 83, 2 | 1)  # Ctrl+Alt+S
        wait_js(session, "return 1;")
        deadline = time.time() + 10
        while time.time() < deadline and len(files) < 2:
            time.sleep(0.2)
            files = sorted(snapshot_dir.glob("*.json"))
        check("Ctrl/Cmd+Alt+S takes another snapshot", len(files) == 2,
              json.dumps([f.name for f in files]))

        # ---- the two plugins' tools and hides coexist ----
        settings = js(session, """
            return {
              tabs: [...document.querySelectorAll('[role=tab]')].map(t => t.textContent.trim()),
              atlasPanel: [...document.querySelectorAll('[role=tab]')]
                .some(t => t.textContent.trim().startsWith('Atlas')),
              snapshotsPanel: [...document.querySelectorAll('[role=tab]')]
                .some(t => t.textContent.trim().startsWith('Snapshots')),
            };
        """)
        check("one plugin contributes several tabs",
              settings["atlasPanel"] and settings["snapshotsPanel"], json.dumps(settings))
        check("zen's hide is still in effect while atlas runs",
              not any("Agents" in t for t in settings["tabs"]), json.dumps(settings))
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
