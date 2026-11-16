#!/usr/bin/env python3
"""Drive the rail in a real Chromium and check a plugin's chips are drawn.

The Rust and unit tests prove the data path. This proves the one thing they
cannot: that a person looking at the workspace actually sees the chip, in the
right row, with the plugin's own word in it.

Usage:
    python3 scripts/verify_session_chips.py [--binary target/debug/karucode] [--out DIR]
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
from scripts.verify_plugin_ui import check, FAIL, PASS, wait_js  # noqa: E402

PLUGIN_NAME = "session-status"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    plugin_bin = ROOT / "target/debug" / PLUGIN_NAME
    for needed in (args.binary, plugin_bin):
        if not needed.exists():
            raise SystemExit(f"missing {needed}; run `cargo build -p kc-server -p {PLUGIN_NAME}`")

    out = args.out or Path(tempfile.mkdtemp(prefix="chips-verify-"))
    workspace = out / "project"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# chips fixture\n")
    home = out / "home"
    home.mkdir(exist_ok=True)
    data_dir = out / "data"
    plugin_dir = data_dir / "plugins" / PLUGIN_NAME
    plugin_dir.mkdir(parents=True, exist_ok=True)
    (plugin_dir / "plugin.toml").write_text(
        f'name = "{PLUGIN_NAME}"\ncommand = ["{plugin_bin}"]\nenabled = true\n'
    )

    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(out / "provider.jsonl")],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    config = out / "config.toml"
    config.write_text('host = "127.0.0.1"\nlog_level = "warn"\n[approvals]\nmode = "auto"\n')
    port = free_port()
    env = {
        **os.environ, "HOME": str(home), "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(data_dir),
        "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder", "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(port), "--workspace", str(workspace)],
        cwd=workspace, env=env, stdout=(out / "server.log").open("w"), stderr=subprocess.STDOUT,
    )
    base = f"http://127.0.0.1:{port}"
    chrome = None
    try:
        wait_for(f"{base}/api/health")
        client = Client(port)
        client.post("/api/plugins/reload")
        status, projects = client.get("/api/projects")
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

        wait_js(session, "return document.querySelector('.session-row') ? 1 : 0", 20)
        chips = session.evaluate("""
            return [...document.querySelectorAll('.session-row')].map(row => ({
              title: (row.querySelector('.session-title')||{}).textContent || '',
              chips: [...row.querySelectorAll('.session-chip')].map(c => c.textContent.trim()),
            }));
        """)
        check("the rail drew at least two rows", len(chips) >= 2, json.dumps(chips)[:300])
        by_title = {row["title"]: row["chips"] for row in chips}
        check("the open session carries an 'open' chip",
              by_title.get("open work") == ["open"], json.dumps(by_title)[:300])
        check("the completed session carries a 'completed' chip",
              by_title.get("finished work") == ["completed"], json.dumps(by_title)[:300])
        # Text is the signal, not only a colour: WCAG 1.4.1.
        check("the chips are words, not colour alone",
              all(any(c.strip() for c in row) for row in by_title.values()),
              json.dumps(by_title)[:200])

        # The toned class is applied, so the chip is visually distinguishable.
        toned = session.evaluate("""
            const chip = document.querySelector('.session-chip');
            return chip ? chip.className : '';
        """)
        check("the chip carries a tone class", "session-chip-" in str(toned), str(toned))

        session.screenshot(out / "rail.png")
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
