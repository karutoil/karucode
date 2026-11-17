#!/usr/bin/env python3
"""Reproduce "the right-click menu vanishes while a chat is answering".

Reported from real use: right-clicking a project tile (or a session row) opens
the menu, and a second later - while a run is still streaming - the menu is
gone. It only happens "if an active chat is going", which points at the
transcript: `useStickyBottom` writes `scrollTop` on every chunk of an answer,
and `lib/menu.tsx` dismissed every pointer-anchored menu on *any* scroll event
it saw in the document (a window listener in the capture phase). The subject of
the menu - the rail tile, the session row - never moved.

This drives the built bundle in headless Chromium against a real kc-server and a
stub provider that streams a long answer, then, mid-stream:

  1. right-clicks a project-rail tile and watches the menu for 2.5s;
  2. right-clicks a session row and watches it for 2.5s;
  3. asserts the menu never disappears while unrelated scrolls are happening,
     and that scrolling the *anchor's own* container still dismisses it (the
     behaviour the dismissal exists for: the menu is a snapshot of a point).

Nothing here is a unit test replacement: jsdom has no layout, so it cannot show
that the transcript scrolls at all. Hence a real browser.

Run: cargo build -p kc-server --bin karucode && (cd web && npm run build)
     python3 scripts/verify_context_menu_streaming.py [--keep] [--viewport 1280x900]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import ROOT, Session, free_port, http_json, wait_for

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f'  -> {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f'  -> {detail}' if detail else ''}", flush=True)
    return ok


# ---------------------------------------------------------------------------
# Stub provider: a long, slow answer, so the run really streams
# ---------------------------------------------------------------------------

DELTAS = 400
DELTA_SLEEP = 0.015


def _chunk(delta: dict, finish: str | None = None) -> dict:
    return {
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }


def sse_line(payload: dict) -> bytes:
    return f"data: {json.dumps(payload)}\n\n".encode()


class Provider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            body = json.dumps({"data": [{"id": "stub-model", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def write(payload: dict) -> None:
            try:
                self.wfile.write(sse_line(payload))
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                raise SystemExit(0)

        # A screenful every second: the transcript overflows and the sticky
        # bottom writes `scrollTop` on every one of these.
        for index in range(DELTAS):
            write(_chunk({"content": f"line {index} " + "filler words that wrap " * 4 + "\n\n"}))
            time.sleep(DELTA_SLEEP)
        write(_chunk({}, "stop"))
        try:
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


# ---------------------------------------------------------------------------
# In-page probe: scroll events, menu lifetime, rail remounts
# ---------------------------------------------------------------------------

PROBE = r"""
window.__menuProbe = {
  scrolls: [],
  gone: [],
  railMounts: 0,
  init() {
    if (this.started) return 'already';
    this.started = true;
    const stamp = (event) => ({
      t: Math.round(performance.now()),
      type: event.type,
      cls: String((event.target && event.target.className) || '').slice(0, 60),
      tag: (event.target && event.target.tagName) || '',
    });
    // Capture, exactly what the menu primitive itself listens with: every
    // scroller in the document, not just the window's.
    document.addEventListener('scroll', (event) => this.scrolls.push(stamp(event)), true);
    new MutationObserver((records) => {
      for (const record of records) {
        record.addedNodes.forEach((node) => {
          if (node instanceof Element && node.matches('.project-rail')) this.railMounts += 1;
        });
      }
    }).observe(document.body, { childList: true, subtree: true });
    new MutationObserver(() => {
      if (document.querySelector('.menu-layer')) {
        this.openedAt = Math.round(performance.now());
        return;
      }
      if (this.openedAt !== undefined && this.goneAt === undefined) {
        this.goneAt = Math.round(performance.now());
        const last = this.scrolls[this.scrolls.length - 1];
        this.gone.push({
          openedAt: this.openedAt,
          goneAt: this.goneAt,
          lastScrollMsBefore: last ? this.goneAt - last.t : null,
          lastScroll: last || null,
        });
      }
    }).observe(document.body, { childList: true, subtree: true });
    return 'ok';
  },
  snapshot() {
    const layer = document.querySelector('.menu-layer');
    const menu = document.querySelector('[role="menu"]');
    return {
      layer: !!layer,
      menu: !!menu,
      label: menu ? menu.getAttribute('aria-label') : null,
      items: menu ? [...menu.querySelectorAll('[role="menuitem"]')].map((n) => (n.textContent || '').trim()) : [],
      scrolls: this.scrolls.length,
      railMounts: this.railMounts,
      gone: this.gone.length,
    };
  },
};
window.__menuProbe.init();
'ready';
"""


def page_js(body: str) -> str:
    # `Session.evaluate` already wraps the body in an awaited async function;
    # wrapping it again would return the inner promise instead of its value.
    return body


def snapshot(session: Session) -> dict:
    """The probe's view of the page.

    `Session.evaluate` runs its body inside an async function, so a bare
    expression evaluates to `undefined`; the `return` is what gets the value out.
    """
    return session.evaluate("return window.__menuProbe.snapshot();") or {}


def tile_rect(session: Session) -> dict:
    return session.evaluate(
        page_js(
            """
            const tile = document.querySelector('.project-rail-item-wrap.is-active .project-rail-item')
              || document.querySelector('.project-rail-item');
            if (!tile) return { error: 'no tile' };
            const rect = tile.getBoundingClientRect();
            return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) };
            """
        )
    )


def session_row_rect(session: Session) -> dict:
    return session.evaluate(
        page_js(
            """
            const row = document.querySelector('.session-row[data-session-id]');
            if (!row) return { error: 'no session row' };
            const rect = row.getBoundingClientRect();
            return { x: Math.round(rect.x + Math.min(rect.width - 8, 60)), y: Math.round(rect.y + rect.height / 2) };
            """
        )
    )


def right_click(session: Session, x: int, y: int) -> str:
    """Right-press through CDP; fall back to the DOM event the app listens for."""
    session.cdp(
        "Input.dispatchMouseEvent",
        {"type": "mousePressed", "x": x, "y": y, "button": "right", "clickCount": 1, "buttons": 2},
    )
    session.cdp(
        "Input.dispatchMouseEvent",
        {"type": "mouseReleased", "x": x, "y": y, "button": "right", "clickCount": 1, "buttons": 0},
    )
    time.sleep(0.25)
    if session.evaluate("!!document.querySelector('.menu-layer')"):
        return "cdp"
    session.evaluate(
        page_js(
            f"""
            const target = document.elementFromPoint({x}, {y});
            if (!target) return 'no-target';
            target.dispatchEvent(new MouseEvent('contextmenu', {{
              bubbles: true, cancelable: true, clientX: {x}, clientY: {y}, button: 2,
            }}));
            return 'dispatched';
            """
        )
    )
    time.sleep(0.25)
    return "dom"


def watch(session: Session, seconds: float = 2.5, step: float = 0.1) -> dict:
    """Sample the menu and the scroll counter for `seconds`."""
    samples = []
    deadline = time.time() + seconds
    while time.time() < deadline:
        samples.append(snapshot(session))
        time.sleep(step)
    first, last = samples[0], samples[-1]
    return {
        "samples": samples,
        "ever_open": all(sample["menu"] for sample in samples),
        "first_open": first["menu"],
        "last_open": last["menu"],
        "scrolls": last["scrolls"] - first["scrolls"],
        # Scrolls that happened while the menu was up: the whole point.
        "total_scrolls": last["scrolls"],
        "rail_mounts": last["railMounts"] - first["railMounts"],
        "gone": session.evaluate("return window.__menuProbe.gone;"),
    }


def start_stream(api: str, session_id: str, prompt: str, timeout: float = 40.0) -> None:
    """Send a message, waiting out a run the previous phase left in flight.

    `POST /api/sessions/:id/messages` answers 409 while a run is active; the
    phases here take a few seconds each and the stub streams for six, so the
    previous answer can still be going when the next phase starts.
    """
    deadline = time.time() + timeout
    while True:
        request = urllib.request.Request(
            f"{api}/api/sessions/{session_id}/messages",
            data=json.dumps({"content": prompt}).encode(),
            method="POST",
        )
        request.add_header("X-Karucode-Client", "web")
        request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=20):
                return
        except urllib.error.HTTPError as error:
            if error.code != 409 or time.time() > deadline:
                raise SystemExit(f"POST messages -> {error.code}: {error.read().decode('utf-8', 'replace')}")
            time.sleep(0.5)



def close_menu(session: Session) -> None:
    session.evaluate(
        page_js(
            """
            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            return 'closed';
            """
        )
    )
    time.sleep(0.2)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    parser.add_argument("--viewport", default="1280x900")
    parser.add_argument("--stream-seconds", type=float, default=6.0)
    args = parser.parse_args()

    binary = ROOT / "target/debug/karucode"
    if not binary.exists():
        raise SystemExit("build first: cargo build -p kc-server --bin karucode")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("build the web bundle first: cd web && npm run build")

    tmp = Path(tempfile.mkdtemp(prefix="kc-menu-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()

    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), Provider)
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
                "[approvals]",
                'mode = "auto"',
                "",
                "[limits]",
                "max_iterations = 4",
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
                'context_window = 200000',
                "reasoning = true",
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

    print(f"workdir {tmp}", flush=True)
    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(binary),
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
            f"--window-size={args.viewport.replace('x', ',')}",
            "about:blank",
        ],
        env=chrome_env,
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    session: Session | None = None
    try:
        wait_for(f"http://127.0.0.1:{port}/api/health")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        api = f"http://127.0.0.1:{port}"

        projects = http_json(f"{api}/api/projects")["projects"]
        project_id = projects[0]["id"]
        created = http_json(f"{api}/api/sessions", "POST", {"title": "Menu target", "project": project_id})
        session_id = created.get("id") or (created.get("session") or {}).get("id")

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(target for target in targets if target["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Runtime.enable")
        session.cdp("Page.addScriptToEvaluateOnNewDocument", {"source": PROBE})
        session.cdp("Page.navigate", {"url": f"{api}/"})

        # The rail can come up collapsed, and a collapsed rail replaces its rows
        # with a hover fly-out (`RailPeek`) - no `data-session-id` row to click.
        # The stored preference is the supported way back to the expanded rail.
        session.evaluate(
            page_js(
                """
                localStorage.setItem('karucode.railCollapsed', 'false');
                return localStorage.getItem('karucode.railCollapsed');
                """
            )
        )
        session.cdp("Page.reload")

        boot = session.evaluate(
            page_js(
                """
                const deadline = Date.now() + 30000;
                while (Date.now() < deadline) {
                  const rail = document.querySelector('.rail.session-nav, .sessions-rail, .rail');
                  const row = document.querySelector('.session-row[data-session-id]');
                  if (rail && row) return 'ready';
                  await new Promise((r) => setTimeout(r, 100));
                }
                return 'timeout:' + location.href + ':' + document.readyState + ':' +
                  (document.body.innerText || '').slice(0, 300);
                """
            )
        )
        check("the app boots with an expanded rail", boot == "ready", str(boot)[:300])
        if boot != "ready":
            return 1

        # Enter the session explicitly: the page restores whatever it was last on.
        session.evaluate(
            page_js(
                f"""
                const row = document.querySelector('.session-row[data-session-id="{session_id}"]');
                if (row) row.querySelector('button, [role="button"], a')?.click() ?? row.click();
                await new Promise((r) => setTimeout(r, 400));
                return 'clicked';
                """
            )
        )
        entered = session.evaluate(
            page_js(
                """
                const deadline = Date.now() + 15000;
                while (Date.now() < deadline) {
                  if (document.querySelector('.chat-scroll') && document.querySelector('.composer-input')) return 'in-session';
                  await new Promise((r) => setTimeout(r, 100));
                }
                return 'no-composer:' + (document.body.innerText || '').slice(0, 200);
                """
            )
        )
        check("a session is open with a composer", entered == "in-session", str(entered)[:300])
        session.evaluate(PROBE)

        # ---------------------------------------------------------------
        # Phase A: a session-rail row, right-clicked mid-stream. The rail is
        # expanded in the default desktop layout, and this is the surface a
        # reader uses most.
        # ---------------------------------------------------------------
        def phase_row() -> None:
            start_stream(api, session_id, "A: write a long answer please")
            time.sleep(1.5)  # the answer streams; the transcript is scrolling
            row = session_row_rect(session)
            if not check("the session row is on screen", "error" not in row, json.dumps(row)):
                return
            how = right_click(session, row["x"], row["y"])
            opened = snapshot(session)
            check(
                "right-clicking a session row opens its menu",
                bool(opened["menu"]),
                f"{how}: {json.dumps(opened)[:300]}",
            )
            watched = watch(session)
            check(
                "the streaming transcript really scrolls while that menu is open",
                watched["scrolls"] > 0,
                f"{watched['scrolls']} scroll events over {len(watched['samples'])} samples",
            )
            check(
                "the session menu stays open through that streaming",
                watched["ever_open"],
                f"first={watched['first_open']} last={watched['last_open']} gone={watched['gone'][:2]}",
            )
            close_menu(session)

        phase_row()

        # ---------------------------------------------------------------
        # Phase B: the project rail (the tile Toil right-clicked). It only
        # exists while the session list is collapsed, so collapse it through the
        # real control first.
        # ---------------------------------------------------------------
        collapsed = session.evaluate(
            page_js(
                """
                const toggle = [...document.querySelectorAll('button')].find(
                  (node) => (node.getAttribute('aria-label') || '') === 'Toggle session list');
                if (!toggle) return 'no-toggle';
                toggle.click();
                const deadline = Date.now() + 5000;
                while (Date.now() < deadline) {
                  if (document.querySelector('.project-rail-item')) return 'collapsed';
                  await new Promise((r) => setTimeout(r, 100));
                }
                return 'no-project-tile';
                """
            )
        )
        check("the project rail shows tiles once the session list collapses", collapsed == "collapsed", str(collapsed))

        def phase_tile() -> None:
            start_stream(api, session_id, "B: another long answer please")
            time.sleep(1.5)
            tile = tile_rect(session)
            if not check("the rail tile is on screen", "error" not in tile, json.dumps(tile)):
                return
            how = right_click(session, tile["x"], tile["y"])
            opened = snapshot(session)
            check(
                "right-clicking the rail tile opens the project menu",
                bool(opened["menu"]) and "Project actions" in (opened["label"] or ""),
                f"{how}: {json.dumps(opened)[:300]}",
            )
            watched = watch(session)
            check(
                "the project menu stays open through the streaming scrolls",
                watched["ever_open"] and watched["scrolls"] > 0,
                f"scrolls={watched['scrolls']} gone={watched['gone'][:2]}",
            )
            check(
                "the rail was not remounted by the streaming re-render",
                watched["rail_mounts"] == 0,
                f"mounts={watched['rail_mounts']}",
            )
            close_menu(session)

        if collapsed == "collapsed":
            phase_tile()

        # ---------------------------------------------------------------
        # Phase D: the model picker, as a canary for the other popovers. It is
        # not a `lib/menu.tsx` menu, it lives in the header that re-renders on
        # every chunk of the answer, and it holds typed state - so if a run
        # closes popovers or resets their state by re-rendering, this shows it.
        # ---------------------------------------------------------------
        def phase_model_picker() -> None:
            start_stream(api, session_id, "D: keep answering while the picker is open")
            time.sleep(1.5)
            opened = session.evaluate(
                page_js(
                    """
                    const button = document.querySelector('.model-switcher-button');
                    if (!button) return { error: 'no model switcher' };
                    if (button.getAttribute('aria-expanded') !== 'true') button.click();
                    const deadline = Date.now() + 5000;
                    while (Date.now() < deadline) {
                      if (document.querySelector('.model-switcher-menu')) return { open: true };
                      await new Promise((r) => setTimeout(r, 100));
                    }
                    return { error: 'menu did not open' };
                    """
                )
            )
            if not check("the model picker opens mid-stream", opened.get("open") is True, json.dumps(opened)):
                return
            session.evaluate(
                page_js(
                    """
                    const input = document.querySelector('.model-switcher-filter');
                    if (!input) return { error: 'no filter' };
                    input.focus();
                    input.value = 'st';
                    input.dispatchEvent(new Event('input', { bubbles: true }));
                    return { typed: true };
                    """
                )
            )
            time.sleep(2.5)
            after = session.evaluate(
                page_js(
                    """
                    const input = document.querySelector('.model-switcher-filter');
                    return {
                      menu: !!document.querySelector('.model-switcher-menu'),
                      filter: input ? input.value : null,
                      focused: input ? document.activeElement === input : false,
                      scrolls: window.__menuProbe.scrolls.length,
                    };
                    """
                )
            )
            check(
                "the model picker stays open, keeps its typed filter and its focus",
                bool(after.get("menu")) and after.get("filter") == "st" and bool(after.get("focused")),
                json.dumps(after),
            )
            close_menu(session)

        phase_model_picker()

        # ---------------------------------------------------------------
        # Phase C: with nothing running the gesture still works, and Escape
        # still closes - the fix must not turn the menu into a sticky overlay.
        # ---------------------------------------------------------------
        time.sleep(max(0.0, args.stream_seconds))
        tile = tile_rect(session)
        if "error" not in tile:
            right_click(session, tile["x"], tile["y"])
            opened = snapshot(session)
            check("the menu still opens while idle", bool(opened["menu"]), json.dumps(opened)[:200])
            close_menu(session)
            after = snapshot(session)
            check("Escape still closes it", not after["menu"], json.dumps(after)[:200])

        print(f"\n{len(PASS)} passed, {len(FAIL)} failed", flush=True)
        if FAIL:
            print("failed: " + ", ".join(FAIL), flush=True)
            return 1
        return 0
    finally:
        if args.keep:
            print(f"kept: server on {port}, cdp on {cdp_port}, workdir {tmp}", flush=True)
        else:
            if session is not None:
                session.close()
            for proc in (chrome, server):
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
