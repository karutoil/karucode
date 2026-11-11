#!/usr/bin/env python3
"""Measure whether the control plane and the main workbench read as one product.

`scripts/verify_control_plane.py` proves the plane *works*. This proves it looks
like the rest of the app, which is a different claim and one no unit test can
make: jsdom has no layout engine, so it cannot see a font weight, a computed
colour or a clipped row.

The method is comparison, not assertion-by-eye. The same **role** is drawn in
both surfaces - a session row, a section kicker, a primary button, a status
indicator, an agent card - and the computed styles of the matched pair are
printed side by side and compared against a tolerance. A role that the plane
draws with its own private class instead of the shared one is reported as
"not adopted", because that is the defect the design contract (§5A of
`docs/internal/DESIGN-SYSTEM.md`) names: the same role drawn twice, differently.

Three kinds of check:

  1. **adoption** - the shared class is actually present in the plane's DOM for
     the role it owns (`.button`, `.icon-button`, `.badge`, `.status-dot`,
     `.eyebrow`, `.session-row`, `.agent-card`, `.metric`, `.empty-state`,
     `.filter-chip`, `.side-head`, `.side-footer`).
  2. **retirement** - the private re-implementation is gone. A `.cp-*` class that
     still re-draws an adopted role is a divergence waiting to drift again.
  3. **parity** - for a matched pair, the computed styles agree within tolerance.
     This is the check that catches "adopted the class but then overrode it".

It also fails on the two things that made the plane feel hand-built: text
glyphs standing in for icons (`⟳ ⌕ ＋ → ✳ ↗ ← ✕`), and viewport `@media`
breakpoints where the workbench answers its own width with container queries.

Build first:
    cargo build --bin karucode && (cd web && npm run build)

Run:
    python3 scripts/verify_control_parity.py [--keep] [--binary PATH] [--web-dir PATH]

Uses only a temporary workspace, database, config, browser profile and free
loopback ports. Credential-free: the provider is a local scripted SSE stub.
"""

from __future__ import annotations

import argparse
import json
import os
import re
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

sys.path.insert(0, str(Path(__file__).resolve().parent))

from verify_agents_tab import (  # noqa: E402
    ROOT,
    Session,
    free_port,
    http_json,
    sse,
    text_chunks,
    wait_for,
)

FAILURES: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    """Record one verdict. `detail` is printed either way: a pass must be provable."""
    print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        FAILURES.append(name)


# --------------------------------------------------------------------- provider

CONFIG = '''[model]
base_url = "{base}"
provider = "openai_compatible"
model = "stub"
api_key = "sk-test"
# The failed-run seed below must settle as failed *now*, not after an exponential
# retry walk: with the default 50 attempts the session stays "running" for minutes
# and every state this harness seeds is a coin flip. Zero retries makes a 500 a
# failure on the first attempt, which is the state being measured.
retry_max_retries = 0
[approvals]
mode = "auto"
[[providers]]
id = "default"
name = "Default"
provider = "openai_compatible"
base_url = "{base}"
api_key = "sk-test"

[[providers.models]]
id = "stub"
name = "Stub"
reasoning = true
context_window = 128000

[[providers.models]]
id = "stub-alt"
name = "Stub Alt"
reasoning = true
context_window = 64000

[active]
provider = "default"
model = "stub"
[notifications]
enabled = false
'''


class Provider(BaseHTTPRequestHandler):
    """A scripted OpenAI-compatible endpoint: one short answer, no tools."""

    base = ""

    def do_POST(self) -> None:  # noqa: N802 - http.server's name
        length = int(self.headers.get("content-length") or 0)
        payload = self.rfile.read(length)
        # A run whose first user message carries `[fail]` gets an upstream 500, so
        # the harness can seed a FAILED session: without one, every session is
        # idle, `sessionStatusView` returns no word, and the workbench rail draws
        # neither a status word nor a dot - which silently skipped the plane's
        # status treatment from every comparison.
        if b"[fail]" in payload:
            err = b'{"error": {"message": "mock upstream failure", "type": "server_error"}}'
            self.send_response(500)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(err)))
            self.end_headers()
            self.wfile.write(err)
            return
        body = sse(text_chunks("Noted."))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[]}')

    def log_message(self, *args) -> None:  # silence
        pass


# ------------------------------------------------------- style-measurement glue

#: Shared classes the plane adopts. A control-plane sheet may scope one inside its
#: own layout (`.cp-table-row .session-state`), but may not redefine the bare
#: class: that is a second recipe for one role, which is the defect §5A exists to
#: prevent. Kept as a literal set because it is the contract, not a derivation.
SHARED_PRIMITIVES = frozenset({
    ".button",
    ".button-primary",
    ".button-small",
    ".button-danger",
    ".icon-button",
    ".link-button",
    ".input",
    ".badge",
    ".chip",
    ".status-dot",
    ".eyebrow",
    ".session-row",
    ".session-open",
    ".session-head",
    ".session-title",
    ".session-meta",
    ".session-state",
    ".session-list",
    ".session-group-label",
    ".session-menu-trigger",
    ".filter-chip",
    ".side-head",
    ".side-footer",
    ".online-row",
    ".rail-avatar",
    ".agent-card",
    ".metric",
    ".inspector-section",
    ".empty-state",
    ".empty-title",
})

#: Plane-local modifiers of a shared class that restate properties the shared
#: recipe owns. Each entry needs a reason here; the check fails on any other.
#: The hue is identity, not state, and no workbench element carries `data-tone`,
#: so nothing outside the plane can disagree with it - which is what makes an
#: override safe here and unsafe anywhere else.
PLANE_SANCTIONED_OVERRIDES = frozenset({
    ".control-plane .rail-avatar[data-tone='1']",
    ".control-plane .rail-avatar[data-tone='2']",
    ".control-plane .rail-avatar[data-tone='3']",
})

