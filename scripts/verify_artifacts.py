#!/usr/bin/env python3
"""Prove chat artifacts render in a real Chromium against a real kc-server.

The unit tests parse fences and draw cards from a mocked API; they cannot prove
the thing this feature is *for*: that an HTML mock painted in the transcript is a
real document running in its own origin, that an SVG is drawn inline, that a
Mermaid fence becomes a diagram, that a table is a table, and that a fence which
is still streaming is still just code.

It also checks the parts that are security decisions rather than cosmetics:

  * the frame's document is served by `GET /api/artifacts/<id>` with a route-local
    policy whose origin is opaque (`sandbox allow-scripts`, never
    `allow-same-origin`) and whose network is closed (`connect-src 'none'`, no
    wildcard source);
  * the mock's own inline `<script>` *does* run (it posts a message the page sees),
    which is the whole point of serving a document instead of `srcdoc`;
  * the same POST twice is one stored document (content-addressed).

Build first: `cargo build -p kc-server --bin karucode` and `(cd web && npm run build)`.
Run: python3 scripts/verify_artifacts.py [--keep]

Like `scripts/verify_agents_tab.py`, it uses raw CDP (the chrome-devtools plugin
cannot launch Chromium on this host), a temporary workspace/data dir, free
loopback ports and a throwaway browser profile.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import JS_HELPERS, ROOT, Session, free_port, http_json, wait_for

# ---------------------------------------------------------------------------
# What the model answers
# ---------------------------------------------------------------------------

HTML_MARKER = "ARTIFACT-HTML-OK"
SVG_MARKER = "SVG-OK"

ARTIFACT_ANSWER = """Here are the artifacts you asked for.

```artifact html title="Checkout mock"
<!doctype html><html><head><style>body{font:14px system-ui;background:#ffffff;color:#111827;margin:0;padding:18px}h1{color:#0b7285;font-size:20px}</style></head><body>
<h1 id="mock-marker">ARTIFACT-HTML-OK</h1>
<p>The mock's own stylesheet and script apply.</p>
<script>parent.postMessage('artifact-script-ran:'+document.getElementById('mock-marker').textContent,'*')</script>
</body></html>
```

```artifact svg title="Badge"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40" width="120" height="40"><rect width="120" height="40" rx="8" fill="#0b7285"/><text x="14" y="26" fill="#ffffff" font-size="16">SVG-OK</text></svg>
```

```mermaid title="Flow"
graph TD
  A[Start] --> B{Decision}
  B -->|yes| C[Finish]
  B -->|no| A
```

```artifact csv title="Parts"
part,qty
bolt,4
"nut, hex",7
```

```artifact json title="Payload"
{"ok":true,"items":[1,2,3],"note":"json ok"}
```

```artifact markdown title="Doc"
## Markdown artifact
This is a **rendered** document, not a code block.
```
"""

# Split inside the HTML artifact's script so the stream is observed mid-fence.
SPLIT = ARTIFACT_ANSWER.index("</h1>")

# A second answer, for the unhappy path: a diagram Mermaid cannot parse. The
# unit tests cover the failure state with a mocked renderer; this is the real one.
BROKEN_ANSWER = """Here is that diagram.

