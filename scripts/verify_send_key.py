#!/usr/bin/env python3
"""Prove the composer's send key in a real Chromium over raw CDP.

Settings -> Appearance -> *Theme and layout* -> **Composer send key** offers two
behaviours, and the unit tests only prove the handler in jsdom. This script proves
the three things that only a browser can:

  * **Ctrl/Cmd + Enter sends** (the default): a real Enter key press puts a newline
    in the box and nothing is sent; a real Ctrl+Enter sends, the box empties and
    the user turn reaches the server;
  * **Enter sends**: after choosing it in the real settings sheet - no reload - a
    real Enter sends, while Ctrl+Enter and Shift+Enter both insert a newline and
    send nothing;
  * the choice is remembered per browser: it survives a page reload, and the
    setting reaches the composer's foot hint and the Send button's accessible
    name, so the key that sends is never a guess.

Key events go through `Input.dispatchKeyEvent`, not synthetic DOM events: an
Enter only proves `preventDefault` was (not) called if the browser's own newline
is what the textarea ends up with. The `keyDown` carries `text: "\\r"`, exactly
what a keyboard produces.

The three things this host forces, all learned the hard way:

  * Chromium needs `--no-sandbox`, an explicit `--user-data-dir`, `--jitless`
    (this host refuses PROT_EXEC mappings, so V8 cannot reserve a code range) and
    `setsid nohup` (a plain background job dies with the command's process group);
  * `KARUCODE_WEB_DIR` is mandatory, because the embedded `dist` is a build-time
    snapshot;
  * one persistent CDP WebSocket for the whole run, spoken directly from Python
    (`websockets.sync`), not through a helper process.

Usage: python3 scripts/verify_send_key.py [--keep]
"""

from __future__ import annotations

import argparse
import base64
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

# CDP modifier bits, as `Input.dispatchKeyEvent` wants them.
MOD_CTRL = 2
MOD_SHIFT = 8


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
        return result.get("result", {}).get("value")

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


BOOT = """
const deadline = Date.now() + 30000;
while (Date.now() < deadline) {
  if (document.querySelector('#composer-input')) return 'ready';
  await new Promise((r) => setTimeout(r, 100));
}
return 'timeout';
"""

COMPOSER_STATE = """
const box = document.querySelector('#composer-input');
const hint = document.querySelector('.composer-hint');
const send = Array.from(document.querySelectorAll('.compose-bottom button')).find(
  (node) => /^Send message/.test(node.getAttribute('aria-label') || '')
);
return {
  value: box ? box.value : null,
  focused: box ? document.activeElement === box : false,
  hint: hint ? (hint.textContent || '').replace(/\\s+/g, ' ').trim() : null,
  sendLabel: send ? send.getAttribute('aria-label') : null,
};
"""


def composer(session: Session) -> dict:
    return session.evaluate(COMPOSER_STATE)


def focus_composer(session: Session) -> None:
    """Put the caret in the box with real focus, and select whatever is there."""
    session.evaluate(
        """
        const box = document.querySelector('#composer-input');
        if (!box) return 'no-box';
        box.focus();
        box.select();
        return 'ok';
        """
    )
    session.cdp("Page.bringToFront")
    time.sleep(0.1)


def type_text(session: Session, text: str) -> str:
    """Type with the browser's own input path, so the composer's `input` fires."""
    focus_composer(session)
    session.cdp("Input.insertText", {"text": text})
    time.sleep(0.15)
    return session.evaluate("return document.querySelector('#composer-input').value;")


def press_enter(session: Session, modifiers: int = 0) -> None:
    """Press Enter the way a keyboard does: `keyDown` carrying the newline text.

    The `text` matters in every mode. Ctrl and Shift do not suppress a keyboard's
    text, so a real Ctrl+Enter still produces `\\r` - and that is exactly what
    makes the assertion honest: if the composer does not take the key, the
    browser's own newline lands in the textarea, and if it does, `preventDefault`
    is what stops it.
    """
    session.cdp("Page.bringToFront")
    time.sleep(0.1)
    session.cdp(
        "Input.dispatchKeyEvent",
        {
            "type": "keyDown",
            "key": "Enter",
            "code": "Enter",
            "windowsVirtualKeyCode": 13,
            "nativeVirtualKeyCode": 13,
            "text": "\r",
            "unmodifiedText": "\r",
            "modifiers": modifiers,
        },
    )
    session.cdp(
        "Input.dispatchKeyEvent",
        {
            "type": "keyUp",
            "key": "Enter",
            "code": "Enter",
            "windowsVirtualKeyCode": 13,
            "nativeVirtualKeyCode": 13,
            "modifiers": modifiers,
        },
    )
    time.sleep(0.4)


def user_turns(base: str, session_id: str) -> list[str]:
    payload = http_json(f"{base}/api/sessions/{session_id}")
    return [
        str(message.get("content") or "")
        for message in payload.get("messages", [])
        if str(message.get("role")) == "user"
    ]


