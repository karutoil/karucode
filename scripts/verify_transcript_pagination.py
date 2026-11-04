#!/usr/bin/env python3
"""Prove transcript pagination in a real Chromium against a real kc-server.

The unit tests cover the store page, the route cursor and the reducer, but none
of them prove the thing a person does: open a very long session in the browser,
notice that the start is missing, press **Load older history**, and see the
earlier messages appear while the "Earlier history is not loaded." notice goes
away. This does exactly that, and it is the check that would have caught a
1 MiB response cap that arrives with no way to ask for the rest.

Build first: `cargo build -p kc-server --bin karucode`, `(cd web && npm run build)`.
Run: python3 scripts/verify_transcript_pagination.py [--keep]

Like `scripts/verify_agents_tab.py`, it uses raw CDP (the chrome-devtools plugin
cannot launch Chromium on this host), a temporary workspace/data dir, free
loopback ports and a throwaway browser profile.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import JS_HELPERS, ROOT, Session, free_port, http_json, sse, text_chunks, wait_for

# Enough stored payload to exceed one page (the server page budget is 1 MiB of
# stored column bytes) without making the browser render an absurd document.
MESSAGE_COUNT = 60
MESSAGE_BYTES = 40_000
NEWEST = "PAGE-NEWEST-MARKER"
OLDER = "PAGE-OLDEST-MARKER"


class Provider(BaseHTTPRequestHandler):
    """A provider that is never called: the transcript is seeded in SQLite."""

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
        body = sse(text_chunks("ok"))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def seed_transcript(db: Path, session_id: str) -> None:
    """Write a transcript that is several pages long, oldest marker first."""
    now = int(time.time() * 1000)
    rows = []
    for index in range(MESSAGE_COUNT):
        role = "user" if index % 2 == 0 else "assistant"
        marker = ""
        if index == 0:
            marker = f"{OLDER} "
        if index == MESSAGE_COUNT - 1:
            marker = f"{NEWEST} "
        body = marker + (f"message {index} " * (MESSAGE_BYTES // len(f"message {index} ")))
        rows.append((session_id, None, None, role, body, now + index))
    # The server holds the writer connection; WAL lets a second process append.
    with sqlite3.connect(db, timeout=10) as conn:
        conn.executemany(
            "INSERT INTO messages (session_id, run_id, agent_id, role, content, created_at)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            rows,
        )
        conn.commit()


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'ok  ' if ok else 'FAIL'} {name}{('  -> ' + detail) if detail and not ok else ''}")
    if not ok:
        raise SystemExit(1)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="leave the server and browser running")
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="kc-pagination-"))
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
            "--window-size=1280,900",
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

        # The rail lists the active project, so the session has to belong to it.
        projects = http_json(f"{api}/api/projects")["projects"]
        check("a project exists", bool(projects), json.dumps(projects)[:300])
        project_id = projects[0]["id"]
        created = http_json(
            f"{api}/api/sessions",
            "POST",
            {"title": "Pagination target", "project": project_id},
        )
        session_id = created.get("id") or (created.get("session") or {}).get("id")
        check("session created", bool(session_id), json.dumps(created)[:300])

        db = data_dir / "karucode.db"
        check("database present", db.exists(), str(list(data_dir.iterdir())))
        seed_transcript(db, session_id)

        detail = http_json(f"{api}/api/sessions/{session_id}")
        print(
            "newest page from the API:",
            len(detail.get("messages") or []),
            "messages, truncated:",
            detail.get("messages_truncated"),
            "before:",
            detail.get("messages_before"),
        )

        with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list", timeout=5) as response:
            targets = json.loads(response.read())
        page = next(t for t in targets if t["type"] == "page")
        session = Session(page["webSocketDebuggerUrl"])
        session.cdp("Page.enable")
        session.cdp("Page.navigate", {"url": f"{api}/"})
        session.evaluate(JS_HELPERS)

        # Open the seeded session from the rail and wait for its newest page.
        bottom = session.evaluate(
            f"""
            const row = await window.__kc.until(() =>
              [...document.querySelectorAll('button')].find((b) =>
                (b.textContent || '').includes('Pagination target')));
            row.click();
            await window.__kc.until(() => (document.body.innerText || '').includes('{NEWEST}'), 20000);
            return document.body.innerText;
            """
        )
        check("the newest page renders", NEWEST in bottom)
        check("the oldest page is not loaded yet", OLDER not in bottom)

        # The transcript is windowed: the head of the conversation exists in the
        # DOM only once it is scrolled to, exactly like a person pressing Home.
        top = session.evaluate(
            """
            for (const node of document.querySelectorAll('*')) {
              if (node.scrollHeight > node.clientHeight && node.clientHeight > 0) node.scrollTop = 0;
            }
            await window.__kc.sleep(500);
            return document.body.innerText;
            """
        )
        check("the missing-start notice is shown", "Earlier history is not loaded." in top)
        check(
            "the load-older control is offered",
            bool(
                session.evaluate(
                    """
                    return !![...document.querySelectorAll('button')].find((b) =>
                      (b.textContent || '').trim() === 'Load older history');
                    """
                )
            ),
        )

        # Press the control a person presses - repeatedly, because 60 large
        # messages are more than two pages: the control must stay offered while
        # older pages remain and disappear once the first message is in.
        for page_number in range(1, 8):
            clicked = session.evaluate(
                """
                const button = [...document.querySelectorAll('button')].find((b) =>
                  (b.textContent || '').trim() === 'Load older history');
                if (!button) return false;
                button.click();
                await window.__kc.sleep(1200);
                return true;
                """
            )
            if not clicked:
                break
            session.evaluate(
                """
                for (const node of document.querySelectorAll('*')) {
                  if (node.scrollHeight > node.clientHeight && node.clientHeight > 0) node.scrollTop = 0;
                }
                await window.__kc.sleep(400);
                return true;
                """
            )
            print(f"ok   loaded older page {page_number}")

        seen = session.evaluate(
            """
            for (const node of document.querySelectorAll('*')) {
              if (node.scrollHeight > node.clientHeight && node.clientHeight > 0) node.scrollTop = 0;
            }
            await window.__kc.sleep(400);
            return document.body.innerText;
            """
        )
        check("the first message rendered after paging back", OLDER in seen)
        check(
            "the load-older control is gone once the start is loaded",
            not session.evaluate(
                """
                return !![...document.querySelectorAll('button')].find((b) =>
                  (b.textContent || '').trim() === 'Load older history');
                """
            ),
        )
        check(
            "the missing-start notice is gone",
            "Earlier history is not loaded." not in session.evaluate("return document.body.innerText;"),
        )
        bottom = session.evaluate(
            """
            for (const node of document.querySelectorAll('*')) {
              if (node.scrollHeight > node.clientHeight && node.clientHeight > 0) node.scrollTop = node.scrollHeight;
            }
            await window.__kc.sleep(500);
            return document.body.innerText;
            """
        )
        check("the newest message is still the tail", NEWEST in bottom)
        print(f"\nlogs: {tmp}")
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)
        return 0
    finally:
        if session is not None:
            session.close()
        if not args.keep:
            for process in (chrome, server):
                try:
                    os.killpg(os.getpgid(process.pid), 15)
                except ProcessLookupError:
                    pass
            provider.shutdown()
            chrome_log.close()
            server_log.close()
        else:
            print(f"kept running: server={api} chrome-profile={profile}")


if __name__ == "__main__":
    raise SystemExit(main())