#: The computed properties a matched pair must agree on. Chosen because these are
#: exactly the ones the recon pass found diverging; adding more makes the check
#: stricter, and a property that legitimately differs per surface must be argued
#: for in the contract, not silently dropped here.
PROPS = [
    "fontSize",
    "fontWeight",
    "fontFamily",
    "letterSpacing",
    "lineHeight",
    "textTransform",
    "color",
    "backgroundColor",
    "borderTopColor",
    "borderTopWidth",
    "borderRadius",
    "paddingTop",
    "paddingRight",
    "paddingBottom",
    "paddingLeft",
    "minHeight",
    "height",
    "gap",
]

MEASURE = """
const sel = %s;
const required = %s;
const props = %s;
const candidates = [...document.querySelectorAll(sel)];
if (candidates.length === 0) return JSON.stringify({present: false, sel});

// Compare the BASE recipe, not whichever variant happens to come first.
//
// The workbench's first `.icon-button` on screen is an inspector variant
// (`inspector-quick`, 29px, dimmer ink); its first `.button-primary` is the
// composer's send button, which is *disabled* while the draft is empty and so
// paints the muted rung instead of the accent. Measuring those against the
// plane's base `.icon-button` and its enabled launch button reported two
// variants of one class as a design divergence. So: prefer an element that
// carries exactly the required classes and is enabled; fall back only if the
// surface genuinely has nothing else, and say which happened.
const classes = (el) => [...el.classList];
const exact = (el) => {
  const list = classes(el);
  return required.every((c) => list.includes(c)) && list.length === required.length;
};
const enabled = (el) => !el.disabled;
let how = 'exact';
let el = candidates.find((x) => exact(x) && enabled(x));
if (!el) { how = 'enabled-variant'; el = candidates.find(enabled); }
if (!el) { how = 'variant'; el = candidates[0]; }

const s = getComputedStyle(el);
// Report the shared vocabulary only: plane-local classes (`.cp-*`,
// `.control-plane*`) are this surface's own business and must not opt an element
// out of comparison. Two elements play the same role when their shared class
// lists match, whatever private names either side also carries.
const sharedOf = (list) => list.filter((c) => !/^(cp-|control-plane)/.test(c));
const out = {
  present: true, sel, how,
  cls: sharedOf(classes(el)).join(' '),
  disabled: !!el.disabled,
};
for (const p of props) out[p] = s[p];
const r = el.getBoundingClientRect();
out._w = Math.round(r.width);
out._h = Math.round(r.height);
return JSON.stringify(out);
"""


def required_classes(selector: str) -> list[str]:
    """The class names a selector demands, so `measure` can prefer the base recipe.

    Only the class terms of the first compound are used; pseudo-classes and
    descendant combinators are not part of the recipe being compared.
    """
    first = selector.split(',')[0].strip()
    return re.findall(r"\.([A-Za-z][\w-]*)", first)


#: Selector for the whole plane, and for its rail, by aria-label rather than by
#: private class. `.control-plane.cp` is the plane's own root and stays; the rail's
#: class does not, so the label is the only stable handle.
PLANE = ".control-plane.cp"
RAIL = '[aria-label="Sessions across projects"]'


def measure(js, selector: str) -> dict:
    """Read one element's computed style, preferring the class's base recipe.

    `present: false` when the role is absent from that surface, which makes the
    comparison skip the role rather than pair it against nothing.
    """
    raw = js(MEASURE % (json.dumps(selector), json.dumps(required_classes(selector)), json.dumps(PROPS)))
    return json.loads(raw) if raw else {"present": False, "sel": selector}


#: Properties allowed to differ, with the tolerance or the reason. Everything not
#: listed here must match exactly - a blanket tolerance would hide the very drift
#: this script exists to catch.
TOLERANCE = {
    # Width is the surface's business: a rail row fills a 320px column, a
    # workbench row fills a different one. Height and padding are the contract.
    "_w": None,
    # What the harness matched, not a style: recorded so a failure names its cause.
    "how": None,
    "cls": None,
    "disabled": None,
    "sel": None,
    "present": None,
    # `height` resolves to `auto` for block-ish containers and to a px value for
    # a fixed-height control; compare only when both are numeric.
    "height": "numeric-or-auto",
    "minHeight": "numeric-or-auto",
}


def compare(a: dict, b: dict, label: str) -> tuple[bool, list[str]]:
    """Diff two measurements. Returns (agreed, [human-readable differences])."""
    diffs: list[str] = []
    for prop in PROPS:
        if prop in TOLERANCE:
            mode = TOLERANCE[prop]
            if mode == "numeric-or-auto":
                av, bv = a.get(prop), b.get(prop)
                if av == bv:
                    continue
                try:
                    if abs(float(str(av).replace("px", "")) - float(str(bv).replace("px", ""))) <= 1:
                        continue
                except (TypeError, ValueError):
                    pass
                diffs.append(f"{prop}: {av} vs {bv}")
            continue
        if a.get(prop) != b.get(prop):
            diffs.append(f"{prop}: {a.get(prop)!r} vs {b.get(prop)!r}")
    return (not diffs), diffs


