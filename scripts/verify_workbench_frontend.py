#!/usr/bin/env python3
"""A disposable, real-backend / real-Chromium check of the Workbench.

Run `python3 scripts/verify_workbench_frontend.py` after `npm run build` in web/.
The provider, database, git repository and both browser profiles are temporary.
No running karucode data directory, user terminal or real-provider key is used.
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

    out = args.out or Path(tempfile.mkdtemp(prefix="workbench-verify-"))
    workspace = out / "fixture-repo"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# Workbench fixture\n\nReal bytes for the file viewer.\n")
    (workspace / "lib.rs").write_text("pub fn ready() -> bool { true }\n")
    git_env = {**os.environ, "HOME": str(out / "home"), "GIT_CONFIG_NOSYSTEM": "1"}
    (out / "home").mkdir(exist_ok=True)
    def git(*values: str) -> None:
        subprocess.run(["git", "-C", str(workspace), *values], env=git_env, check=True, stdout=subprocess.DEVNULL)
    git("init", "-b", "workbench-ui")
    git("config", "user.email", "workbench-fixture@example.invalid")
    git("config", "user.name", "Workbench fixture")
    git("add", ".")
    git("commit", "-m", "fixture baseline")
    # A real, reviewable modification inside the disposable project only.
    (workspace / "README.md").write_text("# Workbench fixture\n\nReviewed in the browser.\n")

    provider_log = out / "provider.jsonl"
    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port), "--log", str(provider_log)],
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
        "KARUCODE_RETRY_MAX_RETRIES": "2",
        "KARUCODE_RETRY_BASE_DELAY_MS": "50",
        "KARUCODE_RETRY_MAX_DELAY_MS": "200",
        "KARUCODE_RETRY_MAX_STREAM_RESTARTS": "3",
    }
    server_log = (out / "server.log").open("w")
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(server_port), "--workspace", str(workspace)],
        cwd=workspace,
        env=env,
        stdout=server_log,
        stderr=subprocess.STDOUT,
    )
    browser = None
    session = None
    checks: list[dict] = []
    last_result_geometry: dict = {}

    def check(name: str, condition: bool) -> None:
        checks.append({"name": name, "passed": bool(condition)})
        if not condition:
            raise AssertionError(name)
        print(f"PASS {name}", flush=True)

    def evaluate(body: str):
        return session.evaluate(body)

    def button(name: str, selector: str = "button"):
        return evaluate(
            "return !!([...document.querySelectorAll(" + json.dumps(selector) + ")].find((el) => el.getBoundingClientRect().width > 0 && el.getBoundingClientRect().height > 0 && " +
            "(el.getAttribute('aria-label') || el.textContent || '').replace(/\\s+/g, ' ').trim() === " + json.dumps(name) + "));"
        )

    def click_button(name: str, selector: str = "button"):
        until = time.time() + 10
        while time.time() < until and not button(name, selector):
            time.sleep(0.05)
        if not button(name, selector):
            detail = evaluate("return [...document.querySelectorAll(" + json.dumps(selector) + ")].map((e) => ({name: e.getAttribute('aria-label') || e.textContent?.trim(), disabled: !!e.disabled, width: e.getBoundingClientRect().width, height: e.getBoundingClientRect().height}));")
            raise AssertionError(f"no visible control: {name}; available: {detail}")
        evaluate("const el=[...document.querySelectorAll(" + json.dumps(selector) + ")].find((e) => (e.getAttribute('aria-label') || e.textContent || '').replace(/\\s+/g, ' ').trim() === " + json.dumps(name) + " && e.getBoundingClientRect().width > 0 && e.getBoundingClientRect().height > 0); el.click(); return true;")

    def press_escape() -> None:
        session._send("Input.dispatchKeyEvent", {"type": "keyDown", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})
        session._send("Input.dispatchKeyEvent", {"type": "keyUp", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})

    def press_key(key: str, code: str, virtual_key: int, shift: bool = False) -> None:
        modifiers = 8 if shift else 0
        payload = {"key": key, "code": code, "windowsVirtualKeyCode": virtual_key, "modifiers": modifiers}
        session._send("Input.dispatchKeyEvent", {"type": "keyDown", **payload})
        session._send("Input.dispatchKeyEvent", {"type": "keyUp", **payload})

    def result_metrics():
        metrics = evaluate("""
        const separator=document.querySelector('.result-resizer');
        const body=separator?.closest('.center-body');
        const result=body?.querySelector(':scope > .inspector');
        const chat=body?.querySelector(':scope > .chat-panel');
        if(!separator||!body||!result||!chat)return null;
        const box=separator.getBoundingClientRect();
        return {x:box.x,y:box.y,width:box.width,height:box.height,
          right:box.right,resultWidth:result.getBoundingClientRect().width,
          resultRight:result.getBoundingClientRect().right,bodyRight:body.getBoundingClientRect().right,
          chatWidth:chat.getBoundingClientRect().width,
          bodyLeft:body.getBoundingClientRect().left,
          resultLeft:result.getBoundingClientRect().left,
          chatRight:chat.getBoundingClientRect().right,
          transcriptWidth:chat.querySelector('.chat-inner').getBoundingClientRect().width,
          composerWidth:chat.querySelector('.composer-inner').getBoundingClientRect().width,
          composerBoxWidth:chat.querySelector('.composer').getBoundingClientRect().width,
          transcriptMaxWidth:getComputedStyle(chat.querySelector('.chat-inner')).maxWidth,
          composerMaxWidth:getComputedStyle(chat.querySelector('.composer-inner')).maxWidth,
          separatorLeft:box.left,
          transitionProperty:getComputedStyle(result).transitionProperty,
          gridColumns:getComputedStyle(body).gridTemplateColumns,
          bodyWidth:body.getBoundingClientRect().width,bodyClientWidth:body.clientWidth,
          bodyPadding:getComputedStyle(body).padding,bodyBorder:getComputedStyle(body).borderRightWidth,
          bodyOverflow:getComputedStyle(body).overflow,bodyGap:getComputedStyle(body).gap,
          bodyScrollbarWidth:getComputedStyle(body).scrollbarWidth,
          bodyScrollbarGutter:getComputedStyle(body).scrollbarGutter,
          bodyJustify:getComputedStyle(body).justifyContent,
          bodyChildren:[...body.children].map(el=>({tag:el.tagName,cls:el.className,id:el.id,hidden:el.hidden,
            display:getComputedStyle(el).display,position:getComputedStyle(el).position,
            gridColumn:getComputedStyle(el).gridColumn,gridRow:getComputedStyle(el).gridRow,
            left:el.getBoundingClientRect().left,right:el.getBoundingClientRect().right,width:el.getBoundingClientRect().width})),
          resultMargin:getComputedStyle(result).margin,resultJustifySelf:getComputedStyle(result).justifySelf,
          resultWidthCss:getComputedStyle(result).width,resultBorderRight:getComputedStyle(result).borderRightWidth,
          chatFlex:getComputedStyle(chat).flex,chatWidthCss:getComputedStyle(chat).width,
          resultFlex:getComputedStyle(result).flex,resultMaxWidth:getComputedStyle(result).maxWidth,
          dragging:separator.getAttribute('data-dragging'),
          value:Number(separator.getAttribute('aria-valuenow')),
          min:Number(separator.getAttribute('aria-valuemin')),
          max:Number(separator.getAttribute('aria-valuemax'))};
        """)
        last_result_geometry.clear()
        last_result_geometry.update(metrics or {})
        return metrics

    def wait_for_text(text: str, timeout: float = 12) -> None:
        until = time.time() + timeout
        while time.time() < until:
            if evaluate("return document.body.innerText.includes(" + json.dumps(text) + ");"):
                return
            time.sleep(0.1)
        raise AssertionError(f"text never appeared: {text}")

    def measure() -> None:
        metrics = evaluate("""
        return {
          width: innerWidth, height: innerHeight,
          viewport: { documentWidth: document.documentElement.scrollWidth },
          frame: [...document.querySelectorAll('.main-head,.workspace-tabs,.result-resizer,.inspector,.composer')]
            .filter(e => !(e.classList.contains('inspector') && innerWidth <= 1023 &&
              !e.classList.contains('is-open') && e.getAttribute('data-context-open') !== 'true'))
            .map(e => ({
            cls: e.className, x: e.getBoundingClientRect().x, y: e.getBoundingClientRect().y,
            width: e.getBoundingClientRect().width, height: e.getBoundingClientRect().height,
            right: e.getBoundingClientRect().right, bottom: e.getBoundingClientRect().bottom,
          })),
          errors: window.__workbenchErrors || [],
        };
        """)
        assert metrics["viewport"]["documentWidth"] <= metrics["width"] + 1, metrics
        for frame in metrics["frame"]:
            assert frame["right"] <= metrics["width"] + 1, frame
        assert not metrics["errors"], metrics["errors"]
        (out / "metrics.json").write_text(json.dumps(metrics, indent=2))

    try:
        wait_http(f"http://127.0.0.1:{provider_port}/health")
        wait_http(f"http://127.0.0.1:{server_port}/api/health")
        client = Client(server_port)
        existing_projects = client.ok("GET", "/api/projects")["projects"]
        # --workspace may already have registered a canonical project for the
        # disposable directory; never register a second alias for it.
        project = next((item for item in existing_projects if item["path"] == str(workspace.resolve())), None)
        if project is None:
            project = client.ok("POST", "/api/projects", {"path": str(workspace), "name": "workbench-fixture"})
        def wait_project_width(expected: int) -> None:
            until = time.time() + 10
            while time.time() < until:
                projects = client.ok("GET", "/api/projects")["projects"]
                saved = next(item for item in projects if item["id"] == project["id"])
                if saved.get("result_panel_width") == expected:
                    return
                time.sleep(0.1)
            raise AssertionError(f"project width never reached {expected}: {saved}")

        session_data = client.ok("POST", "/api/sessions", {"title": "Workbench verification", "project": project["id"]})
        session_id = session_data["id"]
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
        base = f"http://127.0.0.1:{server_port}"
        session.navigate(base)
        wait_for_text("Workbench verification")
        check("the fixture server supplies the freshly built Workbench", evaluate("return !!document.querySelector('.chat-main.is-workbench');"))
        session.screenshot(out / "workbench-desktop.png")
        check("desktop has no clipped document or runtime error", evaluate("return document.documentElement.scrollWidth <= innerWidth + 1;"))
        measure()

        check("one task header owns views and all secondary actions", evaluate("""
        const header=document.querySelector('.main-head');
        return !!header?.querySelector('.workspace-tabs') &&
          !document.querySelector('.workspace-tabbar') &&
          ['.notifications-head','.context-header-btn','.command-head-btn','.workbench-task-menu > button']
            .every(selector => header.querySelector(selector)?.getBoundingClientRect().width > 0);
        """))
        metrics = result_metrics()
        assert metrics is not None, "desktop result separator is not mounted"
        check("desktop result separator has a 24px touch target", metrics["width"] == 24 and metrics["height"] > 100)

        # Native mouse input moves the actual grid and writes only the final value.
        before = metrics
        x, y = before["x"] + before["width"] / 2, before["y"] + before["height"] / 2
        session._send("Input.dispatchMouseEvent", {"type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1})
        session._send("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x - 80, "y": y, "button": "left", "buttons": 1})
        during_mouse = result_metrics()
        check("during mouse drag the Results outer edge stays pinned to the workspace right",
              abs(during_mouse["resultRight"] - during_mouse["bodyRight"]) <= 1)
        session.screenshot(out / "workbench-during-drag.png")
        session._send("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": x - 80, "y": y, "button": "left", "buttons": 0, "clickCount": 1})
        after_mouse = result_metrics()
        check("mouse drag widens the real result pane and preserves the conversation minimum",
              after_mouse["resultWidth"] > before["resultWidth"] + 40 and after_mouse["chatWidth"] >= 329 and
              abs(after_mouse["resultRight"] - after_mouse["bodyRight"]) <= 1)
        wait_project_width(after_mouse["value"])

        # CDP touch input takes the same Pointer Events path, with no mouse-only handler.
        session._send("Emulation.setTouchEmulationEnabled", {"enabled": True, "maxTouchPoints": 1})
        before = after_mouse
        x, y = before["x"] + before["width"] / 2, before["y"] + before["height"] / 2
        session._send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y, "id": 0}]})
        session._send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x + 60, "y": y, "id": 0}]})
        during_touch = result_metrics()
        check("during touch drag the Results outer edge stays pinned to the workspace right",
              abs(during_touch["resultRight"] - during_touch["bodyRight"]) <= 1)
        session._send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
        session._send("Emulation.setTouchEmulationEnabled", {"enabled": False})
        after_touch = result_metrics()
        released = before["resultWidth"] - after_touch["resultWidth"]
        check("touch drag transfers all released Results width to Conversation",
              after_touch["resultWidth"] < before["resultWidth"] - 40 and
              abs(after_touch["chatWidth"] - before["chatWidth"] - released) <= 1 and
              abs(after_touch["chatRight"] - after_touch["separatorLeft"]) <= 1 and
              abs(after_touch["resultRight"] - after_touch["bodyRight"]) <= 1)
        wait_project_width(after_touch["value"])

        # Trusted keyboard input checks both available bounds, then chooses 580.
        evaluate("document.querySelector('.result-resizer').focus(); return true;")
        press_key("End", "End", 35)
        at_end = result_metrics()
        check("keyboard End uses available width without hiding conversation",
              at_end["value"] == at_end["max"] and at_end["chatWidth"] >= 329 and
              abs(at_end["resultRight"] - at_end["bodyRight"]) <= 1)
        press_key("Home", "Home", 36)
        at_home = result_metrics()
        check("minimum Results width leaves the full remainder to Conversation",
              at_home["value"] == 280 and abs(at_home["resultWidth"] - 280) <= 1 and
              abs(at_home["chatWidth"] + at_home["width"] + at_home["resultWidth"] - at_home["bodyWidth"]) <= 1 and
              abs(at_home["chatRight"] - at_home["separatorLeft"]) <= 1 and
              abs(at_home["resultRight"] - at_home["bodyRight"]) <= 1)
        # The fixed reading-width cap must not strand a wide conversation again.
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1920, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        wide_home = result_metrics()
        check("wide transcript matches the composer box without filling Conversation",
              wide_home["value"] == 280 and wide_home["transcriptWidth"] >= wide_home["chatWidth"] * 0.5 - 1 and
              wide_home["composerWidth"] >= wide_home["chatWidth"] * 0.5 - 1 and
              abs(wide_home["transcriptWidth"] - wide_home["composerBoxWidth"]) <= 1 and
              wide_home["transcriptWidth"] < wide_home["chatWidth"] - 100)
        session.screenshot(out / "workbench-wide-minimum-results.png")
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        for _ in range(6):
            press_key("ArrowLeft", "ArrowLeft", 37, shift=True)
        chosen = result_metrics()
        check("coalesced Shift+Arrow keys choose 580px", chosen["value"] == 580 and
              abs(chosen["resultWidth"] - 580) <= 1 and abs(chosen["resultRight"] - chosen["bodyRight"]) <= 1)
        wait_project_width(580)
        (out / "drag-geometry.json").write_text(json.dumps({
            "mouse_during": during_mouse,
            "touch_during": during_touch,
            "keyboard_end": at_end,
            "keyboard_home": at_home,
            "wide_keyboard_home": wide_home,
            "chosen": chosen,
        }, indent=2))
        session.screenshot(out / "workbench-resized.png")
        measure()

        # Read-only mock-provider tool calls through the real server and UI.
        evaluate("""
        const input = document.querySelector('#composer-input');
        const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        set.call(input, '[tool:multi] Inspect the disposable project without changing it.');
        input.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
        """)
        click_button("Send message (Command or Control + Enter)")
        wait_for_text("Done. The mock provider finished the task.")
        # Built-in themes start with the working steps open: a finished turn
        # arrives with its narration, its run of calls and the calls inside that
        # run on screen, and the summary line is the header that folds it away.
        check("completed work starts expanded", evaluate("""
        const fold=document.querySelector('.process-fold');
        const group=document.querySelector('.tool-group-head');
        return !!fold && fold.getAttribute('aria-expanded')==='true' &&
          !!group && group.getAttribute('aria-expanded')==='true' &&
          fold.textContent.includes('Hide the working');
        """))
        check("an open turn has no permanently mounted tool output", evaluate("return document.querySelectorAll('.tool-body').length === 0;"))
        # Folding the summary hides that turn's work; it does not touch another row.
        evaluate("document.querySelector('.process-fold').click(); return true;")
        time.sleep(0.35)
        check("the summary line folds the working away", evaluate("""
        const fold=document.querySelector('.process-fold');
        return !!fold && fold.getAttribute('aria-expanded')==='false' &&
          fold.textContent.includes('Show the working') && !document.querySelector('.tool-group-head');
        """))
        evaluate("document.querySelector('.process-fold').click(); return true;")
        time.sleep(0.35)
        check("the same line opens it again", evaluate(
            "return document.querySelector('.process-fold')?.getAttribute('aria-expanded')==='true';"))
        until = time.time() + 10
        while time.time() < until and not evaluate("return [...document.querySelectorAll('.tool-row-head,.tool-expand,.command-summary')].some(e => e.getBoundingClientRect().width>0)"):
            time.sleep(0.1)
        check("tool group really expands for completed work", evaluate("return document.querySelector('.tool-group-head')?.getAttribute('aria-expanded')==='true';"))
        check("completed tool details are expandable", evaluate("return [...document.querySelectorAll('.tool-row-head,.tool-expand,.command-summary')].some(e => e.getBoundingClientRect().width>0);"))
        evaluate("const detail=document.querySelector('.tool-row-head,.tool-expand,.command-summary'); detail.click(); return true;")
        check("expanded real tool output is visible", evaluate("return document.querySelectorAll('.tool-body').length > 0;"))

        # Read the complete populated transcript in the wide, minimum-Results split.
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1920, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        evaluate("document.querySelector('.result-resizer').focus(); return true;")
        press_key("Home", "Home", 36)
        tool_alignment = evaluate("""
        const rect = selector => {
          const element=document.querySelector(selector);
          if(!element)return null;
          const box=element.getBoundingClientRect();
          return {left:box.left,right:box.right,width:box.width};
        };
        return {chat:rect('.chat-panel'),inner:rect('.chat-inner'),transcript:rect('.transcript'),
          composer:rect('.composer'),user:rect('.msg-user'),
          userText:rect('.user-question'),assistant:rect('.msg-assistant'),assistantBody:rect('.msg-assistant .msg-body'),
          toolGroup:rect('.tool-group'),toolRow:rect('.tool-row'),toolBody:rect('.tool-body'),
          transcriptPadding:parseFloat(getComputedStyle(document.querySelector('.chat-inner')).paddingLeft)};
        """)
        check("transcript matches the composer, with tools left and the user message right",
              abs(tool_alignment["inner"]["width"] - tool_alignment["composer"]["width"]) <= 1 and
              tool_alignment["transcript"]["left"] > tool_alignment["chat"]["left"] + tool_alignment["transcriptPadding"] + 1 and
              abs(tool_alignment["user"]["right"] - tool_alignment["transcript"]["right"]) <= 1 and
              tool_alignment["toolGroup"]["left"] < tool_alignment["user"]["left"] and
              abs(tool_alignment["toolGroup"]["left"] - tool_alignment["assistantBody"]["left"]) <= 1 and
              tool_alignment["toolBody"]["left"] >= tool_alignment["toolRow"]["left"])
        (out / "tool-alignment.json").write_text(json.dumps(tool_alignment, indent=2))
        session.screenshot(out / "workbench-tool-alignment.png")
        for _ in range(6):
            press_key("ArrowLeft", "ArrowLeft", 37, shift=True)
        wait_project_width(580)
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False})

        # A real command turn, then read its (already expanded) working and open
        # the command row. Compact rows historically nested .cmd-output and
        # .command-scroll.
        folds_before = evaluate("return document.querySelectorAll('.process-fold').length;")
        evaluate("""
        const input = document.querySelector('#composer-input');
        const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        set.call(input, '[tool:run] Inspect the disposable command output.');
        input.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
        """)
        click_button("Send message (Command or Control + Enter)")
        until = time.time() + 12
        while time.time() < until and evaluate("return document.querySelectorAll('.process-fold').length;") <= folds_before:
            time.sleep(0.1)
        # New turns arrive expanded; make sure the one about to be read is open.
        evaluate("""
        const fold=[...document.querySelectorAll('.process-fold')].filter(e=>e.getBoundingClientRect().width>0).at(-1);
        if(fold && fold.getAttribute('aria-expanded')!=='true')fold.click();
        return fold?.getAttribute('aria-expanded');
        """)
        time.sleep(0.35)
        until = time.time() + 10
        while time.time() < until and not evaluate("return !!document.querySelector('.command-row-head');"):
            time.sleep(0.1)
        evaluate("const row=[...document.querySelectorAll('.command-row-head')].at(-1); if(row && row.getAttribute('aria-expanded')!=='true')row.click(); return true;")
        command_scroll = evaluate("""
        const outer=document.querySelector('.command-row[data-open="true"] > .cmd-output');
        const inner=outer?.querySelector('.command-scroll');
        const pre=inner?.querySelector('.output-pre');
        if(!outer||!inner||!pre)return null;
        return {outerOverflow:getComputedStyle(outer).overflowY,outerMaxHeight:getComputedStyle(outer).maxHeight,
          innerOverflow:getComputedStyle(inner).overflowY,innerMaxHeight:getComputedStyle(inner).maxHeight,
          preOverflow:getComputedStyle(pre).overflowY,preMaxHeight:getComputedStyle(pre).maxHeight};
        """)
        (out / "command-scroll.json").write_text(json.dumps(command_scroll, indent=2))
        check("expanded command output has exactly one bounded scroller",
              command_scroll is not None and command_scroll["outerOverflow"] == "visible" and
              command_scroll["outerMaxHeight"] == "none" and command_scroll["innerOverflow"] == "auto" and
              command_scroll["innerMaxHeight"] != "none" and command_scroll["preOverflow"] == "visible" and
              command_scroll["preMaxHeight"] == "none")
        session.screenshot(out / "workbench-command-scroll.png")

        # The real project/workspace link, not a synthetic context fixture.
        evaluate("document.querySelector('.context-header-btn').click(); return true;")
        wait_for_text("Workspace and session")
        check("Context exposes actual project and session facts", evaluate(f"return document.querySelector('#workbench-context').innerText.includes({str(workspace)!r}) && document.querySelector('#workbench-context').innerText.includes('Workbench verification');"))
        for label in ("Usage", "Access"):
            evaluate(f"const tab=document.querySelector('#context-tab-{label.lower()}');tab.click();return true;")
            check(f"Context {label} is real and bounded", evaluate(f"return !document.querySelector('#context-panel-{label.lower()}').hidden && document.querySelector('#workbench-context').getBoundingClientRect().right <= innerWidth + 1;"))
        session.screenshot(out / "workbench-context.png")
        evaluate("document.querySelector('#context-tab-now').click();return true;")
        click_button("Source control", "#workbench-context button")
        check("Context source-control link opens real working-tree view", evaluate("return !!document.querySelector('#inspector-panel-changes .gitm-page');"))
        wait_for_text("README.md")
        session.screenshot(out / "workbench-review.png")

        # Files -> modal preview -> source editing -> save. This project is disposable.
        evaluate("document.querySelector('#inspector-tab-workspace').click();return true;")
        until = time.time() + 12
        while time.time() < until and not evaluate("return !!document.querySelector('.tree-row[data-tree-path]');"):
            time.sleep(0.1)
        assert button("README.md", '.tree-row .tree-name'), "README.md is not visible after loading the file tree"
        evaluate("""
        const target=[...document.querySelectorAll('[data-tree-path]')].find(e=>e.querySelector('.tree-name')?.textContent?.trim()==='README.md');
        if(!target)throw new Error('Fixture rows: '+[...document.querySelectorAll('[data-tree-path]')].map(e=>e.dataset.treePath).join(', '));
        target.click(); return target.dataset.treePath;
        """)
        wait_for_text("Workbench fixture")
        check("file viewer uses the real file route", evaluate("return !!document.querySelector('[role=\"dialog\"]')?.textContent.includes('README.md');"))

        # The file viewer is one bar over the document, not a stack of grey
        # headers: it used to draw a path header, a metadata toolbar and a
        # separate Preview/Source band (about 140px of chrome) over a document
        # whose name the session breadcrumb already carried. jsdom cannot measure
        # this, so it is measured here, in the real browser.
        dialog_chrome = evaluate("""
        const dialog=document.querySelector('[role="dialog"]');
        const bar=dialog?.querySelector('.file-toolbar');
        const status=dialog?.querySelector('.file-statusbar');
        if(!dialog||!bar||!status)return null;
        return {bars:dialog.querySelectorAll('.file-toolbar').length,
          pathHeader:!!dialog.querySelector('.file-view-head'),
          tabbar:!!dialog.querySelector('.file-tabbar'),
          height:Math.round(bar.getBoundingClientRect().height),
          background:getComputedStyle(bar).backgroundColor,
          hasPath:!!dialog.querySelector('.file-status-path'),
          status:status.innerText.replace(/\\s+/g,' ').trim()};
        """)
        (out / "file-chrome.json").write_text(json.dumps(dialog_chrome, indent=2))
        check("the file viewer draws one bar over the document, not three stacked bands",
              isinstance(dialog_chrome, dict) and dialog_chrome["bars"] == 1
              and dialog_chrome["pathHeader"] is False and dialog_chrome["tabbar"] is False)
        check("the file viewer's one bar is compact and paints no band of its own",
              isinstance(dialog_chrome, dict) and dialog_chrome["height"] <= 48
              and dialog_chrome["background"] == "rgba(0, 0, 0, 0)")
        check("the file viewer states its facts once, in the status bar",
              isinstance(dialog_chrome, dict) and "markdown" in dialog_chrome["status"]
              and "lines" in dialog_chrome["status"]
              # The dialog's title bar already names the file.
              and dialog_chrome["hasPath"] is False)

        # The same document in the workspace strip: the tab already names it and
        # closes it, so the panel has no title row of its own and no close button.
        click_button("Open in tab")
        until = time.time() + 12
        while time.time() < until and not evaluate("return !!document.querySelector('.file-workspace .file-toolbar');"):
            time.sleep(0.1)
        tab_chrome = evaluate("""
        const panel=document.querySelector('.file-workspace');
        const bar=panel?.querySelector('.file-toolbar');
        const status=panel?.querySelector('.file-statusbar');
        if(!panel||!bar||!status)return null;
        return {bars:panel.querySelectorAll('.file-toolbar').length,
          pathHeader:!!panel.querySelector('.file-view-head'),
          tabbar:!!panel.querySelector('.file-tabbar'),
          height:Math.round(bar.getBoundingClientRect().height),
          closeButton:!!panel.querySelector('[aria-label^="Close "]'),
          path:panel.querySelector('.file-status-path')?.textContent ?? null,
          status:status.innerText.replace(/\\s+/g,' ').trim()};
        """)
        (out / "file-tab-chrome.json").write_text(json.dumps(tab_chrome, indent=2))
        check("a file tab has one bar and no path header of its own",
              isinstance(tab_chrome, dict) and tab_chrome["bars"] == 1
              and tab_chrome["pathHeader"] is False and tab_chrome["tabbar"] is False
              and tab_chrome["height"] <= 48 and tab_chrome["closeButton"] is False)
        check("a file tab names the document in its status bar, and the strip owns closing",
              isinstance(tab_chrome, dict) and tab_chrome["path"] == "README.md"
              and "README.md" in tab_chrome["status"]
              and evaluate("""
              const tab=[...document.querySelectorAll('.workspace-tabs [role="tab"]')]
                .find(el=>(el.textContent||'').trim()==='README.md');
              return !!tab && !!document.querySelector('.workspace-tabs .tab-close');
              """))
        session.screenshot(out / "workbench-file-tab.png")

        click_button("Source", ".file-view-tabs button")
        evaluate("const b=[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Edit');if(b)b.click();return true;")
        evaluate(r"""
        const e=document.querySelector('.file-editor');
        const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set;
        set.call(e,'# Workbench fixture\n\nSaved only in the disposable project.\n');
        e.dispatchEvent(new Event('input',{bubbles:true})); return true;
        """)
        evaluate("const editor=document.querySelector('.file-editor');editor.focus();editor.dispatchEvent(new KeyboardEvent('keydown',{key:'s',ctrlKey:true,bubbles:true,cancelable:true}));return editor.value;")
        until = time.time() + 10
        while time.time() < until and "disposable project" not in (workspace / "README.md").read_text():
            time.sleep(0.1)
        assert "disposable project" in (workspace / "README.md").read_text()
        check("real file save is confined to the fixture", True)
        session.screenshot(out / "workbench-file.png")
        press_escape()

        # Full terminal remains available and keeps its real server-side lifecycle.
        evaluate("document.querySelector('#workspace-tab-terminal').click();return true;")
        until = time.time() + 12
        while time.time() < until and not button("New terminal"):
            time.sleep(0.1)
        click_button("New terminal")
        until = time.time() + 12
        while time.time() < until and not evaluate("return !!document.querySelector('.xterm')"):
            time.sleep(0.1)
        check("disposable terminal attaches a real xterm", evaluate("return !!document.querySelector('.xterm');"))
        evaluate("document.querySelector('.context-header-btn').click(); return true;")
        check("terminal stays mounted under Context", evaluate("return !!document.querySelector('.xterm');"))
        press_escape()
        evaluate("document.querySelector('#workspace-tab-chat').click();return true;")

        # Existing command, themes and session/global actions remain live.
        evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'k',ctrlKey:true,bubbles:true}));return true;")
        check("command palette opens", evaluate("return !!document.querySelector('.dialog-palette');"))
        press_escape()
        click_button("Task actions")
        click_button("Theme studio", '[role="menuitem"]')
        check("theme studio opens from Workbench task actions", evaluate("return !!document.querySelector('.theme-studio');"))
        press_escape()
        evaluate("document.querySelector('.context-header-btn').click();return true;")
        evaluate("document.querySelector('#context-tab-access').click();return true;")
        click_button("Open approval settings", '#workbench-context button')
        check("actual settings open from Access", evaluate("return !!document.querySelector('.dialog-settings');"))
        evaluate("""
        window.__workbenchKeyTrace=[];
        document.addEventListener('keydown',event=>window.__workbenchKeyTrace.push({key:event.key,trusted:event.isTrusted,defaultPrevented:event.defaultPrevented,target:event.target.closest?.('[aria-label]')?.getAttribute('aria-label')||event.target.tagName}),{capture:true});
        return{documentHasFocus:document.hasFocus(),active:document.activeElement.getAttribute('aria-label')||document.activeElement.tagName};
        """)
        press_escape()
        check("Escape reaches the focused dialog path", evaluate("return JSON.stringify({trace:window.__workbenchKeyTrace,settings:!!document.querySelector('.dialog-settings'),active:document.activeElement.getAttribute('aria-label')||document.activeElement.tagName});"))
        check("Settings dismisses with Escape before navigation continues", evaluate("return !document.querySelector('.dialog-settings');"))

        # Collapsed desktop projects still provide the real RailPeek.
        evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'b',ctrlKey:true,bubbles:true}));return true;")
        check("collapsed mode preserves the project rail", evaluate("return !!document.querySelector('.project-rail');"))
        evaluate("document.querySelector('.project-rail-item-wrap').dispatchEvent(new MouseEvent('mouseenter'));return true;")
        time.sleep(0.4)
        check("collapsed project navigation still peeks at its sessions", evaluate("return !!document.querySelector('.rail-peek');"))
        evaluate("document.querySelector('.project-rail-item-wrap').dispatchEvent(new MouseEvent('mouseleave'));return true;")

        # A phone keeps a real draft through workspace modes and closes the focused Context sheet safely.
        evaluate("document.querySelector('#composer-input').focus(); const i=document.querySelector('#composer-input');const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set;set.call(i,'A draft that must survive layout changes.');i.dispatchEvent(new Event('input',{bubbles:true}));return true;")
        session._send("Emulation.setDeviceMetricsOverride", {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True})
        time.sleep(0.3)
        check("phone has no horizontal document overflow", evaluate("return document.documentElement.scrollWidth <= innerWidth + 1;"))
        evaluate("document.getElementById('workspace-tab-changes').click(); return true;")
        check("phone can inspect the full existing source-control view", evaluate("return !!document.querySelector('#workspace-panel-changes .gitm-page');"))
        click_button("Conversation")
        check("workspace switch preserves the phone composer draft", evaluate("return document.querySelector('#composer-input').value.includes('draft that must survive');"))
        evaluate("document.querySelector('.context-header-btn').click();return true;")
        check("phone Context is a focused dialog", evaluate("return document.querySelector('#workbench-context').getAttribute('aria-modal')==='true' && document.querySelector('#workbench-context').contains(document.activeElement);"))
        session.screenshot(out / "workbench-mobile-context.png")
        press_escape()
        check("dismissal restores the phone conversation", evaluate("return !document.querySelector('#workbench-context');"))
        measure()

        # A genuinely separate user-data-dir proves this is a project preference,
        # not localStorage or a surviving Preact signal in the first window.
        session.ws.close()
        browser.terminate()
        browser.wait(timeout=5)
        browser_log.close()
        browser = None
        session = None
        browser_log = (out / "chromium-fresh.log").open("w")
        cdp_port = free_port()
        browser = subprocess.Popen(
            ["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage",
             f"--remote-debugging-port={cdp_port}", f"--user-data-dir={out / 'fresh-browser-profile'}",
             "--window-size=1440,1000", "about:blank"],
            stdout=browser_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        wait_http(f"http://127.0.0.1:{cdp_port}/json/version")
        targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list").read())
        page = next(target for target in targets if target.get("type") == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session._send("Runtime.enable")
        session._send("Emulation.setDeviceMetricsOverride", {"width": 1440, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
        evaluate("""
        window.__workbenchErrors = [];
        window.addEventListener('error', e => window.__workbenchErrors.push(String(e.message || e.type)));
        window.addEventListener('unhandledrejection', e => window.__workbenchErrors.push(String(e.reason || e.type)));
        return true;
        """)
        session.navigate(base)
        wait_for_text("Workbench verification")
        until = time.time() + 10
        while time.time() < until and result_metrics() is None:
            time.sleep(0.1)
        restored = result_metrics()
        check("fresh Chromium profile restores the 580px server project preference",
              restored is not None and restored["value"] == 580 and abs(restored["resultWidth"] - 580) <= 1 and
              abs(restored["resultRight"] - restored["bodyRight"]) <= 1)
        check("width restoration does not come from browser-local result state", evaluate(
            "return Object.keys(localStorage).every(key => !key.toLowerCase().includes('result') && !key.toLowerCase().includes('width'));"
        ))
        check("fresh desktop keeps views in the one header without overflow", evaluate("""
        return !!document.querySelector('.main-head > .workspace-tabs') &&
          !document.querySelector('.workspace-tabbar') && document.documentElement.scrollWidth <= innerWidth + 1;
        """))
        session.screenshot(out / "workbench-fresh-profile.png")

        # The stored desktop width must not become a phone split or an off-screen sheet.
        session._send("Emulation.setDeviceMetricsOverride", {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True})
        time.sleep(0.35)
        check("fresh phone hides the divider and preserves visible workspace controls", evaluate("""
        const separator=document.querySelector('.result-resizer');
        const header=document.querySelector('.main-head');
        const visible = selector => {
          const control=document.querySelector(selector);
          return control && control.getBoundingClientRect().width > 0 && control.getBoundingClientRect().right <= innerWidth + 1;
        };
        return separator && getComputedStyle(separator).display === 'none' && document.documentElement.scrollWidth <= innerWidth + 1 &&
          ['#workspace-tab-chat','#workspace-tab-changes','.sessions-toggle','.notifications-head','.context-header-btn','.workbench-task-menu > button'].every(visible);
        """))
        session.screenshot(out / "workbench-fresh-phone.png")
        measure()

        report = {"checks": checks, "workspace_is_temporary": True, "fresh_browser_profile": True, "session_id": session_id, "out": str(out), "log": str(out / 'server.log')}
        (out / "report.json").write_text(json.dumps(report, indent=2))
        print(f"\n{len(checks)} Workbench browser checks passed.\nArtifacts: {out}", flush=True)
        return 0
    except BaseException as error:
        (out / "failure.txt").write_text(f"{type(error).__name__}: {error}")
        (out / "failure-geometry.json").write_text(json.dumps(last_result_geometry, indent=2))
        if session is not None:
            session.screenshot(out / "failure.png")
            diagnostics = evaluate("return { keys:window.__workbenchKeyTrace || [], active:document.activeElement?.outerHTML?.slice(0,1000), modal:!!document.querySelector('.dialog-settings'), hasFocus:document.hasFocus(), profile:navigator.userAgent };")
            (out / "failure-inputs.json").write_text(json.dumps(diagnostics, indent=2))
            dump = evaluate("return document.querySelector('.transcript')?.outerHTML || document.body.innerHTML;")
            (out / "failure-dom.html").write_text(dump or "no DOM")
        server_path = out / "server.log"
        if server_path.exists():
            print(server_path.read_text()[-3000:], file=sys.stderr)
        print(f"Workbench verification failed at {len(checks)+1}: {error}\nArtifacts: {out}", file=sys.stderr)
        return 1
    finally:
        for process in (browser, server, provider):
            if process is not None and process.poll() is None:
                process.terminate()
        if session is not None:
            session.ws.close()
        browser_log.close()
        if server_log:
            server_log.close()


if __name__ == "__main__":
    raise SystemExit(main())