```mermaid title="Broken flow"
graph TD
  A[Start --> B{Unclosed
  -->>
```
"""


def newest_user(request: dict) -> str:
    """The newest user turn, as text - that is what scripts the mock."""
    text = ""
    for message in request.get("messages") or []:
        if message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            text = content
        elif isinstance(content, list):
            text = "".join(
                part.get("text", "")
                for part in content
                if isinstance(part, dict) and isinstance(part.get("text"), str)
            )
    return text




class Provider(BaseHTTPRequestHandler):
    """Streams the artifact answer in two chunks, with a pause in the middle.

    The pause is what makes the "no cards while streaming" rule observable: the
    first chunk ends inside a fence, so a renderer that drew artifacts eagerly
    would paint a frame there and then.
    """

    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def _chunk(self, text: str) -> bytes:
        return (
            "data: "
            + json.dumps(
                {
                    "id": "chatcmpl-artifacts",
                    "object": "chat.completion.chunk",
                    "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}],
                }
            )
            + "\n\n"
        ).encode()

    def _finish(self) -> bytes:
        """The final chunk: `finish_reason` plus the usage block a real endpoint sends."""
        return (
            "data: "
            + json.dumps(
                {
                    "id": "chatcmpl-artifacts",
                    "object": "chat.completion.chunk",
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 34, "total_tokens": 46},
                }
            )
            + "\n\n"
        ).encode()

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
        raw = self.rfile.read(length) if length else b"{}"
        try:
            request = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            request = {}
        answer = BROKEN_ANSWER if "BROKEN" in newest_user(request) else ARTIFACT_ANSWER
        if not request.get("stream"):
            body = json.dumps(
                {
                    "id": "chatcmpl-artifacts",
                    "object": "chat.completion",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": answer},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        # A declared length, like every real endpoint: without one the runtime
        # treats the body as truncated and the turn ends with
        # "provider stream ended without a finish_reason".
        pieces = (
            [self._chunk(answer[:SPLIT]), self._chunk(answer[SPLIT:])]
            if answer == ARTIFACT_ANSWER
            else [self._chunk(answer)]
        )
        tail = self._finish() + b"data: [DONE]\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(sum(len(piece) for piece in pieces) + len(tail)))
        self.end_headers()
        for index, piece in enumerate(pieces):
            self.wfile.write(piece)
            self.wfile.flush()
            # The pause is what makes "no cards while streaming" observable.
            if index == 0 and len(pieces) > 1:
                time.sleep(1.5)
        self.wfile.write(tail)
        self.wfile.flush()


# ---------------------------------------------------------------------------
# In-page helpers
# ---------------------------------------------------------------------------

PAGE_HELPERS = (
    JS_HELPERS
    + r"""
window.__kc.artifacts = () => Array.from(document.querySelectorAll('[data-testid="artifact-card"]'));
window.__kc.artifactKind = (index) => {
  const card = window.__kc.artifacts()[index];
  return card ? card.getAttribute('data-kind') : null;
};
window.__kc.card = (kind) => window.__kc.artifacts().find((node) => node.getAttribute('data-kind') === kind) || null;
window.__kc.pressed = (card, label) =>
  Array.from((card || document).querySelectorAll('button')).find(
    (node) => (node.textContent || '').trim() === label,
  );
window.__kc.tool = (card, label) =>
  Array.from((card || document).querySelectorAll('button')).find((node) =>
    (node.getAttribute('aria-label') || '').startsWith(label),
  );
