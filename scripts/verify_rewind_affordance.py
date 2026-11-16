#!/usr/bin/env python3
"""Real-browser check that Rewind is one hover away on every stored row.

Three defects motivated this, and none of them is visible in jsdom:

  * a turn's Rewind lived only on the newest turn's header, so an older turn - or
    a turn that was nothing but tool calls - offered nothing;
  * it was hidden for the whole length of a run *and* the rows a run leaves behind
    carry no stored message id (events carry run ids and call ids, not row ids),
    so in practice Rewind appeared only after a page refresh;
  * a tool call had no rewind at all.

So the script drives a real server, a purpose-written provider stub and a real
Chromium. It sends one turn shaped like work (a call, then the answer) and checks
what a reader actually sees:

  * a user row, an answer row and the tool call all offer Rewind, with no reload
    after the run;
  * before any hover every one of them is fully transparent - the transcript does
    not read as a page of buttons - and hovering one row paints *only* that row;
  * the call's Rewind floats inside the call's own header and revealing it does
    not reflow the header or the card;
  * on a phone (390 px, no hover) the controls are visible, take their own room
    instead of covering the call, and nothing overflows;
  * the page raises no errors.

`chrome --headless=new` on this machine reports `(hover: none)` and `(pointer:
coarse)`, and `Emulation.setEmulatedMedia` does not override either - so a hover
test would silently run through the sheet's touch tier (every control painted
solid) and prove nothing. The Blink hover/pointer settings are therefore set at
launch, and the phone phase gets its own browser with the touch values.

Usage (after `cargo build -p kc-server` and `npm run build` in web/):
    python3 scripts/verify_rewind_affordance.py
    python3 scripts/verify_rewind_affordance.py --keep   # keep the screenshots

Exit code 0 only if every check passes.
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
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.design_verify import Session, free_port, wait_for  # noqa: E402
from scripts.e2e_smoke import Client  # noqa: E402

PASS: list[str] = []
FAIL: list[str] = []

# Every transcript style: a call is a card or a compact row, and a command too.
CALL_SELECTOR = ".tool-card, .tool-row, .command-card, .command-row"

# Blink's hover/pointer enums: HoverNone=1, HoverHover=2; PointerNone=1,
# PointerCoarse=2, PointerFine=4.
BLINK_HOVER_FINE = "--blink-settings=primaryHoverType=2,availableHoverTypes=2,primaryPointerType=4,availablePointerTypes=4"
BLINK_TOUCH = "--blink-settings=primaryHoverType=1,availableHoverTypes=1,primaryPointerType=2,availablePointerTypes=2"


def check(name: str, ok: bool, detail: str = "") -> None:
    (PASS if ok else FAIL).append(name)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{f' — {detail}' if detail else ''}", flush=True)


# ---------------------------------------------------------------------------
# The provider stub: one call, then the answer that follows its result.
# ---------------------------------------------------------------------------

ANSWER = "Read the fixture. Done."


class Provider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # keep the run quiet
        return

    def _json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            self._json(200, {"object": "list", "data": [{"id": "mock-coder", "object": "model"}]})
            return
        if self.path.rstrip("/") in ("/health", "/v1/health"):
            self._json(200, {"status": "ok"})
            return
        self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        payload = json.loads(self.rfile.read(length) or b"{}")
        messages = payload.get("messages") or []
        newest_user = 0
        for index, message in enumerate(messages):
            if message.get("role") == "user":
                newest_user = index
        ran_tools = any(m.get("role") == "tool" for m in messages[newest_user:])
        self._stream(payload.get("model") or "mock-coder", ran_tools)

    def _stream(self, model: str, ran_tools: bool) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def chunk(delta: dict, finish: str | None = None) -> None:
            body = {
                "id": "chatcmpl-rewind",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            }
            data = f"data: {json.dumps(body)}\n\n".encode()
            try:
                self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                raise SystemExit(0)

        chunk({"role": "assistant", "content": ""})
        if ran_tools:
            chunk({"content": ANSWER})
            chunk({}, "stop")
        else:
            chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_rewind_1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": json.dumps({"path": "README.md"}),
                            },
                        }
                    ]
                }
            )
            chunk({}, "tool_calls")
        usage = {
            "id": "chatcmpl-rewind",
            "object": "chat.completion.chunk",
            "created": int(time.time()),
            "model": model,
            "choices": [],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
        }
        data = f"data: {json.dumps(usage)}\n\n".encode()
        self.wfile.write(hex(len(data))[2:].encode() + b"\r\n" + data + b"\r\n")
        done = b"data: [DONE]\n\n"
        self.wfile.write(hex(len(done))[2:].encode() + b"\r\n" + done + b"\r\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


# ---------------------------------------------------------------------------
# Page-side probes
# ---------------------------------------------------------------------------

MEASURE = """
const rows = [...document.querySelectorAll('.msg-user, .msg-assistant')].map((el) => {
  const button = el.querySelector('.rewind-button');
  return {
    where: el.classList.contains('msg-user') ? 'user' : 'answer',
    kind: el.getAttribute('data-entry-key'),
    has: !!button,
    opacity: button ? Number(getComputedStyle(button).opacity) : null,
  };
});
const calls = [...document.querySelectorAll('.tool-card, .tool-row, .command-card, .command-row')].map((el) => {
  const button = el.querySelector(':scope > .rewind-tool');
  const head = el.querySelector('.tool-head, .tool-row-head, .command-summary, .command-row-head');
  const card = el.getBoundingClientRect();
  const box = button ? button.getBoundingClientRect() : null;
  const headBox = head ? head.getBoundingClientRect() : null;
  return {
    kind: el.className,
    has: !!button,
    opacity: button ? Number(getComputedStyle(button).opacity) : null,
    position: button ? getComputedStyle(button).position : null,
    insideCard: box ? box.top >= card.top - 1 && box.bottom <= card.bottom + 1 : null,
    insideHead: box && headBox ? box.top >= headBox.top - 1 && box.bottom <= headBox.bottom + 1 : null,
    headHeight: headBox ? Math.round(headBox.height) : null,
    headWidth: headBox ? Math.round(headBox.width) : null,
    cardWidth: Math.round(card.width),
    label: button ? button.textContent.trim() : null,
  };
});
return {
  rows,
  calls,
  overflow: document.documentElement.scrollWidth - innerWidth,
  hover: matchMedia('(hover: hover)').matches,
  pointerFine: matchMedia('(pointer: fine)').matches,
};
"""


class Browser:
    """One Chromium with a stated hover capability, driven over CDP."""

    def __init__(self, profile: Path, width: int, height: int, hover: bool) -> None:
        self.log = (profile.parent / f"chromium-{profile.name}.log").open("w")
        cdp_port = free_port()
        self.proc = subprocess.Popen(
            [
                "chromium",
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--hide-scrollbars",
                BLINK_HOVER_FINE if hover else BLINK_TOUCH,
                f"--remote-debugging-port={cdp_port}",
                f"--user-data-dir={profile}",
                f"--window-size={width},{height}",
                "about:blank",
            ],
            stdout=self.log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list").read())
        page = next(target for target in targets if target.get("type") == "page")
        self.session = Session(page["webSocketDebuggerUrl"])
        self.session._send("Page.enable")
        self.session._send("Runtime.enable")
        self.session._send(
            "Emulation.setDeviceMetricsOverride",
            {"width": width, "height": height, "deviceScaleFactor": 1, "mobile": not hover},
        )
        self.instrumented = False

    def open(self, port: int, session_id: str, ready: str) -> None:
        """Open the UI on `session_id`. The app restores the open session from localStorage."""
        # Installed before the page's own scripts: the API client binds `fetch`
        # when its module is imported, so a wrapper added after load never sees a
        # single request - instrumentation that silently does nothing is worse
        # than none.
        self.session._send(
            "Page.addScriptToEvaluateOnNewDocument",
            {
                "source": """
