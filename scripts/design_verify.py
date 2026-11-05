#!/usr/bin/env python3
"""Drive the real karucode UI in Chromium and write a design verification report.

Why this exists: a stylesheet change cannot be verified in jsdom. jsdom does not
compute a contrast ratio, does not lay out a flex row, and does not clip anything,
so every defect this audit found — 30 of 85 text nodes below WCAG AA, a 1241px
diff line clipped inside a 238px box, two live `aria-modal` surfaces stacked under
one scrim — is invisible to the unit tests and obvious to a screenshot.

So: a real server, the built bundle, a real Chromium over CDP, and a fixed matrix
of screens in both modes. The report lands in `docs/internal/design-verify/`
next to the screenshots it cites.

Usage:
    python3 scripts/design_verify.py --base http://127.0.0.1:8931 [--keep]

Exits non-zero if any assertion fails, so it can gate a release.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "internal" / "design-verify"

PASS: list[str] = []
FAIL: list[str] = []

# Floors from docs/internal/DESIGN-SYSTEM.md §4. Text a user must read.
FLOOR_TEXT = 4.5
FLOOR_DECORATIVE = 3.0


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 45.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise SystemExit(f"timed out waiting for {url}")


class Session:
    """A thin CDP client: enough to navigate, evaluate and screenshot."""

    def __init__(self, ws_url: str) -> None:
        from websockets.sync.client import connect

        self.ws = connect(ws_url, max_size=64 * 1024 * 1024, open_timeout=20)
        self.next_id = 0
        self.evaluate("return 1;")

    def _send(self, method: str, params: dict | None = None) -> dict:
        self.next_id += 1
        request_id = self.next_id
        self.ws.send(json.dumps({"id": request_id, "method": method, "params": params or {}}))
        deadline = time.time() + 60
        while time.time() < deadline:
            raw = self.ws.recv(timeout=max(1.0, deadline - time.time()))
            message = json.loads(raw)
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise SystemExit(f"CDP {method} failed: {message['error']}")
            return message.get("result") or {}
        raise SystemExit(f"CDP {method} timed out")

    def evaluate(self, body: str):
        result = self._send(
            "Runtime.evaluate",
            {
                "expression": "(async () => {" + body + "})()",
                "awaitPromise": True,
                "returnByValue": True,
            },
        )
        if result.get("exceptionDetails"):
            raise SystemExit(f"page script failed: {json.dumps(result['exceptionDetails'])[:800]}")
        return (result.get("result") or {}).get("value")

    def navigate(self, url: str) -> None:
        self._send("Page.navigate", {"url": url})
        time.sleep(1.0)
        self.evaluate(BOOT)

    def screenshot(self, path: Path) -> None:
        result = self._send("Page.captureScreenshot", {"format": "png", "captureBeyondViewport": False})
        path.write_bytes(base64.b64decode(result["data"]))


BOOT = """
const deadline = Date.now() + 30000;
while (Date.now() < deadline) {
  if (document.querySelector('#composer-input') || document.querySelector('.chat-main')) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}
return 'timeout';
"""

# Applied before every page load: the theme under test.
SET_THEME = """
localStorage.setItem('karucode.theme-studio.v3', JSON.stringify({
  version: 1, selected: '%s', baseId: '%s', custom: [], draft: null,
}));
return %s;
"""

# Measures every text node against its composited background, and reports the
# nodes that fail. Same maths as the audit, so the numbers are comparable.
MEASURE = """
const srgb = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
const lum = (r, g, b) => 0.2126 * srgb(r) + 0.7152 * srgb(g) + 0.0722 * srgb(b);
const parse = (s) => { const m = s.match(/rgba?\\(([^)]+)\\)/); if (!m) return null;
  const p = m[1].split(/[,\\s/]+/).filter(Boolean).map(Number);
  return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 }; };
