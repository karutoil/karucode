#!/usr/bin/env python3
"""Drive steering, follow-up and recall in a real Chromium over raw CDP.

What the unit and HTTP tests cannot prove, and this does:

  * while a run is working, the composer *queues* instead of sending - the Send
    button is gone and a Steer/Follow-up toggle takes its place;
  * a message queued with an attachment really lands on the server with the
    rendered body, the draft and the chip (read back from
    `GET /api/sessions/:id/queues`, not from the DOM);
  * the pending panel appears **above** the composer box, with the kind, the draft
    and the file chip;
  * **Recall** takes the message out of the queue - the server's queue really
    empties - and puts the *draft* (not the rendered body) back in the textarea
    plus the chip, and a second recall appends rather than replacing;
  * queueing the recalled message again produces byte-identical content, which is
    the whole promise of "recall restores the form";
  * a recalled message that the run already applied is refused, and the panel says
    so instead of reviving it (the 404 branch, exercised in the browser);
  * the dock still fits at phone width with the panel on screen.

The three things this host forces, all learned the hard way:

  * Chromium needs `--no-sandbox`, an explicit `--user-data-dir`, and
    `setsid nohup` (a plain background job dies with the command's process group);
  * `KARUCODE_WEB_DIR` is mandatory, because the embedded `dist` is a build-time
    snapshot;
  * one persistent CDP WebSocket for the whole run — and it is spoken directly
    from Python (`websockets.sync`), not through a helper process: this host
    refuses `PROT_EXEC` mappings (`mmap(PROT_EXEC)` = EPERM), which makes V8 abort
    with "Check failed: 12 == (*__errno_location ())" a moment after start-up.

Usage: python3 scripts/verify_queues.py [--keep]
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


def http_json(url: str, method: str = "GET", body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("X-Karucode-Client", "web")
    if body is not None:
        request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read())





class Session:
    """One CDP connection for the whole run, driven synchronously over websockets."""

    def __init__(self, ws_url: str) -> None:
        from websockets.sync.client import connect

        # A cap on message size matters: a screenshot is a ~1 MB base64 blob, and
        # the default limit would drop the frame.
        self.ws = connect(ws_url, max_size=64 * 1024 * 1024, open_timeout=20)
        self.next_id = 0
        self.evaluate("return 1;")

    def _send(self, method: str, params: dict | None = None) -> dict:
        """Send one CDP command and return the result of the *matching* reply.

        Events (console messages, navigations) arrive interleaved on the same
        socket, so a reply is matched by `id` rather than by taking the next
        frame.
        """
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

    def evaluate(self, body: str) -> object:
        result = self._send(
            "Runtime.evaluate",
            {
                "expression": "(async () => {" + body + "})()",
                "awaitPromise": True,
                "returnByValue": True,
            },
        )
        if result.get("exceptionDetails"):
            raise SystemExit(f"page script failed: {json.dumps(result['exceptionDetails'])[:600]}")
        value = result.get("result", {})
        return value.get("value")

    def cdp(self, method: str, params: dict | None = None) -> object:
        return self._send(method, params)

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f' — {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f' — {detail}' if detail else ''}", flush=True)


# Waits for the app to boot and returns once the composer exists.
BOOT = """
const deadline = Date.now() + 30000;
while (Date.now() < deadline) {
  const box = document.querySelector('#composer-input');
  if (box) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}