window.__errors = [];
window.addEventListener('error', (e) => window.__errors.push(String(e.message || e.type)));
window.__traffic = [];
const realFetch = window.fetch.bind(window);
window.fetch = async (input, init) => {
  const url = typeof input === 'string' ? input : (input && input.url) || String(input);
  const row = { url, status: null, body: null };
  window.__traffic.push(row);
  try {
    const response = await realFetch(input, init);
    row.status = response.status;
    if (!response.ok || url.includes('/rewind')) {
      row.body = (await response.clone().text()).slice(0, 400);
    }
    return response;
  } catch (error) {
    row.status = 0;
    row.body = String(error);
    throw error;
  }
};
""",
            },
        )
        self.session.navigate(f"http://127.0.0.1:{port}")
        self.session.evaluate(f"localStorage.setItem('karucode.lastSession', {json.dumps(session_id)}); return true;")
        self.session.navigate(f"http://127.0.0.1:{port}")
        self.session.evaluate(
            f"""
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {{
  if (document.querySelector({json.dumps(ready)})) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}}
return 'timeout';
"""
        )
        self.instrumented = True

    def close(self) -> None:
        try:
            self.session.ws.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()


def settle(session: Session, timeout: float = 45.0) -> bool:
    """Wait until a stored call and a stored answer are on screen and the run ended."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        state = session.evaluate(
            """
return {
  running: !!document.querySelector('.tag-live') || !!document.querySelector('.stream-caret'),
  calls: document.querySelectorAll('.tool-card > .rewind-tool, .tool-row > .rewind-tool, .command-card > .rewind-tool, .command-row > .rewind-tool').length,
  answers: document.querySelectorAll('.msg-assistant .rewind-button').length,
};
"""
        ) or {}
        if not state.get("running") and state.get("calls") and state.get("answers"):
            return True
        time.sleep(0.2)
    return False