def wait_until_idle(base: str, session_id: str, timeout: float = 40.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if not http_json(f"{base}/api/sessions/{session_id}")["running"]:
            return True
        time.sleep(0.25)
    return False


def wait_for_user_turn(base: str, session_id: str, text: str, timeout: float = 20.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if any(text in body for body in user_turns(base, session_id)):
            return True
        time.sleep(0.25)
    return False


def open_appearance(session: Session) -> dict:
    """Open Settings and the Appearance tab through the real controls."""
    return session.evaluate(
        """
        const gear = Array.from(document.querySelectorAll('button')).find(
          (button) => /open settings/i.test(button.getAttribute('aria-label') || ''),
        );
        if (!gear) return { opened: false, reason: 'no gear' };
        gear.click();
        const deadline = Date.now() + 15000;
        while (Date.now() < deadline) {
          const tab = Array.from(document.querySelectorAll('[role=tab]')).find(
            (candidate) =>
              ((candidate.querySelector('.settings-nav-label') || {}).textContent || '').trim() ===
              'Appearance',
          );
          if (tab) {
            tab.click();
            await new Promise((r) => setTimeout(r, 400));
            const select = document.getElementById('settings-send-key');
            return {
              opened: true,
              options: select ? Array.from(select.options).map((option) => option.value) : null,
              value: select ? select.value : null,
              hint: (document.getElementById('settings-send-key-hint') || {}).textContent || '',
            };
          }
          await new Promise((r) => setTimeout(r, 100));
        }
        return { opened: false, reason: 'no Appearance tab' };
        """
    )


def choose_send_key(session: Session, value: str) -> dict:
    """Change the select the way the component listens for it (a change event)."""
    return session.evaluate(
        """
        const select = document.getElementById('settings-send-key');
        if (!select) return { changed: false };
        select.value = %s;
        select.dispatchEvent(new Event('change', { bubbles: true }));
        await new Promise((r) => setTimeout(r, 300));
        return {
          changed: true,
          value: select.value,
          hint: (document.getElementById('settings-send-key-hint') || {}).textContent || '',
          stored: localStorage.getItem('karucode.composerSendKey'),
        };
        """
        % json.dumps(value)
    )


def close_settings(session: Session) -> None:
    session.evaluate(
        """
        const close = Array.from(document.querySelectorAll('button')).find(
          (button) => /close settings/i.test(button.getAttribute('aria-label') || ''),
        );
        if (close) close.click();
        await new Promise((r) => setTimeout(r, 500));
        return 'ok';
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
    tmp = Path(tempfile.mkdtemp(prefix="kc-send-key-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    (workspace / "README.md").write_text("# demo\n")
    data_dir = tmp / "data"
    data_dir.mkdir()

    provider_port = free_port()
    mock = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(tmp / "provider.jsonl")],
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
                # A model-written title would need its own turn and only adds noise.
                "[titles]",
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

        project = http_json(f"{base}/api/projects")["projects"][0]["id"]
        session_id = http_json(
            f"{base}/api/sessions", "POST", {"title": "Send key demo", "project": project}
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
                (node) => (node.textContent || '').includes('Send key demo'),
              );
              if (row) { row.click(); return 'clicked'; }
              await new Promise((r) => setTimeout(r, 150));
            }
            return 'not-found';
            """
        )
        check("the session can be selected from the rail", picked == "clicked", str(picked))
        wait_until_idle(base, session_id)

        # ---- the default: the chord sends, Enter starts a line ---------------
        state = composer(session)
        check(
            "the sheet is untouched, so the default is the chord",
            state["hint"] == "Ctrl Enter to send·@ reference files",
            repr(state["hint"]),
        )
        check(
            "the Send button names the chord",
            state["sendLabel"] == "Send message (Command or Control + Enter)",
            repr(state["sendLabel"]),
        )

        typed = type_text(session, "first line[text]")
        check("typing reaches the box", typed == "first line[text]", repr(typed))
        press_enter(session)
        after_enter = composer(session)
        check(
            "a plain Enter starts a new line instead of sending",
            after_enter["value"] == "first line[text]\n",
            repr(after_enter["value"]),
        )
        check(
            "nothing was sent by that Enter",
            user_turns(base, session_id) == [],
            str(user_turns(base, session_id)),
        )

        # Clear the box, then send with the chord.
        type_text(session, "sent by the chord[text]")
        press_enter(session, MOD_CTRL)
        check("Ctrl+Enter sends the draft", composer(session)["value"] == "", repr(composer(session)["value"]))
        check(
            "the sent turn is on the server, once",
            wait_for_user_turn(base, session_id, "sent by the chord[text]")
            and sum("sent by the chord[text]" in body for body in user_turns(base, session_id)) == 1,
            str(user_turns(base, session_id)),
        )
        wait_until_idle(base, session_id)

        # ---- switch to Enter-sends in the real settings sheet ----------------
        opened = open_appearance(session)
        check("the settings sheet opens on Appearance", bool(opened.get("opened")), str(opened))
        check(
            "the send key offers exactly two modes, chord first",
            opened.get("options") == ["modifier-enter", "enter"],
            str(opened.get("options")),
        )
        check("...defaulting to the chord", opened.get("value") == "modifier-enter", str(opened.get("value")))
        check(
            "...and its hint describes the mode in force",
            "Ctrl/Cmd+Enter sends" in str(opened.get("hint")),
            repr(opened.get("hint")),
        )

        chosen = choose_send_key(session, "enter")
        check("the mode changes in place", chosen.get("value") == "enter", str(chosen))
        check(
            "the choice is written to this browser",
            chosen.get("stored") == "enter",
            repr(chosen.get("stored")),
        )
        check(
            "...and the hint follows it",
            "Enter sends;" in str(chosen.get("hint")),
            repr(chosen.get("hint")),
        )
        close_settings(session)

        live = composer(session)
        check(
            "the composer answers without a reload: Enter is the send key",
            live["hint"] == "Enter to send·@ reference files",
            repr(live["hint"]),
        )
        check(
            "the Send button is renamed with it",
            live["sendLabel"] == "Send message (Enter)",
            repr(live["sendLabel"]),
        )

        # ---- Enter now sends, the chords start a line -----------------------
        type_text(session, "sent by Enter[text]")
        press_enter(session)
        check("a plain Enter sends the draft", composer(session)["value"] == "", repr(composer(session)["value"]))
        check(
            "the Enter-sent turn is on the server, once",
            wait_for_user_turn(base, session_id, "sent by Enter[text]")
            and sum("sent by Enter[text]" in body for body in user_turns(base, session_id)) == 1,
            str(user_turns(base, session_id)),
        )
        wait_until_idle(base, session_id)

        type_text(session, "ctrl newline[text]")
        press_enter(session, MOD_CTRL)
        ctrl_state = composer(session)
        check(
            "Ctrl+Enter starts a new line instead of sending",
            ctrl_state["value"] == "ctrl newline[text]\n",
            repr(ctrl_state["value"]),
        )
        check(
            "that Ctrl+Enter sent nothing",
            sum("ctrl newline" in body for body in user_turns(base, session_id)) == 0,
            str(user_turns(base, session_id)),
        )

        type_text(session, "shift newline[text]")
        press_enter(session, MOD_SHIFT)
        shift_state = composer(session)
        check(
            "Shift+Enter starts a new line too",
            shift_state["value"] == "shift newline[text]\n",
            repr(shift_state["value"]),
        )
        check(
            "that Shift+Enter sent nothing",
            sum("shift newline" in body for body in user_turns(base, session_id)) == 0,
            str(user_turns(base, session_id)),
        )

        # ---- an IME's Enter never sends ------------------------------------
        # While a composition is open, Enter confirms the word; sending there
        # would submit half a sentence and throw the composition away.
        composing = session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.focus();
            box.value = '';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            return box.value;
            """
        )
        check("the box is clear for the composition check", composing == "", repr(composing))
        session.cdp("Input.imeSetComposition", {"text": "こんにちは", "selectionStart": 5, "selectionEnd": 5})
        time.sleep(0.2)
        press_enter(session)
        time.sleep(0.4)
        composed = composer(session)
        check(
            "an IME's Enter does not send",
            sum("こんにちは" in body for body in user_turns(base, session_id)) == 0,
            f"box={composed['value']!r} turns={user_turns(base, session_id)}",
        )
        session.evaluate(
            """
            const box = document.querySelector('#composer-input');
            box.value = '';
            box.dispatchEvent(new Event('input', { bubbles: true }));
            return 'cleared';
            """
        )

        # ---- the choice survives a reload ----------------------------------
        session.cdp("Page.navigate", {"url": f"{base}/"})
        check("the app boots again after a reload", session.evaluate(BOOT) == "ready")
        time.sleep(1.0)
        reloaded = composer(session)
        check(
            "the chosen send key is still in force after a reload",
            reloaded["hint"] == "Enter to send·@ reference files"
            and reloaded["sendLabel"] == "Send message (Enter)",
            f'hint={reloaded["hint"]!r} label={reloaded["sendLabel"]!r}',
        )

        # And it really still sends: proof the reload did not leave a dead mode.
        type_text(session, "after reload[text]")
        press_enter(session)
        check(
            "Enter still sends after the reload",
            wait_for_user_turn(base, session_id, "after reload[text]"),
            str(user_turns(base, session_id)),
        )

        shot = session.cdp("Page.captureScreenshot", {"format": "png"})
        if isinstance(shot, dict) and shot.get("data"):
            out = Path("/tmp/kc-send-key.png")
            out.write_bytes(base64.b64decode(shot["data"]))
            print(f"  screenshot: {out}", flush=True)

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