#!/usr/bin/env python3
"""Measure the Firefox (Gecko) CSS fixes on a fixture page, in real Firefox.

jsdom has no layout and Chromium's overlay scrollbars measure 0px, so neither
harness can see the defects this script exists for: a scrollbar whose width is
chosen by `scrollbar-width` (which Gecko honours and `::-webkit-scrollbar` does
not), a `::placeholder` the UA paints at 54% opacity, a `<select>` whose arrow
was removed by `appearance:none`, a focus ring clipped by its scrollport, a
sticky table header inside a collapsed table, and a `<details>` that kept the
platform marker.

The page is a throwaway fixture, not the app: it holds one of each shape the
stylesheet has to dress, and `<link>`s the real `web/src/styles/index.css` over
HTTP (so the `@import` order is the shipped order). The stylesheet directory is
served straight from the working tree — nothing is built or copied — and the
same page is run before and after a change to read a before/after number.

Usage:
    python3 scripts/verify_ff_css.py [--port 4477] [--styles-dir PATH] [--label after]

Requires, and does not build:
    /tmp/ffpkg/firefox/firefox, /tmp/ffpkg/geckodriver

Exit code is non-zero when any check fails. stdlib only: urllib speaks W3C
WebDriver over plain HTTP and http.server serves the fixture.
"""

from __future__ import annotations

import argparse
import base64
import json
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIREFOX_BIN = Path("/tmp/ffpkg/firefox/firefox")
GECKODRIVER_BIN = Path("/tmp/ffpkg/geckodriver")
LOG_PATH = Path("/tmp/ff-css-geckodriver.log")

# A scrollbar wider than this is the platform default, not a chosen width.
MAX_SCROLLBAR_PX = 10

FIXTURE_PATH = "/__ff_css_fixture.html"

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    number = len(PASS) + len(FAIL) + 1
    if ok:
        PASS.append(name)
        print(f"PASS {number}. {name}" + (f" — {detail}" if detail else ""), flush=True)
    else:
        FAIL.append(name)
        print(f"FAIL {number}. {name}" + (f" — {detail}" if detail else ""), flush=True)
    return ok


def note(text: str) -> None:
    print(f"      {text}", flush=True)


# --------------------------------------------------------------------------
# the fixture page
# --------------------------------------------------------------------------

# One of each shape the stylesheet has to dress. Heights are inline because they
# are the *scenario* (a short scrollport), not the thing under test: every
# declaration being measured still comes from the real stylesheet.
FIXTURE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>karucode CSS fixture</title>
<link rel="stylesheet" href="/styles/index.css">
<style>
  body { margin: 0; padding: 24px; display: flex; flex-direction: column; gap: 24px; }
  .box { display: block; width: 320px; }
</style>
</head>
<body>

<!-- F1/F3: an inner scroller inside the Theme Studio's own scroll region. -->
<div class="theme-studio box">
  <div class="theme-studio-scroll" id="ts-scroll" style="height: 160px">
    <div class="theme-section">
      <label class="theme-field-label" for="density">Density</label>
      <select id="density">
        <option>Comfortable</option>
        <option>Compact</option>
        <option>Spacious</option>
      </select>
    </div>
    <div class="theme-section">
      <label class="theme-field-label" for="font">Interface font</label>
      <select id="font">
        <option>Inter</option>
        <option>System</option>
      </select>
    </div>
    <div class="theme-section">
      <label class="theme-field-label" for="theme-input">Preset name</label>
      <input class="theme-name-input" id="theme-input" placeholder="Name this theme">
    </div>
    <p class="theme-description">Filler so the region overflows: it is the scroller being measured.</p>
    <p class="theme-description">Filler.</p>
    <p class="theme-description">Filler.</p>
    <p class="theme-description">Filler.</p>
  </div>
</div>

<!-- F4, magnified: the caret is 8x4px, so the same rule at `zoom: 6` is what a
     screenshot can actually judge. Nothing here is measured. -->
<div class="theme-studio" style="zoom: 6; width: 320px">
  <select id="density-zoom">
    <option>Comfortable</option>
    <option>Compact</option>
  </select>
</div>

