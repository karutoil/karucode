#!/usr/bin/env python3
"""`New session` is a draft: nothing is written until the first message.

Proves, in real Chromium against a real kc-server:

* pressing **New session** (the rail button, twice) writes no session at all —
  the server's own list is still empty afterwards — and draws no rail row;
* the draft is a usable conversation: header title, starter prompts, enabled
  composer, mounted model/reasoning pills, and a model chosen in the draft;
* the first message creates exactly one session, titled from that message, and
  the model picked while drafting is the model the session is pinned to;
* an abandoned draft survives nothing: a reload still shows one session, and a
  new draft never inherits the previous draft's typed text.

Build first: cargo build -p kc-server --bin karucode; (cd web && npm ci && npm run build).
Uses only temporary workspace, database, config, browser profile and free loopback ports.
Run: python3 scripts/verify_draft_session.py [--keep] [--binary PATH] [--web-dir PATH]
"""
import argparse
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import ROOT, Session, free_port, http_json, sse, text_chunks, wait_for

model_calls: list[dict] = []


class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        model_calls.append(payload)
        body = sse(text_chunks('Draft-session scripted answer.'))
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = '') -> None:
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{f' — {detail}' if detail else ''}", flush=True)
    (PASS if ok else FAIL).append(name)


def config_text(base: str) -> str:
    return f'''[model]
base_url = "{base}"
provider = "openai_compatible"
model = "stub"
api_key = "sk-test"

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

[[providers.models]]
id = "stub-two"
name = "Stub Two"

[active]
provider = "default"
model = "stub"

[titles]
enabled = false

[notifications]
enabled = false
'''


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('--keep', action='store_true', help='retain temporary logs and data')
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/karucode')
    parser.add_argument('--web-dir', type=Path, default=ROOT / 'web/dist')
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix='kc-draft-session-'))
    workspace = tmp / 'workspace'
    data = tmp / 'data'
    workspace.mkdir()
    data.mkdir()

    provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{provider.server_port}/v1'
    config = tmp / 'config.toml'
    config.write_text(config_text(base))

    port, cdp_port = free_port(), free_port()
    api = f'http://127.0.0.1:{port}'
    server = chrome = session = None
    server_log = open(tmp / 'server.log', 'wb')
    chrome_log = open(tmp / 'chrome.log', 'wb')
    try:
        server = subprocess.Popen(
            [str(args.binary), '--port', str(port), '--workspace', str(workspace),
             '--data-dir', str(data), '--config', str(config)],
            env={**os.environ, 'KARUCODE_WEB_DIR': str(args.web_dir), 'KARUCODE_LOG': 'warn'},
            stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
        chrome_env = {k: v for k, v in os.environ.items()
                      if k not in ('DBUS_SESSION_BUS_ADDRESS', 'QT_ACCESSIBILITY')}
        chrome = subprocess.Popen(
            ['chromium', '--headless=new', '--no-sandbox', '--disable-gpu',
             '--disable-dev-shm-usage', f'--remote-debugging-port={cdp_port}',
             f'--user-data-dir={tmp / "profile"}', '--window-size=1280,900', 'about:blank'],
            env=chrome_env, stdout=chrome_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for(api + '/api/health')
        wait_for(f'http://127.0.0.1:{cdp_port}/json/version')
        with urllib.request.urlopen(f'http://127.0.0.1:{cdp_port}/json/list') as response:
            page = next(p for p in json.load(response) if p['type'] == 'page')
        session = Session(page['webSocketDebuggerUrl'])
        session.cdp('Page.enable')

        def js(body):
            return session.evaluate(body)

        def wait(body, timeout=20, label=''):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                value = js(body)
                if value:
                    return value
                time.sleep(0.2)
            raise AssertionError('browser timeout: ' + (label or body) + '\n' + str(js('return document.body.innerText;')))

        def sessions():
            return http_json(api + '/api/sessions')['sessions']

        def navigate():
            session.cdp('Page.navigate', {'url': api + '/'})
            wait("return !!document.querySelector('#composer-input');", 30, 'app boots')

        def click_new_session():
            # The rail's own button. Its accessible name carries the shortcut, so
            # match on the prefix rather than the exact label.
            assert js("""const b=[...document.querySelectorAll('button.new-session')]
              .find(x=>x.innerText.includes('New session') && !x.disabled);
              if(!b)return false; b.click(); return true;"""), 'New session button'

        def type_draft(text):
            js("const e=document.querySelector('#composer-input'); e.focus(); "
               f"e.value={json.dumps(text)}; e.dispatchEvent(new Event('input',{{bubbles:true}})); return true;")

        navigate()
        check('boots with no sessions', sessions() == [], str(sessions()))

        # ---- the press itself writes nothing -----------------------------
        click_new_session()
        wait("return document.querySelector('.main-title')?.textContent.trim() === 'New session';", 10,
             'header names the draft')
        check('header names the unsent draft "New session"',
              js("return document.querySelector('.main-title')?.textContent.trim();") == 'New session')
        check('the draft shows the starter prompts',
              bool(js("return !!document.querySelector('.empty-state .quick-prompt');")))
        check('the composer is usable before the session exists',
              js("const e=document.querySelector('#composer-input'); return !!e && !e.disabled;") is True)
        check('the model pills are mounted for the draft',
              bool(js("return !!document.querySelector('.composer-controls [aria-label^=\"Model for this session\"]');")))

        time.sleep(1.0)
        check('pressing New session writes no session', sessions() == [], str(sessions()))
        check('and draws no rail row',
              js("return document.querySelectorAll('.session-row').length;") == 0)

        # A second press is "focus the box", not a second draft or a create.
        click_new_session()
        time.sleep(0.5)
        check('a second press still writes nothing', sessions() == [], str(sessions()))

        # ---- the model chosen in the draft -------------------------------
        js("""const t=document.querySelector('.composer-controls [aria-label^="Model for this session"]');
          t.click(); return true;""")
        wait("return !!document.querySelector('[role=option]');", 5, 'model menu opens')
        js("""const o=[...document.querySelectorAll('[role=option]')].find(x=>x.innerText.includes('Stub Two'));
          if(!o)return false; o.click(); return true;""")
        wait("""return (document.querySelector('.composer-controls [aria-label^="Model for this session"]')?.getAttribute('aria-label')||'').includes('Stub Two');""", 5,
             'pill shows the drafted model')
        check('the draft can pick a model before it exists', True)

        # ---- the first message creates it --------------------------------
        type_draft('Draft check: what does this project do?')
        js("""const b=[...document.querySelectorAll('button')]
          .find(x=>(x.getAttribute('aria-label')||'').includes('Send message') && !x.disabled);
          if(!b)return false; b.click(); return true;""")
        wait("return document.body.innerText.includes('Draft-session scripted answer.');", 40, 'the answer streams in')

        created = sessions()
        check('the first message creates exactly one session', len(created) == 1, str(created))
        one = created[0] if created else {}
        check('the new session is titled from that message',
              (one.get('title') or '').startswith('Draft check'), one.get('title'))
        check('the session keeps the model chosen while drafting',
              (one.get('model') or {}).get('model') == 'stub-two', str(one.get('model')))
        check('the message reached the provider once', len(model_calls) == 1, str(len(model_calls)))
        check('the rail shows the new row',
              wait("return document.querySelectorAll('.session-row').length === 1;", 15) is True)
        check('the composer draft was cleared after sending',
              js("return document.querySelector('#composer-input').value;") == '')

        # ---- abandoning a draft leaves nothing behind --------------------
        click_new_session()
        wait("return document.querySelector('.main-title')?.textContent.trim() === 'New session';", 10)
        type_draft('this text must not follow the next draft')
        navigate()
        check('a reload after an abandoned draft adds no session', len(sessions()) == 1, str(sessions()))
        check('the reload keeps the one real session and no draft',
              wait("return document.querySelectorAll('.session-row').length === 1;", 20) is True)

        # Opening the session and starting another draft must start clean.
        js("""const b=[...document.querySelectorAll('.session-row button')]
          .find(x=>x.classList.contains('session-open') || x.getAttribute('aria-label')?.startsWith('Open'));
          if(!b)return false; b.click(); return true;""")
        wait("return !!document.querySelector('.composer');", 10)
        click_new_session()
        wait("return document.querySelector('.main-title')?.textContent.trim() === 'New session';", 10)
        check('a fresh draft starts with an empty box',
              js("return document.querySelector('#composer-input').value;") == '')

        shot = tmp / 'draft-1280.png'
        shot.write_bytes(__import__('base64').b64decode(
            session.cdp('Page.captureScreenshot', {'format': 'png'})['data']))
        session.cdp('Emulation.setDeviceMetricsOverride',
                    {'width': 390, 'height': 844, 'deviceScaleFactor': 2, 'mobile': True})
        time.sleep(0.6)
        overflow = js("""return {scroll: document.documentElement.scrollWidth, inner: window.innerWidth,
          title: document.querySelector('.main-title')?.textContent.trim(),
          composer: !!document.querySelector('#composer-input')};""")
        phone = tmp / 'draft-390.png'
        phone.write_bytes(__import__('base64').b64decode(
            session.cdp('Page.captureScreenshot', {'format': 'png'})['data']))
        session.cdp('Emulation.clearDeviceMetricsOverride')
        check('the draft still names itself and keeps the box on a phone',
              overflow['title'] == 'New session' and overflow['composer'] is True, str(overflow))
        check('the draft does not overflow the phone viewport',
              overflow['scroll'] <= overflow['inner'], f"{overflow['scroll']} <= {overflow['inner']}")

        print(f'\n{len(PASS)} passed, {len(FAIL)} failed', flush=True)
        print(f'screenshots: {shot} {phone}', flush=True)
        return 0 if not FAIL else 1
    finally:
        if session is not None:
            session.close()
        for process in (chrome, server):
            if process is not None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except Exception:
                    process.kill()
        provider.shutdown()
        server_log.close()
        chrome_log.close()
        if args.keep:
            print(f'kept {tmp}', flush=True)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    raise SystemExit(main())
