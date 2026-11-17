#!/usr/bin/env python3
"""Drive the modal family in a real browser and report what it measured.

The unit suites can pin a stylesheet string; only a browser can answer "is the
footer inside the dialog", "is there exactly one `aria-modal` surface" and "how
wide is the drawer actually drawn". This script answers those for every dialog
in the product, at a desktop and a phone viewport, and writes a screenshot of
each so the numbers can be checked against a picture.

It is deliberately not wired into CI: it needs `target/debug/karucode`,
`web/dist` and Chromium, and it is a review tool rather than a gate (the repo's
`verify_*` scripts are the same shape). Every run uses a temporary data
directory, workspace, config and browser profile, so it never touches a running
karucode or a real project.

    python3 scripts/verify_modal_family.py            # both viewports
    python3 scripts/verify_modal_family.py --desktop-only
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_ROOT = ROOT / "docs" / "internal" / "design-verify" / "modal-family"

# --------------------------------------------------------------------------- #
# The CDP REPL (the same one `verify_settings_tabs.py` uses: one persistent
# WebSocket, because a device-metrics override issued on a connection that then
# closes is not in force for the next one).
# --------------------------------------------------------------------------- #
CDP_HELPER = r"""
const wsUrl = process.argv[2];
const readline = (await import('node:readline')).createInterface({ input: process.stdin });
const ws = new WebSocket(wsUrl);
const opened = new Promise((resolve) => ws.addEventListener('open', resolve));
let nextId = 0;
const pending = new Map();
ws.addEventListener('message', (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id !== undefined && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) reject(new Error(JSON.stringify(msg.error)));
    else resolve(msg.result);
  }
});
function send(method, params) {
  const requestId = ++nextId;
  return new Promise((resolve, reject) => {
    pending.set(requestId, { resolve, reject });
    ws.send(JSON.stringify({ id: requestId, method, params: params || {} }));
  });
}
readline.on('line', async (line) => {
  if (!line.trim()) return;
  const plan = JSON.parse(line);
  try {
    await opened;
    let value = null;
    if (plan.kind === 'cdp') {
      const result = await send(plan.method, plan.params || {});
      value = result === undefined ? null : result;
    } else {
      const result = await send('Runtime.evaluate', {
        expression: '(async () => {' + plan.body + '})()',
        awaitPromise: true,
        returnByValue: true,
      });
      if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
      value = result.result.value === undefined ? null : result.result.value;
    }
    console.log(JSON.stringify({ ok: value }));
  } catch (error) {
    console.log(JSON.stringify({ error: String(error && error.message ? error.message : error) }));
  }
});
readline.on('close', () => process.exit(0));
"""


class Session:
    def __init__(self, ws_url: str) -> None:
        helper = Path(tempfile.gettempdir()) / "kc-cdp-repl.mjs"
        helper.write_text(CDP_HELPER)
        self.proc = subprocess.Popen(
            ["node", str(helper), ws_url],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.evaluate("return 1;")

    def _send(self, plan: dict) -> object:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(plan) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise SystemExit(f"CDP session died: {self.proc.stderr.read() if self.proc.stderr else ''}")
        payload = json.loads(line)
        if "error" in payload:
            raise SystemExit(f"CDP call failed: {payload['error']}")
        return payload["ok"]

    def evaluate(self, body: str) -> object:
        return self._send({"kind": "evaluate", "body": body})

    def cdp(self, method: str, params: dict | None = None) -> object:
        return self._send({"kind": "cdp", "method": method, "params": params or {}})

    def screenshot(self, path: Path) -> None:
        data = self.cdp("Page.captureScreenshot", {"format": "png"})
        path.write_bytes(__import__("base64").b64decode(data["data"]))

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    (PASS if ok else FAIL).append(name)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{f' — {detail}' if detail else ''}", flush=True)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 45) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2):
                return
        except Exception:
            time.sleep(0.2)
    raise SystemExit(f"timeout waiting for {url}")


# --------------------------------------------------------------------------- #
# Page-side probes. Each returns geometry, so a failure names the number.
# --------------------------------------------------------------------------- #

GEOMETRY = r"""
const layer = document.querySelector('.dialog-layer');
const dialog = document.querySelector('[role=dialog]');
if (!dialog) return { missing: true };
const r = dialog.getBoundingClientRect();
const header = dialog.querySelector('.overlay-header, .dialog-header, .settings-pane-head, .theme-studio-top');
const body = dialog.querySelector('.overlay-body');
const footer = dialog.querySelector('.overlay-footer, .settings-footer, .theme-studio-foot, .dialog-actions');
const h = header ? header.getBoundingClientRect() : null;
const f = footer ? footer.getBoundingClientRect() : null;
return {
  role: dialog.dataset.role || null,
  cls: dialog.className,
  x: Math.round(r.x), y: Math.round(r.y),
  w: Math.round(r.width), h: Math.round(r.height),
  right: Math.round(r.right), bottom: Math.round(r.bottom),
  vw: window.innerWidth, vh: window.innerHeight,
  insetLeft: Math.round(r.x),
  insetRight: Math.round(window.innerWidth - r.right),
  insetTop: Math.round(r.y),
  insetBottom: Math.round(window.innerHeight - r.bottom),
  headerH: h ? Math.round(h.height) : null,
  footerH: f ? Math.round(f.height) : null,
  footerInside: f ? f.bottom <= r.bottom + 0.5 && f.top >= r.top - 0.5 : null,
  bodyScrolls: body ? body.scrollHeight > body.clientHeight + 1 : null,
  bodyOverflow: body ? getComputedStyle(body).overflowY : null,
  ariaModalCount: document.querySelectorAll('[aria-modal="true"]').length,
  veils: document.querySelectorAll('.dialog-backdrop, .theme-shade').length,
  title: (dialog.querySelector('.overlay-title, .dialog-title, .settings-pane-title, .theme-studio-title') || {}).textContent || null,
  titleSize: (() => {
    const t = dialog.querySelector('.overlay-title, .dialog-title');
    return t ? getComputedStyle(t).fontSize : null;
  })(),
};
"""

CLOSE_ALL = r"""
const esc = (el, type) => el.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', bubbles: true }));
esc(document.activeElement || document.body, 'keydown');
for (let i = 0; i < 4; i += 1) {
  const dialog = document.querySelector('[role=dialog]');
  if (!dialog) break;
  const close = dialog.querySelector('[aria-label^="Close"], .overlay-close, .settings-close, .theme-close');
  if (close) close.click();
  else esc(dialog, 'keydown');
  await new Promise((r) => setTimeout(r, 150));
}
return document.querySelectorAll('[role=dialog]').length;
"""

# The palette: Ctrl+K, or the header button on a phone (no chord available).
OPEN_PALETTE = r"""
const open = () => {
  const button = [...document.querySelectorAll('button')].find((b) => /search|palette/i.test((b.getAttribute('aria-label') || '') + b.className));
  if (button) { button.click(); return true; }
  return false;
};
document.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', code: 'KeyK', ctrlKey: true, bubbles: true }));
await new Promise((r) => setTimeout(r, 400));
if (!document.querySelector('.palette-search')) open();
await new Promise((r) => setTimeout(r, 400));
return !!document.querySelector('.palette-search');
"""

OPEN_SETTINGS = r"""
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {
  if (document.querySelector('[role=tablist][aria-label="Settings sections"]')) return true;
  const gear = [...document.querySelectorAll('button')].find((b) => /open settings/i.test(b.getAttribute('aria-label') || ''));
  if (gear) { gear.click(); await new Promise((r) => setTimeout(r, 500)); }
  else await new Promise((r) => setTimeout(r, 200));
}
return false;
"""

OPEN_THEME_STUDIO = r"""
// The studio lives behind "Task actions" in the header menu (the rail's own
// button was removed before this change; the menu is the documented path the
// repo's `verify_appearance_matrix.py` uses too).
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {
  if (document.querySelector('[data-testid=theme-studio]')) return 'opened';
  const menuButton = [...document.querySelectorAll('button')].find((b) => /task actions/i.test(b.getAttribute('aria-label') || ''));
  if (menuButton) {
    menuButton.click();
    await new Promise((r) => setTimeout(r, 350));
    const item = [...document.querySelectorAll('[role=menuitem], button')].find((b) => /theme studio/i.test((b.textContent || '') + (b.getAttribute('aria-label') || '')));
    if (item) {
      item.click();
      await new Promise((r) => setTimeout(r, 700));
      if (document.querySelector('[data-testid=theme-studio]')) return 'opened';
    }
  }
  await new Promise((r) => setTimeout(r, 250));
}
return {
  state: 'not-opened',
  hasMenu: [...document.querySelectorAll('button')].some((b) => /task actions/i.test(b.getAttribute('aria-label') || '')),
  dialogs: [...document.querySelectorAll('[role=dialog]')].map((d) => d.className),
};
"""

# A confirmation: deleting the project the fixture owns. This is the compact
# role — the shape every "are you sure" in the product takes.
OPEN_CONFIRM = r"""
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {
  const kebab = [...document.querySelectorAll('button')].find((b) => /project options|project actions|project menu/i.test(b.getAttribute('aria-label') || ''));
  if (kebab) {
    kebab.click();
    await new Promise((r) => setTimeout(r, 350));
    const remove = [...document.querySelectorAll('[role=menuitem], button')].find((b) => /delete project/i.test((b.textContent || '') + (b.getAttribute('aria-label') || '')));
    if (remove) {
      remove.click();
      await new Promise((r) => setTimeout(r, 500));
      const dialog = document.querySelector('[role=dialog]');
      if (dialog) return dialog.className;
    }
  }
  await new Promise((r) => setTimeout(r, 250));
}
return null;
"""


def probe(session: Session, label: str, shots: Path, width: int) -> dict:
    geometry = session.evaluate(GEOMETRY)
    shot = shots / f"{width}-{label}.png"
    session.screenshot(shot)
    if isinstance(geometry, dict) and geometry.get("missing"):
        check(f"{label} @{width}: a dialog is on screen", False, "none found")
        return {}
    assert isinstance(geometry, dict)
    # Every band, so a band that sits outside its dialog names itself. Visually
    # hidden headings (`.sr-only`, a 1px clipped box) are excluded: they are the
    # accessible name, not a band.
    outside = session.evaluate(
        """
        const dialog = document.querySelector('[role=dialog]');
        const box = dialog.getBoundingClientRect();
        return [...dialog.children]
          .filter((child) => !child.classList.contains('sr-only'))
          .map((child) => {
            const r = child.getBoundingClientRect();
            return {
              cls: child.className.split(' ')[0],
              top: Math.round(r.top - box.top),
              bottom: Math.round(r.bottom - box.top),
              h: Math.round(r.height),
            };
          });
        """
    )
    print(
        f"    {label} @{width}: "
        + json.dumps({k: geometry[k] for k in ("role", "w", "h", "insetLeft", "insetRight", "insetTop", "insetBottom", "headerH", "footerH")})
        + f" bands={[(b['cls'], b['h'], b['top'], b['bottom']) for b in outside]}",
        flush=True,
    )
    # The contract every modal shares.
    check(f"{label} @{width}: one aria-modal surface", geometry["ariaModalCount"] == 1, str(geometry["ariaModalCount"]))
    check(f"{label} @{width}: one veil", geometry["veils"] == 1, str(geometry["veils"]))
    check(
        f"{label} @{width}: inside the viewport",
        geometry["insetLeft"] >= 0 and geometry["insetRight"] >= 0 and geometry["insetTop"] >= 0,
        f"L{geometry['insetLeft']} R{geometry['insetRight']} T{geometry['insetTop']}",
    )
    outside_bands = [
        band for band in outside if band["top"] < -0.5 or band["bottom"] > geometry["h"] + 0.5
    ]
    check(
        f"{label} @{width}: no band outside the dialog",
        not outside_bands,
        f"ok (dialog {geometry['h']}px)" if not outside_bands else str(outside_bands),
    )
    return geometry


def run(width: int, height: int, out: Path) -> None:
    shots = out / "screens"
    shots.mkdir(parents=True, exist_ok=True)

    # The modality matrix: each row opens a dialog and reports its geometry.
    mode = "phone" if width < 640 else "desktop"
    session = None
    tmp = Path(tempfile.mkdtemp(prefix="kc-modal-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# fixture\n")
    data_dir = tmp / "data"
    data_dir.mkdir()
    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                "[model]",
                'base_url = "http://127.0.0.1:9/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
                "",
                "[[providers]]",
                'id = "default"',
                'name = "Default"',
                'provider = "openai_compatible"',
                'base_url = "http://127.0.0.1:9/v1"',
                'api_key = "sk-test"',
                "",
                "[[providers.models]]",
                'id = "gpt-4o-mini"',
                'name = "GPT-4o mini"',
                "",
                "[active]",
                'provider = "default"',
                'model = "gpt-4o-mini"',
                "",
            ]
        )
    )

    port = free_port()
    cdp_port = free_port()
    profile = tmp / "chrome-profile"
    profile.mkdir()

    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(ROOT / "target/debug/karucode"),
            "--port", str(port),
            "--workspace", str(workspace),
            "--data-dir", str(data_dir),
            "--config", str(config),
        ],
        env={**os.environ, "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"), "KARUCODE_LOG": "warn"},
        stdout=server_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    chrome_log = open(tmp / "chrome.log", "wb")
    chrome = subprocess.Popen(
        [
            "chromium", "--headless=new", "--no-sandbox", "--disable-gpu",
            "--disable-dev-shm-usage", "--hide-scrollbars",
            f"--remote-debugging-port={cdp_port}", f"--user-data-dir={profile}",
            f"--window-size={width},{height}", "about:blank",
        ],
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    try:
        wait_for(f"http://127.0.0.1:{port}/api/health")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")
        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Emulation.setDeviceMetricsOverride", {"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 640})
        session.cdp("Page.navigate", {"url": f"http://127.0.0.1:{port}/"})
        session.evaluate("""
          const deadline = Date.now() + 30000;
          while (Date.now() < deadline) {
            if (document.querySelector('.app, .main-head, .composer')) return true;
            await new Promise((r) => setTimeout(r, 200));
          }
          return false;
        """)

        # ---- confirmation (compact) -------------------------------------
        opened = session.evaluate(OPEN_CONFIRM)
        if opened:
            check(f"confirm @{width}: the compact role", "overlay-compact" in str(opened), str(opened)[:120])
            probe(session, "confirm", shots, width)
        else:
            check(f"confirm @{width}: reachable", False, "no project delete path")
        session.evaluate(CLOSE_ALL)

        # ---- command palette -------------------------------------------
        if session.evaluate(OPEN_PALETTE):
            session.evaluate("const i=document.querySelector('.palette-input'); if(i) i.focus(); return true;")
            probe(session, "palette", shots, width)
        else:
            check(f"palette @{width}: reachable", False, "no chord or button")
        session.evaluate(CLOSE_ALL)

        # ---- settings (tall) -------------------------------------------
        if session.evaluate(OPEN_SETTINGS):
            probe(session, "settings", shots, width)
            # Each pane: the header band's height and whether the panel scrolls.
            for tab in ["Appearance", "Models & providers", "Agent", "Plugins & MCP"]:
                opened = session.evaluate(
                    """
                    const label = %s;
                    const tab = [...document.querySelectorAll('.settings-nav-item')].find(
                      (t) => (t.querySelector('.settings-nav-label') || {}).textContent === label,
                    );
                    if (!tab) return false;
                    tab.click();
                    await new Promise((r) => setTimeout(r, 350));
                    return true;
                    """
                    % json.dumps(tab)
                )
                if opened:
                    label = tab.lower().replace(' & ', '-').replace(' ', '-')
                    probe(session, f"settings-{label}", shots, width)
        else:
            check(f"settings @{width}: reachable", False, "no gear button")
        session.evaluate(CLOSE_ALL)

        # ---- theme studio (drawer) -------------------------------------
        theme = session.evaluate(OPEN_THEME_STUDIO)
        if theme == "opened":
            probe(session, "theme-studio", shots, width)
        else:
            check(f"theme studio @{width}: reachable", False, json.dumps(theme)[:600])
        session.evaluate(CLOSE_ALL)
    finally:
        if session:
            session.close()
        for process in (chrome, server):
            try:
                os.killpg(os.getpgid(process.pid), 15)
            except Exception:
                pass
        chrome_log.close()
        server_log.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--desktop-only", action="store_true")
    parser.add_argument("--phone-only", action="store_true")
    args = parser.parse_args()

    if not (ROOT / "target" / "debug" / "karucode").exists():
        raise SystemExit("build the server first: cargo build -p kc-server --bin karucode")
    if not (ROOT / "web" / "dist" / "index.html").exists():
        raise SystemExit("build the UI first: (cd web && npm run build)")

    OUT_ROOT.mkdir(parents=True, exist_ok=True)
    if not args.phone_only:
        print("desktop 1440x900", flush=True)
        run(1440, 900, OUT_ROOT)
    if not args.desktop_only:
        print("phone 390x740", flush=True)
        run(390, 740, OUT_ROOT)

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    for failure in FAIL:
        print(f"  FAILED {failure}")
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