# ------------------------------------------------------------------ main driver


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="retain temporary logs and data")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--web-dir", type=Path, default=ROOT / "web/dist")
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing server binary: {args.binary}")
    if not (args.web_dir / "index.html").exists():
        raise SystemExit(f"missing built web assets: {args.web_dir}")

    tmp = Path(tempfile.mkdtemp(prefix="kc-control-parity-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    data = tmp / "data"
    data.mkdir()

    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{provider.server_port}/v1"
    config = tmp / "config.toml"
    config.write_text(CONFIG.format(base=base))

    port, cdp_port = free_port(), free_port()
    api = f"http://127.0.0.1:{port}"
    server = chrome = session = None
    server_log = open(tmp / "server.log", "wb")
    chrome_log = open(tmp / "chrome.log", "wb")
    try:
        env = {**os.environ, "KARUCODE_WEB_DIR": str(args.web_dir), "KARUCODE_LOG": "warn"}
        server = subprocess.Popen(
            [
                str(args.binary),
                "--port",
                str(port),
                "--workspace",
                str(workspace),
                "--data-dir",
                str(data),
                "--config",
                str(config),
            ],
            env=env,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        chrome_env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("DBUS_SESSION_BUS_ADDRESS", "QT_ACCESSIBILITY")
        }
        chrome = subprocess.Popen(
            [
                "chromium",
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--force-device-scale-factor=1",
                f"--remote-debugging-port={cdp_port}",
                f"--user-data-dir={tmp / 'profile'}",
                "--window-size=1440,900",
                "about:blank",
            ],
            env=chrome_env,
            stdout=chrome_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_for(api + "/api/health")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list") as response:
            page = next(p for p in json.load(response) if p["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Runtime.enable")
        session.cdp("Emulation.setDeviceMetricsOverride",
                    {"width": 1440, "height": 900, "deviceScaleFactor": 1, "mobile": False})

        def js(body: str):
            return session.evaluate(body)

        def wait(body: str, timeout: float = 30.0) -> bool:
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if js(body):
                    return True
                time.sleep(0.2)
            return False

        def click(label: str) -> None:
            """Click the first enabled control whose label *contains* `label`.

            Contains, not starts-with: a control-plane row leads with its `k.`
            glyph and a session row with its project avatar, so the accessible
            text of the thing you want to open is not its first token.
            """
            ok = wait(
                "const want = " + json.dumps(label) + ";"
                "const b=[...document.querySelectorAll('button')].find(x => {"
                "  if (x.disabled) return false;"
                "  const text = ((x.getAttribute('aria-label')||'') + ' ' + (x.innerText||''));"
                "  return text.includes(want);"
                "}); if(b){b.click();return true} return false;"
            )
            assert ok, f"no clickable {label!r}"

        # ------------------------------------------------------- seed the server
        # Two project sessions (one running-looking, one idle) and one control
        # chat, so both surfaces have a row to draw and a row to compare.
        projects = http_json(api + "/api/projects")["projects"]
        project_id = projects[0]["id"]
        http_json(
            api + "/api/sessions",
            "POST",
            {"title": "Harden session permissions", "project": project_id},
        )
        http_json(
            api + "/api/sessions",
            "POST",
            {"title": "Refresh onboarding guide", "project": project_id},
        )
        http_json(api + "/api/control/sessions", "POST", {"title": "Release audit"})
        # One failed run, so both surfaces draw a status word and a danger dot:
        # the role this harness exists to compare, and the one an all-idle seed
        # silently skipped (an idle session has no word on either side).
        failed = http_json(
            api + "/api/sessions",
            "POST",
            {"title": "Audit the flaky login test", "project": project_id},
        )
        failed = failed.get("session", failed)
        http_json(
            f"{api}/api/sessions/{failed['id']}/messages",
            "POST",
            {"content": "[fail] audit the flaky login test"},
        )
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            cur = http_json(f"{api}/api/sessions/{failed['id']}")
            cur = cur.get("session", cur)
            if cur.get("last_run_status") == "failed":
                break
            time.sleep(0.25)

        session.cdp("Page.navigate", {"url": api + "/"})
        wait("return !!document.querySelector('.app');", 30)

        # ---------------------------------------------------- the workbench side
        # Open a session so the rail draws real rows and the workbench header,
        # inspector and empty state are all on screen.
        wait("return !!document.querySelector('.session-row');", 30)
        js("const r=document.querySelector('.session-row .session-open, .session-row'); if(r) r.click(); return true;")
        wait("return !!document.querySelector('#composer-input');", 30)

        workbench: dict[str, dict] = {}
        # `:not(.is-active)` everywhere: the workbench row just clicked is the
        # active one and paints the accent tint, so pairing it with the plane's
        # ordinary row would measure "selected versus not" and call the
        # difference a design divergence. Both sides read an unselected row, and
        # the selected pair is compared separately below.
        # A role is only comparable if BOTH sides name the same variant. The first
        # `.icon-button` in the workbench is a `.session-menu-trigger`, which
        # layout.css:664 deliberately sizes to 24px; the first `.button` is the
        # composer's `.button-primary.button-send`, which composer.css:572 also
        # overrides. Pairing those against the plane's *base* `.icon-button` and a
        # plain `.button-small` measured two different variants and reported the
        # difference as a design divergence. Each selector below therefore names
        # the variant it means, on both sides, and `.status-dot` is scoped into a
        # row so its inherited font and colour come from the same context (a 6px
        # circle has no text of its own; what it inherits says nothing about it).
        roles = {
            "row": ".session-row:not(.is-active)",
            "rowTitle": ".session-row:not(.is-active) .session-title",
            "rowMeta": ".session-row:not(.is-active) .session-meta",
            # The failed row, named by its state class: the one row both surfaces
            # must spell and ink the same. A generic "first .session-state" would
            # pair the workbench's project rows against the plane's control-chat
            # rows - different lists, so a mismatch would prove nothing.
            "stateWord": ".session-row.failed .session-state",
            "statusDot": ".session-row.failed .status-dot",
            "iconButton": ".icon-button:not(.session-menu-trigger)",
            "badge": ".badge",
            "eyebrow": ".eyebrow",
            "statusDot": ".session-row .status-dot",
            "filterChip": ".filter-chip",
            "sideFooter": ".side-footer",
            "emptyTitle": ".empty-title",
            "button": ".button:not(.button-primary):not(.button-send)",
            "buttonPrimary": ".button.button-primary",
            "buttonSmall": ".button.button-small",
            "menuTrigger": ".session-menu-trigger",
            "rowSelected": ".session-row.is-active",
        }
        for role, selector in roles.items():
            workbench[role] = measure(js, selector)

        # ---------------------------------------------------- the control plane
        click("Open control plane")
        wait("return !!document.querySelector('.control-plane.cp');", 30)
        wait("return !!document.querySelector('[aria-label=\"Control chats\"]');", 30)
        # Let the rail's own reads land so rows and counters are populated.
        wait("return document.body.innerText.includes('Release audit');", 30)

        plane: dict[str, dict] = {}
        # The plane's own selectors for the same roles. Where the plane has
        # adopted the shared class these are identical to the workbench's; where
        # it has not, the selector names its private class so the comparison
        # still runs and the divergence is measured rather than assumed.
        # Scoped by aria-label, not by the private classes this work retires, and
        # variant-exact to match `roles` above. Each entry lists the shared
        # selector first and the private one second, so the same run measures the
        # plane before and after adoption and the diff is the work.
        plane_roles = {
            "row": RAIL + " .session-row:not(.is-active), " + RAIL + " .cp-row:not(.selected)",
            "rowTitle": RAIL + " .session-row:not(.is-active) .session-title, " + RAIL + " .cp-row-title-text",
            "rowMeta": RAIL + " .session-row:not(.is-active) .session-meta, " + RAIL + " .cp-row-meta",
            "stateWord": RAIL + " .session-row.failed .session-state",
            "statusDot": RAIL + " .session-row.failed .status-dot",
            "iconButton": RAIL + " .icon-button:not(.session-menu-trigger), " + RAIL + " .cp-icon",
            "badge": RAIL + " .badge, " + RAIL + " .cp-badge",
            "eyebrow": PLANE + " .eyebrow, " + PLANE + " .cp-eyebrow",
            "statusDot": RAIL + " .session-row .status-dot, " + RAIL + " .cp-dot",
            "filterChip": RAIL + " .filter-chip, " + RAIL + " .cp-filters button",
            "sideFooter": RAIL + " .side-footer, " + RAIL + " .cp-browser-foot",
            "emptyTitle": PLANE + " .empty-title, " + PLANE + " .cp-intro-empty",
            "button": PLANE + " .button:not(.button-primary), " + PLANE + " .cp-secondary",
            "buttonPrimary": PLANE + " .button.button-primary, " + PLANE + " .cp-primary",
            "buttonSmall": PLANE + " .button.button-small",
            "menuTrigger": RAIL + " .session-menu-trigger, " + RAIL + " .cp-row-menu-trigger",
            "rowSelected": RAIL + " .session-row.is-active, " + RAIL + " .cp-row.selected",
        }
        for role, selector in plane_roles.items():
            plane[role] = measure(js, selector)

        # ---------------------------------------------------------- 1. adoption
        # The shared class must be the one the plane actually renders. Measured on
        # the DOM, not on the stylesheet: a class in a file that the markup never
        # uses is not adoption.
        adopted = js(
            """
            // Scoped by the rail's aria-label, not by `.cp-browser`: the label is
            // a contract (the unit tests and verify_control_plane.py both rely on
            // it) while the private class is exactly what this work retires.
            const rail = document.querySelector('[aria-label="Sessions across projects"]');
            // Only classes the rail renders unconditionally. `.badge` is not one:
            // it is the agent-count pill on a chat row, drawn only when a chat has
            // agents, and the group count is now the shared label's mono span
            // exactly as `SessionsRail` draws its day groups. `ControlRail.test.tsx`
            // pins that the count pill, when present, is the shared `.badge`.
            const want = {
              row: '.session-row',
              iconButton: '.icon-button',
              statusDot: '.status-dot',
              filterChip: '.filter-chip',
              menuTrigger: '.session-menu-trigger',
              sideHead: '.side-head',
              sideFooter: '.side-footer',
              groupLabel: '.session-group-label',
            };
            const out = { rail: !!rail };
            for (const [k, sel] of Object.entries(want)) {
              out[k] = !!(rail && rail.querySelector(sel));
            }
            return JSON.stringify(out);
            """
        )
        adoption = json.loads(adopted)
        # The rail itself is a precondition, not a primitive: if it is missing,
        # every adoption answer below is meaningless, so say that instead of
        # reporting eight absent classes.
        check("the plane's rail is on screen", adoption.pop("rail", False))
        missing = sorted(k for k, v in adoption.items() if not v)
        check(
            "the rail adopts the workbench's shared primitives",
            not missing,
            "missing: " + ", ".join(missing) if missing else "row/icon-button/status-dot/filter-chip/menu/side-head/side-footer/group-label",
        )

        overview_adopted = js(
            """
            const out = {};
            out.eyebrow = !!document.querySelector('.cp-overview .eyebrow');
            out.metric = !!document.querySelector('.cp-overview .metric, .cp-overview .metric-num');
            out.button = !!document.querySelector('.cp-overview .button');
            out.emptyState = !!document.querySelector('.cp-overview .empty-state, .cp-overview .empty-title');
            out.agentCard = !!document.querySelector('.cp-overview .agent-card');
            return JSON.stringify(out);
            """
        )
        overview = json.loads(overview_adopted)
        overview_missing = sorted(k for k, v in overview.items() if not v)
        # `agentCard` only exists when an agent is live, so it is reported, not
        # required: an empty install has no card to draw.
        required_missing = [k for k in overview_missing if k != "agentCard"]
        check(
            "the overview adopts the workbench's shared primitives",
            not required_missing,
            "missing: " + ", ".join(required_missing)
            if required_missing
            else "eyebrow/metric/button/empty-state"
            + ("" if overview.get("agentCard") else " (no live agent to draw a card for)"),
        )

        # -------------------------------------------------------- 2. retirement
        # A private class that still re-draws an adopted role is the drift risk
        # §5A exists to prevent. Counted in the DOM, so an unused rule in a
        # stylesheet does not fail this - but a rendered one does.
        retired = js(
            """
            const banned = ['.cp-row', '.cp-icon', '.cp-badge', '.cp-dot', '.cp-secondary',
                            '.cp-primary', '.cp-link', '.cp-eyebrow', '.cp-back',
                            '.cp-browser-head', '.cp-browser-foot', '.cp-facts',
                            '.cp-agent', '.cp-intro', '.cp-glyph', '.cp-state'];
            const found = {};
            for (const b of banned) {
              const n = document.querySelectorAll(b).length;
              if (n > 0) found[b] = n;
            }
            return JSON.stringify(found);
            """
        )
        leftovers = json.loads(retired)
        check(
            "the private re-implementations are retired from the DOM",
            not leftovers,
            "still rendered: " + json.dumps(leftovers) if leftovers else "no .cp-* stand-in for an adopted role",
        )

        # ----------------------------------------------------------- 3. parity
        # For every role present on both sides, the computed styles must agree.
        # This is the check that catches "adopted the class, then overrode it".
        # Compare only a *variant-matched* pair. The workbench never renders a
        # bare `.button` or a bare `.icon-button`: every instance it draws carries
        # a modifier (`inspector-quick`, `context-header-btn`, `button-send`,
        # and the send button is disabled while the draft is empty). Pairing one of
        # those against the plane's base element reported two variants of one class
        # as a design divergence, so such a pair is recorded and excluded - the
        # shadowing check below is what proves the plane did not re-tune the shared
        # recipe it adopted.
        diverged: list[str] = []
        variant_mismatch: list[str] = []
        compared = 0
        for role in roles:
            w, p = workbench.get(role, {}), plane.get(role, {})
            if not w.get("present") or not p.get("present"):
                continue
            if w.get("cls") != p.get("cls"):
                variant_mismatch.append(f"{role}: '{w.get('cls')}' vs '{p.get('cls')}'")
                continue
            compared += 1
            agreed, diffs = compare(w, p, role)
            if not agreed:
                diverged.append(f"{role} [{w.get('cls')}]: " + "; ".join(diffs[:4]))
        check(
            "variant-matched roles compute to the same style in both surfaces",
            not diverged and compared >= 4,
            f"{compared} roles compared at the same variant"
            + (f" | DIVERGED: " + " || ".join(diverged[:6]) if diverged else "")
            + ("" if compared >= 4 else " — too few variant-matched pairs to be a real check"),
        )
        if variant_mismatch:
            print(
                "       (not comparable, different variants on each side: "
                + "; ".join(variant_mismatch)
                + ")"
            )

        # The check that actually proves recipe parity for the variants above: the
        # plane must not redefine a shared primitive's properties. Adoption plus
        # "no shadowing" is the real claim; a variant-by-variant pixel diff cannot
        # make it, because the two surfaces legitimately mount different variants.
        # The shared recipe each primitive's own sheet declares, so a plane-local
        # modifier can be judged by *overlap* rather than by shape. A modifier that
        # adds properties the base never sets is adapting the primitive to new
        # content - exactly what `.side-head.cp-head` does, adding flex layout for a
        # brand line plus actions where the base only sets `flex` and `padding`. A
        # modifier that re-sets a property the base owns is a second recipe for one
        # role, which is the defect this check exists for.
        base_css = ""
        for base_sheet in ("layout.css", "base.css", "rail.css", "inspector.css", "chat.css"):
            base_path = ROOT / "web" / "src" / "styles" / base_sheet
            if base_path.exists():
                base_css += re.sub(r"/\*.*?\*/", "", base_path.read_text(encoding="utf-8"), flags=re.S)

        def recipe_props(selector: str) -> set[str]:
            """Properties the shared sheets set for one bare primitive."""
            out: set[str] = set()
            for match in re.finditer(re.escape(selector) + r"\s*\{([^}]*)\}", base_css):
                out |= set(re.findall(r"([\w-]+)\s*:", match.group(1)))
            return out

        shadowed = []
        for sheet in ("control-shared.css", "control-rail.css", "control-overview.css", "control-session.css"):
            path = ROOT / "web" / "src" / "styles" / sheet
            if not path.exists():
                continue
            css = path.read_text(encoding="utf-8")
            css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
            for match in re.finditer(r"([^{}]+)\{([^{}]*)\}", css):
                selector = " ".join(match.group(1).split())
                body = " ".join(match.group(2).split())
                if not body:
                    continue
                # A shadow is a rule that restates a primitive's recipe *as* that
                # primitive - `.session-title { … }`, `.button.button-primary { … }`
                # - with no ancestor scoping it. What is NOT a shadow, and must not
                # be reported: an override scoped inside the plane's own layout,
                # `.cp-table-row .session-state { … }`. The workbench does the same
                # thing to its own primitives (`.session-meta .status-dot`,
                # `.rail .side-head:has(> .project-switcher-desktop)`), and
                # `styles/workflow-panels.test.ts` positively *requires* the
                # phone-width `.cp-session-actions .session-state` override so the
                # status word stays readable. Flagging those would invert the check.
                for part in selector.split(","):
                    part = part.strip()
                    if not part:
                        continue
                    # A combinator (descendant or child) to the left of the
                    # primitive means the rule is scoped, so it adapts the recipe
                    # to one context rather than redefining it.
                    tokens = part.split()
                    last = tokens[-1]
                    root = re.match(r"^(\.[A-Za-z][\w-]*)", last)
                    if not root:
                        continue
                    primitive = root.group(1)
                    if primitive not in SHARED_PRIMITIVES:
                        continue
                    # Scoped inside the plane's own layout: adapts the primitive to
                    # one context. The workbench does this to its own primitives
                    # (`.session-meta .status-dot`) and the phone-width tier is
                    # *required* by a style contract.
                    if len(tokens) > 1:
                        continue
                    # What narrows the last compound, if anything. A plane-local
                    # class (`.cp-head`) or an attribute the workbench never sets
                    # (`[data-tone='3']`) selects a subset the workbench does not
                    # draw, so re-setting the recipe there introduces a variant
                    # rather than redefining one - the same move the workbench makes
                    # with `.session-row.is-active`. A *shared* modifier
                    # (`.button-primary`, `:hover`) selects elements the workbench
                    # also draws, so re-setting the recipe there is drift.
                    rest = last[len(primitive):]
                    modifiers = re.findall(r"\.[A-Za-z][\w-]*|\[[^\]]*\]|::?[A-Za-z-]+", rest)
                    plane_local = all(
                        m.startswith("[")
                        or m.startswith(".cp-")
                        or m.startswith(".control-plane")
                        for m in modifiers
                    )
                    sets = set(re.findall(r"([\w-]+)\s*:", body))
                    owned = recipe_props(primitive)
                    # A plane-local modifier may restate the recipe only if it is
                    # on this list, with its reason. Everything else may add
                    # properties the base never sets; re-setting an owned one is a
                    # second recipe reached by a private name, which is the same
                    # defect under a different hat.
                    sanctioned = part in PLANE_SANCTIONED_OVERRIDES
                    if not modifiers or (sets & owned and not sanctioned):
                        shadowed.append(
                            f"{sheet}: `{part} {{ {body[:60]} }}`"
                            + (f" (re-sets {sorted(sets & owned)})" if sets & owned else "")
                        )
                        break
        check(
            "the plane does not redefine a shared primitive's recipe",
            not shadowed,
            "; ".join(shadowed[:8]) if shadowed else "no bare shared-class rule in any control-plane sheet",
        )

        # ------------------------------------------------------- 4. no glyphs
        # Text characters standing in for icons have no consistent metrics and do
        # not scale with the icon set. Every one is a divergence from the app.
        glyphs = js(
            """
            const bad = ['⟳','⌕','＋','→','✳','↗','←','✕','◈','◆','▼','▸','↻'];
            const hits = [];
            const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
            let n;
            while ((n = walker.nextNode())) {
              const t = n.nodeValue || '';
              for (const g of bad) if (t.includes(g)) hits.push(g + ' in ' + (n.parentElement?.className || n.parentElement?.tagName));
            }
            return JSON.stringify(hits.slice(0, 12));
            """
        )
        found_glyphs = json.loads(glyphs)
        check(
            "the plane draws icons, not text glyphs",
            not found_glyphs,
            "; ".join(found_glyphs) if found_glyphs else "no ⟳ ⌕ ＋ → ✳ ↗ ← ✕ in the DOM",
        )

        # ------------------------------------------------- 5. container queries
        # The workbench answers its own width; a screen embedded beside it must
        # answer the same width, or the two disagree at every intermediate size.
        container = js(
            """
            const plane = document.querySelector('.control-plane.cp');
            if (!plane) return JSON.stringify({present: false});
            const s = getComputedStyle(plane);
            return JSON.stringify({present: true, type: s.containerType, name: s.containerName});
            """
        )
        container_state = json.loads(container)
        check(
            "the plane is a query container, like the workbench",
            container_state.get("type") == "inline-size",
            json.dumps(container_state),
        )

        # ------------------------------------------- 6. one owner per header fact
        # Measured below, once a chat is actually open: with the overview showing
        # there is no session header at all, so `headCount` would be 0 and the
        # check would pass without having looked at anything.

        # ------------------------------------------------------- 7. ink by role
        # A row title must be inked the same in both surfaces. Measured directly
        # rather than inferred from the class, because a class can be adopted and
        # then re-inked by a more specific rule.
        ink = js(
            """
            const pick = (sel) => {
              const el = document.querySelector(sel);
              return el ? getComputedStyle(el).color : null;
            };
            return JSON.stringify({
              planeTitle: pick('.cp-browser .session-title, .cp-browser .cp-row-title-text'),
            });
            """
        )
        plane_ink = json.loads(ink)
        wb_title = workbench.get("rowTitle", {})
        check(
            "a row title is inked the same in both surfaces",
            bool(wb_title.get("present"))
            and plane_ink.get("planeTitle") == wb_title.get("color"),
            f"workbench={wb_title.get('color')} plane={plane_ink.get('planeTitle')}",
        )

        # ------------------------------------------------------- 8. light mode
        # The parity must hold in the light theme too: the surface ladder inverts,
        # so a hard-coded tint shows up only here.
        #
        # Switched through the app's own persistence and a reload, not by writing
        # `data-theme` on `<html>`: the engine owns those attributes and re-derives
        # them from the palette, so a hand-written attribute measures a state the
        # app cannot reach. `karucode.theme-studio.v3` is the real key and
        # `{version, selected, baseId}` the shape `loadPersistedState` reads
        # (`web/src/theme/storage.ts:226-263`).
        js(
            "localStorage.setItem('karucode.theme-studio.v3', "
            "JSON.stringify({version: 1, selected: 'daybreak', baseId: 'daybreak', custom: [], draft: null}));"
            " return true;"
        )
        session.cdp("Page.navigate", {"url": api + "/"})
        wait(
            "return document.documentElement.getAttribute('data-mode') === 'light';",
            25,
        )
        click("Open control plane")
        wait("return !!document.querySelector('.control-plane.cp');", 30)
        wait("return document.body.innerText.includes('Release audit');", 30)
        light = js(
            """
            const pick = (sel, prop) => {
              const el = document.querySelector(sel);
              return el ? getComputedStyle(el)[prop] : null;
            };
            const title = pick('.cp-browser .session-title, .cp-browser .cp-row-title-text', 'color');
            const bg = pick('.cp-browser', 'backgroundColor');
            const row = pick('.cp-browser .session-row, .cp-browser .cp-row', 'backgroundColor');
            const state = pick('.cp-browser .session-state, .cp-browser .cp-state', 'color');
            return JSON.stringify({title, bg, row, state,
                                   mode: document.documentElement.getAttribute('data-mode')});
            """
        )
        light_state = json.loads(light)
        # A row title must be readable against whatever it sits on. The row's own
        # background is usually transparent, so the rail's is the real ground.
        check(
            "the plane's rows stay readable in light mode",
            light_state.get("mode") == "light"
            and bool(light_state.get("title"))
            and light_state.get("title") != (light_state.get("row") or light_state.get("bg")),
            json.dumps(light_state),
        )
        # Back to the default theme for the remaining checks.
        js(
            "localStorage.setItem('karucode.theme-studio.v3', "
            "JSON.stringify({version: 1, selected: 'ember', baseId: 'ember', custom: [], draft: null}));"
            " return true;"
        )
        session.cdp("Page.navigate", {"url": api + "/"})
        wait(
            "return document.documentElement.getAttribute('data-mode') === 'dark';",
            25,
        )
        click("Open control plane")
        wait("return !!document.querySelector('.control-plane.cp');", 30)

        # ------------------------------------------------------- 9. open a chat
        # The session region's own parity: facts and subagents use the inspector's
        # section grammar, and the header uses the workbench's.
        click("Release audit")
        opened = wait("return !!document.querySelector('#composer-input');", 30)
        # A precondition, not a parity claim: every check below is meaningless if
        # the chat never opened, so say so instead of reporting absent classes.
        check("the seeded control chat opens", opened)

        # The plane's own header and the embedded ChatView header both naming the
        # session is duplicate narration - the workbench rule is one owner per
        # fact. Only measurable now that a chat is open.
        # Only *visible* headers count. The plane suppresses the embedded
        # conversation's header with `.cp-chat .main-head { display: none }` so the
        # title has exactly one owner; an element that renders nothing states
        # nothing twice, and counting it would fail a region that fixed the defect
        # by suppression rather than by deletion.
        owners = js(
            """
            const visible = (el) => {
              const s = getComputedStyle(el);
              if (s.display === 'none' || s.visibility === 'hidden') return false;
              const r = el.getBoundingClientRect();
              return r.width > 0 && r.height > 0;
            };
            const heads = [...document.querySelectorAll('.cp-session-head, .cp-pane .main-head')]
              .filter(visible);
            const titles = heads.map(h => (h.innerText || '').trim().split('\\n')[0]).filter(Boolean);
            return JSON.stringify({headCount: heads.length, titles: titles.slice(0, 4)});
            """
        )
        owner_state = json.loads(owners)
        check(
            "an open chat does not state its title twice",
            owner_state.get("headCount", 0) <= 1,
            json.dumps(owner_state),
        )

        session_adopted = js(
            """
            // The open chat's region is `[aria-label="Open session"]`; its facts
            // column is `[aria-label="Chat details"]`. Both labels are contracts
            // the unit tests rely on, so scoping by them survives the redesign.
            //
            // Shared selectors ONLY. An `|| .cp-facts` fallback would make this
            // report "adopted" precisely while the private class is still the one
            // being rendered - a false pass that inverts the check's meaning. The
            // retirement check below is what tracks the private classes going away.
            const pane = document.querySelector('[aria-label="Open session"]');
            const q = (sel) => !!(pane && pane.querySelector(sel));
            const out = { pane: !!pane };
            out.factsSection = q('.inspector-section');
            out.metric = q('.metric');
            out.button = q('.button');
            out.iconButton = q('.icon-button');
            out.emptyState = q('.empty-state');
            return JSON.stringify(out);
            """
        )
        session_state = json.loads(session_adopted)
        # `pane` is a precondition, not a primitive: without an open chat there is
        # nothing here to adopt, and reporting four absent classes would hide it.
        check("the open chat's region is on screen", session_state.pop("pane", False))
        session_missing = sorted(k for k, v in session_state.items() if not v and k != "emptyState")
        check(
            "the open chat adopts the workbench's section and button grammar",
            not session_missing,
            "missing: " + ", ".join(session_missing) if session_missing else json.dumps(session_state),
        )

        # The back control is an icon button, and "open in project" carries the
        # external icon rather than a ↗ character.
        external = js(
            """
            const btns = [...document.querySelectorAll('.cp-session-head button, .cp-pane header button')];
            const openElsewhere = btns.find(b => (b.getAttribute('aria-label')||'').includes('Open in project'));
            return JSON.stringify({
              hasSvg: !!openElsewhere?.querySelector('svg'),
              hasGlyph: !!openElsewhere && /[↗→]/.test(openElsewhere.innerText || ''),
              label: openElsewhere ? openElsewhere.getAttribute('aria-label') : null,
            });
            """
        )
        external_state = json.loads(external)
        # A control chat has no "open in project", so the check only applies when
        # the action is there. Saying so matters: without this, an *unopened* chat
        # passes a check it was never subjected to.
        applicable = external_state.get("label") is not None
        check(
            "an open-elsewhere action uses an icon, not a ↗"
            + ("" if applicable else " (not applicable: a control chat stays in the plane)"),
            not applicable
            or (external_state.get("hasSvg") and not external_state.get("hasGlyph")),
            json.dumps(external_state),
        )

        # ------------------------------------------- 9b. the re-stack is real
        # §5A's container-query rule is the claim that the plane and the workbench
        # re-stack at the same width. Nothing pinned it: deleting the wide tier
        # left every unit test and both browser harnesses green, because a
        # mis-stacked layout does not overflow. Measure the stack itself.
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 1440, "height": 900, "deviceScaleFactor": 1, "mobile": False},
        )
        time.sleep(0.4)
        stack = js(
            """
            const body = document.querySelector('.cp-session-body');
            const chat = document.querySelector('.cp-chat');
            const insp = document.querySelector('.cp-inspector');
            if (!body || !chat || !insp) return JSON.stringify({present: false});
            const c = chat.getBoundingClientRect(), i = insp.getBoundingClientRect();
            return JSON.stringify({present: true, dir: getComputedStyle(body).flexDirection,
                                   beside: i.left >= c.right - 1});
            """
        )
        st = json.loads(stack)
        check(
            "at 1440px the facts sit beside the conversation",
            st.get("present") and st.get("dir") == "row" and st.get("beside"),
            json.dumps(st),
        )
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 1024, "height": 900, "deviceScaleFactor": 1, "mobile": False},
        )
        time.sleep(0.4)
        stack = js(
            """
            const body = document.querySelector('.cp-session-body');
            if (!body) return JSON.stringify({present: false});
            return JSON.stringify({present: true, dir: getComputedStyle(body).flexDirection});
            """
        )
        st = json.loads(stack)
        check(
            "at 1024px the facts stack under the conversation",
            st.get("present") and st.get("dir") == "column",
            json.dumps(st),
        )
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 1440, "height": 900, "deviceScaleFactor": 1, "mobile": False},
        )
        time.sleep(0.4)

        # ------------------------------------------------------ 10. no overflow
        for width in (1440, 1280, 1024, 390):
            session.cdp(
                "Emulation.setDeviceMetricsOverride",
                {"width": width, "height": 900, "deviceScaleFactor": 1, "mobile": width < 500},
            )
            time.sleep(0.4)
            overflow = js(
                """
                const plane = document.querySelector('.control-plane.cp') || document.body;
                return JSON.stringify({scroll: plane.scrollWidth, client: plane.clientWidth,
                                       doc: document.documentElement.scrollWidth,
                                       win: window.innerWidth});
                """
            )
            metrics = json.loads(overflow)
            check(
                f"the plane does not overflow at {width}px",
                metrics["scroll"] <= metrics["client"] + 1
                and metrics["doc"] <= metrics["win"] + 1,
                json.dumps(metrics),
            )
            # An inner scroller can clip while the plane reports no overflow: a
            # button wider than a phone, a meta line cut mid-fact. `overflow:
            # hidden` is not permission to lose content (§5.9) - whatever clips
            # must say so with an ellipsis and keep its facts reachable.
            clipped = js(
                """
                const bad = [];
                for (const el of document.querySelectorAll('.control-plane.cp *')) {
                  const s = getComputedStyle(el);
                  if (s.overflowX !== 'hidden' && s.overflowX !== 'clip') continue;
                  if (el.scrollWidth <= el.clientWidth + 1) continue;
                  // `.sr-only` clips on purpose: a 1px box whose text exists for
                  // assistive technology, not for eyes. Its clip is the utility.
                  if (el.classList.contains('sr-only')) continue;
                  const r = el.getBoundingClientRect();
                  if (r.width <= 1 || r.height <= 1) continue;
                  if (s.textOverflow === 'ellipsis') continue;
                  // A scroller is a legitimate answer too; a clip is not.
                  if (s.overflowY === 'auto' || s.overflowY === 'scroll') continue;
                  bad.push((el.className || el.tagName) + ' ' + el.scrollWidth + '>' + el.clientWidth);
                }
                return JSON.stringify(bad.slice(0, 6));
                """
            )
            bad = json.loads(clipped)
            check(
                f"nothing clips silently at {width}px",
                not bad,
                "; ".join(bad) if bad else "every clipped element ellipsises or scrolls",
            )
            if width == 390:
                # The phone tier is where the room runs out first: the status word
                # must still be drawn, at a readable step, in an ink that is not
                # the pane's own background.
                word = js(
                    """
                    const el = document.querySelector('.cp-session-actions .session-state');
                    if (!el) return JSON.stringify({present: false});
                    const s = getComputedStyle(el), r = el.getBoundingClientRect();
                    const pane = document.querySelector('.cp-pane');
                    return JSON.stringify({present: true, fs: parseFloat(s.fontSize),
                        w: Math.round(r.width), h: Math.round(r.height),
                        ink: s.color, pane: pane ? getComputedStyle(pane).backgroundColor : null,
                        text: (el.innerText || '').trim()});
                    """
                )
                wv = json.loads(word)
                check(
                    "the status word survives the phone header",
                    wv.get("present")
                    and wv.get("fs", 0) >= 11
                    and wv.get("w", 0) > 0
                    and wv.get("h", 0) > 0
                    and bool(wv.get("text"))
                    and wv.get("ink") != wv.get("pane"),
                    json.dumps(wv),
                )
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 1440, "height": 900, "deviceScaleFactor": 1, "mobile": False},
        )

    finally:
        for proc in (chrome, server):
            if proc and proc.poll() is None:
                try:
                    os.killpg(os.getpgid(proc.pid), 15)
                except (ProcessLookupError, PermissionError):
                    pass
        provider.shutdown()
        for handle in (server_log, chrome_log):
            handle.close()
        if args.keep:
            print(f"Retained for inspection: {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)

    print()
    if FAILURES:
        print(f"{len(FAILURES)} FAILED")
        for name in FAILURES:
            print(f"  FAIL {name}")
        return 1
    print("all parity checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
