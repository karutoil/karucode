#!/usr/bin/env python3
"""Real-Firefox verification of the Workbench: the checks jsdom cannot make.

Every other browser harness in this repo drives Chromium over CDP. That answers
"does the workspace behave in a Blink engine", and nothing about Gecko, which is
where a whole class of *layout* defects lives:

  * **Scrollbars.** Firefox ignores `::-webkit-scrollbar` entirely and lays out
    the platform scrollbar by `scrollbar-width`. `scrollbar-width` is *not* an
    inherited property, so `html { scrollbar-width: thin }` in base.css sizes the
    root's scrollbar and nothing inside it: every inner scroller falls back to
    `auto`, which on this host is a platform-thick bar. jsdom has no layout at
    all, and Chromium's overlay scrollbars measure 0px, so the defect is
    invisible to every harness we have. This script measures
    `offsetWidth - clientWidth` on the real scroll ports.
  * **Placeholder legibility.** Firefox's UA stylesheet paints `::placeholder` at
    54% opacity. No unit test can read `getComputedStyle(el, '::placeholder')`,
    because there is no pseudo-element box without an engine.
  * **Real overflow at phone width.** `scrollWidth` against `innerWidth` at 420px
    is a layout fact, not a stylesheet fact.

So: a disposable server, the real `web/dist`, the scripted provider, and a real
headless Firefox over W3C WebDriver. Nothing about the user's own karucode
instance is touched: the data directory, config, workspace and ports are all
temporary.

Two measured facts about this engine are worth knowing before reading a failure:

  * headless Firefox clamps the content viewport to a floor of 500px, so the
    420px phone check is measured at 500x800 and says so;
  * the platform scrollbar here is 12px, which is what every scroller that does
    not set `scrollbar-width` gets.

Usage:
    python3 scripts/verify_firefox.py [--keep] [--port 4466]

Requires, and does not build:
    target/debug/karucode      (cargo build -p kc-server)
    web/dist/index.html        (cd web && npm run build)
    /tmp/ffpkg/firefox/firefox, /tmp/ffpkg/geckodriver

Exit code is non-zero when any check fails. Instrumentation failures (no
`web/dist`, no Firefox, no binary) exit non-zero with the instruction instead of
a check line. A CDP-free, stdlib-only script: urllib speaks W3C WebDriver.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIREFOX_BIN = Path("/tmp/ffpkg/firefox/firefox")
GECKODRIVER_BIN = Path("/tmp/ffpkg/geckodriver")
SHOT_DIR = Path("/tmp/ffpkg")

# Headless Gecko will not lay the content viewport out narrower than this, so a
# phone-width check below it measures the floor and says which width it really got.
WIDTH_FLOOR = 500

PASS: list[str] = []
FAIL: list[str] = []

# A scrollbar wider than this is the platform default rather than a chosen one.
MAX_SCROLLBAR_PX = 10

# geckodriver's own lines start with a millisecond timestamp; the browser's
# stdout (`devtools.console.stdout.content`) does not.
GECKO_OWN_LINE = re.compile(r"^\d{10,}\t")
# A page exception names the URL and line it came from.
JS_EXCEPTION = re.compile(r"^JavaScript error: (?P<url>\S+?), line (?P<line>\d+): (?P<message>.*)$")
# `console.error` is also how Firefox's own modules (Nimbus experiments, the
# updater) report; only the in-page collector can tell the page's calls apart.
CONSOLE_ERROR = re.compile(r"^console\.error: ?(?P<message>.*)$")


# --------------------------------------------------------------------------
# house helpers, same shape as the other scripts/verify_*.py
# --------------------------------------------------------------------------


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def port_in_use(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def wait_for(url: str, timeout: float = 45.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.15)
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
# W3C WebDriver over plain urllib
# --------------------------------------------------------------------------


class WebDriverError(RuntimeError):
    pass


class Firefox:
    """One headless Firefox driven over the W3C WebDriver HTTP protocol."""

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
        self.capabilities = created.get("capabilities", {})

    # -- transport ---------------------------------------------------------

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
        # W3C errors come back as `{"value": {"error": ...}}`, sometimes at 200.
        if isinstance(value, dict) and "error" in value:
            raise WebDriverError(f"{method} {path}: {value}")
        return value if value is not None else {}

    # -- commands ----------------------------------------------------------

    def navigate(self, url: str) -> None:
        self._request("POST", f"/session/{self.session_id}/url", {"url": url})

    def evaluate(self, script: str):
        return self._request(
            "POST",
            f"/session/{self.session_id}/execute/sync",
            {"script": script, "args": []},
        )

    def set_window(self, width: int, height: int) -> dict:
        return self._request(
            "POST",
            f"/session/{self.session_id}/window/rect",
            {"width": width, "height": height},
        )

    def actions(self, keys: list[str]) -> None:
        steps = []
        for value in keys:
            steps.append({"type": "keyDown", "value": value})
        for value in reversed(keys):
            steps.append({"type": "keyUp", "value": value})
        self._request(
            "POST",
            f"/session/{self.session_id}/actions",
            {"actions": [{"type": "key", "id": "keyboard", "actions": steps}]},
        )
        self._request("DELETE", f"/session/{self.session_id}/actions")

    def screenshot(self, path: Path) -> int:
        encoded = self._request("GET", f"/session/{self.session_id}/screenshot")
        raw = base64.b64decode(encoded)
        path.write_bytes(raw)
        return len(raw)

    def quit(self) -> None:
        try:
            self._request("DELETE", f"/session/{self.session_id}")
        except Exception:
            pass


# --------------------------------------------------------------------------
# page scripts
# --------------------------------------------------------------------------

COLLECT = r"""
window.__ffErrors = window.__ffErrors || [];
if (!window.__ffCollectorInstalled) {
  window.__ffCollectorInstalled = true;
  var originalError = console.error;
  console.error = function () {
    var parts = [];
    for (var i = 0; i < arguments.length; i += 1) {
      var value = arguments[i];
      parts.push(value && value.message ? value.message : String(value));
    }
    window.__ffErrors.push('console.error: ' + parts.join(' '));
    return originalError.apply(console, arguments);
  };
  window.addEventListener('error', function (event) {
    window.__ffErrors.push('error: ' + String((event && event.message) || 'error'));
  });
  window.addEventListener('unhandledrejection', function (event) {
    var reason = event && event.reason;
    window.__ffErrors.push('rejection: ' + String((reason && reason.message) || reason));
  });
}
return window.__ffErrors.length;
"""

BOOTED = r"""
var field = document.querySelector('#composer-input');
var rail = document.querySelector('[aria-label="Sessions"]') || document.querySelector('.rail');
var shell = rail || document.querySelector('.chat-main') || document.querySelector('.app');
return {
  ready: !!(shell && (field || document.querySelector('.composer'))),
  rail: !!rail,
  field: !!field,
  railClass: rail ? rail.className : null,
  text: (document.body ? document.body.innerText : '').slice(0, 1200),
  title: document.title
};
"""

BASELINE = r"""
return {
  ua: navigator.userAgent,
  platform: navigator.platform,
  dpr: devicePixelRatio,
  locks: typeof navigator.locks,
  abortAny: typeof AbortSignal.any,
  abortTimeout: typeof AbortSignal.timeout,
  scrollbarWidthThin: CSS.supports('scrollbar-width', 'thin'),
  scrollbarGutterStable: CSS.supports('scrollbar-gutter', 'stable'),
  dvh: CSS.supports('height', '100dvh'),
  colorMix: CSS.supports('color', 'color-mix(in oklab, red 50%, blue)'),
  has: CSS.supports('selector(:has(*))'),
  containerQueries: CSS.supports('container-type', 'inline-size'),
  nesting: CSS.supports('selector(& > *)'),
  scrollbarColor: CSS.supports('scrollbar-color', 'red transparent')
};
"""

OPEN_SESSION = r"""
var rows = document.querySelectorAll('.session-row');
for (var i = 0; i < rows.length; i += 1) {
  var text = (rows[i].textContent || '').replace(/\s+/g, ' ');
  if (text.indexOf(TITLE) !== -1) {
    rows[i].click();
    return {clicked: true, rows: rows.length};
  }
}
return {clicked: false, rows: rows.length,
  sample: document.querySelector('[aria-label="Sessions"]') ?
    document.querySelector('[aria-label="Sessions"]').innerText.slice(0, 300) : ''};
