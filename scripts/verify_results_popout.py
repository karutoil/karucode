#!/usr/bin/env python3
"""A disposable, real-backend / real-Chromium check of the Results panel's two presentations.

Run `python3 scripts/verify_results_popout.py` after `npm run build` in web/.

What it proves, end to end and against a freshly built bundle:

* the default presentation is the docked split (one `has-result` row, a docked
  separator, no results button in the session header);
* Settings -> General -> Appearance -> **Results panel** switches this browser to
  *Pop out* (`karucode.inspectorMode`), and the panel starts closed so nothing
  covers the conversation until it is asked for;
* in the pop-out the conversation keeps the whole pane, the card is a fixed,
  rounded, elevated overlay inset from the workspace's right edge, and the drag
  handle sits exactly on that edge;
* the handle resizes the card from the keyboard with the dock's own keys and
  bounds, and the card follows the handle;
* Escape closes the card instead of reaching the run;
* opening Context closes the card first, so the two right-hand overlays never
  stack;
* below 1024px nothing changes — no results button, no fixed card, no horizontal
  overflow;
* switching back to *Docked* restores the split.

The provider, database and browser profile are temporary. No running karucode
data directory, user terminal or real-provider key is used.
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


def free_port() -> int:
    with __import__("socket").socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_http(url: str, timeout: float = 20) -> None:
    until = time.time() + timeout
    while time.time() < until:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.1)
    raise AssertionError(f"timeout waiting for {url}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run the documented server build")

    from scripts.design_verify import Session
    from scripts.e2e_smoke import Client

    out = args.out or Path(tempfile.mkdtemp(prefix="results-popout-verify-"))
    workspace = out / "fixture-repo"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# Results pop-out fixture\n")

    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    config = out / "config.toml"
    config.write_text('host = "127.0.0.1"\nlog_level = "warn"\n[approvals]\nmode = "auto"\n')
    server_port = free_port()
    env = {
        **os.environ,
        "HOME": str(out / "home"),
        "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(out / "data"),
        "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder",
        "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    (out / "home").mkdir(exist_ok=True)
    server_log = (out / "server.log").open("w")
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(server_port), "--workspace", str(workspace)],
        cwd=workspace,
        env=env,
        stdout=server_log,
        stderr=subprocess.STDOUT,
    )
    browser = None
    checks: list[dict] = []

    def check(name: str, condition: bool, detail: object = None) -> None:
        checks.append({"name": name, "passed": bool(condition), "detail": detail})
        suffix = f" :: {json.dumps(detail)[:400]}" if detail is not None and not condition else ""
        print(("PASS " if condition else "FAIL ") + name + suffix, flush=True)
        if not condition:
            raise AssertionError(name)

    session = None

    def evaluate(body: str):
        return session.evaluate(body)

    def wait_js(body: str, timeout: float = 20) -> bool:
        until = time.time() + timeout
        while time.time() < until:
            if evaluate(body):
                return True
            time.sleep(0.1)
        return False

    def press_escape() -> None:
        session._send("Input.dispatchKeyEvent", {"type": "keyDown", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})
        session._send("Input.dispatchKeyEvent", {"type": "keyUp", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})
        time.sleep(0.25)

    def press_key(key: str, code: str, virtual_key: int) -> None:
        payload = {"key": key, "code": code, "windowsVirtualKeyCode": virtual_key}
        session._send("Input.dispatchKeyEvent", {"type": "keyDown", **payload})
        session._send("Input.dispatchKeyEvent", {"type": "keyUp", **payload})
        time.sleep(0.2)

    def frame():
        """Everything the two presentations are judged on, measured in the page."""
        return evaluate("""
        const body = document.querySelector('.center-body');
        const card = document.querySelector('#workbench-results');
        const separator = document.querySelector('.result-resizer');
        const chat = document.querySelector('.chat-panel');
        const header = document.querySelector('.main-head');
        const box = (el) => (el ? el.getBoundingClientRect() : null);
        const cardBox = box(card);
        const chatBox = box(chat);
        const sepBox = box(separator);
        const cardStyle = card ? getComputedStyle(card) : null;
        const resultsButton = header?.querySelector('.results-head-btn');
        const contextButton = header?.querySelector('.context-header-btn');
        const contextRing = contextButton?.querySelector('.context-header-ring-fill');
        return {
          width: innerWidth,
          height: innerHeight,
          scrollWidth: document.documentElement.scrollWidth,
          cardInset: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--space-6')) || 0,
          mode: document.querySelector('.app')?.getAttribute('data-inspector-mode'),
          collapsed: document.querySelector('.app')?.getAttribute('data-inspector-collapsed'),
          stored: localStorage.getItem('karucode.inspectorMode'),
          hasResult: !!body?.classList.contains('has-result'),
          bodyX: body ? body.getBoundingClientRect().x : null,
          bodyWidth: body ? body.getBoundingClientRect().width : null,
          card: cardBox && cardStyle && {
            x: cardBox.x, right: cardBox.right, width: cardBox.width,
            top: cardBox.top, bottom: cardBox.bottom,
            display: cardStyle.display, position: cardStyle.position,
            visibility: cardStyle.visibility,
            radius: parseFloat(cardStyle.borderTopLeftRadius),
            shadow: cardStyle.boxShadow !== 'none',
            role: card.getAttribute('role'),
            popout: card.getAttribute('data-results-popout'),
          },
          separator: sepBox && {
            x: sepBox.x, right: sepBox.right, width: sepBox.width,
            top: sepBox.top, bottom: sepBox.bottom,
            popout: separator.classList.contains('result-resizer-popout'),
            value: Number(separator.getAttribute('aria-valuenow')),
            max: Number(separator.getAttribute('aria-valuemax')),
            display: getComputedStyle(separator).display,
          },
          chat: chatBox && { x: chatBox.x, right: chatBox.right, width: chatBox.width },
          resultsButton: resultsButton && {
            label: resultsButton.getAttribute('aria-label'),
            expanded: resultsButton.getAttribute('aria-expanded'),
            width: resultsButton.getBoundingClientRect().width,
          },
          contextButton: contextButton && {
            label: contextButton.getAttribute('aria-label'),
            text: (contextButton.textContent || '').trim(),
            width: contextButton.getBoundingClientRect().width,
            ring: !!contextRing,
            dashOffset: contextRing ? Number(contextRing.getAttribute('stroke-dashoffset')) : null,
          },
          contextOpen: !!document.querySelector('#workbench-context'),
          errors: window.__workbenchErrors || [],
        };
        """)

    def open_settings_appearance() -> object:
        return evaluate("""
        const deadline = Date.now() + 20000;
        while (Date.now() < deadline) {
          const select = document.querySelector('#settings-results-mode');
          if (select) return { state: 'ready', value: select.value };
          const tablist = document.querySelector('[role=tablist][aria-label="Settings sections"]');
          if (!tablist) {
            const gear = Array.from(document.querySelectorAll('button')).find(
              (button) => /open settings/i.test(button.getAttribute('aria-label') || ''),
            );
            if (gear) gear.click();
          }
          await new Promise((resolve) => setTimeout(resolve, 200));
        }
        return { state: 'timeout', dialog: !!document.querySelector('[role=dialog]') };
        """)

    def choose_results_mode(mode: str) -> object:
        return evaluate("""
        const select = document.querySelector('#settings-results-mode');
        if (!select) return { state: 'no-select' };
        const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set;
        setter.call(select, %s);
        select.dispatchEvent(new Event('change', { bubbles: true }));
        await new Promise((resolve) => setTimeout(resolve, 250));
        return {
          state: 'changed',
          value: select.value,
          mode: document.querySelector('.app').getAttribute('data-inspector-mode'),
          collapsed: document.querySelector('.app').getAttribute('data-inspector-collapsed'),
          stored: localStorage.getItem('karucode.inspectorMode'),
        };
        """ % json.dumps(mode))

    try:
        wait_http(f"http://127.0.0.1:{provider_port}/health")
        wait_http(f"http://127.0.0.1:{server_port}/api/health")
        client = Client(server_port)
        projects = client.ok("GET", "/api/projects")["projects"]
        project = next((item for item in projects if item["path"] == str(workspace.resolve())), None)
        if project is None:
            project = client.ok("POST", "/api/projects", {"path": str(workspace), "name": "results-fixture"})
        session_data = client.ok("POST", "/api/sessions", {"title": "Results popout verification", "project": project["id"]})

        cdp_port = free_port()
        browser_log = (out / "chromium.log").open("w")
        browser = subprocess.Popen(
            ["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
             f"--remote-debugging-port={cdp_port}", f"--user-data-dir={out / 'browser-profile'}",
             "--window-size=1440,1000", "about:blank"],
            stdout=browser_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        wait_http(f"http://127.0.0.1:{cdp_port}/json/version")
        targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list").read())
        page = next(target for target in targets if target.get("type") == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session._send("Runtime.enable")
        evaluate("""
        window.__workbenchErrors = [];
        window.addEventListener('error', e => window.__workbenchErrors.push(String(e.message || e.type)));
        window.addEventListener('unhandledrejection', e => window.__workbenchErrors.push(String(e.reason || e.type)));
        return true;
        """)
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        session.navigate(f"http://127.0.0.1:{server_port}")
        check("the freshly built Workbench boots", wait_js("return !!document.querySelector('.chat-main.is-workbench');", 30))
        check("the session opens with a composer", wait_js(
            "return !!document.querySelector('#composer-input') && document.body.innerText.includes('Results popout verification');", 30))
        time.sleep(0.4)

        # ---------------------------------------------------------------- docked
        docked = frame()
        check("the default presentation is the docked split",
              docked["mode"] == "docked" and docked["hasResult"] and docked["separator"] is not None
              and not docked["separator"]["popout"] and docked["card"]["position"] == "static", docked)
        check("a docked panel needs no header button of its own",
              docked["resultsButton"] is None and docked["card"]["display"] != "none", docked)
        check("the docked separator carries the dock's width and bounds",
              docked["separator"]["width"] == 24 and docked["separator"]["max"] > 280, docked["separator"])
        session.screenshot(out / "01-docked.png")

        # ------------------------------------------------- the appearance option
        opened = open_settings_appearance()
        check("Appearance -> Results panel exists in the settings sheet",
              isinstance(opened, dict) and opened.get("state") == "ready" and opened.get("value") == "docked", opened)
        chosen = choose_results_mode("popout")
        check("choosing Pop out is stored in this browser and closes the panel",
              isinstance(chosen, dict) and chosen.get("state") == "changed" and chosen.get("value") == "popout"
              and chosen.get("mode") == "popout" and chosen.get("collapsed") == "true"
              and chosen.get("stored") == "popout", chosen)
        press_escape()
        check("the settings sheet closes", wait_js("return !document.querySelector('#settings-results-mode');", 5))

        # --------------------------------------------------------- popped out
        popped = frame()
        check("a pop-out takes the split away from the layout",
              not popped["hasResult"] and popped["chat"]["width"] >= popped["bodyWidth"] - 1, popped)
        check("a closed pop-out covers nothing",
              popped["card"]["display"] == "none" and popped["separator"] is None, popped["card"])
        check("the session header grows an Open results button",
              popped["resultsButton"] is not None and popped["resultsButton"]["label"] == "Open results"
              and popped["resultsButton"]["expanded"] == "false"
              and popped["resultsButton"]["width"] > 0, popped["resultsButton"])
        check("the context button is an icon-only usage ring in the header",
              popped["contextButton"] is not None and popped["contextButton"]["ring"]
              and "Context" not in popped["contextButton"]["text"]
              and popped["contextButton"]["width"] > 0, popped["contextButton"])
        session.screenshot(out / "02-popout-closed.png")

        evaluate("document.querySelector('.results-head-btn').click(); return true;")
        time.sleep(0.35)
        opened_card = frame()
        check("opening it floats a card inset from the workspace's right edge",
              opened_card["card"]["display"] != "none" and opened_card["card"]["position"] == "fixed"
              and abs(opened_card["width"] - opened_card["card"]["right"] - opened_card["cardInset"]) <= 1
              and opened_card["card"]["radius"] > 0 and opened_card["card"]["shadow"]
              and opened_card["card"]["popout"] == "true" and opened_card["card"]["role"] == "dialog", opened_card["card"])
        check("the conversation still owns the whole pane underneath",
              not opened_card["hasResult"] and opened_card["chat"]["width"] >= opened_card["bodyWidth"] - 1
              and opened_card["resultsButton"]["expanded"] == "true", opened_card)
        check("the card's left edge is the drag handle",
              opened_card["separator"] is not None and opened_card["separator"]["popout"]
              and opened_card["separator"]["width"] == 24
              and abs(opened_card["separator"]["right"] - (opened_card["card"]["x"] + 12)) <= 1
              and opened_card["separator"]["display"] != "none", opened_card["separator"])
        session.screenshot(out / "03-popout-open.png")

        # ------------------------------------------------------- resizing it
        evaluate("document.querySelector('.result-resizer-popout').focus(); return true;")
        before = frame()
        press_key("ArrowLeft", "ArrowLeft", 37)
        wider = frame()
        check("ArrowLeft widens the card by 10px",
              abs(wider["card"]["width"] - before["card"]["width"] - 10) <= 1
              and wider["separator"]["value"] == before["separator"]["value"] + 10, {
                  "before": before["card"]["width"], "after": wider["card"]["width"],
                  "value": wider["separator"]["value"],
              })
        check("the handle stays on the card's edge while it moves",
              abs(wider["separator"]["right"] - (wider["card"]["x"] + 12)) <= 1
              and abs(wider["width"] - wider["card"]["right"] - wider["cardInset"]) <= 1, wider["separator"])
        press_key("Home", "Home", 36)
        narrowest = frame()
        check("Home picks the safe minimum, not a collapsed card",
              narrowest["separator"]["value"] == 280 and abs(narrowest["card"]["width"] - 280) <= 1, narrowest["card"])
        press_key("End", "End", 35)
        widest = frame()
        check("End stops at the dock's ceiling, leaving the conversation its floor",
              widest["separator"]["value"] == widest["separator"]["max"]
              and widest["card"]["x"] - widest["bodyX"] >= 329
              and widest["separator"]["right"] <= widest["card"]["x"] + 13, widest["card"])
        session.screenshot(out / "04-popout-resized.png")

        # --------------------------------------------------------- Escape out
        press_escape()
        after_escape = frame()
        check("Escape closes the card instead of stopping the run",
              after_escape["card"]["display"] == "none" and after_escape["separator"] is None
              and after_escape["resultsButton"]["label"] == "Open results", after_escape["card"])

        # --------------------------------------------------- no stacked overlays
        evaluate("document.querySelector('.results-head-btn').click(); return true;")
        time.sleep(0.3)
        evaluate("document.querySelector('.context-header-btn').click(); return true;")
        time.sleep(0.35)
        with_context = frame()
        check("opening Context closes the pop-out, so neither covers the other",
              with_context["contextOpen"] and with_context["card"]["display"] == "none"
              and with_context["resultsButton"]["label"] == "Open results", with_context)
        session.screenshot(out / "05-context-over-closed-popout.png")
        press_escape()
        check("closing Context leaves the pop-out closed", wait_js(
            "return !document.querySelector('#workbench-context') && document.querySelector('#workbench-results')"
            " && getComputedStyle(document.querySelector('#workbench-results')).display === 'none';", 5))

        # -------------------------------------------------------------- phone
        session._send("Emulation.setDeviceMetricsOverride", {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True})
        time.sleep(0.5)
        phone = frame()
        hidden_on_phone = (
            phone["card"]["display"] == "none"
            or phone["card"]["visibility"] == "hidden"
            or phone["card"]["x"] >= phone["width"] - 1
        )
        check("a phone keeps its sheet contract: no desktop card, no header button",
              phone["resultsButton"] is None
              and phone["card"]["popout"] is None
              and phone["separator"] is None
              and hidden_on_phone
              and phone["scrollWidth"] <= phone["width"] + 1, phone)
        session.screenshot(out / "06-phone.png")
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        time.sleep(0.4)

        # ------------------------------------------------------ back to docked
        opened_again = open_settings_appearance()
        check("the option reads back the stored choice",
              isinstance(opened_again, dict) and opened_again.get("value") == "popout", opened_again)
        restored = choose_results_mode("docked")
        press_escape()
        check("choosing Docked restores the split",
              isinstance(restored, dict) and restored.get("mode") == "docked"
              and restored.get("collapsed") == "false" and restored.get("stored") == "docked", restored)
        final = frame()
        check("the docked separator is back and the card is a column again",
              final["hasResult"] and final["separator"] is not None
              and not final["separator"]["popout"] and final["card"]["position"] == "static"
              and final["resultsButton"] is None, final)
        check("both presentations stay inside the viewport with no runtime error",
              final["scrollWidth"] <= final["width"] + 1 and not final["errors"], final["errors"])
        session.screenshot(out / "07-docked-again.png")

        (out / "report.json").write_text(json.dumps({
            "checks": checks,
            "session_id": session_data["id"],
            "project_id": project["id"],
            "out": str(out),
            "log": str(out / "server.log"),
        }, indent=2))
        print(f"\n{len(checks)} Results pop-out browser checks passed.\nArtifacts: {out}", flush=True)
        return 0
    except BaseException as error:
        (out / "failure.txt").write_text(f"{type(error).__name__}: {error}")
        raise
    finally:
        for process in (browser, server, provider):
            if process is not None:
                process.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