window.__kc.rgb = (value) => {
  const parts = String(value).match(/[\d.]+/g) || [];
  return parts.slice(0, 3).map(Number);
};
window.__kc.luminance = (value) => {
  const channels = window.__kc.rgb(value).map((channel) => {
    const c = channel / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
};
window.__kc.contrast = (a, b) => {
  const la = window.__kc.luminance(a);
  const lb = window.__kc.luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
};
window.__kc.rect = (node) => {
  const box = node.getBoundingClientRect();
  return { x: box.x, y: box.y, width: box.width, height: box.height };
};
"""
)

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    if ok:
        PASS.append(name)
        print(f"ok   {name}")
    else:
        FAIL.append(f"{name}: {detail}" if detail else name)
        print(f"FAIL {name}" + (f"  -> {detail}" if detail else ""))
    return ok



def screenshot(session: "Session", path: Path, clip: dict | None = None) -> None:
    """Save one PNG through CDP, so a person can look at what was measured."""
    params: dict = {"format": "png", "captureBeyondViewport": clip is not None}
    if clip is not None:
        params["clip"] = {**clip, "scale": 1}
    result = session.cdp("Page.captureScreenshot", params)
    path.write_bytes(base64.b64decode(result["data"]))
    print(f"   shot: {path}")

def http_get(url: str) -> tuple[int, dict, bytes]:
    request = urllib.request.Request(url, headers={"X-Karucode-Client": "web"})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            # `dict(headers)` loses the case-insensitivity of HTTP header names,
            # and the checks below read them by their conventional spelling.
            return (
                response.status,
                {key.lower(): value for key, value in response.headers.items()},
                response.read(),
            )
    except urllib.error.HTTPError as error:
        return (
            error.code,
            {key.lower(): value for key, value in error.headers.items()},
            error.read(),
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    parser.add_argument(
        "--shots",
        metavar="DIR",
        help="save PNGs of the transcript (dark and light) for a human to look at",
    )
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="kc-artifacts-"))
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

    server_log = open(tmp / "server.log", "wb")
    server = subprocess.Popen(
        [
            str(ROOT / "target/debug/karucode"),
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
            "--window-size=1440,900",
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
        check("a project exists", bool(projects), json.dumps(projects)[:300])
        project_id = projects[0]["id"]
        created = http_json(
            f"{api}/api/sessions",
            "POST",
            {"title": "Artifacts target", "project": project_id},
        )
        session_id = created.get("id") or (created.get("session") or {}).get("id")
        session_title = (created.get("session") or created).get("title") or "Artifacts target"
        check("session created", bool(session_id), json.dumps(created)[:300])

        # ---- the API half, before the browser is involved -------------------
        html_source = '<!doctype html><html><body><h1>API</h1></body></html>'
        first = http_json(f"{api}/api/artifacts", "POST", {"html": html_source, "theme": "dark"})
        second = http_json(f"{api}/api/artifacts", "POST", {"html": html_source, "theme": "dark"})
        check(
            "posting the same artifact twice is one document",
            first.get("id") == second.get("id") and bool(first.get("id")),
            json.dumps([first, second])[:300],
        )
        check(
            "the id is the documented 32 lowercase hex characters",
            isinstance(first.get("id"), str) and re.fullmatch(r"[0-9a-f]{32}", first["id"]) is not None,
            str(first.get("id")),
        )
        status, headers, body = http_get(f"{api}/api/artifacts/{first.get('id')}")
        policy = headers.get("content-security-policy", "")
        check("the document is served as HTML", status == 200 and headers.get("content-type", "").startswith("text/html"), f"{status} {headers.get('content-type')}")
        check("the served document is the artifact", b"<h1>API</h1>" in body, body[:200].decode(errors="replace"))
        check("its origin is opaque, never same-origin", "sandbox allow-scripts" in policy and "allow-same-origin" not in policy, policy)
        check("its network is closed", "connect-src 'none'" in policy, policy)
        check("no wildcard source is allowed", "*" not in policy, policy)
        check(
            "an unknown id is a 404, so a client re-posts",
            http_get(f"{api}/api/artifacts/{'0' * 32}")[0] == 404,
        )
        check("a malformed id is refused too", http_get(f"{api}/api/artifacts/not-an-id")[0] == 404)

        # ---- the browser half ----------------------------------------------
        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(target for target in targets if target["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Runtime.enable")
        # Catch console errors from the very first frame, before the app boots.
        session.cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            {
                "source": """
                window.__errors = [];
                const original = console.error;
                console.error = (...args) => {
                  window.__errors.push(args.map((a) => String(a)).join(' '));
                  original(...args);
                };
                window.addEventListener('error', (event) => {
                  window.__errors.push('error: ' + (event.message || 'unknown'));
                });
                window.__artifacts = [];
                window.__sources = [];
                window.addEventListener('message', (event) => {
                  const data = event.data;
                  window.__artifacts.push(
                    data && typeof data === 'object' ? JSON.stringify(data) : String(data),
                  );
                  const frame = document.querySelector('.artifact iframe');
                  window.__sources.push(
                    frame ? (event.source === frame.contentWindow ? 'same' : 'other') : 'no-frame',
                  );
                });
                """
            },
        )
        session.cdp("Page.navigate", {"url": f"{api}/"})
        session.evaluate(PAGE_HELPERS)

        opened = session.evaluate(
            f"""
            const row = await window.__kc.until(() =>
              [...document.querySelectorAll('button')].find((b) =>
                (b.textContent || '').includes({json.dumps(session_title)})));
            if (!row) return null;
            row.click();
            await window.__kc.until(() => document.querySelector('#composer-input'), 20000);
            return true;
            """
        )
        check("the session opens with a composer", opened is True)

        streamed = session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'Show me the artifacts, please.';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await window.__kc.sleep(120);
            const send = await window.__kc.until(() =>
              [...document.querySelectorAll('button')].find((b) =>
                /^Send message/i.test(b.getAttribute('aria-label') || '')));
            if (!send) return { error: 'no send button' };
            send.click();

            let streamingWithoutCards = false;
            let midStreamCodeBlocks = 0;
            let sawStreaming = false;
            const deadline = Date.now() + 45000;
            while (Date.now() < deadline) {
              const streaming = document.querySelector('.msg-assistant.is-streaming');
              const cards = window.__kc.artifacts().length;
              if (streaming) {
                sawStreaming = true;
                midStreamCodeBlocks = Math.max(
                  midStreamCodeBlocks,
                  document.querySelectorAll('.msg-assistant .code-block').length,
                );
                if (cards === 0 && midStreamCodeBlocks > 0) streamingWithoutCards = true;
              }
              if (!streaming && cards > 0) break;
              await window.__kc.sleep(80);
            }
            return {
              streamingWithoutCards,
              sawStreaming,
              midStreamCodeBlocks,
              kinds: window.__kc.artifacts().map((c) => c.getAttribute('data-kind')),
              messages: document.querySelectorAll('.msg-assistant').length,
            };
            """
        )
        check("the turn streamed", bool(streamed and streamed.get("sawStreaming")), json.dumps(streamed)[:400])
        check(
            "a fence that is still streaming is still code, not a card",
            bool(streamed and streamed.get("streamingWithoutCards")),
            json.dumps(streamed)[:400],
        )
        check(
            "every artifact became a card, in order",
            bool(streamed) and streamed.get("kinds") == ["html", "svg", "mermaid", "csv", "json", "markdown"],
            json.dumps(streamed and streamed.get("kinds")),
        )

        html_card = session.evaluate(
            """
            const card = window.__kc.card('html');
            if (!card) return { error: 'no html card' };
            // The frame is published over `POST /api/artifacts` before it exists,
            // so "no frame yet" is a race, not a failure.
            const frame = await window.__kc.until(() => card.querySelector('iframe'), 15000);
            if (!frame) return { error: 'no frame' };
            // The document reports its own height once it has loaded, which is a
            // separate moment from the frame existing.
            await window.__kc.until(
              () => window.__kc.rect(frame).height < 300,
              8000,
            );
            const box = window.__kc.rect(frame);
            const source = card.querySelector('[data-testid="artifact-source"]');
            return {
              src: frame.getAttribute('src'),
              sandbox: frame.getAttribute('sandbox'),
              title: frame.getAttribute('title'),
              height: Math.round(box.height),
              width: Math.round(box.width),
              hasSource: Boolean(source),
              note: (card.textContent || '').includes('isolated frame'),
              posted: window.__artifacts.slice(),
            };
            """
        )
        check("the HTML artifact has a frame", bool(html_card and html_card.get("src")), json.dumps(html_card)[:300])
        if html_card and html_card.get("src"):
            check(
                "the frame loads the artifact route",
                re.fullmatch(r"/api/artifacts/[0-9a-f]{32}", html_card["src"]) is not None,
                html_card["src"],
            )
            check(
                "the frame is sandboxed without same-origin",
                html_card.get("sandbox") == "allow-scripts",
                str(html_card.get("sandbox")),
            )
            check(
                "the frame is laid out, not collapsed",
                html_card.get("height", 0) >= 80 and html_card.get("width", 0) >= 400,
                json.dumps({k: html_card.get(k) for k in ('height', 'width')}),
            )
            # The mock is about 100px tall; a fixed letterbox would be 420. The
            # document reports its own height, so the card shrinks to it.
            check(
                "the frame is sized to the document, not to a fixed letterbox",
                80 <= html_card.get("height", 0) <= 300,
                str(html_card.get("height")),
            )
            check("the card says the frame is isolated", bool(html_card.get("note")))

        # The mock's own script posts a message; receiving it proves the document
        # is a page (inline script allowed) and that its DOM parsed.
        posted = session.evaluate(
            """
            await window.__kc.sleep(400);
            return window.__artifacts.slice();
            """
        )
        print("   messages seen:", json.dumps(posted)[:600])
        check(
            "the mock's own inline script ran inside the frame",
            any(str(entry).startswith(f"artifact-script-ran:{HTML_MARKER}") for entry in posted),
            json.dumps(posted)[:300],
        )

        others = session.evaluate(
            """
            const svg = window.__kc.card('svg');
            const mermaid = window.__kc.card('mermaid');
            const csv = window.__kc.card('csv');
            const json = window.__kc.card('json');
            const markdown = window.__kc.card('markdown');
            // SVG and diagram output live in a shadow root: an SVG can carry a
            // <style> element whose selectors would otherwise restyle the app.
            const svgNode = svg && svg.querySelector('.artifact-svg')?.shadowRoot?.querySelector('svg');
            const diagram = mermaid && mermaid.querySelector('.artifact-diagram')?.shadowRoot?.querySelector('svg');
            const table = csv && csv.querySelector('table');
            return {
              svgText: svgNode ? svgNode.textContent.trim() : null,
              svgWidth: svgNode ? Math.round(window.__kc.rect(svgNode).width) : 0,
              diagramText: diagram ? (diagram.textContent || '').replace(/\\s+/g, ' ').trim() : null,
              diagramWidth: diagram ? Math.round(window.__kc.rect(diagram).width) : 0,
              tableHeaders: table ? [...table.querySelectorAll('th')].map((th) => th.textContent) : null,
              tableRows: table ? [...table.querySelectorAll('tbody tr')].map((tr) =>
                [...tr.querySelectorAll('td')].map((td) => td.textContent)) : null,
              jsonSource: Boolean(json && json.querySelector('[data-testid="artifact-source"]')),
              markdownHeading: markdown ? (markdown.querySelector('h2') || {}).textContent : null,
              markdownStrong: markdown ? (markdown.querySelector('strong') || {}).textContent : null,
              mermaidKind: mermaid ? mermaid.getAttribute('data-kind') : null,
            };
            """
        )
        check(
            "the SVG artifact is drawn inline",
            bool(others) and others.get("svgText") == SVG_MARKER and others.get("svgWidth", 0) > 40,
            json.dumps(others and {k: others.get(k) for k in ('svgText', 'svgWidth')}),
        )
        check(
            "the Mermaid artifact became a diagram",
            bool(others) and "Start" in (others.get("diagramText") or "") and others.get("diagramWidth", 0) > 100,
            json.dumps(others and {k: others.get(k) for k in ('diagramText', 'diagramWidth')}),
        )
        check(
            "the CSV artifact is a table with its header row",
            bool(others)
            and others.get("tableHeaders") == ["part", "qty"]
            and others.get("tableRows") == [["bolt", "4"], ["nut, hex", "7"]],
            json.dumps(others and {k: others.get(k) for k in ('tableHeaders', 'tableRows')}),
        )
        check(
            "the Markdown artifact is rendered, not quoted",
            bool(others)
            and others.get("markdownHeading") == "Markdown artifact"
            and others.get("markdownStrong") == "rendered",
            json.dumps(others and {k: others.get(k) for k in ('markdownHeading', 'markdownStrong')}),
        )

        json_preview = session.evaluate(
            """
            const card = window.__kc.card('json');
            if (!card) return { error: 'no json card' };
            const preview = window.__kc.pressed(card, 'Preview');
            if (!preview) return { error: 'no preview button' };
            preview.click();
            await window.__kc.sleep(150);
            const drawn = card.querySelector('[data-testid="artifact-json"]');
            return {
              text: drawn ? drawn.textContent.replace(/\\s+/g, ' ').trim() : null,
              pressed: (card.querySelector('[aria-pressed="true"]') || {}).textContent,
            };
            """
        )
        check(
            "the JSON artifact can be previewed, pretty-printed",
            bool(json_preview) and '"ok": true' in (json_preview.get("text") or ""),
            json.dumps(json_preview)[:300],
        )

        toggled = session.evaluate(
            """
            const card = window.__kc.card('html');
            const source = window.__kc.pressed(card, 'Source');
            if (!source) return { error: 'no source button' };
            source.click();
            await window.__kc.sleep(120);
            const showingSource = {
              frame: Boolean(card.querySelector('iframe')),
              text: (card.querySelector('[data-testid="artifact-source"]') || {}).textContent || '',
            };
            window.__kc.pressed(card, 'Preview').click();
            await window.__kc.until(() => card.querySelector('iframe'), 10000);
            return {
              frameWhileSource: showingSource.frame,
              sourceHasMarkup: showingSource.text.includes('mock-marker'),
              frameBack: Boolean(card.querySelector('iframe')),
            };
            """
        )
        check(
            "Source shows the markup and Preview brings the frame back",
            bool(toggled)
            and toggled.get("frameWhileSource") is False
            and toggled.get("sourceHasMarkup") is True
            and toggled.get("frameBack") is True,
            json.dumps(toggled)[:300],
        )

        expanded = session.evaluate(
            """
            const card = window.__kc.card('mermaid');
            const open = window.__kc.tool(card, 'Open this artifact full screen');
            if (!open) return { error: 'no expand button' };
            open.click();
            const dialog = await window.__kc.until(() => document.querySelector('[role="dialog"]'), 5000);
            if (!dialog) return { error: 'no dialog' };
            const box = window.__kc.rect(dialog);
            const host = dialog.querySelector('.artifact-diagram');
            const diagram = await window.__kc.until(
              () => host?.shadowRoot?.querySelector('svg'),
              15000,
            );
            const result = {
              width: Math.round(box.width),
              heading: (dialog.querySelector('.dialog-title') || {}).textContent,
              diagram: Boolean(diagram) ? Math.round(window.__kc.rect(diagram).width) : 0,
            };
            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            await window.__kc.sleep(200);
            result.closed = !document.querySelector('[role="dialog"]');
            return result;
            """
        )
        check(
            "expanding opens a real dialog at full size, and Escape closes it",
            bool(expanded)
            and expanded.get("width", 0) >= 900
            and expanded.get("diagram", 0) > 100
            and expanded.get("closed") is True,
            json.dumps(expanded)[:300],
        )

        if args.shots:
            session.evaluate(
                """
                const card = window.__kc.card('html');
                const open = card && [...card.querySelectorAll('button')].find((b) =>
                  (b.getAttribute('aria-label') || '').startsWith('Open this artifact full screen'));
                if (open) open.click();
                await window.__kc.until(() => document.querySelector('[role="dialog"] iframe'), 10000);
                await window.__kc.sleep(600);
                return 1;
                """
            )
            screenshot(session, Path(args.shots) / "artifacts-expanded.png")
            session.evaluate(
                """
                document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
                await window.__kc.sleep(250);
                return 1;
                """
            )

        # ---- the unhappy path: a diagram that cannot parse -------------------
        failure = session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = 'Show me a BROKEN diagram please.';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            await window.__kc.sleep(120);
            const send = [...document.querySelectorAll('button')].find((b) =>
              /^Send message/i.test(b.getAttribute('aria-label') || ''));
            if (!send) return { error: 'no send button' };
            send.click();
            const node = await window.__kc.until(
              () => document.querySelector('.artifact-failure'),
              30000,
            );
            if (!node) return { error: 'no failure card' };
            node.closest('.artifact').scrollIntoView({ block: 'center' });
            await window.__kc.sleep(300);
            return {
              text: (node.textContent || '').replace(/\\s+/g, ' ').trim(),
              // Mermaid draws its own error graphic unless it is told not to; that
              // must not be left in our document.
              straySvgs: [...document.querySelectorAll('body > svg, body > div > svg')].length,
            };
            """
        )
        check(
            "a diagram that does not parse explains itself",
            bool(failure)
            and not failure.get("error")
            and "could not be rendered" in (failure.get("text") or "")
            and "Parse error" in (failure.get("text") or ""),
            json.dumps(failure)[:400],
        )
        check(
            "and it leaves no stray graphic in the page",
            bool(failure) and failure.get("straySvgs") == 0,
            json.dumps(failure and failure.get("straySvgs")),
        )
        if args.shots:
            shot_dir = Path(args.shots)
            shot_dir.mkdir(parents=True, exist_ok=True)
            screenshot(session, shot_dir / "artifacts-failure.png")

        escaped = session.evaluate(
            """
            const failures = [...document.querySelectorAll('.artifact-failure')];
            const node = failures[failures.length - 1];
            const card = node.closest('.artifact');
            const button = [...node.querySelectorAll('button')].find(
              (b) => (b.textContent || '').trim() === 'Show source',
            );
            if (!button) return { error: 'no way out of the failure' };
            button.click();
            await window.__kc.sleep(200);
            card.scrollIntoView({ block: 'center' });
            await window.__kc.sleep(200);
            return {
              view: card.getAttribute('data-view'),
              source: (card.querySelector('[data-testid="artifact-source"]') || {}).textContent || '',
              failuresLeft: document.querySelectorAll('.artifact-failure').length,
              dialog: Boolean(document.querySelector('[role="dialog"]')),
            };
            """
        )
        check(
            "and its source is one click away, with the failure left behind",
            bool(escaped)
            and escaped.get("view") == "source"
            and "graph TD" in (escaped.get("source") or "")
            and escaped.get("failuresLeft") == 0
            and escaped.get("dialog") is False,
            json.dumps(escaped)[:300],
        )
        if args.shots:
            screenshot(session, Path(args.shots) / "artifacts-source.png")

        contrast = session.evaluate(
            """
            const card = window.__kc.card('html');
            const head = card.querySelector('.artifact-head');
            const body = card.querySelector('.artifact-body');
            const title = card.querySelector('.artifact-title') || card.querySelector('.artifact-kind');
            const style = getComputedStyle(card);
            const headStyle = getComputedStyle(head);
            const bodyStyle = getComputedStyle(body);
            return {
              mode: document.documentElement.dataset.mode,
              theme: document.documentElement.dataset.theme,
              headBg: headStyle.backgroundColor,
              bodyBg: bodyStyle.backgroundColor,
              cardBg: style.backgroundColor,
              titleColor: getComputedStyle(title).color,
              borderColor: headStyle.borderBottomColor,
              headVsBody: window.__kc.contrast(headStyle.backgroundColor, bodyStyle.backgroundColor),
              titleContrast: window.__kc.contrast(getComputedStyle(title).color, headStyle.backgroundColor),
              borderContrast: window.__kc.contrast(headStyle.borderBottomColor, headStyle.backgroundColor),
            };
            """
        )
        check(
            "the card head is distinguishable from its body",
            bool(contrast) and contrast.get("headVsBody", 0) >= 1.04,
            json.dumps(contrast)[:400],
        )
        check(
            "the card's own text is readable",
            bool(contrast) and contrast.get("titleContrast", 0) >= 4.5,
            json.dumps(contrast)[:400],
        )
        check(
            "the card's hairline is visible against its head",
            bool(contrast) and contrast.get("borderContrast", 0) >= 1.06,
            json.dumps(contrast)[:400],
        )
        print("   default theme:", json.dumps(contrast))

        if args.shots:
            shots = Path(args.shots)
            shots.mkdir(parents=True, exist_ok=True)
            for index, name in ((0, "ember-1"), (3, "ember-2")):
                session.evaluate(
                    f"""
                    const cards = window.__kc.artifacts();
                    const card = cards[{index}];
                    if (card) card.scrollIntoView({{ block: 'start' }});
                    await window.__kc.sleep(500);
                    return 1;
                    """
                )
                screenshot(session, shots / f"artifacts-{name}.png")

        # ---- the second theme, so a light card is measured too ---------------
        light = session.evaluate(
            """
            const open = document.querySelector('[aria-label="Open theme studio"]');
            if (!open) return { error: 'no theme studio button' };
            open.click();
            const preset = await window.__kc.until(
              () => document.querySelector('[data-testid="theme-preset-daybreak"]'),
              5000,
            );
            if (!preset) return { error: 'no daybreak preset' };
            preset.click();
            await window.__kc.sleep(300);
            const card = window.__kc.card('html');
            const head = card.querySelector('.artifact-head');
            const body = card.querySelector('.artifact-body');
            const title = card.querySelector('.artifact-title') || card.querySelector('.artifact-kind');
            const headStyle = getComputedStyle(head);
            const bodyStyle = getComputedStyle(body);
            const result = {
              mode: document.documentElement.dataset.mode,
              theme: document.documentElement.dataset.theme,
              headVsBody: window.__kc.contrast(headStyle.backgroundColor, bodyStyle.backgroundColor),
              titleContrast: window.__kc.contrast(getComputedStyle(title).color, headStyle.backgroundColor),
              bodyTextContrast: window.__kc.contrast(
                getComputedStyle(body).color,
                bodyStyle.backgroundColor,
              ),
            };
            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            return result;
            """
        )
        check(
            "the card holds up in the light theme too",
            bool(light)
            and light.get("mode") == "light"
            and light.get("headVsBody", 0) >= 1.02
            and light.get("titleContrast", 0) >= 4.5,
            json.dumps(light)[:400],
        )
        print("   daybreak:", json.dumps(light))

        if args.shots:
            shots = Path(args.shots)
            shots.mkdir(parents=True, exist_ok=True)
            session.evaluate(
                """
                const cards = window.__kc.artifacts();
                if (cards[0]) cards[0].scrollIntoView({ block: 'start' });
                await window.__kc.sleep(500);
                return 1;
                """
            )
            screenshot(session, shots / "artifacts-daybreak.png")

        # ---- layout on a narrow viewport ------------------------------------
        session.cdp(
            "Emulation.setDeviceMetricsOverride",
            {"width": 390, "height": 780, "deviceScaleFactor": 1, "mobile": True},
        )
        session.evaluate("await window.__kc.sleep(400); return 1;")
        narrow = session.evaluate(
            """
            const card = window.__kc.card('html');
            const head = card.querySelector('.artifact-head');
            const box = window.__kc.rect(card);
            return {
              column: document.documentElement.clientWidth,
              cardWidth: Math.round(box.width),
              headWidth: Math.round(window.__kc.rect(head).width),
              overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
              titleVisible: Boolean(card.querySelector('.artifact-title')),
            };
            """
        )
        check(
            "nothing overflows at 390px, and the card still fits",
            bool(narrow)
            and narrow.get("overflow", 99) <= 1
            and narrow.get("cardWidth", 0) <= narrow.get("column", 0) + 1,
            json.dumps(narrow)[:400],
        )
        # A diagram is scaled to fit its card at any width; it must not be cropped
        # or stretched on the way down.
        diagram_fit = session.evaluate(
            """
            const card = window.__kc.card('mermaid');
            const host = card && card.querySelector('.artifact-diagram');
            const svg = host && host.shadowRoot && host.shadowRoot.querySelector('svg');
            if (!svg) return { error: 'no diagram' };
            const box = window.__kc.rect(svg);
            const cardBox = window.__kc.rect(card);
            const viewBox = (svg.getAttribute('viewBox') || '').split(/[ ,]+/).map(Number);
            const ratio = viewBox.length === 4 && viewBox[2] && viewBox[3]
              ? viewBox[2] / viewBox[3]
              : null;
            return {
              width: Math.round(box.width),
              height: Math.round(box.height),
              cardWidth: Math.round(cardBox.width),
              hostHeight: host.clientHeight,
              // A cropped diagram is one the host has to scroll to show.
              scrolls: host.scrollHeight > host.clientHeight + 1 || host.scrollWidth > host.clientWidth + 1,
              ratio: ratio === null ? null : Math.round(ratio * 100) / 100,
              drawn: box.height > 0 ? Math.round((box.width / box.height) * 100) / 100 : null,
            };
            """
        )
        check(
            "a diagram is scaled to fit the card, not cropped",
            bool(diagram_fit)
            and diagram_fit.get("width", 0) <= diagram_fit.get("cardWidth", 0) + 1
            and diagram_fit.get("height", 0) <= diagram_fit.get("hostHeight", 0) + 1
            and diagram_fit.get("scrolls") is False
            and diagram_fit.get("ratio") is not None
            and abs((diagram_fit.get("drawn") or 0) - (diagram_fit.get("ratio") or 0)) < 0.15,
            json.dumps(diagram_fit)[:300],
        )
        if args.shots:
            session.cdp(
                "Emulation.setDeviceMetricsOverride",
                {"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": True},
            )
            session.evaluate(
                """
                const card = window.__kc.card('html');
                if (card) card.scrollIntoView({ block: 'start' });
                await window.__kc.sleep(500);
                return 1;
                """
            )
            screenshot(session, Path(args.shots) / "artifacts-narrow.png")
        session.cdp("Emulation.clearDeviceMetricsOverride")

        errors = session.evaluate("return window.__errors || []")
        check("the page logged no console errors", not errors, json.dumps(errors)[:400])

        reloaded = session.evaluate(
            """
            location.reload();
            await window.__kc.until(() => document.querySelector('#composer-input'), 20000);
            await window.__kc.until(() => window.__kc.artifacts().length >= 6, 20000);
            return {
              kinds: window.__kc.artifacts().map((c) => c.getAttribute('data-kind')),
              frames: document.querySelectorAll('.artifact iframe').length,
              errors: window.__errors || [],
            };
            """
        )
        expected_kinds = list((streamed or {}).get("kinds") or []) + ["mermaid"]
        check(
            "a reload re-renders every artifact from stored messages",
            bool(reloaded)
            and reloaded.get("kinds") == expected_kinds
            and reloaded.get("frames", 0) >= 1,
            json.dumps({"got": (reloaded or {}).get("kinds"), "want": expected_kinds})[:400],
        )
        check("the reload logged no console errors", not (reloaded or {}).get("errors"), json.dumps((reloaded or {}).get("errors"))[:300])

        stored = http_json(f"{api}/api/sessions/{session_id}")["messages"]
        print(
            f"\nstored messages: {len(stored)}; artifacts posted by the browser:",
            "see server.log at",
            tmp / "server.log",
        )
    finally:
        if args.keep:
            print(f"\nkept: server on {port}, profile {profile}, logs {tmp}")
        else:
            if session is not None:
                session.close()
            for process in (chrome, server):
                try:
                    process.terminate()
                    process.wait(timeout=5)
                except Exception:
                    process.kill()
            provider.shutdown()

    print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    for failure in FAIL:
        print(" -", failure)
    return 0 if not FAIL else 1


if __name__ == "__main__":
    raise SystemExit(main())