<!-- F9: the first row of a list pane is flush with the scrollport's edge. The
     starter button is only there so a real Tab can land on the first row: a
     scripted .focus() would not match :focus-visible in Gecko. The inline
     `flex` is the scenario, not the subject: base.css gives `body` `height:100%`,
     so a `flex: 1 1 auto` pane would grow instead of scrolling. -->
<button id="before-nav" style="width: 120px">start</button>
<nav class="session-nav box" id="session-nav" style="flex: 0 0 120px; height: 120px">
  <div class="session-list-wrap">
    <div class="session-row"><button class="session-open" id="row-0"><span class="session-title">First session</span></button></div>
    <div class="session-row"><button class="session-open"><span class="session-title">Second session</span></button></div>
    <div class="session-row"><button class="session-open"><span class="session-title">Third session</span></button></div>
    <div class="session-row"><button class="session-open"><span class="session-title">Fourth session</span></button></div>
    <div class="session-row"><button class="session-open"><span class="session-title">Fifth session</span></button></div>
  </div>
</nav>

<!-- F2: the quiet-token placeholders the audit named, plus one input nothing
     styles at all: that is the UA's own placeholder dimming, unmodified. -->
<div class="box">
  <input class="palette-input" id="palette-input" placeholder="Search commands and sessions">
  <input class="composer-input" id="composer-input" placeholder="Ask karucode">
  <input id="plain-input" placeholder="Nothing in the app styles this one">
</div>

<!-- F2/F9: the palette list, its placeholder-bearing input and its rows. -->
<button id="before-palette" style="width: 120px">start</button>
<div class="overlay">
  <div class="palette-list" id="palette-list" style="max-height: 140px; height: 140px">
    <button class="palette-item" id="palette-0"><span class="palette-label">First command</span></button>
    <button class="palette-item"><span class="palette-label">Second command</span></button>
    <button class="palette-item"><span class="palette-label">Third command</span></button>
    <button class="palette-item"><span class="palette-label">Fourth command</span></button>
    <button class="palette-item"><span class="palette-label">Fifth command</span></button>
  </div>
</div>

<!-- F5: a sticky header inside a scrolling wrap. -->
<div class="artifact-card">
  <div class="artifact-table-wrap" id="table-wrap" style="max-height: 120px">
    <table class="artifact-table" id="artifact-table">
      <thead><tr><th>Name</th><th>Value</th></tr></thead>
      <tbody id="table-body"></tbody>
    </table>
  </div>
</div>

<!-- F11: the disclosure that has to draw the app's own marker. -->
<details class="goals-details" id="goals-details" open>
  <summary id="goals-summary">Limits</summary>
  <p class="goals-subtle">Engine options live here.</p>
</details>

<script>
  /* `name` is a built-in window property: `var name = ...` in a classic script
     rebinds nothing, so `tr.appendChild(name)` threw and the fixture body stayed
     empty. Named for what they are. */
  window.__fixtureRows = 0;
  window.__fixtureTbody = !!document.getElementById('table-body');
  var fixtureBody = document.getElementById('table-body');
  for (var i = 0; i < 24; i += 1) {
    var row = document.createElement('tr');
    var labelCell = document.createElement('td');
    labelCell.textContent = 'row ' + i;
    var numberCell = document.createElement('td');
    numberCell.className = 'is-numeric';
    numberCell.textContent = String(i * 7);
    row.appendChild(labelCell);
    row.appendChild(numberCell);
    fixtureBody.appendChild(row);
    window.__fixtureRows += 1;
  }
</script>
</body>
</html>
"""


# --------------------------------------------------------------------------
# the measurements, one page script for the facts and one per interaction
# --------------------------------------------------------------------------

MEASURE = r"""
var out = {};

function scrollbarOf(el) {
  if (!el) return null;
  var css = getComputedStyle(el);
  return {
    selector: el.id || el.className,
    offsetWidth: el.offsetWidth,
    clientWidth: el.clientWidth,
    scrollbarPx: el.offsetWidth - el.clientWidth,
    scrollbarHeightPx: el.offsetHeight - el.clientHeight,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
    children: el.children.length,
    scrollbarWidth: css.scrollbarWidth,
    scrollbarColor: css.scrollbarColor,
    gutter: css.scrollbarGutter,
    overflowY: css.overflowY,
    overflowing: el.scrollHeight > el.clientHeight + 1
  };
}