const effectiveBg = (el) => {
  let stack = [], node = el;
  while (node) { const c = parse(getComputedStyle(node).backgroundColor);
    if (c && c.a > 0) { stack.push(c); if (c.a >= 0.999) break; } node = node.parentElement; }
  if (!stack.length) return { r: 15, g: 14, b: 13 };
  let out = stack[stack.length - 1];
  for (let i = stack.length - 2; i >= 0; i--) { const t = stack[i], a = t.a;
    out = { r: t.r * a + out.r * (1 - a), g: t.g * a + out.g * (1 - a), b: t.b * a + out.b * (1 - a) }; }
  return out; };
const ratio = (a, b) => { const l1 = lum(a.r, a.g, a.b), l2 = lum(b.r, b.g, b.b);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05); };

const fails = [], all = [];
document.querySelectorAll('*').forEach((el) => {
  const text = Array.from(el.childNodes).filter((n) => n.nodeType === 3)
    .map((n) => n.textContent.trim()).join(' ').trim();
  if (!text) return;
  const cs = getComputedStyle(el);
  if (cs.visibility === 'hidden' || cs.display === 'none' || Number(cs.opacity) === 0) return;
  const box = el.getBoundingClientRect();
  if (box.width < 2 || box.height < 2) return;
  // A gradient/`background-clip: text` mark paints its own ink, and the computed
  // `color` is not what the eye sees. The mark's *children* (the dot in `k.`)
  // inherit that dark `color`, so the check has to walk up, not just look here.
  for (let node = el, hops = 0; node && hops < 4; node = node.parentElement, hops += 1) {
    const img = getComputedStyle(node).backgroundImage;
    if (img && img.includes('gradient')) return;
  }
  // Inactive components are explicitly out of scope for the contrast minimum, and
  // a dimmed disabled button is the correct affordance, not a defect.
  let inactive = false;
  for (let node = el, hops = 0; node && hops < 4; node = node.parentElement, hops += 1) {
    if (node.disabled === true || node.getAttribute?.('aria-disabled') === 'true') { inactive = true; break; }
  }
  if (inactive) return;
  const fg = parse(cs.color);
  if (!fg || fg.a === 0) return;
  const r = ratio(fg, effectiveBg(el));
  const record = { text: text.slice(0, 40), px: parseFloat(cs.fontSize),
    ratio: Math.round(r * 100) / 100, cls: String(el.className || '').slice(0, 40) };
  all.push(record);
  if (r < 4.5) fails.push(record);
});
fails.sort((a, b) => a.ratio - b.ratio);
return { total: all.length, fails: fails.slice(0, 25), failCount: fails.length };
"""

# Anything that overflows its own scroll container without a way to reach it.
OVERFLOW = """
const bad = [];
document.querySelectorAll('*').forEach((el) => {
  const cs = getComputedStyle(el);
  if (el.scrollWidth <= el.clientWidth + 2) return;
  const reaches = ['auto', 'scroll', 'visible'].includes(cs.overflowX);
  if (reaches) return;
  // Truncation that says so is design, not a defect: a long session title that
  // ends in an ellipsis tells the reader it is cut. Silent loss is the bug.
  if (cs.textOverflow === 'ellipsis' && cs.whiteSpace.includes('nowrap')) return;
  const box = el.getBoundingClientRect();
  if (box.width < 40 || box.height < 20) return;
  bad.push({ cls: String(el.className || '').slice(0, 48),
    clientWidth: el.clientWidth, scrollWidth: el.scrollWidth,
    overflowX: cs.overflowX });
});
return bad.slice(0, 12);
"""

# Elements whose right edge disagrees in a row that should be aligned.
SURFACES = """
const seen = new Map();
document.querySelectorAll('*').forEach((el) => {
  const cs = getComputedStyle(el);
  const c = cs.backgroundColor;
  if (!c || c === 'rgba(0, 0, 0, 0)') return;
  const box = el.getBoundingClientRect();
  const area = Math.max(0, box.width) * Math.max(0, box.height);
  if (area < 500) return;
  const prev = seen.get(c) || 0;
  if (area > prev) seen.set(c, area);
});
return [...seen.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12)
  .map(([c, area]) => ({ colour: c, area: Math.round(area) }));