return 'timeout';
"""

# Types into the composer the way a user does: value + a bubbling `input` event,
# which is what the controlled textarea listens for.
TYPE_INTO = """
const box = document.querySelector('#composer-input');
if (!box) return 'no-box';
box.focus();
box.value = %s;
box.dispatchEvent(new Event('input', { bubbles: true }));
return box.value;
"""

# Attaches a small text file through the real <input type=file>, so the composer's
# own classification (inline vs upload) runs.
ATTACH_FILE = """
const input = document.querySelector('.composer-file-input');
if (!input) return 'no-input';
const data = new DataTransfer();
data.items.add(new File([%s], 'notes.md', { type: 'text/plain' }));
input.files = data.files;
input.dispatchEvent(new Event('change', { bubbles: true }));
return 'attached';
"""

CLICK_BY_LABEL = """
const match = %s;
// Wait for the button to appear: a queue button only exists while a run is in
// flight, and the row re-renders after the draft changes.
const deadline = Date.now() + 10000;
while (Date.now() < deadline) {
  const button = Array.from(document.querySelectorAll('button')).find(
    (node) => match.test(node.getAttribute('aria-label') || '')
  );
  if (button) {
    if (button.disabled) return 'disabled';
    button.click();
    return 'clicked';
  }
  await new Promise((r) => setTimeout(r, 100));
}
return 'not-found';
"""

# Types a draft and clicks a button in ONE evaluate call.
#
# Two separate round trips are a race: a `[slow]` mock turn is a fixed 30 s sleep,
# so a run that is in flight when the draft is typed can settle before the click
# arrives - and the queue button goes with it. Doing both in one script removes
# the window entirely.
TYPE_AND_CLICK = """
const text = %s;
// Which queue the message should go into; null leaves whatever is armed alone.
const wanted = %s;
const match = %s;
// Wait on the *queue button appearing*, not on its label: which label it carries
// depends on the armed mode, and the mode is only switchable once the box has
// text - so matching the label first would deadlock against a Follow-up toggle.
const deadline = Date.now() + 20000;
while (Date.now() < deadline) {
  const box = document.querySelector('#composer-input');
  const slot = document.querySelector('.button-queue');
  if (box && slot) {
    box.focus();
    box.value = text;
    box.dispatchEvent(new Event('input', { bubbles: true }));
    // One frame for the controlled value to commit and the row to re-render.
    await new Promise((r) => setTimeout(r, 150));
    if (wanted) {
      const radio = Array.from(document.querySelectorAll('.queue-mode-option'))
        .find((node) => (node.textContent || '').trim() === wanted);
      if (!radio) return 'no-toggle';
      radio.click();
      await new Promise((r) => setTimeout(r, 150));
    }
    const button = Array.from(document.querySelectorAll('button')).find(
      (node) => match.test(node.getAttribute('aria-label') || '')
    );
    if (!button) return 'wrong-label:' + Array.from(document.querySelectorAll('.button-queue'))
      .map((node) => node.getAttribute('aria-label') || '').join('|');
    if (button.disabled) return 'disabled';
    button.click();
    return 'clicked';
  }
  await new Promise((r) => setTimeout(r, 100));
}
return 'not-found';
"""



def start_long_run(session: Session, base: str, session_id: str) -> str:
    """Settle the session, start a run that will stay in flight, and wait for the UI.

    A queue needs something to queue against, and the mock's `[slow]` turn is a
    fixed 30 s sleep: every step below has to begin from a run that was just
    started, not from one that happened to still be running.
    """
    http_json(f"{base}/api/sessions/{session_id}/cancel", "POST", {})
    for _ in range(160):
        if not http_json(f"{base}/api/sessions/{session_id}")["running"]:
            break
        time.sleep(0.25)
    http_json(
        f"{base}/api/sessions/{session_id}/messages",
        "POST",
        {"content": "[slow] long job"},
    )
    return session.evaluate(
        """
        const deadline = Date.now() + 25000;
        while (Date.now() < deadline) {
          if (document.querySelector('.button-queue')) return 'ready';
          await new Promise((r) => setTimeout(r, 100));
        }
        return 'timeout';
        """
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    if not (ROOT / "target/debug/karucode").exists():
        raise SystemExit("build first: cargo build -p kc-server")
    if not (ROOT / "web/dist/index.html").exists():
        raise SystemExit("build the UI first: (cd web && npm run build)")

    session: Session | None = None
    tmp = Path(tempfile.mkdtemp(prefix="kc-queues-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()
    log_path = tmp / "provider.jsonl"

    provider_port = free_port()
    mock = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(log_path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )

    config = tmp / "config.toml"
    config.write_text(
        "\n".join(
            [
                "[model]",
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'provider = "openai_compatible"',
                'model = "gpt-4o-mini"',
                'api_key = "sk-test"',
                "",
                "[[providers]]",
                'id = "default"',
                'name = "Default"',
                'provider = "openai_compatible"',
                f'base_url = "http://127.0.0.1:{provider_port}/v1"',
                'api_key = "sk-test"',
                "",
                "[[providers.models]]",
                'id = "gpt-4o-mini"',
                'name = "GPT-4o mini"',
                "context_window = 128000",
                "",
                "[active]",
                'provider = "default"',
                'model = "gpt-4o-mini"',
                "",
                "[titles]",
                # The heuristic title is deterministic; a model-written one would
                # need its own turn and only adds noise here.
                "enabled = false",
                "",
                "[notifications]",
                "enabled = false",
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
            "--window-size=1440,960",
            # This host refuses PROT_EXEC mappings, so V8 cannot reserve a code
            # range and the renderer dies with "V8 process OOM (Failed to reserve
            # virtual memory for CodeRange)" before it answers any CDP command.
            # --jitless runs the interpreter only, which is what makes the page
            # execute at all here.
            "--js-flags=--jitless",
            "about:blank",
        ],
        stdout=chrome_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )

    base = f"http://127.0.0.1:{port}"
    try:
        wait_for(f"{base}/api/health")
        wait_for(f"http://127.0.0.1:{provider_port}/v1/models")
        wait_for(f"http://127.0.0.1:{cdp_port}/json/version")

        # Sessions must be created in the project or the rail never shows them.
        project = http_json(f"{base}/api/projects")["projects"][0]["id"]
        session_id = http_json(
            f"{base}/api/sessions",
            "POST",
            {"title": "Steering demo", "project": project},
        )["id"]

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{base}/"})
        check("the app boots", session.evaluate(BOOT) == "ready")

        picked = session.evaluate(
            """
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {
              const row = Array.from(document.querySelectorAll('button, [role=button]')).find(
                (node) => (node.textContent || '').includes('Steering demo')
              );
              if (row) { row.click(); return 'clicked'; }
              await new Promise((r) => setTimeout(r, 150));
            }
            return 'not-found';
            """
        )
        check("the session can be selected from the rail", picked == "clicked", str(picked))
        time.sleep(0.6)

        # ---- start a run that stays in flight -------------------------------
        # `[slow]` makes the mock provider sleep before answering, so the run is
        # provably working while everything below happens.
        typed = session.evaluate(TYPE_INTO % json.dumps("[slow] long job"))
        check("the composer takes a draft", typed == "[slow] long job", str(typed))
        sent = session.evaluate(CLICK_BY_LABEL % r"/^Send message/i")
        check("the message can be sent", sent == "clicked", str(sent))

        running = session.evaluate(
            """
            const deadline = Date.now() + 20000;
            while (Date.now() < deadline) {
              if (document.querySelector('.button-stop')) return 'running';
              await new Promise((r) => setTimeout(r, 100));
            }
            return 'idle';
            """
        )
        check("the run goes into flight", running == "running", str(running))

        # ---- the box queues instead of sending -----------------------------
        # The mode toggle only exists while the box has text (an empty box has
        # nothing to aim), so type first and wait for the row to settle.
        session.evaluate(TYPE_INTO % json.dumps("draft while working"))
        buttons = session.evaluate(
            """
            const deadline = Date.now() + 10000;
            let snapshot = null;
            while (Date.now() < deadline) {
              const labels = Array.from(document.querySelectorAll('.compose-bottom button'))
                .map((node) => node.getAttribute('aria-label') || node.textContent || '');
              snapshot = {
                labels,
                send: labels.some((label) => /^Send message/.test(label)),
                steer: labels.some((label) => /steer the running turn/.test(label)),
                stop: labels.some((label) => /^Stop run/.test(label)),
                modes: Array.from(document.querySelectorAll('.queue-mode-option'))
                  .map((node) => [node.textContent, node.getAttribute('aria-checked')]),
              };
              if (snapshot.steer && snapshot.modes.length === 2) return snapshot;
              await new Promise((r) => setTimeout(r, 100));
            }
            return snapshot;
            """
        )
        check("there is no Send button while the agent works", not buttons["send"], str(buttons["labels"]))
        check("there is a Steer button instead", buttons["steer"], str(buttons["labels"]))
        check("Stop is still there", buttons["stop"], str(buttons["labels"]))
        check(
            "the mode toggle offers Steer and Follow-up, with Steer armed",
            sorted(mode[0] for mode in buttons["modes"]) == ["Follow-up", "Steer"]
            and any(mode == ["Steer", "true"] for mode in buttons["modes"]),
            str(buttons["modes"]),
        )

        # ---- queue a steer with an attachment ------------------------------
        attached = session.evaluate(ATTACH_FILE % json.dumps("# Note\n"))
        check("a file can be attached", attached == "attached", str(attached))
        chip = session.evaluate(
            """
            const deadline = Date.now() + 10000;
            while (Date.now() < deadline) {
              const meta = document.querySelector('.attachment-chip-meta');
              if (meta && /Inlined/.test(meta.textContent || '')) {
                return { name: (document.querySelector('.attachment-chip-name') || {}).textContent };
              }
              await new Promise((r) => setTimeout(r, 100));
            }
            return null;
            """
        )
        check("the chip says the file was inlined", chip is not None, str(chip))

        session.evaluate(TYPE_INTO % json.dumps("summarise"))
        time.sleep(0.2)
        steered = session.evaluate(CLICK_BY_LABEL % r"/steer the running turn/i")
        check("the steer can be queued", steered == "clicked", str(steered))

        queued = None
        for _ in range(60):
            queued = http_json(f"{base}/api/sessions/{session_id}/queues")
            if queued["steers"]:
                break
            time.sleep(0.2)
        check("the server queued the steer", len(queued["steers"]) == 1, str(queued["steers"]))
        entry = queued["steers"][0] if queued["steers"] else {}
        check("it is a steer", entry.get("kind") == "steer", str(entry.get("kind")))
        check(
            "the draft is kept separately from the rendered body",
            entry.get("draft") == "summarise" and "### notes.md (inlined, 7 B)" in entry.get("content", ""),
            json.dumps({"draft": entry.get("draft"), "content": (entry.get("content") or "")[:60]}),
        )
        attachment = (entry.get("attachments") or [{}])[0]
        check(
            "the attachment reached the server with its inline body",
            attachment.get("name") == "notes.md"
            and attachment.get("mode") == "inline"
            and attachment.get("content") == "# Note\n",
            json.dumps(attachment),
        )
        first_content = entry.get("content", "")

        # ---- panel above the box, listing what is waiting -------------------
        panel = session.evaluate(
            """
            const deadline = Date.now() + 10000;
            let panel = null;
            while (Date.now() < deadline) {
              panel = document.querySelector('.queued-panel');
              if (panel) break;
              await new Promise((r) => setTimeout(r, 100));
            }
            if (!panel) return null;
            const box = document.querySelector('form.composer');
            const panelBox = panel.getBoundingClientRect();
            const boxRect = box.getBoundingClientRect();
            return {
              head: (panel.querySelector('.queued-head') || {}).textContent || '',
              kinds: Array.from(panel.querySelectorAll('.queued-kind')).map((n) => n.textContent),
              texts: Array.from(panel.querySelectorAll('.queued-text')).map((n) => n.textContent),
              chips: Array.from(panel.querySelectorAll('.queued-attachment')).map((n) => n.textContent),
              recalls: Array.from(panel.querySelectorAll('.queued-recall')).length,
              aboveBox: Math.round(panelBox.bottom) <= Math.round(boxRect.top) + 1,
              width: Math.round(panelBox.width),
              composerWidth: Math.round(boxRect.width),
            };
            """
        )
        check("the pending panel appears", panel is not None, "")
        if panel:
            check("it sits above the composer box", panel["aboveBox"], f"{panel}")
            check("it matches the box's width", abs(panel["width"] - panel["composerWidth"]) <= 2,
                  f"{panel['width']} vs {panel['composerWidth']}")
            check("it names the kind", panel["kinds"] == ["Steer"], str(panel["kinds"]))
            check("it shows the draft the user typed, not the rendered body",
                  panel["texts"] == ["summarise"], str(panel["texts"]))
            check("it shows the attachment chip", any("notes.md" in c for c in panel["chips"]),
                  str(panel["chips"]))
            check("it offers a Recall button", panel["recalls"] == 1, str(panel["recalls"]))
            check("the composer box was cleared after queueing",
                  session.evaluate("return (document.querySelector('#composer-input') || {}).value;") == "")

        # ---- a follow-up is a different kind -------------------------------
        # Type first: the toggle only renders once the box has content, because
        # with an empty box it would be a choice about nothing.
        session.evaluate(TYPE_INTO % json.dumps("then run the tests"))
        time.sleep(0.2)
        armed = session.evaluate(
            """
            const radio = Array.from(document.querySelectorAll('.queue-mode-option'))
              .find((node) => (node.textContent || '') === 'Follow-up');
            if (!radio) return 'no-toggle';
            radio.click();
            await new Promise((r) => setTimeout(r, 150));
            const again = Array.from(document.querySelectorAll('.queue-mode-option'))
              .find((node) => (node.textContent || '') === 'Follow-up');
            const steer = Array.from(document.querySelectorAll('.queue-mode-option'))
              .find((node) => (node.textContent || '') === 'Steer');
            return {
              followUp: again ? again.getAttribute('aria-checked') : null,
              steer: steer ? steer.getAttribute('aria-checked') : null,
              label: (document.querySelector('.button-queue') || {}).textContent || '',
            };
            """
        )
        check(
            "the toggle arms Follow-up and the button says so",
            isinstance(armed, dict)
            and armed.get("followUp") == "true"
            and armed.get("steer") == "false"
            and "Queue" in armed.get("label", ""),
            str(armed),
        )
        followed = session.evaluate(CLICK_BY_LABEL % r"/queue this follow-up/i")
        check("a follow-up can be queued", followed == "clicked", str(followed))

        queues = None
        for _ in range(60):
            queues = http_json(f"{base}/api/sessions/{session_id}/queues")
            if queues["follow_ups"]:
                break
            time.sleep(0.2)
        check("the follow-up lands in its own queue", len(queues["follow_ups"]) == 1,
              str(queues["follow_ups"]))
        check("...and the steer is still queued", len(queues["steers"]) == 1,
              str(len(queues["steers"])))
        order = session.evaluate(
            """
            const rows = Array.from(document.querySelectorAll('.queued-row'));
            return rows.map((row) => [row.dataset.kind, (row.querySelector('.queued-text') || {}).textContent]);
            """
        )
        check("both are listed, steers before follow-ups",
              [row[0] for row in order] == ["steer", "follow_up"], str(order))

        # ---- recall the follow-up -----------------------------------------
        recalled = session.evaluate(CLICK_BY_LABEL % r"/^Recall follow-up/i")
        check("the follow-up can be recalled", recalled == "clicked", str(recalled))
        time.sleep(0.6)
        after = http_json(f"{base}/api/sessions/{session_id}/queues")
        check("recalling takes it off the server too", after["follow_ups"] == [], str(after["follow_ups"]))
        check("...and leaves the steer alone", len(after["steers"]) == 1, str(len(after["steers"])))
        text = session.evaluate("return (document.querySelector('#composer-input') || {}).value;")
        check("the recalled draft is back in the box", text == "then run the tests", repr(text))

        # ---- recall the steer, chips and all ------------------------------
        recalled = session.evaluate(CLICK_BY_LABEL % r"/^Recall steer/i")
        check("the steer can be recalled", recalled == "clicked", str(recalled))
        time.sleep(0.6)
        after = http_json(f"{base}/api/sessions/{session_id}/queues")
        check("the queues are empty on the server", after["steers"] == [] and after["follow_ups"] == [],
              str(after))
        restored = session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            const chips = Array.from(document.querySelectorAll('.attachment-chip-name'))
              .map((node) => node.textContent);
            return {
              text: box ? box.value : null,
              chips,
              panel: Boolean(document.querySelector('.queued-panel')),
            };
            """
        )
        check(
            "the recalled draft is appended below what was typed since",
            restored["text"] == "then run the tests\n\nsummarise",
            repr(restored["text"]),
        )
        check("the attachment chip came back", restored["chips"] == ["notes.md"], str(restored["chips"]))
        check("the pending panel is gone", not restored["panel"], str(restored["panel"]))

        # ---- re-queueing the recall reproduces the request exactly --------
        # The chip's inline body came back from the server, so the message the
        # model would see must be byte-identical to the first one. The `[slow]`
        # run may have settled by now (each mock turn is a fixed sleep), so make
        # sure one is in flight - a steer with no run is a documented 409.
        rerun = start_long_run(session, base, session_id)
        check("a run is in flight again for the round trip", rerun == "ready", str(rerun))

        # Replace whatever is in the box with the draft alone (the follow-up text
        # from the earlier recall is still appended below it) and queue it in the
        # same round trip, so the run cannot settle in between.
        again = session.evaluate(
            TYPE_AND_CLICK % (json.dumps("summarise"), json.dumps("Steer"), r"/steer the running turn/i")
        )
        check("the recalled message can be queued again", again == "clicked", str(again))
        requeued = None
        for _ in range(60):
            requeued = http_json(f"{base}/api/sessions/{session_id}/queues")
            if requeued["steers"]:
                break
            time.sleep(0.2)
        content = requeued["steers"][0]["content"] if requeued["steers"] else ""
        check("re-queueing reproduces the rendered body byte for byte",
              content == first_content, f"{content!r} vs {first_content!r}")

        # ---- stop the run, then try to recall what it applied -------------
        # A steer queued against an idle session is refused, so the honest end to
        # the story is: cancel, and the queued message is still there to recall.
        stopped = session.evaluate(CLICK_BY_LABEL % r"/^Stop run/i")
        check("the run can be stopped", stopped == "clicked", str(stopped))
        for _ in range(80):
            if not http_json(f"{base}/api/sessions/{session_id}")["running"]:
                break
            time.sleep(0.25)

        # Now drain the queue by recalling it, and prove a second recall of the
        # same id is refused rather than silently succeeding.
        entry_id = (http_json(f"{base}/api/sessions/{session_id}/queues")["steers"] or [{}])[0].get("id")
        if entry_id:
            http_json(
                f"{base}/api/sessions/{session_id}/queues/{entry_id}",
                "DELETE",
            )
            try:
                http_json(f"{base}/api/sessions/{session_id}/queues/{entry_id}", "DELETE")
                refused = False
                status = "ok"
            except urllib.error.HTTPError as error:
                refused = error.code == 404
                status = f"{error.code}"
            check("recalling an id twice is a 404, not a silent success",
                  refused, status)

        # The panel is gone now, and the composer is idle again: Send is back.
        idle_buttons = session.evaluate(
            """
            const labels = Array.from(document.querySelectorAll('.compose-bottom button'))
              .map((node) => node.getAttribute('aria-label') || node.textContent || '');
            return { send: labels.some((l) => /^Send message/.test(l)), labels };
            """
        )
        check("Send comes back once the run is over", idle_buttons["send"], str(idle_buttons["labels"]))

        # ---- the phone layout still fits, panel and all -------------------
        # Prove it with the panel *on screen*: queue a message, then measure at
        # phone width. (Measured before, the panel had already been drained and
        # there was nothing to size.)
        phone_run = start_long_run(session, base, session_id)
        check("a run is in flight for the phone check", phone_run == "ready", str(phone_run))
        phone_queued = session.evaluate(
            TYPE_AND_CLICK % (json.dumps("phone draft"), json.dumps("Steer"), r"/steer the running turn/i")
        )
        check("a message can be queued again for the phone check",
              phone_queued == "clicked", str(phone_queued))
        session.cdp("Emulation.setDeviceMetricsOverride",
                    {"width": 390, "height": 844, "deviceScaleFactor": 2, "mobile": True})
        time.sleep(0.6)
        phone = session.evaluate(
            """
            const doc = document.documentElement;
            const panelNode = document.querySelector('.queued-panel');
            const bottom = document.querySelector('.compose-bottom');
            return {
              pageOverflow: doc.scrollWidth - doc.clientWidth,
              panelWidth: panelNode ? Math.round(panelNode.getBoundingClientRect().width) : 0,
              viewport: doc.clientWidth,
              rows: panelNode ? panelNode.querySelectorAll('.queued-row').length : 0,
              bottomOverflow: bottom ? Math.round(bottom.getBoundingClientRect().right - doc.clientWidth) : 0,
            };
            """
        )
        check("no page-level horizontal overflow with the panel open",
              phone["pageOverflow"] <= 1, str(phone))
        check("the panel fits the phone column",
              0 < phone["panelWidth"] <= phone["viewport"], str(phone))
        check("...with its row still rendered", phone["rows"] >= 1, str(phone))
        check("the action row does not overflow the phone column",
              phone["bottomOverflow"] <= 1, str(phone))
        session.cdp("Emulation.clearDeviceMetricsOverride")

        # A picture, so a human can check the panel is readable rather than present.
        session.cdp("Emulation.setDeviceMetricsOverride",
                    {"width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": False})
        time.sleep(0.3)
        import base64

        shot = session.cdp("Page.captureScreenshot", {"format": "png"})
        if isinstance(shot, dict) and shot.get("data"):
            out = Path("/tmp/kc-queues.png")
            out.write_bytes(base64.b64decode(shot["data"]))
            print(f"  screenshot: {out}", flush=True)
        session.cdp("Emulation.clearDeviceMetricsOverride")

        print("", flush=True)
        print(f"{len(PASS)} passed, {len(FAIL)} failed", flush=True)
        if FAIL:
            print("failed:", ", ".join(FAIL), flush=True)
        return 1 if FAIL else 0
    finally:
        if session and not args.keep:
            session.close()
        if not args.keep:
            for proc in (chrome, server, mock):
                try:
                    proc.terminate()
                    proc.wait(timeout=10)
                except Exception:
                    proc.kill()
            server_log.close()
            chrome_log.close()


if __name__ == "__main__":
    sys.exit(main())