out.html = scrollbarOf(document.documentElement);
out.scrollers = [
  scrollbarOf(document.getElementById('ts-scroll')),
  scrollbarOf(document.getElementById('session-nav')),
  scrollbarOf(document.getElementById('palette-list')),
  scrollbarOf(document.getElementById('table-wrap'))
].filter(function (row) { return row !== null; });

function placeholderOf(el) {
  if (!el) return null;
  var css = getComputedStyle(el, '::placeholder');
  return {
    selector: el.id,
    color: css.color,
    opacity: css.opacity,
    visibleOpacity: parseFloat(css.opacity) * (parseFloat(getComputedStyle(el).opacity) || 1)
  };
}

out.placeholders = [
  placeholderOf(document.getElementById('palette-input')),
  placeholderOf(document.getElementById('composer-input')),
  placeholderOf(document.getElementById('plain-input')),
  placeholderOf(document.getElementById('theme-input'))
].filter(function (row) { return row !== null; });

out.tokens = {
  textSubtle: getComputedStyle(document.documentElement).getPropertyValue('--text-subtle').trim(),
  faint: getComputedStyle(document.documentElement).getPropertyValue('--faint').trim(),
  lineStrong: getComputedStyle(document.documentElement).getPropertyValue('--line-strong').trim()
};

function selectOf(el) {
  if (!el) return null;
  var css = getComputedStyle(el);
  return {
    selector: el.id,
    appearance: css.appearance || css.webkitAppearance,
    backgroundImage: css.backgroundImage,
    backgroundSize: css.backgroundSize,
    backgroundPosition: css.backgroundPosition,
    paddingRight: css.paddingRight,
    height: Math.round(el.getBoundingClientRect().height)
  };
}

out.selects = [
  selectOf(document.getElementById('density')),
  selectOf(document.getElementById('font'))
].filter(function (row) { return row !== null; });

var details = document.getElementById('goals-details');
var summary = document.getElementById('goals-summary');
if (summary) {
  var summaryCss = getComputedStyle(summary);
  var markerCss = getComputedStyle(summary, '::before');
  out.details = {
    display: summaryCss.display,
    listStyleType: summaryCss.listStyleType,
    webkitMarker: getComputedStyle(summary, '::-webkit-details-marker').display,
    markerContent: markerCss.content,
    markerColor: markerCss.color
  };
}

var wrap = document.getElementById('table-wrap');
var table = document.getElementById('artifact-table');
var th = table ? table.querySelector('th') : null;
out.table = {
  borderCollapse: table ? getComputedStyle(table).borderCollapse : null,
  borderSpacing: table ? getComputedStyle(table).borderSpacing : null,
  thPosition: th ? getComputedStyle(th).position : null,
  bodyRows: document.querySelectorAll('#artifact-table tbody tr').length,
  fixtureRows: window.__fixtureRows,
  fixtureTbody: window.__fixtureTbody
};

out.supportsMozAppearance = CSS.supports('-moz-appearance', 'none');