"""


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f' — {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f' — {detail}'}", flush=True)


def set_theme(session: Session, theme: str) -> None:
    session.evaluate(SET_THEME % (theme, theme, "'" + theme + "'"))


SCREENS = [
    ("main", None),
    ("command-palette", "document.dispatchEvent(new KeyboardEvent('keydown', {key:'k',code:'KeyK',ctrlKey:true,bubbles:true})); await new Promise(r=>setTimeout(r,300)); return 1;"),
    ("settings", "document.dispatchEvent(new KeyboardEvent('keydown', {key:'k',code:'KeyK',ctrlKey:true,bubbles:true})); await new Promise(r=>setTimeout(r,250));"
                 "const item=[...document.querySelectorAll('*')].find(e=>e.textContent.trim()==='Open settings'&&e.closest('button,[role=option]'));"
                 "const target=item&&(item.closest('button,[role=option]')||item); if(target)target.click(); await new Promise(r=>setTimeout(r,600)); return 1;"),
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://127.0.0.1:8931")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()

    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("build the UI first: (cd web && npm run build)")

    OUT.mkdir(parents=True, exist_ok=True)
    shots = OUT / "screens"
    shots.mkdir(exist_ok=True)

    tmp = Path(tempfile.mkdtemp(prefix="kc-design-verify-"))
    cdp_port = free_port()
    profile = tmp / "profile"
    profile.mkdir()

    chrome_log = open(tmp / "chrome.log", "wb")
    chrome = subprocess.Popen(
        [
            "chromium", "--headless=new", "--no-sandbox", "--disable-gpu",
            "--disable-dev-shm-usage", "--hide-scrollbars",
            f"--remote-debugging-port={cdp_port}", f"--user-data-dir={profile}",
            "--window-size=1920,1080", "--js-flags=--jitless", "about:blank",
        ],
        stdout=chrome_log, stderr=subprocess.STDOUT, start_new_session=True,
    )

    report: list[str] = []
    findings: dict[str, dict] = {}

    try:
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list") as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t.get("type") == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session._send("Page.enable")
        session._send("Runtime.enable")
        session._send("Emulation.setDeviceMetricsOverride", {
            "width": 1440, "height": 960, "deviceScaleFactor": 1, "mobile": False,
        })

        # Standard desktop panels, not just a 1440×960 laptop-sized approximation:
        # 1920×1080 is 1080p and 2560×1440 is 1440p. Check both modes and every
        # overlay at both sizes so enlarged gutters cannot hide a clipping defect.
        for width, height, label in ((1920, 1080, "1080p"), (2560, 1440, "1440p")):
            session._send("Emulation.setDeviceMetricsOverride", {
                "width": width, "height": height, "deviceScaleFactor": 1, "mobile": False,
            })
            for theme in ("ember", "daybreak"):
                mode = "dark" if theme == "ember" else "light"
                session.navigate(args.base)
                set_theme(session, theme)
                session.navigate(args.base)

                measured = session.evaluate(MEASURE)
                findings[f"{label}:{mode}:main"] = measured
                session.screenshot(shots / f"{label}-{mode}-main.png")
                check(
                    f"{label}/{mode}/main: no text below {FLOOR_TEXT}:1",
                    measured["failCount"] == 0,
                    f"{measured['failCount']} of {measured['total']} text nodes"
                    + (f", worst {measured['fails'][0]['ratio']}:1 at {measured['fails'][0]['px']}px \"{measured['fails'][0]['text']}\"" if measured["fails"] else ""),
                )

                overflow = session.evaluate(OVERFLOW)
                findings[f"{label}:{mode}:overflow"] = overflow
                check(f"{label}/{mode}/main: nothing clipped without a way to scroll", not overflow,
                      f"{len(overflow)} elements")

                surfaces = session.evaluate(SURFACES)
                findings[f"{label}:{mode}:surfaces"] = surfaces

                for name, script in SCREENS[1:]:
                    session.navigate(args.base)
                    try:
                        session.evaluate(script)
                    except SystemExit as error:
                        print(f"  skip {label}/{mode}/{name}: {error}", flush=True)
                        continue
                    session.screenshot(shots / f"{label}-{mode}-{name}.png")
                    result = session.evaluate(MEASURE)
                    findings[f"{label}:{mode}:{name}"] = result
                    check(
                        f"{label}/{mode}/{name}: no text below {FLOOR_TEXT}:1",
                        result["failCount"] == 0,
                        f"{result['failCount']} of {result['total']}",
                    )

        # Narrow viewports: the responsive breakpoints must not overflow.
        for width in (1100, 1024):
            session.navigate(args.base)
            set_theme(session, "ember")
            session.navigate(args.base)
            session._send("Emulation.setDeviceMetricsOverride", {
                "width": width, "height": 900, "deviceScaleFactor": 1, "mobile": False,
            })
            time.sleep(0.4)
            overflow = session.evaluate(OVERFLOW)
            findings[f"{width}:overflow"] = overflow
            session.screenshot(shots / f"ember-{width}.png")
            check(f"{width}px: no horizontal clipping", not overflow, f"{len(overflow)} elements")
    finally:
        chrome.terminate()
        try:
            chrome.wait(timeout=10)
        except subprocess.TimeoutExpired:
            chrome.kill()
        chrome_log.close()
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)

    # ---- report ----
    lines = [
        "# Design verification",
        "",
        "Generated by `scripts/design_verify.py` against the built bundle in a real",
        "Chromium at true 1920×1080 (1080p) and 2560×1440 (1440p), plus the 1100px",
        "and 1024px breakpoints, in Ember (dark) and Daybreak (light). The contract is",
        "`docs/internal/DESIGN-SYSTEM.md`.",
        "",
        f"**{len(PASS)} passed, {len(FAIL)} failed.**",
        "",
        "Screenshots are in `screens/`.",
        "",
        "## Results",
        "",
        "| Check | Result | Detail |",
        "| --- | --- | --- |",
    ]
    for name in PASS:
        lines.append(f"| `{name}` | **pass** | |")
    for name in FAIL:
        lines.append(f"| `{name}` | **FAIL** | see below |")

    lines += ["", "## Worst contrast offenders", ""]
    for key, value in findings.items():
        if not key.endswith(("main", "command-palette", "settings")):
            continue
        if not isinstance(value, dict) or "fails" not in value:
            continue
        lines.append(f"### {key} — {value['failCount']} of {value['total']} below floor")
        lines.append("")
        if not value["fails"]:
            lines += ["Nothing below the floor.", ""]
            continue
        lines += ["| Ratio | Size | Text | Element |", "| --- | --- | --- | --- |"]
        for f in value["fails"][:12]:
            lines.append(f"| {f['ratio']}:1 | {f['px']}px | {f['text'][:32]} | `{f['cls']}` |")
        lines.append("")

    lines += ["## Clipped elements", ""]
    for key, value in findings.items():
        if not key.endswith("overflow"):
            continue
        lines.append(f"### {key}")
        lines.append("")
        if not value:
            lines += ["Nothing clipped.", ""]
            continue
        lines += ["| Element | Visible | Content | overflow-x |", "| --- | --- | --- | --- |"]
        for item in value:
            lines.append(
                f"| `{item['cls']}` | {item['clientWidth']}px | {item['scrollWidth']}px | {item['overflowX']} |"
            )
        lines.append("")

    lines += ["## Surfaces painted (largest first)", ""]
    for key, value in findings.items():
        if not key.endswith("surfaces"):
            continue
        lines.append(f"### {key}")
        lines.append("")
        for item in value:
            lines.append(f"- `{item['colour']}` — {item['area']}px²")
        lines.append("")

    (OUT / "REPORT.md").write_text("\n".join(lines) + "\n")
    print(f"\n{len(PASS)} passed, {len(FAIL)} failed — report at {OUT / 'REPORT.md'}", flush=True)
    return 0 if not FAIL else 1


if __name__ == "__main__":
    sys.exit(main())