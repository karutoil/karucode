#!/usr/bin/env python3
"""Drive the uplifted control plane in a real Chromium over raw CDP.

Build first: cargo build -p kc-server --bin karucode; (cd web && npm ci && npm run build).

What this proves, which the unit tests cannot:

  * the control plane's rail really separates **control chats** from **project
    sessions**, and a project session never appears in the chats group;
  * an untitled control chat is named by its **first message** - deterministically
    at once, then by the scripted model, which is what `[titles]` buys a chat;
  * the open chat's **model and reasoning** controls write the session pin
    (`PATCH /api/sessions/:id/model`) and the server agrees;
  * the chat's **subagents** are listed in its inspector from
    `agentsForControlChat`, and the agent API agrees;
  * `GET /api/control/overview` is one snapshot that matches what the hero
    counters draw, and control chats are excluded from its project-session lists;
  * nothing overflows at 1280 px or 390 px.

Uses only a temporary workspace, database, config, browser profile and free
loopback ports. Credential-free: the provider is a local scripted SSE stub.

Run: python3 scripts/verify_control_plane.py [--keep] [--binary PATH] [--web-dir PATH]
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
    tool_chunks,
    turns_so_far,
    user_text,
    wait_for,
)

# The title the scripted model returns for a titling request. Deliberately not a
# substring of the opening message: the test must prove the *model's* answer is
# what the rail shows, not a copy of the prompt.
MODEL_TITLE = "Flaky login test audit"

PASS: list[str] = []
FAIL: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASS.append(name)
        print(f"  ok   {name}{f' — {detail}' if detail else ''}", flush=True)
    else:
        FAIL.append(name)
        print(f"  FAIL {name}{f' — {detail}' if detail else ''}", flush=True)


requests: list[dict] = []


class Provider(BaseHTTPRequestHandler):
    """A scripted OpenAI-compatible endpoint, keyed by the newest user turn."""

    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get('Content-Length') or 0)
        payload = json.loads(self.rfile.read(length) or b'{}')
        messages = payload.get('messages', [])
        requests.append(payload)
        system = (messages[0].get('content') or '').lstrip() if messages else ''
        text = user_text(messages)
        turn = turns_so_far(messages)

        if system.startswith('Write a title for a coding session'):
            # The titling turn: no tools, one short answer.
            chunks = text_chunks(MODEL_TITLE)
        elif system.startswith('You are a subagent') or system.startswith(
            'You are a focused subagent'
        ):
            chunks = text_chunks('child finished: the login test is green')
        elif 'SPAWN' in text and turn == 0:
            chunks = tool_chunks(
                'spawn-1',
                'spawn_agent',
                {'task': 'CHILD-TASK review the login test', 'name': 'login-reviewer', 'role': 'tester'},
            )
        else:
            chunks = text_chunks('Scripted control response.')

        body = sse(chunks)
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


CONFIG = '''[model]
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

TITLE_INSTRUCTION = 'Write a title for a coding session'


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('--keep', action='store_true', help='retain temporary logs and data')
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/karucode')
    parser.add_argument('--web-dir', type=Path, default=ROOT / 'web/dist')
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f'missing server binary: {args.binary}')
    if not (args.web_dir / 'index.html').exists():
        raise SystemExit(f'missing built web assets: {args.web_dir}')

    tmp = Path(tempfile.mkdtemp(prefix='kc-control-plane-'))
    workspace = tmp / 'workspace'
    workspace.mkdir()
    second = tmp / 'second-project'
    second.mkdir()
    data = tmp / 'data'
    data.mkdir()

    provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{provider.server_port}/v1'
    config = tmp / 'config.toml'
    config.write_text(CONFIG.format(base=base))

    port, cdp_port = free_port(), free_port()
    api = f'http://127.0.0.1:{port}'
    server = chrome = session = None
    server_log = open(tmp / 'server.log', 'wb')
    chrome_log = open(tmp / 'chrome.log', 'wb')
    try:
        env = {**os.environ, 'KARUCODE_WEB_DIR': str(args.web_dir), 'KARUCODE_LOG': 'warn'}
        server = subprocess.Popen(
            [
                str(args.binary),
                '--port',
                str(port),
                '--workspace',
                str(workspace),
                '--data-dir',
                str(data),
                '--config',
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
            if k not in ('DBUS_SESSION_BUS_ADDRESS', 'QT_ACCESSIBILITY')
        }
        chrome = subprocess.Popen(
            [
                'chromium',
                '--headless=new',
                '--no-sandbox',
                '--disable-gpu',
                '--disable-dev-shm-usage',
                f'--remote-debugging-port={cdp_port}',
                f'--user-data-dir={tmp / "profile"}',
                '--window-size=1280,900',
                'about:blank',
            ],
            env=chrome_env,
            stdout=chrome_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_for(api + '/api/health')
        wait_for(f'http://127.0.0.1:{cdp_port}/json/version')

        with urllib.request.urlopen(f'http://127.0.0.1:{cdp_port}/json/list') as response:
            page = next(p for p in json.load(response) if p['type'] == 'page')
        session = Session(page['webSocketDebuggerUrl'])
        session.cdp('Page.enable')
        session.cdp('Runtime.enable')
        session.cdp('Page.navigate', {'url': api + '/'})

        def js(body: str):
            return session.evaluate(body)

        def wait(body: str, timeout: float = 20.0):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if js(body):
                    return True
                time.sleep(0.2)
            raise AssertionError(
                'browser timeout: ' + body + '\n' + str(js('return document.body.innerText;'))
            )

        def click(label: str) -> None:
            ok = wait(
                "const b=[...document.querySelectorAll('button')].find(x => "
                "((x.getAttribute('aria-label')||x.innerText)||'').trim().startsWith("
                + json.dumps(label)
                + ") && !x.disabled); if(b){b.click();return true} return false;"
            )
            assert ok

        def api_wait(url: str, predicate, timeout: float = 30.0, what: str = ''):
            deadline = time.monotonic() + timeout
            last = None
            while time.monotonic() < deadline:
                last = http_json(url)
                value = predicate(last)
                if value:
                    return value
                time.sleep(0.2)
            raise AssertionError(f'{what or url} never became true: {last}')

        # ---------------------------------------------------------------- setup
        projects = http_json(api + '/api/projects')['projects']
        assert any(p['path'] == str(workspace) for p in projects), projects
        second_project = http_json(
            api + '/api/projects', 'POST', {'path': str(second), 'name': 'Second'}
        )['project']
        print('Created second project:', second_project['id'], flush=True)

        # A project session exists before the control plane opens, so "separate
        # from ordinary sessions" is a real assertion and not an empty-set one.
        project_id = second_project['id']
        ordinary = http_json(
            api + '/api/sessions', 'POST', {'title': 'Ordinary project session', 'project': project_id}
        )
        ordinary_id = ordinary.get('id') or ordinary['session']['id']
        wait_for(api + '/api/health')

        session.cdp('Page.navigate', {'url': api + '/'})
        click('Open control plane')
        wait("return !!document.querySelector('.control-plane.cp');")
        check('control plane opens', True)

        # ------------------------------------------------- 1. the separated rail
        wait("return !!document.querySelector('[aria-label=\"Control chats\"]');")
        check('rail has a Control chats group', True)
        # The project session is in its own group and not in the chats group.
        js(
            "const chats=document.querySelector('[aria-label=\"Control chats\"]');"
            "globalThis.__chatsHasOrdinary = chats ? chats.innerText.includes('Ordinary project session') : null;"
            "return true;"
        )
        body_text = js('return document.body.innerText;') or ''
        check(
            'a project session is listed in the plane',
            'Ordinary project session' in body_text,
            'body listing',
        )
        check(
            'a project session is not a control chat',
            js('return globalThis.__chatsHasOrdinary;') is False,
            'chats group excludes project sessions',
        )

        # ------------------------------------------------- 2. create and name one
        click('New control chat')
        chat = api_wait(
            api + '/api/control/sessions',
            lambda body: next(
                (s for s in body['sessions'] if s['title'] == ''), None
            ),
            what='an untitled control chat',
        )
        chat_id = chat['id']
        check('a new control chat starts untitled', chat['title'] == '', chat_id)
        check(
            'the new chat is marked as one',
            chat['control'] is True and chat['project_id'] is None,
            str({k: chat[k] for k in ('control', 'project_id') if k in chat}),
        )
        wait("return !!document.querySelector('#composer-input');")
        js(
            "const e=document.querySelector('#composer-input');"
            "e.value='audit the flaky login test';"
            "e.dispatchEvent(new Event('input',{bubbles:true}));return true;"
        )
        click('Send message')
        named = api_wait(
            f'{api}/api/sessions/{chat_id}',
            lambda body: body['session']['title'] if body['session']['title'] else None,
            what='a title on the new chat',
        )
        check(
            'the chat is named from its first message',
            named == MODEL_TITLE,
            f'{named!r}',
        )
        titled_requests = [
            request
            for request in requests
            if request['messages']
            and (request['messages'][0].get('content') or '').lstrip().startswith(TITLE_INSTRUCTION)
        ]
        check(
            'the model was asked for that title',
            bool(titled_requests),
            f'{len(titled_requests)} titling request(s)',
        )
        chat = api_wait(
            f'{api}/api/control/sessions',
            lambda body: next((s for s in body['sessions'] if s['id'] == chat_id), None),
            what='the chat in the listing',
        )
        check(
            'the chat row carries model, agent counts and the control flag',
            isinstance(chat.get('model_label'), str)
            and chat['model_label'] != ''
            and isinstance(chat.get('agents'), dict)
            and chat['agents'].get('total') == 0
            and chat.get('control') is True,
            json.dumps({k: chat.get(k) for k in ('model_label', 'agents', 'control')}),
        )
        wait(
            "return [...document.querySelectorAll('[aria-label=\"Control chats\"] .cp-row')]"
            ".some(r => r.innerText.includes(" + json.dumps(MODEL_TITLE) + "));",
            30,
        )
        check('the rail shows the generated title', True, MODEL_TITLE)
        wait("return !!document.querySelector('.cp-badge-model');")
        model_badge = js(
            "const b=document.querySelector('.cp-row .cp-badge-model');"
            "return b ? (b.innerText + '|' + (b.getAttribute('title')||'')) : null;"
        )
        check('the chat row shows a model badge', bool(model_badge), str(model_badge))

        # --------------------------------------- 3. model + reasoning selection
        click('Model for this session')
        wait("return !!document.querySelector('[role=\"option\"]');")
        picked = wait(
            "const o=[...document.querySelectorAll('[role=\"option\"]')]"
            ".find(x => x.innerText.includes('Stub Alt'));"
            "if(!o)return false; o.click(); return true;"
        )
        assert picked
        pinned = api_wait(
            f'{api}/api/sessions/{chat_id}',
            lambda body: body['session'].get('model') or None,
            what='a model pin on the chat',
        )
        check(
            'choosing a model pins this chat only',
            pinned.get('model') == 'stub-alt',
            json.dumps(pinned),
        )
        click('Reasoning effort:')
        high = wait(
            "const o=[...document.querySelectorAll('[role=\"option\"]')]"
            ".find(x => (x.innerText||'').trim().startsWith('High'));"
            "if(!o)return false; o.click(); return true;"
        )
        assert high
        effort = api_wait(
            f'{api}/api/sessions/{chat_id}',
            lambda body: (body['session'].get('model') or {}).get('reasoning_effort'),
            what='a reasoning effort on the chat',
        )
        check('choosing an effort pins it too', effort == 'high', str(effort))
        default_session = http_json(f'{api}/api/sessions/{ordinary_id}')
        check(
            'the project session kept the install default',
            not default_session['session'].get('model'),
            json.dumps(default_session['session'].get('model')),
        )

        # ------------------------------------------------ 4. subagents, listed
        js(
            "const e=document.querySelector('#composer-input');"
            "e.value='SPAWN a reviewer for the login test';"
            "e.dispatchEvent(new Event('input',{bubbles:true}));return true;"
        )
        click('Send message')
        agent = api_wait(
            f'{api}/api/agents?session={chat_id}',
            lambda body: (body.get('agents') or [None])[0],
            timeout=45,
            what='a subagent of the control chat',
        )
        check(
            'the control chat really spawned a subagent',
            agent['session_id'] == chat_id,
            f"{agent['name']} ({agent['status']})",
        )
        wait("return !!document.querySelector('[aria-label=\"Subagents of this chat\"]');", 30)
        wait(
            "const s=document.querySelector('[aria-label=\"Subagents of this chat\"]');"
            "return !!s && s.innerText.includes('login-reviewer');",
            30,
        )
        row_text = js(
            "const s=document.querySelector('[aria-label=\"Subagents of this chat\"]');"
            "const row=s && s.querySelector('.cp-agent');"
            "return row ? row.innerText : (s ? s.innerText : null);"
        )
        check(
            'the inspector lists it with its status word',
            bool(row_text) and 'login-reviewer' in row_text,
            (row_text or '').replace('\n', ' / '),
        )
        chat = api_wait(
            f'{api}/api/control/sessions',
            lambda body: next((s for s in body['sessions'] if s['id'] == chat_id), None),
            what='the chat row with agents',
        )
        check(
            'the row counts the chat’s subagents',
            chat['agents']['total'] >= 1,
            json.dumps(chat['agents']),
        )
        badge = js(
            "const b=document.querySelector('.cp-row .cp-badge-agents');"
            "return b ? (b.innerText + '|' + (b.getAttribute('title')||'')) : null;"
        )
        check('the chat row shows an agent badge', bool(badge), str(badge))

        # ------------------------------------------------------- 5. the overview
        click('Back to overview')
        wait("return !!document.querySelector('[aria-label=\"Workspace summary\"]');")
        overview = http_json(api + '/api/control/overview')
        for key in ('generated_at', 'sessions', 'agents', 'projects', 'control', 'running', 'attention', 'recent', 'live_agents'):
            assert key in overview, f'{key} missing from the overview: {overview}'
        check(
            'the overview lists the chat as a control chat',
            any(row['id'] == chat_id for row in overview['control']),
            f"{len(overview['control'])} chats",
        )
        check(
            'project-session lists exclude control chats',
            all(
                row['id'] != chat_id
                for key in ('running', 'attention', 'recent')
                for row in overview[key]
            ),
            'running/attention/recent',
        )
        check(
            'the overview counts every project session',
            overview['sessions']['total'] >= 1 and overview['sessions']['control'] >= 1,
            json.dumps(overview['sessions']),
        )
        summary = js(
            "const s=document.querySelector('[aria-label=\"Workspace summary\"]');"
            "return s ? s.innerText.replace(/\\n/g,' ') : null;"
        )
        check(
            'the hero draws those counts',
            bool(summary)
            and re.search(
                rf"{overview['sessions']['control']}\s+CHATS", summary, re.IGNORECASE
            )
            is not None,
            str(summary),
        )
        raw = json.dumps(overview)
        check('the overview carries no credential', 'sk-' not in raw, 'no sk- in the snapshot')

        # --------------------------------------------------- 6. narrow viewport
        session.cdp('Emulation.setDeviceMetricsOverride', {
            'width': 390, 'height': 844, 'deviceScaleFactor': 2, 'mobile': True,
        })
        time.sleep(0.6)
        overflow = js(
            "const el=document.querySelector('.control-plane.cp') || document.body;"
            "return JSON.stringify({scroll: el.scrollWidth, client: el.clientWidth});"
        )
        metrics = json.loads(overflow)
        check(
            'the plane does not overflow a 390px phone',
            metrics['scroll'] <= metrics['client'] + 1,
            overflow,
        )
        session.cdp('Emulation.clearDeviceMetricsOverride')
        time.sleep(0.3)
        overflow = js(
            "const el=document.querySelector('.control-plane.cp') || document.body;"
            "return JSON.stringify({scroll: el.scrollWidth, client: el.clientWidth});"
        )
        metrics = json.loads(overflow)
        check(
            'the plane does not overflow at 1280px',
            metrics['scroll'] <= metrics['client'] + 1,
            overflow,
        )

        print(flush=True)
        if FAIL:
            print(f'{len(FAIL)} FAILED, {len(PASS)} passed', flush=True)
            for name in FAIL:
                print('  FAIL', name, flush=True)
            print('Retained for inspection:', tmp, flush=True)
            return 1
        print(f'all {len(PASS)} checks passed', flush=True)
        print('PASS isolated workspace:', tmp, flush=True)
        return 0
    finally:
        if session:
            session.close()
        for proc in (chrome, server):
            if proc:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
        provider.shutdown()
        provider.server_close()
        server_log.close()
        chrome_log.close()
        if args.keep:
            print('Retained:', tmp, flush=True)
        elif FAIL:
            print('Logs:', tmp, flush=True)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    raise SystemExit(main())