return out;
"""

# Scroll the table wrap, then read where the sticky header actually sits.
STICKY = r"""
var wrap = document.getElementById('table-wrap');
var th = document.querySelector('#artifact-table th');
if (!wrap || !th) { return null; }
wrap.scrollTop = 60;
var wrapRect = wrap.getBoundingClientRect();
var thRect = th.getBoundingClientRect();
var styles = getComputedStyle(th);
return {
  scrolled: wrap.scrollTop,
  wrapTop: Math.round(wrapRect.top),
  thTop: Math.round(thRect.top),
  stuckOffsetPx: Math.round(thRect.top - wrapRect.top),
  borderBottom: styles.borderBottomWidth + ' ' + styles.borderBottomColor,
  boxShadow: styles.boxShadow
};
"""

# Tab from a sibling so the ring is a real :focus-visible one.
RING = r"""
var el = document.activeElement;
if (!el) { return null; }
var css = getComputedStyle(el);
var port = el.closest('.session-nav, .palette-list, .artifact-table-wrap');
var rect = el.getBoundingClientRect();
var out = {
  focused: el.id || el.className,
  outlineWidth: parseFloat(css.outlineWidth) || 0,
  outlineOffset: parseFloat(css.outlineOffset) || 0,
  outlineStyle: css.outlineStyle,
  boxShadow: css.boxShadow
};
out.focusVisible = el.matches(':focus-visible');
if (port) {
  var portRect = port.getBoundingClientRect();
  // The scrollport a child is clipped by is the client box: the border box
  // minus its border (`clientLeft`/`clientTop`) and minus the scrollbar
  // (`clientWidth`/`clientHeight` already exclude it).
  var innerLeft = portRect.left + port.clientLeft;
  var innerTop = portRect.top + port.clientTop;
  var innerRight = innerLeft + port.clientWidth;
  var reach = out.outlineWidth + out.outlineOffset;
  out.portTop = Math.round(innerTop);
  out.portLeft = Math.round(innerLeft);
  out.portRight = Math.round(innerRight);
  out.rowTop = Math.round(rect.top);
  out.rowLeft = Math.round(rect.left);
  out.rowRight = Math.round(rect.right);
  out.clippedTop = (rect.top - reach) < innerTop - 0.5;
  out.clippedLeft = (rect.left - reach) < innerLeft - 0.5;
  out.clippedRight = (rect.right + reach) > innerRight + 0.5;
}
return out;
"""


# --------------------------------------------------------------------------
# HTTP: the fixture plus the real stylesheets
# --------------------------------------------------------------------------


def serve(styles_dir: Path, port: int) -> ThreadingHTTPServer:
    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, *args):  # a silent server; the checks are the report
            return

        def end_headers(self):
            self.send_header("Cache-Control", "no-store, max-age=0")
            super().end_headers()

        def do_GET(self):  # noqa: N802 - the stdlib name
            path = self.path.split("?", 1)[0]
            if path == FIXTURE_PATH:
                payload = FIXTURE.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            if path.startswith("/styles/"):
                name = path[len("/styles/"):]
                target = (styles_dir / name).resolve()
                if "/" in name or "\\" in name or not name.endswith(".css"):
                    self.send_error(404, "only *.css from the stylesheet directory")
                    return
                if styles_dir.resolve() not in target.parents or not target.is_file():
                    self.send_error(404, "not a stylesheet")
                    return
                payload = target.read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "text/css; charset=utf-8")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            self.send_error(404, "no such fixture asset")

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


# --------------------------------------------------------------------------
# W3C WebDriver over plain urllib (same shape as scripts/verify_firefox.py)
# --------------------------------------------------------------------------


class WebDriverError(RuntimeError):
    pass


class Firefox:
    def __init__(self, port: int, binary: Path) -> None:
        self.base = f"http://127.0.0.1:{port}"
        capabilities = {
            "capabilities": {
                "alwaysMatch": {
                    "browserName": "firefox",
                    "acceptInsecureCerts": True,
                    "moz:firefoxOptions": {
                        "binary": str(binary),
                        "args": ["-headless"],
                        "prefs": {
                            "devtools.console.stdout.content": True,
                            "app.update.disabledForTesting": True,
                            "browser.shell.checkDefaultBrowser": False,
                        },
                    },
                }
            }
        }
        created = self._request("POST", "/session", capabilities)
        self.session_id = created["sessionId"]

    def _request(self, method: str, path: str, body: dict | None = None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method=method)
        if body is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                payload = json.loads(response.read() or b"{}")
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                details = json.loads(raw)["value"]
            except Exception:
                details = raw[:500]
            raise WebDriverError(f"{method} {path} -> HTTP {error.code}: {details}") from None
        value = payload.get("value")
        if isinstance(value, dict) and "error" in value:
            raise WebDriverError(f"{method} {path}: {value}")
        return value if value is not None else {}

    def navigate(self, url: str) -> None:
        self._request("POST", f"/session/{self.session_id}/url", {"url": url})

    def evaluate(self, script: str):
        return self._request(
            "POST",
            f"/session/{self.session_id}/execute/sync",
            {"script": script, "args": []},
        )

    def screenshot(self, path: Path) -> int:
        encoded = self._request("GET", f"/session/{self.session_id}/screenshot")
        raw = base64.b64decode(encoded)
        path.write_bytes(raw)
        return len(raw)

    def set_window(self, width: int, height: int) -> None:
        self._request(
            "POST",
            f"/session/{self.session_id}/window/rect",
            {"width": width, "height": height},
        )

    def keys(self, keys: list[str]) -> None:
        steps = [{"type": "keyDown", "value": value} for value in keys]
        steps += [{"type": "keyUp", "value": value} for value in reversed(keys)]
        self._request(
            "POST",
            f"/session/{self.session_id}/actions",
            {"actions": [{"type": "key", "id": "keyboard", "actions": steps}]},
        )
        self._request("DELETE", f"/session/{self.session_id}/actions")

    def quit(self) -> None:
        try:
            self._request("DELETE", f"/session/{self.session_id}")
        except Exception:
            pass


# --------------------------------------------------------------------------
# plumbing
# --------------------------------------------------------------------------


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def port_in_use(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def wait_for_port(port: int, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if port_in_use(port):
            return
        time.sleep(0.1)
    raise SystemExit(f"geckodriver never listened on port {port}")


def px(value: float) -> str:
    return f"{value:g}px"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4477, help="geckodriver port (default 4477)")
    parser.add_argument(
        "--styles-dir",
        type=Path,
        default=ROOT / "web" / "src" / "styles",
        help="stylesheet directory to serve at /styles (default web/src/styles)",
    )
    parser.add_argument("--label", default="", help="name this run in the summary (before/after)")
    parser.add_argument(
        "--screenshot",
        type=Path,
        default=None,
        help="also save a PNG of the fixture here (for eyeballing the caret/marker)",
    )
    parser.add_argument("--firefox", type=Path, default=FIREFOX_BIN)
    parser.add_argument("--geckodriver", type=Path, default=GECKODRIVER_BIN)
    args = parser.parse_args()

    styles_dir = args.styles_dir.resolve()
    index = styles_dir / "index.css"
    if not index.is_file():
        raise SystemExit(f"no index.css under {styles_dir}")
    if not args.firefox.is_file() or not args.geckodriver.is_file():
        raise SystemExit(f"need {args.firefox} and {args.geckodriver}")

    label = f" [{args.label}]" if args.label else ""
    print(f"fixture run{label}: styles from {styles_dir}", flush=True)

    server = serve(styles_dir, free_port())
    server_port = server.server_address[1]

    gecko: subprocess.Popen | None = None
    if port_in_use(args.port):
        print(f"  reusing the geckodriver already listening on {args.port}", flush=True)
    else:
        LOG_PATH.write_text("")
        with open(LOG_PATH, "ab") as log:
            gecko = subprocess.Popen(
                [str(args.geckodriver), "--port", str(args.port)],
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        wait_for_port(args.port)

    browser: Firefox | None = None
    try:
        browser = Firefox(args.port, args.firefox)
        browser.set_window(1000, 1400)
        browser.navigate(f"http://127.0.0.1:{server_port}{FIXTURE_PATH}")

        # The page is static, but let the stylesheet finish resolving first.
        for _ in range(50):
            ready = browser.evaluate(
                "return document.readyState === 'complete' && !!document.querySelector('.theme-studio-scroll');"
            )
            if ready:
                break
            time.sleep(0.1)

        metrics = browser.evaluate(MEASURE) or {}

        # ---- (d) what Gecko computes for the root and for an inner scroller --
        root = metrics.get("html") or {}
        note(
            f"html: scrollbar-width={root.get('scrollbarWidth')} "
            f"scrollbar-color={root.get('scrollbarColor')}"
        )
        note(f"@supports (-moz-appearance: none) = {metrics.get('supportsMozAppearance')}")

        widest = 0
        over: list[str] = []
        for row in metrics.get("scrollers") or []:
            widest = max(widest, row.get("scrollbarPx") or 0)
            note(
                f"scrollport #{row.get('selector')}: {px(row.get('scrollbarPx') or 0)} "
                f"(offset {row.get('offsetWidth')} - client {row.get('clientWidth')}; "
                f"content {row.get('scrollHeight')} in {row.get('clientHeight')}px, "
                f"{row.get('children')} children; "
                f"scrollbar-width={row.get('scrollbarWidth')}, "
                f"scrollbar-color={row.get('scrollbarColor')}, "
                f"gutter={row.get('gutter')}, overflowing={row.get('overflowing')})"
            )
            if (row.get("scrollbarPx") or 0) > MAX_SCROLLBAR_PX:
                over.append(f"#{row.get('selector')}={(row.get('scrollbarPx'))}px")
        check(
            f"every inner scrollport reserves <= {MAX_SCROLLBAR_PX}px for its scrollbar (a)",
            not over,
            f"widest {px(widest)}; over budget: {', '.join(over)}" if over else f"widest {px(widest)}",
        )

        # ---- (b) placeholder opacity ---------------------------------------
        tokens = metrics.get("tokens") or {}
        note(
            f"tokens: --text-subtle={tokens.get('textSubtle')} --faint={tokens.get('faint')} "
            f"--line-strong={tokens.get('lineStrong')}"
        )
        for row in metrics.get("placeholders") or []:
            note(
                f"placeholder #{row.get('selector')}: opacity={row.get('opacity')} "
                f"colour={row.get('color')} effective={row.get('visibleOpacity'):.2f}"
            )
        bad = [
            row.get("selector")
            for row in metrics.get("placeholders") or []
            # `.theme-studio ::placeholder { opacity: .85 }` is the one deliberate
            # local dim in the app (its fields sit on the drawer's brighter
            # surface); it is a design choice, not the UA's dimming.
            if row.get("selector") != "theme-input"
            and abs(float(row.get("opacity") or 0) - 1.0) > 0.001
        ]
        check(
            "every placeholder the app styles paints at opacity 1 in Gecko (b)",
            not bad,
            f"not 1: {', '.join(str(name) for name in bad)}" if bad
            else f"{len(metrics.get('placeholders') or [])} placeholder(s) measured "
            f"(theme-input keeps its documented 0.85)",
        )

        # ---- (c) the select keeps an affordance ----------------------------
        for row in metrics.get("selects") or []:
            note(
                f"select #{row.get('selector')}: height={row.get('height')}px "
                f"padding-right={row.get('paddingRight')} appearance={row.get('appearance')} "
                f"background-image={row.get('backgroundImage')[:120]} "
                f"position={row.get('backgroundPosition')}"
            )
        stripped = [
            row.get("selector")
            for row in metrics.get("selects") or []
            if not row.get("backgroundImage") or row.get("backgroundImage") == "none"
        ]
        check(
            "the appearance:none select paints a caret of its own (c)",
            bool(metrics.get("selects")) and not stripped,
            f"no background-image on: {', '.join(str(name) for name in stripped)}" if stripped
            else f"{len(metrics.get('selects') or [])} select(s) carry a background-image",
        )

        # ---- F3: the Theme Studio scroll region ----------------------------
        studio = next(
            (row for row in metrics.get("scrollers") or [] if row.get("selector") == "ts-scroll"),
            {},
        )
        check(
            "the Theme Studio scroll region is thin and themed like the rest",
            studio.get("scrollbarWidth") == "thin"
            and "auto" not in str(studio.get("scrollbarColor"))
            and (studio.get("scrollbarPx") or 0) <= MAX_SCROLLBAR_PX,
            f"scrollbar-width={studio.get('scrollbarWidth')}, "
            f"scrollbar-color={studio.get('scrollbarColor')}, {px(studio.get('scrollbarPx') or 0)}",
        )
        check(
            "the Theme Studio gutter matches the thin bar (no 16px reservation)",
            abs((studio.get("scrollbarPx") or 0) - (root.get("scrollbarPx") or 0)) <= MAX_SCROLLBAR_PX,
            f"studio {px(studio.get('scrollbarPx') or 0)}",
        )

        # ---- F11: the goals disclosure draws the app's marker --------------
        details = metrics.get("details") or {}
        note(
            f"goals summary: display={details.get('display')} "
            f"list-style-type={details.get('listStyleType')} "
            f"::before content={details.get('markerContent')} "
            f"marker colour={details.get('markerColor')}"
        )
        check(
            "the goals disclosure drops the UA marker and draws its own",
            details.get("listStyleType") == "none"
            and details.get("markerContent") not in (None, "none", "normal", ""),
            f"list-style-type={details.get('listStyleType')}, "
            f"::before={details.get('markerContent')}",
        )

        # ---- F5: the sticky header inside the table wrap --------------------
        sticky = browser.evaluate(STICKY) or {}
        note(
            f"artifact table: border-collapse={ (metrics.get('table') or {}).get('borderCollapse') } "
            f"spacing={ (metrics.get('table') or {}).get('borderSpacing') } "
            f"rows={ (metrics.get('table') or {}).get('bodyRows') } "
            f"(fixture appended {(metrics.get('table') or {}).get('fixtureRows')}, "
            f"tbody found={(metrics.get('table') or {}).get('fixtureTbody')}) "
            f"th position={ (metrics.get('table') or {}).get('thPosition') } "
            f"after scrollTop={sticky.get('scrolled')}: th top {sticky.get('thTop')} "
            f"vs wrap top {sticky.get('wrapTop')} (offset {px(sticky.get('stuckOffsetPx') or 0)}), "
            f"border-bottom {sticky.get('borderBottom')}"
        )
        check(
            "the sticky header stays pinned to the wrap in Gecko",
            ((metrics.get("table") or {}).get("thPosition") in ("sticky", "-webkit-sticky"))
            and abs(int(sticky.get("stuckOffsetPx") if sticky.get("stuckOffsetPx") is not None else 99)) <= 1,
            f"offset {px(sticky.get('stuckOffsetPx') or 0)} "
            f"after scrolling {px(sticky.get('scrolled') or 0)}",
        )

        # ---- F9: the focus ring inside a scrollport -------------------------
        measured_rings = []
        for starter in ("before-nav", "before-palette"):
            browser.evaluate(f"var el = document.getElementById('{starter}'); if (el) el.focus(); return true;")
            # Reset per starter: a result must belong to the starter this loop just
            # walked, never to the previous one.
            found = None
            # Tab once per hop: Gecko makes a scroll container itself focusable
            # (`overflow: auto` + keyboard scrolling), so the first Tab lands on the
            # port and the second on its first row.
            for _ in range(3):
                browser.keys(["\ue004"])
                found = browser.evaluate(RING) or {}
                if found.get("focused") not in ("session-nav", "palette-list", "table-wrap"):
                    break
            # Only a row counts as a measured ring: a scrollport has no ring to
            # judge, and recording it would pass the check on the wrong element.
            if found and found.get("rowTop") is not None:
                measured_rings.append(found)
        for found in measured_rings:
            note(
                f"focus ring on {found.get('focused')} (focus-visible={found.get('focusVisible')}): "
                f"outline {found.get('outlineStyle')} {px(found.get('outlineWidth') or 0)} "
                f"offset {px(found.get('outlineOffset') or 0)}; "
                f"row top {found.get('rowTop')} / port {found.get('portTop')}, "
                f"row left {found.get('rowLeft')} / port {found.get('portLeft')}, "
                f"clipped top={found.get('clippedTop')} left={found.get('clippedLeft')}"
            )
        clipped = [
            found.get("focused")
            for found in measured_rings
            if found.get("clippedTop") or found.get("clippedLeft") or found.get("clippedRight")
        ]
        check(
            "the focus ring of a list row stays inside its scrollport",
            bool(measured_rings)
            and all(found.get("focusVisible") for found in measured_rings)
            and not clipped,
            f"clipped on: {', '.join(str(name) for name in clipped)}" if clipped
            else f"{len(measured_rings)} row(s) measured",
        )

    finally:
        if browser is not None:
            if args.screenshot is not None:
                # Take it before the session dies: the fixture is only reachable
                # while the driver is alive.
                args.screenshot.write_bytes(b"")
                size = browser.screenshot(args.screenshot)
                print(f"  screenshot: {args.screenshot} ({size} bytes)", flush=True)
            browser.quit()
        server.shutdown()
        # Stop only the driver this run started, by its own process handle: a
        # `pkill -f 'geckodriver --port N'` would also match (and kill) a driver a
        # concurrent run or the reader started on that port.
        if gecko is not None and gecko.poll() is None:
            gecko.terminate()
            try:
                gecko.wait(timeout=10)
            except subprocess.TimeoutExpired:
                gecko.kill()

    print(
        f"\n{len(PASS)} passed, {len(FAIL)} failed"
        + (f"; failed: {', '.join(FAIL)}" if FAIL else ""),
        flush=True,
    )
    return 0 if not FAIL else 1


if __name__ == "__main__":
    sys.exit(main())
