#!/usr/bin/env python3
"""Drive the uplifted control plane in a real Chromium over raw CDP.

Build first: cargo build -p kc-server --bin karucode; (cd web && npm ci && npm run build).

What this proves, which the unit tests cannot:

  * the control plane's rail really separates **control chats** from **project
    sessions**, and a project session never appears in the chats group;
  * an untitled control chat is named by its **first message** - deterministically
    at once, then by the scripted model, which is what `[titles]` buys a chat;
  * the open chat's **model and reasoning** controls live in the composer, stay
    shown while the snapshot refreshes, and write the session pin
    (`PATCH /api/sessions/:id/model`) with the server agreeing;
  * the chat's **name** carries no model: the rail row shows the title, the
    subagent count, the status word and the time - the model belongs to the
    composer;
  * the chat's **subagents** are listed in its inspector from
    `agentsForControlChat`, and the agent API agrees;
  * `GET /api/control/overview` is one snapshot that matches what the hero
    counters draw, and control chats are excluded from its project-session lists;
  * the plane can be **left**: the rail head's *Back to sessions* is on screen and
    hit-testable at 1280 and at 390 (where the project rail that exits it on a
    desktop is `display: none`), and on a phone it lands on the sessions sheet the
    plane was entered from;
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
        # Opening the plane replaces the workspace, so focus has to move into it:
        # otherwise the next nine tab stops walk the rail behind the new screen.
        # The component retries the move for a few frames (the browser's own
        # fix-up for the unmounted trigger runs after the effect), so wait for it
        # rather than sampling once.
        wait(
            "const el=document.activeElement;"
            "const plane=document.querySelector('.control-plane.cp');"
            "return !!(plane && el && plane.contains(el));",
            10,
        )
        focused = js(
            "const el=document.activeElement;"
            "const plane=document.querySelector('.control-plane.cp');"
            "return JSON.stringify({inside: !!(plane && el && plane.contains(el)),"
            " tag: el ? el.tagName + (el.getAttribute('aria-label') ? '[' + el.getAttribute('aria-label') + ']' : '') : null});"
        )
        check('the plane holds focus once it is open', json.loads(focused).get('inside') is True, focused)

        # ------------------------------------------------- 1. the separated rail
        wait("return !!document.querySelector('[aria-label=\"Control chats\"]');")
        check('rail has a Control chats group', True)
        # Wait for the project list rather than sampling the body once: the rail
        # paints the chats group before `/api/sessions` answers.
        wait(
            "return document.body.innerText.includes('Ordinary project session');",
            30,
        )
        # The session is in its own group and not in the chats group.
        js(
            "const chats=document.querySelector('[aria-label=\"Control chats\"]');"
            "globalThis.__chatsHasOrdinary = chats ? chats.innerText.includes('Ordinary project session') : null;"
            "return true;"
        )
        check(
            'a project session is listed in the plane',
            True,
            'in the Sessions group',
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
        # Two titles land here, by design (see `@kc-doc: session-titles`): the
        # deterministic one derived from the message the moment it is accepted,
        # then the model's, written by a background turn. Polling for "any title"
        # returns the first, so this check raced the titling turn and failed
        # whenever the model's answer had not landed yet - two runs in three,
        # measured. Wait for the title this check actually claims, and report the
        # last one seen if it never arrives.
        def model_title(body: dict):
            title = body['session']['title']
            return title if title == MODEL_TITLE else None

        try:
            named = api_wait(
                f'{api}/api/sessions/{chat_id}',
                model_title,
                what=f'the model-written title {MODEL_TITLE!r}',
            )
        except AssertionError:
            named = http_json(f'{api}/api/sessions/{chat_id}')['session']['title']
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
            'the chat listing carries the model, agent counts and the control flag',
            isinstance(chat.get('model_label'), str)
            and chat['model_label'] != ''
            and isinstance(chat.get('agents'), dict)
            and chat['agents'].get('total') == 0
            and chat.get('control') is True,
            json.dumps({k: chat.get(k) for k in ('model_label', 'agents', 'control')}),
        )
        wait(
            "return [...document.querySelectorAll('[aria-label=\"Control chats\"] .session-row')]"
            ".some(r => r.innerText.includes(" + json.dumps(MODEL_TITLE) + "));",
            30,
        )
        check('the rail shows the generated title', True, MODEL_TITLE)

        # The chat's model is NOT part of its name. The rail row carries no
        # model text at all; the composer is the one place the model is named.
        rail_text = js(
            "const rows=[...document.querySelectorAll('[aria-label=\"Control chats\"] .session-row')];"
            "const row=rows.find(r => r.innerText.includes(" + json.dumps(MODEL_TITLE) + "));"
            "return row ? row.innerText : null;"
        )
        check(
            'the chat row carries no model text',
            bool(rail_text) and 'stub' not in rail_text.lower(),
            (rail_text or '').replace('\n', ' / '),
        )
        check(
            'the rail has no model badge at all',
            js("return document.querySelector('[title^=\"Model:\"]') === null;") is True,
            'no element titled "Model: …" in the rail',
        )
        composer_text = js(
            "const pill=document.querySelector('.composer-session-pins')"
            " || document.querySelector('.compose-bottom .composer-controls');"
            "return pill ? pill.innerText : null;"
        )
        check(
            'the composer is where the model is named',
            bool(composer_text) and 'Stub' in composer_text,
            (composer_text or '').replace('\n', ' / '),
        )

        # The controls STAY shown. The complaint was that they "go in and out":
        # the header copy of them was mounted and unmounted as the 5s snapshot
        # refreshed. Poll for longer than one snapshot period and require the
        # composer's pills on every single sample.
        samples = 0
        missing = 0
        for _ in range(int(8 / 0.25)):
            present = js(
                "return JSON.stringify({"
                "model: !!document.querySelector('[aria-label^=\"Model for this session\"]'),"
                "reasoning: !!document.querySelector('[aria-label^=\"Reasoning effort\"]')"
                "});"
            )
            samples += 1
            try:
                state = json.loads(present)
            except (TypeError, ValueError):
                state = {}
            if not (state.get('model') and state.get('reasoning')):
                missing += 1
            time.sleep(0.25)
        check(
            'the model and reasoning controls stay shown for 8 seconds',
            missing == 0 and samples >= 32,
            f'{samples} samples, {missing} without the controls',
        )

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
            "const row=s && s.querySelector('.agent-card');"
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
            "const b=document.querySelector('.session-row .badge');"
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
        def viewport(width: int, height: int, mobile: bool = False) -> None:
            session.cdp('Emulation.setDeviceMetricsOverride', {
                'width': width, 'height': height, 'deviceScaleFactor': 1, 'mobile': mobile,
            })
            time.sleep(0.5)

        def no_overflow(selector: str) -> str:
            return js(
                "const el=document.querySelector(" + json.dumps(selector) + ") || document.body;"
                "return JSON.stringify({scroll: el.scrollWidth, client: el.clientWidth});"
            )

        viewport(390, 844, mobile=True)
        # The overview is on screen already; open the chat at this width, because
        # at 390 the rail is hidden once a chat is open.
        wait("return !!document.querySelector('[aria-label=\"Control chats\"] .session-row');")
        js(
            "const b=document.querySelector('[aria-label=\"Control chats\"] .session-row .session-open')"
            "  || document.querySelector('[aria-label=\"Control chats\"] .session-row button');"
            "if(b) b.click();"
            "return true;"
        )
        wait("return !!document.querySelector('#composer-input');", 20)
        overflow = no_overflow('.control-plane.cp')
        metrics = json.loads(overflow)
        check(
            'the plane does not overflow a 390px phone',
            metrics['scroll'] <= metrics['client'] + 1,
            overflow,
        )
        # A control that exists but is unreadable is the failure this check is
        # for: at 390 px the reasoning pill had 86 px for a 98 px label and
        # ellipsised to "Rea…". The word now steps aside and the effort shows.
        prefix = js(
            "const p=document.querySelector('.compose-bottom .reasoning-prefix');"
            "const label=document.querySelector('.compose-bottom .reasoning-selector .model-switcher-model');"
            "return JSON.stringify({display: p ? getComputedStyle(p).display : 'missing',"
            " fits: label ? label.scrollWidth <= label.clientWidth + 1 : null,"
            " text: label ? label.innerText.trim() : null});"
        )
        pill = json.loads(prefix)
        check(
            'the reasoning pill is readable on a phone',
            pill['display'] == 'none' and pill['fits'] is True and pill['text'] not in (None, ''),
            prefix,
        )

        # 1024x768 is the tight laptop case: the intro's own "start in this
        # project" buttons were below the fold, and the hero was a 2x3 block.
        viewport(1024, 768)
        click('Back to overview')
        wait("return !!document.querySelector('[aria-label=\"Workspace summary\"]');")
        hero = js(
            "const h=document.querySelector('.cp-hero');"
            "const s=document.querySelector('.cp-stats');"
            "if(!h||!s) return null;"
            "const rows=getComputedStyle(s).gridTemplateColumns.split(' ').length;"
            "return JSON.stringify({height: Math.round(h.getBoundingClientRect().height), rows});"
        )
        hero_metrics = json.loads(hero)
        launch = js(
            "const i=document.querySelector('#cp-launch-input');"
            "if(!i) return JSON.stringify({present:false});"
            "const r=i.getBoundingClientRect();"
            "const hit=document.elementFromPoint(r.left + r.width/2, r.top + r.height/2);"
            "return JSON.stringify({present:true, top: Math.round(r.top),"
            " inViewport: r.top >= 0 && r.bottom <= innerHeight,"
            " hit: hit ? (hit === i || i.contains(hit)) : false});"
        )
        launch_metrics = json.loads(launch)
        check(
            'the overview keeps its launch box above the fold at 1024',
            launch_metrics.get('present') is True
            and launch_metrics.get('inViewport') is True
            and launch_metrics.get('hit') is True,
            launch,
        )
        check(
            'the overview counters stay one row at 1024',
            hero_metrics['rows'] == 5 and hero_metrics['height'] < 240,
            hero,
        )
        # A brand-new chat is the only one with the first-run intro, so open one:
        # the check is that its own buttons are visible, not that some other chat
        # happens to be empty.
        click('New control chat')
        wait("return !!document.querySelector('#composer-input');", 20)
        wait("return !!document.querySelector('.cp-intro-projects button');", 20)
        # The empty-chat intro is the one place a user starts work from; if its
        # buttons are under the composer they cannot be clicked at all.
        intro = js(
            "const btn=document.querySelector('.cp-intro-projects button');"
            "if(!btn) return JSON.stringify({present:false});"
            "btn.scrollIntoView({block:'nearest'});"
            "const r=btn.getBoundingClientRect();"
            "const hit=document.elementFromPoint(r.left + r.width/2, r.top + r.height/2);"
            "return JSON.stringify({present:true, inViewport: r.top >= 0 && r.bottom <= innerHeight,"
            " hit: hit ? (hit === btn || btn.contains(hit)) : false});"
        )
        intro_metrics = json.loads(intro)
        check(
            'the empty chat’s start buttons are on screen at 1024',
            intro_metrics.get('present') is True
            and intro_metrics.get('inViewport') is True
            and intro_metrics.get('hit') is True,
            intro,
        )
        # The stacked inspector must not hide its own facts: the session id and
        # its Copy button used to sit outside a capped, unfocused panel. Scroll it
        # to the end and require the Copy button to be reachable.
        inspector = js(
            "const i=document.querySelector('.cp-inspector');"
            "if(!i) return JSON.stringify({present:false});"
            "const copy=i.querySelector('.cp-facts-id .link-button');"
            "i.scrollTop = i.scrollHeight;"
            "const ir=i.getBoundingClientRect();"
            "const cr=copy ? copy.getBoundingClientRect() : null;"
            "return JSON.stringify({present:true, overflowY: getComputedStyle(i).overflowY,"
            " copyReachable: !!(cr && cr.top >= ir.top - 1 && cr.bottom <= ir.bottom + 1)});"
        )
        inspector_metrics = json.loads(inspector)
        check(
            'the facts panel does not hide the session id at 1024',
            inspector_metrics.get('present') is True
            and inspector_metrics.get('copyReachable') is True,
            inspector,
        )
        brand = js(
            "const m=document.querySelector('.cp-tile-control');"
            "return m ? JSON.stringify({scroll: m.scrollHeight, client: m.clientHeight}) : null;"
        )
        brand_metrics = json.loads(brand)
        check(
            'the k. brand mark stays inside its tile',
            brand_metrics['scroll'] <= brand_metrics['client'] + 1,
            brand,
        )

        session.cdp('Emulation.clearDeviceMetricsOverride')
        time.sleep(0.3)
        overflow = no_overflow('.control-plane.cp')
        metrics = json.loads(overflow)
        check(
            'the plane does not overflow at 1280px',
            metrics['scroll'] <= metrics['client'] + 1,
            overflow,
        )

        # ------------------------------------------------ 7. leaving the plane
        # The plane replaces the whole workspace. A desktop could always leave it
        # by clicking a project tile, which also switches project; a phone could
        # not leave it at all, because `.project-rail` is `display: none` below
        # 1024px and nothing else closed it. The rail head's **Back to sessions**
        # is that exit on both.
        def exit_box() -> str:
            return js(
                "const b=[...document.querySelectorAll('button')]"
                ".find(x => (x.getAttribute('aria-label')||'') === 'Back to sessions');"
                "if(!b) return JSON.stringify({present:false});"
                "const r=b.getBoundingClientRect();"
                "const hit=document.elementFromPoint(Math.round(r.left+r.width/2), Math.round(r.top+r.height/2));"
                "const shown=!!b.getClientRects().length && getComputedStyle(b).visibility !== 'hidden';"
                "return JSON.stringify({present:true, shown,"
                " inViewport: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,"
                " hit: !!(hit && (hit === b || b.contains(hit))),"
                " w: Math.round(r.width), h: Math.round(r.height)});"
            )

        def exit_ok(raw: str) -> bool:
            state = json.loads(raw)
            return (
                state.get('present') is True
                and state.get('shown') is True
                and state.get('inViewport') is True
                and state.get('hit') is True
            )

        def workbench_state() -> str:
            # `list` is the workbench's own session list region, not a guess from a
            # row count: leaving from a control chat lands on a project that may have
            # no sessions, and an honest empty list is still the list.
            return js(
                "const rail=document.querySelector('.rail');"
                "const sheet=!!(rail && getComputedStyle(rail).display !== 'none'"
                "  && !!(rail.getClientRects().length));"
                "return JSON.stringify({plane: !!document.querySelector('.control-plane.cp'),"
                " sheetOpen: !!(rail && rail.classList.contains('is-open')),"
                " sheetRole: rail ? rail.getAttribute('role') : null,"
                " rail: sheet,"
                " list: !!(rail && rail.querySelector('.session-nav[aria-label=\"Session list\"]')),"
                " rows: document.querySelectorAll('.rail .session-row').length});"
            )

        def sheet_rows(timeout: float = 10.0) -> int:
            """Row count once the workbench's own list read has landed."""
            deadline = time.monotonic() + timeout
            rows = 0
            while time.monotonic() < deadline:
                state = json.loads(workbench_state())
                rows = state['rows']
                if rows > 0:
                    return rows
                time.sleep(0.2)
            return rows

        # A chat is open from the section above, so the overview comes first: the
        # chat's own header offers Back to overview, then the rail head is on
        # screen with the exit in it.
        click('Back to overview')
        wait("return !!document.querySelector('[aria-label=\"Workspace summary\"]');")
        desktop_exit = exit_box()
        check(
            'a 1280px plane offers Back to sessions in its rail head',
            exit_ok(desktop_exit),
            desktop_exit,
        )
        click('Back to sessions')
        wait("return !document.querySelector('.control-plane.cp');", 10)
        left = json.loads(workbench_state())
        check(
            'the desktop leaves to the workbench rail, not to a phone sheet',
            left['plane'] is False
            and left['sheetOpen'] is False
            and left['rail'] is True
            and left['list'] is True,
            workbench_state(),
        )

        # Now the reported case: a phone, where the plane is entered from the
        # sessions sheet - so that is what it has to come back to.
        viewport(390, 844, mobile=True)
        # A phone's only route in is the sheet's Control plane row; the desktop
        # rail's button is not rendered at this width.
        click('Toggle session list')
        click('Control plane')
        wait("return !!document.querySelector('.control-plane.cp');")
        wait("return !!document.querySelector('.cp-browser .side-head');")
        phone_exit = exit_box()
        check(
            'a 390px phone offers Back to sessions in the rail head',
            exit_ok(phone_exit),
            phone_exit,
        )
        click('Back to sessions')
        wait("return !document.querySelector('.control-plane.cp');", 10)
        back = json.loads(workbench_state())
        check(
            'the phone leaves the plane onto its sessions sheet',
            back['plane'] is False
            and back['sheetOpen'] is True
            and back['sheetRole'] == 'dialog'
            and back['list'] is True,
            workbench_state(),
        )
        # The entry the reader came through still works, and an open chat keeps a
        # visible way back: with the rail dropped at phone width, Back to overview
        # is the first step and the rail head's exit is the second. The session
        # opened here is a *project* session, so the sheet it lands on has a row.
        click('Control plane')
        wait("return !!document.querySelector('.control-plane.cp');")
        # The rail's own read has to land before a row exists to open.
        wait("return !!document.querySelector('[aria-label=\"Sessions\"] .session-row .session-open');", 20)
        js(
            "const b=document.querySelector('[aria-label=\"Sessions\"] .session-row .session-open');"
            "if(b) b.click();"
            "return true;"
        )
        wait("return !!document.querySelector('#composer-input');", 20)
        hidden = json.loads(exit_box())
        chat_back = js(
            "const b=[...document.querySelectorAll('button')]"
            ".find(x => (x.getAttribute('aria-label')||'') === 'Back to overview');"
            "return !!(b && b.getClientRects().length);"
        )
        check(
            'an open chat on a phone is one Back from the exit',
            hidden.get('present') is True and chat_back is True,
            exit_box(),
        )
        click('Back to overview')
        wait("return !!document.querySelector('.cp-browser')?.getClientRects().length;")
        again = exit_box()
        check(
            'Back to overview puts the exit back on screen',
            exit_ok(again),
            again,
        )
        click('Back to sessions')
        wait("return !document.querySelector('.control-plane.cp');", 10)
        final = json.loads(workbench_state())
        final_rows = sheet_rows()
        check(
            'a phone that leaves a project session lands on that project’s list',
            final['plane'] is False
            and final['sheetOpen'] is True
            and final_rows > 0,
            workbench_state(),
        )

        # The destination is chosen when the exit is pressed, not when the plane
        # opened: a window dragged from the phone tier up to a desktop one must not
        # get a sheet that its layout has no room for.
        click('Control plane')
        wait("return !!document.querySelector('.control-plane.cp');")
        session.cdp('Emulation.clearDeviceMetricsOverride')
        time.sleep(0.4)
        click('Back to sessions')
        wait("return !document.querySelector('.control-plane.cp');", 10)
        time.sleep(0.3)
        resized = json.loads(workbench_state())
        check(
            'a window resized to desktop leaves without a phone sheet',
            resized['plane'] is False
            and resized['sheetOpen'] is False
            and resized['rail'] is True,
            workbench_state(),
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