"""

SCROLLBARS = r"""
// A scroller with nothing to scroll renders no bar at all, and `scrollbar-width`
// is what decides how wide that bar is once it does appear. So: measure what is
// there, and when the bar is not reserved, force overflow with one temporary
// child (removed synchronously, before this script returns) and measure again.
function barWidth(el) {
  var natural = el.offsetWidth - el.clientWidth;
  if (natural > 0 || el.clientHeight < 8) return {px: natural, forced: false};
  var probe = document.createElement('div');
  probe.setAttribute('aria-hidden', 'true');
  probe.style.cssText = 'height:' + Math.round(el.scrollHeight + el.clientHeight + 400) +
    'px;width:1px;flex:0 0 auto;pointer-events:none;';
  el.appendChild(probe);
  var forced = el.offsetWidth - el.clientWidth;
  el.removeChild(probe);
  return {px: forced > 0 ? forced : natural, forced: forced > 0};
}
var selectors = ['.chat-scroll', '.session-nav', '.palette-list', '.workspace-view',
                 '.command-scroll', '.project-rail-scroll'];
var rows = selectors.map(function (selector) {
  var el = document.querySelector(selector);
  if (!el) return {selector: selector, present: false};
  var style = getComputedStyle(el);
  var measured = barWidth(el);
  return {
    selector: selector,
    present: true,
    scrollbarPx: measured.px,
    forced: measured.forced,
    css: style.scrollbarWidth,
    overflowY: style.overflowY,
    gutter: style.scrollbarGutter,
    clientWidth: el.clientWidth,
    offsetWidth: el.offsetWidth,
    overflowing: el.scrollHeight > el.clientHeight + 1
  };
});
var sweep = [];
var all = document.querySelectorAll('body *');
for (var i = 0; i < all.length && sweep.length < 8; i += 1) {
  var node = all[i];
  var style = getComputedStyle(node);
  if (style.overflowY !== 'auto' && style.overflowY !== 'scroll') continue;
  var measured = barWidth(node);
  if (measured.px <= 0) continue;
  sweep.push({
    cls: String(node.className || node.tagName).slice(0, 48),
    px: measured.px,
    css: style.scrollbarWidth
  });
}
return {root: {css: getComputedStyle(document.documentElement).scrollbarWidth}, rows: rows, sweep: sweep};
"""

PLACEHOLDER = r"""
var out = [];
var fields = document.querySelectorAll('input[placeholder], textarea[placeholder]');
for (var i = 0; i < fields.length && out.length < 4; i += 1) {
  var el = fields[i];
  var rect = el.getBoundingClientRect();
  if (!el.placeholder || rect.width < 4 || rect.height < 4) continue;
  var pseudo = getComputedStyle(el, '::placeholder');
  var own = getComputedStyle(el);
  out.push({
    tag: el.tagName,
    id: el.id,
    cls: String(el.className || '').split(' ')[0],
    placeholder: el.placeholder.slice(0, 40),
    pseudoOpacity: pseudo.opacity,
    pseudoColor: pseudo.color,
    pseudoFontSize: pseudo.fontSize,
    fieldColor: own.color,
    box: {w: Math.round(rect.width), h: Math.round(rect.height)}
  });
}
return out;
"""

LAYOUT = r"""
function box(selector) {
  var el = document.querySelector(selector);
  if (!el) return null;
  var rect = el.getBoundingClientRect();
  var style = getComputedStyle(el);
  return {
    w: Math.round(rect.width * 10) / 10,
    h: Math.round(rect.height * 10) / 10,
    x: Math.round(rect.x * 10) / 10,
    right: Math.round(rect.right * 10) / 10,
    display: style.display,
    visibility: style.visibility
  };
}
var wide = [];
var all = document.querySelectorAll('body *');
for (var i = 0; i < all.length; i += 1) {
  var rect = all[i].getBoundingClientRect();
  if (rect.width > 0 && rect.right > innerWidth + 1) {
    wide.push({cls: String(all[i].className || all[i].tagName).slice(0, 48), right: Math.round(rect.right)});
    if (wide.length >= 5) break;
  }
}
return {
  innerWidth: innerWidth,
  innerHeight: innerHeight,
  docScrollWidth: document.documentElement.scrollWidth,
  bodyScrollWidth: document.body.scrollWidth,
  composer: box('.composer'),
  transcript: box('.chat-scroll'),
  rail: box('[aria-label="Sessions"]'),
  projectRail: box('.project-rail'),
  sessionsToggle: box('.sessions-toggle'),
  offscreen: wide
};
"""


def attribute_errors(lines: list[str], in_page: list[str], base: str) -> tuple[list[str], list[str]]:
    """Split the browser's stdout into page-attributable errors and noise.

    Firefox's own modules (`Nimbus` experiment warnings, the updater) log through
    the same `console.error` channel as the page, and they must not be reported
    as app defects. Two rules keep the attribution honest:

      * an exception counts when the URL it names is ours;
      * a `console.error` counts when the page's own collector recorded it, which
        only the page's console object can reach.

    The console match is an *exact* message comparison, not a substring test: a
    short page message would otherwise match any browser line that happens to
    contain it, and browser noise would be reported as an app defect.
    """
    attributed: list[str] = []
    ignored: list[str] = []
    # `shown` is what the page's console.error received, `text` what was rendered.
    page_messages: set[str] = set()
    for item in in_page:
        rest = item.split(": ", 1)[-1].strip()
        page_messages.add(rest)
        if " | " in rest:
            shown, text = rest.split(" | ", 1)
            page_messages.add(shown.strip())
            page_messages.add(text.strip())
    for line in lines:
        stripped = line.strip()
        if not stripped or GECKO_OWN_LINE.match(line):
            continue
        exception = JS_EXCEPTION.match(stripped)
        if exception:
            url = exception.group("url")
            # Ours means the origin under test, not "some local port". The mock
            # provider and geckodriver both live on 127.0.0.1, so a substring test
            # reported their failures as page defects.
            if url.startswith(base) or url.startswith(base.replace("http://", "https://")):
                attributed.append(stripped[:400])
            else:
                ignored.append(stripped[:200])
            continue
        console = CONSOLE_ERROR.match(stripped)
        if console:
            message = console.group("message").strip()
            if message in page_messages:
                attributed.append(stripped[:400])
            else:
                ignored.append(stripped[:200])
    attributed.extend(f"in-page: {item}" for item in in_page)
    return attributed, ignored


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=4466, help="geckodriver port (default 4466)")
    parser.add_argument("--geckodriver", type=Path, default=GECKODRIVER_BIN)
    parser.add_argument("--firefox", type=Path, default=FIREFOX_BIN)
    parser.add_argument("--keep", action="store_true", help="keep the temporary server, logs and data dir")
    args = parser.parse_args()

    # Instrumentation first, and loudly: these are setup facts, not app defects.
    binary = ROOT / "target/debug/karucode"
    if not binary.exists():
        raise SystemExit(f"missing {binary}; build it first (cargo build -p kc-server)")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("missing web/dist; build the UI first: (cd web && npm run build)")
    if not args.firefox.exists():
        raise SystemExit(f"missing Firefox at {args.firefox}")
    if not args.geckodriver.exists():
        raise SystemExit(f"missing geckodriver at {args.geckodriver}")

    tmp = Path(tempfile.mkdtemp(prefix="kc-firefox-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# firefox fixture\n\nReal bytes for the transcript.\n")
    (workspace / "main.rs").write_text('fn main() {\n    println!("firefox");\n}\n')
    data_dir = tmp / "data"
    data_dir.mkdir()

    shot_dir = SHOT_DIR if SHOT_DIR.is_dir() else tmp
    print(f"artifacts: {tmp}", flush=True)

    # A deterministic provider model, so the session opens onto a real
    # transcript surface instead of the empty state.
    provider_port = free_port()
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
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )

    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                # Top-level keys must precede every `[table]`.
                f'project_roots = ["{workspace}"]',
                "",
                "[model]",
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
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

    server_port = free_port()
    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(binary),
            "--port",
            str(server_port),
            "--workspace",
            str(workspace),
            "--data-dir",
            str(data_dir),
            "--config",
            str(config),
        ],
        env={
            **os.environ,
            "KARUCODE_WEB_DIR": str(ROOT / "web/dist"),
            "KARUCODE_PORT": str(server_port),
            "KARUCODE_LOG": "warn",
        },
        stdout=server_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    gecko_log_path = SHOT_DIR / f"gecko-{args.port}.log" if SHOT_DIR.is_dir() else tmp / "geckodriver.log"
    gecko_owned = False
    gecko: subprocess.Popen | None = None
    if port_in_use(args.port):
        print(f"  reusing the geckodriver already listening on {args.port}", flush=True)
    else:
        gecko_log_path.write_text("")
        gecko_out = open(gecko_log_path, "ab")
        gecko = subprocess.Popen(
            [str(args.geckodriver), "--port", str(args.port)],
            stdout=gecko_out,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        gecko_owned = True

    browser: Firefox | None = None
    base = f"http://127.0.0.1:{server_port}"

    def read_log(from_offset: int) -> tuple[list[str], int]:
        try:
            text = gecko_log_path.read_text(errors="replace")
        except FileNotFoundError:
            return [], from_offset
        return text[from_offset:].splitlines(), len(text)

    try:
        wait_for(f"{base}/api/health", timeout=45)
        wait_for(f"http://127.0.0.1:{provider_port}/v1/models", timeout=20)
        wait_for(f"http://127.0.0.1:{args.port}/status", timeout=20)

        # The server bootstraps the workspace as its project; registering it
        # again is a 400, so read the list and create one session inside it.
        projects = http_json(f"{base}/api/projects")["projects"]
        if not projects:
            raise SystemExit(f"server registered no project for {workspace}; cannot boot the UI")
        project_id = projects[0]["id"]
        title = "Firefox verification"
        created = http_json(f"{base}/api/sessions", "POST", {"title": title, "project": project_id})
        note(f"project {projects[0]['name']} · session {created['id']}")

        browser = Firefox(args.port, args.firefox)

    except BaseException:
        for process in (gecko, server, provider):
            if process is not None and process.poll() is None:
                process.terminate()
        raise

    try:
        browser.set_window(1440, 900)
        log_offset = gecko_log_path.stat().st_size if gecko_log_path.exists() else 0
        browser.navigate(f"{base}/")

        # ---- boot: the shell, or a loud report of what the page says --------
        booted = {"ready": False}
        deadline = time.time() + 45
        while time.time() < deadline:
            try:
                booted = browser.evaluate(BOOTED) or {"ready": False}
            except WebDriverError:
                booted = {"ready": False}
            if booted.get("ready"):
                break
            time.sleep(0.2)
        if not booted.get("ready"):
            boot_lines, _ = read_log(0)
            print("\n-- the Workbench did not boot --", file=sys.stderr)
            print(f"page title: {booted.get('title')!r}", file=sys.stderr)
            print(f"page text: {(booted.get('text') or '(empty)')[:1500]}", file=sys.stderr)
            errors, _noise = attribute_errors(boot_lines, [], base)
            if errors:
                print("page errors:", file=sys.stderr)
                for line in errors[:10]:
                    print(f"  {line}", file=sys.stderr)
            print(f"server log: {tmp / 'server.log'} (tail below)", file=sys.stderr)
            print((tmp / "server.log").read_text(errors="replace")[-2000:], file=sys.stderr)
            check("the Workbench boots in real Firefox", False,
                  f"no #composer-input or Sessions rail after 45s; title={booted.get('title')!r}; "
                  f"page said {(booted.get('text') or '')[:200]!r}")
            raise SystemExit(1)

        check("the Workbench boots in real Firefox", True,
              f"title={booted.get('title')!r}, rail={booted.get('rail')}, composer={booted.get('field')}")

        # Error collectors. A reload during boot would wipe them, so install,
        # verify, and reinstall once if the page replaced itself underneath.
        installed = browser.evaluate(COLLECT)
        if installed is None:
            browser.navigate(f"{base}/")
            time.sleep(1.5)
            installed = browser.evaluate(COLLECT)
        note(f"in-page collector armed (errors so far: {installed})")

        # ---- (a) the Gecko baseline ---------------------------------------
        baseline = browser.evaluate(BASELINE)
        engine = re.search(r"Firefox/(\d+)\.(\d+)", baseline["ua"])
        note(f"user agent: {baseline['ua']}")
        note(
            "gecko: navigator.locks={locks} · AbortSignal.any={abortAny} · AbortSignal.timeout={abortTimeout} · "
            "scrollbar-width:thin={scrollbarWidthThin} · scrollbar-gutter:stable={scrollbarGutterStable} · "
            "dvh={dvh} · color-mix={colorMix} · :has()={has} · @container={containerQueries}".format(**baseline)
        )
        baseline_ok = (
            baseline["locks"] == "object"
            and baseline["abortAny"] == "function"
            and baseline["abortTimeout"] == "function"
            and all(baseline[key] for key in
                    ("scrollbarWidthThin", "scrollbarGutterStable", "dvh", "colorMix", "has", "containerQueries"))
        )
        check(
            "Gecko baseline: navigator.locks, AbortSignal.any, scrollbar-width/gutter, dvh, color-mix, :has(), @container",
            baseline_ok,
            f"Firefox/{engine.group(0).split('/')[1] if engine else '?'} "
            f"locks={baseline['locks']} any={baseline['abortAny']} thin={baseline['scrollbarWidthThin']} "
            f"dvh={baseline['dvh']} color-mix={baseline['colorMix']}",
        )

        # ---- open the session, so the transcript scroll port exists --------
        opened = browser.evaluate("var TITLE = " + json.dumps(title) + ";\n" + OPEN_SESSION)
        deadline = time.time() + 15
        while time.time() < deadline and not (opened or {}).get("clicked"):
            time.sleep(0.2)
            opened = browser.evaluate("var TITLE = " + json.dumps(title) + ";\n" + OPEN_SESSION)
        transcript_ready = False
        deadline = time.time() + 15
        while time.time() < deadline:
            transcript_ready = bool(browser.evaluate("return !!document.querySelector('.chat-scroll');"))
            if transcript_ready:
                break
            time.sleep(0.15)
        check("the created session opens from the rail", bool((opened or {}).get("clicked")) and transcript_ready,
              f"rows={opened.get('rows')} transcript={transcript_ready}")

        # ---- one full interaction: native keys, a send key, the palette ----
        browser.evaluate("var el = document.querySelector('#composer-input'); if (el) el.focus(); return true;")
        focused = browser.evaluate("return document.activeElement && document.activeElement.id;")
        browser.actions(["h", "i"])
        typed = browser.evaluate("var el = document.querySelector('#composer-input'); return el ? el.value : null;")
        check("native key input reaches the composer in Firefox", focused == "composer-input" and typed == "hi",
              f"focus={focused!r} value={typed!r}")
        browser.actions(["\ue003", "\ue003"])  # Backspace twice clears the draft.
        cleared = browser.evaluate("var el = document.querySelector('#composer-input'); return el ? el.value : null;")
        check("native key input reverted the draft (no stuck UI state)", cleared == "", f"value={cleared!r}")

        # Ctrl+K is the palette; it mounts the other scroller worth measuring.
        browser.actions(["\ue009", "k"])
        palette = False
        deadline = time.time() + 5
        while time.time() < deadline:
            palette = bool(browser.evaluate("return !!document.querySelector('.palette-list');"))
            if palette:
                break
            time.sleep(0.15)
        check("Ctrl+K opens the command palette in Firefox", palette,
              "palette-list mounted" if palette else "no .palette-list after Ctrl+K")

        # ---- (c) inner-scroller scrollbar width ---------------------------
        measured = browser.evaluate(SCROLLBARS)
        worst = 0
        note(
            f"documentElement scrollbar-width={measured['root']['css']} — the property does not "
            f"inherit, so every scroller below it computes its own"
        )
        for row in measured["rows"]:
            if not row.get("present"):
                note(f"scrollport {row['selector']}: not in this UI")
                continue
            worst = max(worst, row["scrollbarPx"])
            note(
                f"scrollport {row['selector']}: {row['scrollbarPx']}px "
                f"(scrollbar-width={row['css']}, overflow-y={row['overflowY']}, gutter={row['gutter']}, "
                f"client {row['clientWidth']}/{row['offsetWidth']}px, overflowing={row['overflowing']})"
            )
        for item in measured["sweep"]:
            note(f"every scroller: .{item['cls']} = {item['px']}px (scrollbar-width={item['css']})")
        over = [
            f"{row['selector']}={row['scrollbarPx']}px (scrollbar-width: {row['css']})"
            for row in measured["rows"]
            if row.get("present") and row["scrollbarPx"] > MAX_SCROLLBAR_PX
        ]
        forced = [row["selector"] for row in measured["rows"] if row.get("present") and row.get("forced")]
        check(
            f"every inner scrollport keeps its scrollbar <= {MAX_SCROLLBAR_PX}px in Firefox",
            not over,
            (f"widest {worst}px; over budget: {', '.join(over)}"
             + (f" (forced to overflow to measure: {', '.join(forced)})" if forced else ""))
            if over
            else f"widest {worst}px across {sum(1 for row in measured['rows'] if row.get('present'))} probed scrollports",
        )

        # ---- (d) placeholder legibility ------------------------------------
        placeholders = browser.evaluate(PLACEHOLDER) or []
        if not placeholders:
            check("placeholder legibility is measurable", False, "no visible input/textarea with a placeholder")
        else:
            opacities = []
            for item in placeholders:
                try:
                    opacity = float(item["pseudoOpacity"])
                except (TypeError, ValueError):
                    opacity = -1.0
                opacities.append(opacity)
                label = f"<{item['tag'].lower()}"
                label += f"#{item['id']}" if item["id"] else ""
                label += f".{item['cls']}" if item["cls"] else ""
                note(
                    f"::placeholder on {label}> {item['placeholder']!r}: opacity={item['pseudoOpacity']} "
                    f"color={item['pseudoColor']} ({item['pseudoFontSize']}); field color={item['fieldColor']} "
                    f"({item['box']['w']}x{item['box']['h']}px)"
                )
            # Firefox's UA stylesheet paints ::placeholder at 54% opacity; the app
            # has to say otherwise, and only an engine can tell us whether it did.
            check(
                "no placeholder is left at Firefox's 54% default opacity",
                min(opacities) >= 0.99,
                f"opacities {sorted(opacities)} across {len(placeholders)} field(s)"
                + ("" if min(opacities) >= 0.99 else "; Firefox's default is 0.54"),
            )

        browser.actions(["\ue00c"])  # Escape closes the palette.
        deadline = time.time() + 3
        while time.time() < deadline and browser.evaluate("return !!document.querySelector('.palette-list');"):
            time.sleep(0.1)

        # ---- (e)+(f) layout at three real sizes, screenshot each -----------
        width_floor_checked = False

        def fit(width: int, height: int) -> dict:
            """Set the window and report the viewport Firefox actually gives us.

            `window/rect` is the outer window, so the viewport loses the chrome
            height; correct by the measured delta until it settles. Below
            {@link WIDTH_FLOOR} the engine clamps the content viewport (500px on
            this build), so the caller is told which width was really measured
            instead of the run quietly pretending it reached the one it asked for.
            """
            browser.set_window(width, height)
            time.sleep(0.4)
            probe = browser.evaluate("return {w: innerWidth, h: innerHeight};")
            # The first probe tells us whether this width is reachable at all. A
            # correction below the floor is a no-op that never converges (asking
            # 420 gives a 500px viewport, the correction computes 340, the clamp
            # raises it back to 500), so the loop stops rather than spinning and
            # the caller is told the truth about what it is measuring.
            if probe["w"] > width and width < WIDTH_FLOOR:
                probe["requested"] = width
                probe["clamped"] = True
                return probe
            for _ in range(3):
                if probe["w"] == width and probe["h"] == height:
                    break
                # Never ask for less than the floor: past it every correction is a
                # no-op that only adds waiting, and the delta below would keep
                # subtracting from the requested width.
                browser.set_window(
                    max(WIDTH_FLOOR, width + width - probe["w"]), height + height - probe["h"]
                )
                time.sleep(0.4)
                probe = browser.evaluate("return {w: innerWidth, h: innerHeight};")
            # Say which width the assertions below are really measuring.
            probe["requested"] = width
            probe["clamped"] = probe["w"] > width
            return probe

        for width, height in ((1440, 900), (1024, 700), (420, 800)):
            viewport = fit(width, height)
            requested = f"{width}x{height}"
            measured = f"{viewport['w']}x{viewport['h']}"
            if "clamped" not in viewport:
                viewport["clamped"] = viewport["w"] > width
            clamped = viewport["clamped"]
            if clamped:
                if not width_floor_checked:
                    # Prove the floor rather than assert it: ask for less still.
                    floor = fit(360, height)
                    note(f"viewport width floor: asking 360px gave {floor['w']}px, "
                         f"asking {width}px gave {viewport['w']}px — the engine clamps the "
                         f"content viewport, so the phone layout is measured at its narrowest")
                    width_floor_checked = True
                    viewport = fit(width, height)
                    viewport["clamped"] = True
                    measured = f"{viewport['w']}x{viewport['h']}"
                note(f"{requested} requested, Firefox gave a {measured} viewport "
                     f"(content-width floor / window chrome)")
            layout = browser.evaluate(LAYOUT)
            shot = shot_dir / f"ff-workbench-{width}.png"
            size = browser.screenshot(shot)

            def dim(name: str) -> str:
                box = layout.get(name)
                if not box:
                    return "absent"
                return f"{box['w']}x{box['h']} at x={box['x']}"

            note(
                f"{requested} (viewport {measured}): scrollWidth={layout['docScrollWidth']}, "
                f"composer={dim('composer')}, transcript={dim('transcript')}, rail={dim('rail')}, "
                f"project-rail={dim('projectRail')}, rail-toggle={dim('sessionsToggle')}"
            )
            offscreen = [f".{item['cls']} right={item['right']}" for item in layout["offscreen"]]
            # Name what was measured, not what was asked for: on a clamped width the
            # two differ, and a passing "at 420x800" that really ran at 500x800 is a
            # check claiming coverage it does not have.
            label = measured if clamped else requested
            check(
                f"no horizontal overflow at {label}",
                layout["docScrollWidth"] <= layout["innerWidth"] + 1,
                f"scrollWidth={layout['docScrollWidth']} vs viewport={layout['innerWidth']}"
                + (f"; elements drawn past the edge (transformed drawers): {', '.join(offscreen)}" if offscreen else ""),
            )

            def on_screen(box: dict | None) -> bool:
                return bool(box) and box["w"] > 0 and box["h"] > 0 and box["x"] >= -1 and box["right"] <= layout["innerWidth"] + 1

            # The rail collapses to a drawer (with a toggle) on phone widths;
            # either the rail itself or that toggle must be reachable on screen.
            rail_ok = on_screen(layout["rail"]) or on_screen(layout["sessionsToggle"]) or on_screen(layout["projectRail"])
            check(
                f"composer, transcript and rail are on screen at {label} (viewport {measured})",
                on_screen(layout["composer"]) and on_screen(layout["transcript"]) and rail_ok,
                f"composer={dim('composer')}, transcript={dim('transcript')}, "
                f"rail={dim('rail')}, rail-toggle={dim('sessionsToggle')}",
            )
            check(
                f"screenshot captured at {label} (viewport {measured})",
                shot.exists() and size > 2000,
                f"{shot} ({size} bytes, viewport {measured})",
            )

        browser.set_window(1440, 900)
        time.sleep(0.4)

        # ---- (b) no page exceptions, boot and interaction ------------------
        error_lines, _ = read_log(log_offset)
        browser_errors = browser.evaluate("return window.__ffErrors || [];")
        attributed, noise = attribute_errors(error_lines, browser_errors, base)
        for line in attributed[:10]:
            note(f"page error: {line}")
        for line in noise[:5]:
            note(f"ignored (Firefox's own logging): {line}")
        check(
            "no page exception or console.error through boot and a full interaction",
            not attributed,
            f"{len(attributed)} page error line(s); {len(noise)} browser-internal line(s) ignored"
            if attributed else
            (f"clean: {len(error_lines)} browser output line(s) scanned, "
             f"{len(noise)} of them Firefox-internal"),
        )

        # The server stayed healthy through all of it.
        check("the server answered /api/health after the browser session",
              bool(http_json(f"{base}/api/health")), "200 OK")
    finally:
        if browser is not None:
            browser.quit()
        for process in (gecko, server, provider):
            if process is not None and process.poll() is None:
                process.terminate()
        for process in (gecko, server, provider):
            if process is not None:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
        server_log.close()
        if not args.keep:
            pass  # The temp dir is small; --keep only documents the intent.
        print(f"\nartifacts: {tmp}\nserver log: {tmp / 'server.log'}\ngeckodriver log: {gecko_log_path}",
              flush=True)

    total = len(PASS) + len(FAIL)
    print(f"\n{len(PASS)}/{total} Firefox checks passed.", flush=True)
    if FAIL:
        for name in FAIL:
            print(f"  FAIL {name}", flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())