def hover(session: Session, selector: str) -> None:
    """Hover what a reader hovers: a real pointer move inside the element."""
    point = session.evaluate(
        f"""
const els = [...document.querySelectorAll({json.dumps(selector)})];
const el = els[els.length - 1];
if (!el) return null;
el.scrollIntoView({{ block: 'center' }});
await new Promise((r) => setTimeout(r, 200));
const rect = el.getBoundingClientRect();
return {{ x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + Math.min(rect.height / 2, 12)) }};
"""
    )
    if not point:
        raise SystemExit(f"nothing to hover for {selector}")
    session._send(
        "Input.dispatchMouseEvent",
        {"type": "mouseMoved", "x": point["x"], "y": point["y"], "button": "none", "buttons": 0},
    )
    time.sleep(0.4)


def move_away(session: Session) -> None:
    session._send("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": 2, "y": 2, "button": "none", "buttons": 0})
    time.sleep(0.4)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "debug" / "karucode")
    parser.add_argument("--keep", action="store_true", help="keep the output directory")
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run `cargo build -p kc-server`")
    if not (ROOT / "web" / "dist" / "index.html").exists():
        raise SystemExit("missing web/dist; run `npm run build` in web/")

    out = Path(tempfile.mkdtemp(prefix="rewind-verify-"))
    workspace = out / "workspace"
    workspace.mkdir(parents=True)
    (workspace / "README.md").write_text("# Rewind fixture\n\nRead by the scripted call.\n")

    provider_port = free_port()
    provider = ThreadingHTTPServer(("127.0.0.1", provider_port), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

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
    (out / "home").mkdir()
    server_log = (out / "server.log").open("w")
    server = subprocess.Popen(
        [str(args.binary), "--host", "127.0.0.1", "--port", str(server_port), "--workspace", str(workspace)],
        cwd=workspace,
        env=env,
        stdout=server_log,
        stderr=subprocess.STDOUT,
    )
    desktop = None
    phone = None
    try:
        wait_for(f"http://127.0.0.1:{provider_port}/health")
        wait_for(f"http://127.0.0.1:{server_port}/api/health")
        client = Client(server_port)
        project = next(
            (item for item in client.ok("GET", "/api/projects")["projects"] if item["path"] == str(workspace.resolve())),
            None,
        )
        if project is None:
            project = client.ok("POST", "/api/projects", {"path": str(workspace), "name": "rewind-fixture"})
        created = client.ok("POST", "/api/sessions", {"title": "Rewind fixture", "project": project["id"]})
        session_id = created["id"]

        print("\ndesktop, hover", flush=True)
        desktop = Browser(out / "desktop-profile", 1440, 1000, hover=True)
        desktop.open(server_port, session_id, "#composer-input")
        session = desktop.session
        media = session.evaluate(MEASURE) or {}
        check(
            "the browser reports a hovering, fine pointer",
            bool(media.get("hover")) and bool(media.get("pointerFine")),
            f"hover={media.get('hover')} pointerFine={media.get('pointerFine')}",
        )

        client.ok("POST", f"/api/sessions/{session_id}/messages", {"content": "Read the fixture."}, expect=202)
        instrumented = session.evaluate("return Array.isArray(window.__traffic) && Array.isArray(window.__errors);")
        check(
            "the probe is installed in the page (else nothing below is evidence)",
            bool(instrumented),
            f"__traffic/__errors present: {instrumented}",
        )
        settled = settle(session)
        quiet = session.evaluate(MEASURE) or {}
        check(
            "after the run, with no reload, every row offers Rewind",
            settled,
            json.dumps({"rows": quiet.get("rows"), "calls": quiet.get("calls")})[:700],
        )
        rows = quiet.get("rows") or []
        calls = quiet.get("calls") or []
        check(
            "a user message, an answer and a tool call all carry the control",
            len(rows) == 2 and len(calls) == 1 and all(row.get("has") for row in rows) and calls[0].get("has"),
            json.dumps({"rows": rows, "calls": calls})[:700],
        )
        check(
            "none of them is painted before a hover",
            all(row.get("opacity") == 0 for row in rows) and all(call.get("opacity") == 0 for call in calls),
            f"rows={[row.get('opacity') for row in rows]} calls={[call.get('opacity') for call in calls]}",
        )

        move_away(session)
        before = session.evaluate(MEASURE) or {}
        hover(session, CALL_SELECTOR)
        after = session.evaluate(MEASURE) or {}
        hovered = (after.get("calls") or [{}])[0]
        check(
            "hovering the call paints that call's Rewind",
            hovered.get("opacity") == 1,
            f"opacity={hovered.get('opacity')} on {hovered.get('kind')!r}",
        )
        check(
            "and its label says what the cut is",
            hovered.get("label") == "Rewind",
            f"label={hovered.get('label')!r}",
        )
        check(
            "the call's Rewind floats inside the call's own header",
            hovered.get("position") == "absolute"
            and bool(hovered.get("insideCard"))
            and bool(hovered.get("insideHead")),
            f"position={hovered.get('position')} insideCard={hovered.get('insideCard')} "
            f"insideHead={hovered.get('insideHead')} head={hovered.get('headHeight')}px",
        )
        before_call = (before.get("calls") or [{}])[0]
        check(
            "revealing it does not reflow the header or the card",
            before_call.get("headHeight") == hovered.get("headHeight")
            and before_call.get("headWidth") == hovered.get("headWidth")
            and before_call.get("cardWidth") == hovered.get("cardWidth"),
            f"head {before_call.get('headHeight')}→{hovered.get('headHeight')}px "
            f"× {before_call.get('headWidth')}→{hovered.get('headWidth')}px, "
            f"card {before_call.get('cardWidth')}→{hovered.get('cardWidth')}px",
        )
        session.screenshot(out / "rewind-call-hovered.png")

        # The message rows: hovering one must not paint its neighbours.
        hover(session, ".msg-user")
        hovered_rows = (session.evaluate(MEASURE) or {}).get("rows") or []
        check(
            "hovering one message paints only that row's Rewind",
            [row.get("opacity") for row in hovered_rows] == [1, 0],
            f"rows={[row.get('opacity') for row in hovered_rows]}",
        )
        session.screenshot(out / "rewind-message-hovered.png")

        # And the button really is the click target: a press opens the dialog on
        # the message the row names. This is what the whole affordance is for.
        point = session.evaluate(
            """
const button = document.querySelector('.msg-user .rewind-button');
if (!button) return null;
const rect = button.getBoundingClientRect();
const x = Math.round(rect.x + rect.width / 2);
const y = Math.round(rect.y + rect.height / 2);
const hit = document.elementFromPoint(x, y);
return {
  x,
  y,
  hitText: hit ? (hit.className || hit.tagName) : null,
  hitsButton: !!(hit && hit.closest('.rewind-button')),
  onTop: hit ? !!hit.closest('.toast') : false,
};
"""
        )
        check(
            "the click point is the button itself, with nothing over it",
            bool(point) and bool(point.get("hitsButton")),
            json.dumps(point)[:300],
        )
        if point:
            for kind in ("mousePressed", "mouseReleased"):
                session._send(
                    "Input.dispatchMouseEvent",
                    {
                        "type": kind,
                        "x": point["x"],
                        "y": point["y"],
                        "button": "left",
                        "buttons": 1 if kind == "mousePressed" else 0,
                        "clickCount": 1,
                    },
                )
            timeline = session.evaluate(
                """
const samples = [];
const started = Date.now();
while (Date.now() - started < 2500) {
  const dialog = document.querySelector('.dialog-rewind');
  const count = dialog ? dialog.querySelector('.rewind-count') : null;
  samples.push({
    t: Date.now() - started,
    rows: document.querySelectorAll('.msg-user, .msg-assistant').length,
    dialog: !!dialog,
    count: count ? count.textContent.slice(0, 34) : null,
    empty: !!document.querySelector('.empty-state'),
    skeleton: !!document.querySelector('.chat-skeleton'),
    loading: !!document.querySelector('.chat-loading'),
    rewindReqs: (window.__traffic || []).filter((row) => row.url.includes('/rewind')).length,
  });
  await new Promise((r) => setTimeout(r, 60));
}
const seen = [];
let previous = null;
for (const sample of samples) {
  const key = JSON.stringify([sample.rows, sample.dialog, sample.count, sample.empty, sample.skeleton, sample.loading, sample.rewindReqs]);
  if (key !== previous) { seen.push(sample); previous = key; }
}
return seen;
"""
            )
            at_click = session.evaluate(
                """
return {
  rows: document.querySelectorAll('.msg-user, .msg-assistant').length,
  empty: !!document.querySelector('.empty-state'),
  skeleton: !!document.querySelector('.chat-skeleton'),
  rails: document.querySelectorAll('.rail-item, .session-row, [data-session-id]').length,
  title: (document.querySelector('.chat-title, .session-title, h1') || {}).textContent || null,
};
"""
            )
            dialog = None
            deadline = time.time() + 15
            while time.time() < deadline:
                dialog = session.evaluate(
                    """
const el = document.querySelector('.dialog-rewind');
if (!el) return null;
const count = el.querySelector('.rewind-count');
return {
  text: el.innerText.slice(0, 400),
  count: count ? count.textContent : '',
  settled: !!count && !count.textContent.includes('Working out'),
};
"""
                )
                if dialog and dialog.get("settled"):
                    break
                time.sleep(0.2)
            asked = session.evaluate(
                """
return (window.__traffic || []).filter((row) => row.url.includes('/rewind'));
"""
            )
            check(
                "clicking it opens the rewind dialog for that message",
                bool(dialog) and "Read the fixture." in (dialog or {}).get("text", ""),
                json.dumps({"dialog": (dialog or {}).get("text", "")[:200], "atClick": at_click})[:700]
                if dialog
                else "no dialog opened",
            )
            # The whole point of the id work: the row the run left behind names a
            # *stored* message, so the dialog's own request goes out with it. A
            # request that never left, or one carrying `undefined`, is the bug
            # this checks for - the reader used to have to reload to get here.
            check(
                "and the preview it asks for carries the stored message id",
                len(asked) == 1
                and asked[0].get("status") == 200
                and "message=" in asked[0].get("url", "")
                and "undefined" not in asked[0].get("url", ""),
                json.dumps({"rewindRequests": asked, "atClick": at_click, "timeline": timeline})[:1200],
            )
            check(
                "the dialog then describes the cut it would make",
                bool(dialog) and dialog.get("count", "").startswith("3 messages after this one will be removed"),
                f"count={(dialog or {}).get('count')!r}",
            )
            session.screenshot(out / "rewind-dialog.png")
            session.evaluate(
                """
const close = document.querySelector('.dialog-rewind button[aria-label="Close dialog"]');
if (close) close.click();
return true;
"""
            )
            time.sleep(0.3)

        errors = session.evaluate("return window.__errors || [];")
        check("the page raised no errors", not errors, str(errors[:3]))

        print("\nphone, no hover", flush=True)
        phone = Browser(out / "phone-profile", 390, 844, hover=False)
        phone.open(server_port, session_id, ".msg-user")
        mobile = phone.session.evaluate(MEASURE) or {}
        check(
            "the phone browser reports no hover",
            not mobile.get("hover"),
            f"hover={mobile.get('hover')} pointerFine={mobile.get('pointerFine')}",
        )
        mobile_rows = mobile.get("rows") or []
        mobile_calls = mobile.get("calls") or []
        check(
            "with no hover every Rewind is visible, after the same run",
            len(mobile_rows) == 2
            and all(row.get("opacity") == 1 for row in mobile_rows)
            and len(mobile_calls) == 1
            and mobile_calls[0].get("opacity") == 1,
            json.dumps({"rows": mobile_rows, "calls": mobile_calls})[:700],
        )
        check(
            "and the call's control takes its own room instead of covering the call",
            all(call.get("position") == "static" for call in mobile_calls),
            json.dumps([call.get("position") for call in mobile_calls]),
        )
        check(
            "the phone transcript does not overflow",
            (mobile.get("overflow") or 0) <= 1,
            f"overflow={mobile.get('overflow')}px",
        )
        phone.session.screenshot(out / "rewind-phone.png")
    finally:
        if desktop is not None:
            desktop.close()
        if phone is not None:
            phone.close()
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        provider.shutdown()

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    if FAIL:
        print("screenshots: " + str(out))
    elif args.keep:
        print("screenshots: " + str(out))
    else:
        shutil.rmtree(out, ignore_errors=True)
    return 1 if FAIL else 0


if __name__ == "__main__":
    raise SystemExit(main())