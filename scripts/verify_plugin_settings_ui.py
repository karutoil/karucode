#!/usr/bin/env python3
"""Drive the plugin-settings form in a real Chromium.

The Rust tests prove the schema validates and the API stores a value; the vitest
tests prove the form's arithmetic. Neither can prove the thing a person cares
about: that opening Settings → Plugins & MCP shows *this plugin's* fields, and
that changing one there changes what the plugin does.

So this script does it the way a person would: click Settings, open the plugin
tab, expand the plugin's settings, uncheck a box, save, and then look at the rail
to see the session move back into the date list.

Usage:
    python3 scripts/verify_plugin_settings_ui.py [--binary target/debug/karucode] [--out DIR]
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
from scripts.verify_plugin_ui import FAIL, PASS, check, js, wait_js  # noqa: E402

PLUGIN_NAME = "session-status"


def open_plugins_tab(session: Session) -> None:
    """Open Settings → Plugins & MCP and wait for the panel to be visible."""
    js(session, """
        const button = [...document.querySelectorAll('button')]
          .find(b => (b.getAttribute('aria-label') || '').toLowerCase().includes('settings')
                  || (b.textContent || '').trim() === 'Settings');
        if (button) button.click();
        return button ? 1 : 0;
    """)
    deadline = time.time() + 15
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
        return [...document.querySelectorAll('[role=tab]')].map(n => n.textContent.trim());
    """)
    raise AssertionError(f"the Plugins & MCP tab never showed: {detail}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    plugin_bin = ROOT / "target/debug" / PLUGIN_NAME
    for needed in (args.binary, plugin_bin):
        if not needed.exists():
            raise SystemExit(f"missing {needed}; run `cargo build -p kc-server -p {PLUGIN_NAME}`")

    out = args.out or Path(tempfile.mkdtemp(prefix="settings-ui-"))
    workspace = out / "project"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# settings fixture\n")
    home = out / "home"
    home.mkdir(exist_ok=True)
    data_dir = out / "data"
    plugin_dir = data_dir / "plugins" / PLUGIN_NAME
    plugin_dir.mkdir(parents=True, exist_ok=True)
    (plugin_dir / "plugin.toml").write_text(
        f'name = "{PLUGIN_NAME}"\ncommand = ["{plugin_bin}"]\nenabled = true\n'
    )
    web_dir = ROOT / "web" / "dist"
    if not (web_dir / "index.html").exists():
        raise SystemExit("web/dist is missing; run `npm run build` in web/")

    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    config = out / "config.toml"
    config.write_text('host = "127.0.0.1"\nlog_level = "warn"\n[approvals]\nmode = "auto"\n')
    port = free_port()
    env = {
        **os.environ, "HOME": str(home), "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(data_dir),
        "KARUCODE_WEB_DIR": str(web_dir),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder", "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(port),
         "--workspace", str(workspace)],
        cwd=workspace, env=env, stdout=(out / "server.log").open("w"),
        stderr=subprocess.STDOUT,
    )
    base = f"http://127.0.0.1:{port}"
    chrome = None
    try:
        wait_for(f"{base}/api/health")
        client = Client(port)
        client.post("/api/plugins/reload")
        _, projects = client.get("/api/projects")
        project_id = (projects.get("projects") or [{}])[0].get("id")
        _, first = client.post("/api/sessions", {"title": "open work", "project": project_id})
        _, second = client.post("/api/sessions", {"title": "finished work", "project": project_id})
        client.post(f"/api/sessions/{second.get('id')}/messages",
                    {"content": "/plugin-session-status-complete done"})

        cdp_port = free_port()
        chrome = subprocess.Popen(
            ["chromium", "--headless=new", "--no-sandbox", "--disable-gpu",
             "--disable-dev-shm-usage", "--hide-scrollbars",
             f"--remote-debugging-port={cdp_port}",
             f"--user-data-dir={out / 'profile'}", "--window-size=1440,900", base],
            stdout=(out / "chrome.log").open("wb"), stderr=subprocess.STDOUT,
        )
        ws_url = None
        deadline = time.time() + 30
        while time.time() < deadline and not ws_url:
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{cdp_port}/json/list", timeout=1
                ) as response:
                    for page in json.loads(response.read()):
                        if page.get("type") == "page" and page.get("webSocketDebuggerUrl"):
                            ws_url = page["webSocketDebuggerUrl"]
                            break
            except Exception:
                time.sleep(0.2)
        if not ws_url:
            raise AssertionError("Chromium did not open its DevTools endpoint")
        session = Session(ws_url)
        session.navigate(base)
        wait_js(session, "return document.querySelector('.session-row') ? 1 : 0", 25)

        # Before: the completed session is folded into group 1.
        before = js(session, """
            const rows = [...document.querySelectorAll('.session-row')];
            return rows.map(r => ({
              title: (r.querySelector('.session-title')||{}).textContent || '',
              chips: [...r.querySelectorAll('.session-chip')].map(c => c.textContent.trim()),
            }));
        """)
        by_title = {row["title"]: row["chips"] for row in before}
        check("the completed session starts with its chip",
              by_title.get("finished work") == ["completed"], json.dumps(by_title)[:200])

        open_plugins_tab(session)
        session.screenshot(out / "settings-plugins.png")

        # The plugin's own settings are found by its label, not by a hardcoded
        # id: the form is generated from the schema.
        opened = js(session, f"""
            const buttons = [...document.querySelectorAll('.plugin-settings-toggle')];
            const target = buttons.find(b => b.closest('li')?.textContent.includes('{PLUGIN_NAME}'));
            if (target) target.click();
            return buttons.length;
        """)
        check("the plugin row grows a Settings control", opened and opened > 0, str(opened))
        wait_js(session, f"""
            const box = document.querySelector('#plugin-{PLUGIN_NAME}-collapse_completed');
            return box ? 1 : 0;
        """, 10)
        session.screenshot(out / "settings-form.png")

        fields = js(session, f"""
            const body = document.querySelector('#plugin-{PLUGIN_NAME}-collapse_completed')
              ?.closest('.plugin-settings-body');
            if (!body) return {{}};
            return {{
              checkboxes: [...body.querySelectorAll('input[type=checkbox]')].length,
              numbers: [...body.querySelectorAll('input[type=number]')].length,
              texts: [...body.querySelectorAll('input[type=text]')].length,
              labels: [...body.querySelectorAll('.plugin-setting-label')].map(l => l.textContent.trim()),
              checkLabels: [...body.querySelectorAll('.plugin-setting-check')].map(l => l.textContent.trim()),
              help: [...body.querySelectorAll('.plugin-setting-help')].map(h => h.textContent.trim()),
            }};
        """)
        check("the form renders one field per declared setting",
              fields.get("checkboxes") == 1 and fields.get("numbers") == 1
              and fields.get("texts") == 1, json.dumps(fields)[:300])
        # A bool draws its label beside the box (one click target), so its word is
        # in the checkbox's own label rather than in `.plugin-setting-label`.
        check("each field carries the plugin's own label",
              "Fold completed sessions away" in (fields.get("checkLabels") or [])
              and "Group label" in (fields.get("labels") or []),
              json.dumps(fields)[:300])
        check("the help text the plugin wrote is shown",
              any("Finished sessions" in h for h in fields.get("help") or []),
              json.dumps(fields.get("help"))[:200])

        # Uncheck it and save, as a person would. Each step waits for the render
        # it triggers: Preact is asynchronous, and a click issued in the same tick
        # as the previous one lands on a DOM that does not exist yet.
        js(session, f"""
            const box = document.querySelector('#plugin-{PLUGIN_NAME}-collapse_completed');
            box.click();
            return box.checked;
        """)
        wait_js(session, f"""
            const box = document.querySelector('#plugin-{PLUGIN_NAME}-collapse_completed');
            return box && box.checked === false ? 1 : 0;
        """, 10)
        clicked = js(session, f"""
            const save = [...document.querySelectorAll(
              '#plugin-{PLUGIN_NAME}-collapse_completed, .plugin-settings-body button')]
              .find(b => b.textContent.trim().startsWith('Save'));
            if (save && !save.disabled) save.click();
            return (save ? save.textContent.trim() : '') + '|' +
                   (save ? String(save.disabled) : 'missing');
        """)
        check("the form's own Save button is enabled and clicked",
              str(clicked) == "Save and restart|false", str(clicked))
        session.screenshot(out / "settings-saved.png")

        # The API agrees...
        _, schema = client.get(f"/api/plugins/{PLUGIN_NAME}/settings")
        values = {f["key"]: f.get("value") for f in schema.get("settings") or []}
        check("the saved value is what the server now reports",
              values.get("collapse_completed") is False, json.dumps(values)[:200])

        # ...and so does the rail, once the reload lands.
        js(session, """
            const close = document.querySelector('[aria-label="Close settings"], [aria-label="Close"]');
            if (close) close.click();
            return 1;
        """)
        time.sleep(1.0)
        deadline = time.time() + 20
        moved = False
        while time.time() < deadline:
            js(session, """
                const buttons = [...document.querySelectorAll('button')]
                  .filter(b => (b.textContent || '').trim() === 'All');
                if (buttons[0]) buttons[0].click();
                return 1;
            """)
            groups = js(session, """
                return [...document.querySelectorAll('.session-group-label')]
                  .map(g => g.textContent.trim());
            """)
            if not any(g.startswith("Completed") for g in groups or []):
                moved = True
                break
            time.sleep(0.5)
        check("unchecking the setting stops the session being folded away", moved, str(groups))
        session.screenshot(out / "rail-after-setting.png")
    finally:
        if chrome is not None:
            chrome.terminate()
        server.terminate()
        provider.terminate()

    print()
    if FAIL:
        print(f"{len(FAIL)} failed, {len(PASS)} passed")
        for failure in FAIL:
            print(f"  - {failure}")
        return 1
    print(f"all {len(PASS)} checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
