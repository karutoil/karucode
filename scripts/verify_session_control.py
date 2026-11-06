#!/usr/bin/env python3
"""Credential-free cross-project session control in real Chromium.

Build first: cargo build -p kc-server --bin karucode; (cd web && npm ci && npm run build).
Uses only temporary workspace, database, config, browser profile and free loopback ports.
Run: python3 scripts/verify_session_control.py [--keep] [--binary PATH] [--web-dir PATH]
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from verify_agents_tab import ROOT, Session, free_port, http_json, sse, text_chunks, tool_chunks, turns_so_far, user_text, wait_for

requests = []
class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = payload.get('messages', [])
        requests.append(payload)
        user = user_text(messages)
        match = re.search(r'PROJECT_ID=([a-zA-Z0-9_-]+)', user)
        if match and turns_so_far(messages) == 0:
            chunks = tool_chunks('dispatch-1', 'control_start_session', {
                'request_id': 'browser-control-1', 'project_id': match.group(1),
                'title': 'Browser target session', 'prompt': 'TARGET-RUN'})
        elif user == 'TARGET-RUN' and turns_so_far(messages) == 0:
            chunks = tool_chunks('hold-1', 'run_command', {'command': 'sleep 60'})
        else:
            chunks = text_chunks('Scripted control response.')
        body = sse(chunks)
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--keep', action='store_true', help='retain temporary logs and data')
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/karucode')
    parser.add_argument('--web-dir', type=Path, default=ROOT / 'web/dist')
    args = parser.parse_args()
    tmp = Path(tempfile.mkdtemp(prefix='kc-session-control-'))
    workspace = tmp / 'workspace'; workspace.mkdir()
    data = tmp / 'data'; data.mkdir()
    second = tmp / 'second-project'; second.mkdir()
    provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{provider.server_port}/v1'
    config = tmp / 'config.toml'
    config.write_text(f'''[model]\nbase_url = "{base}"\nprovider = "openai_compatible"\nmodel = "stub"\napi_key = "sk-test"\n[approvals]\nmode = "auto"\n[[providers]]\nid = "default"\nname = "Default"\nprovider = "openai_compatible"\nbase_url = "{base}"\napi_key = "sk-test"\n[[providers.models]]\nid = "stub"\nname = "Stub"\n[active]\nprovider = "default"\nmodel = "stub"\n[titles]\nenabled = false\n[notifications]\nenabled = false\n''')
    port, cdp_port = free_port(), free_port()
    api = f'http://127.0.0.1:{port}'
    server = chrome = session = None
    server_log = open(tmp / 'server.log', 'wb'); chrome_log = open(tmp / 'chrome.log', 'wb')
    try:
        server = subprocess.Popen([str(args.binary), '--port', str(port), '--workspace', str(workspace), '--data-dir', str(data), '--config', str(config)], env={**os.environ, 'KARUCODE_WEB_DIR': str(args.web_dir), 'KARUCODE_LOG': 'warn'}, stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
        chrome_env = {k: v for k, v in os.environ.items() if k not in ('DBUS_SESSION_BUS_ADDRESS', 'QT_ACCESSIBILITY')}
        chrome = subprocess.Popen(['chromium', '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', f'--remote-debugging-port={cdp_port}', f'--user-data-dir={tmp / "profile"}', '--window-size=1280,900', 'about:blank'], env=chrome_env, stdout=chrome_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for(api + '/api/health'); wait_for(f'http://127.0.0.1:{cdp_port}/json/version')
        projects = http_json(api + '/api/projects')['projects']
        assert any(p['path'] == str(workspace) for p in projects), projects
        with urllib.request.urlopen(f'http://127.0.0.1:{cdp_port}/json/list') as response:
            page = next(p for p in json.load(response) if p['type'] == 'page')
        session = Session(page['webSocketDebuggerUrl'])
        session.cdp('Page.enable'); session.cdp('Page.navigate', {'url': api + '/'})
        def js(body):
            return session.evaluate(body)
        def wait(body, timeout=20):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                result = js(body)
                if result: return result
                time.sleep(.2)
            raise AssertionError('browser timeout: ' + body + '\n' + str(js('return document.body.innerText;')))
        def click(label):
            assert wait("const b=[...document.querySelectorAll('button')].find(x => (x.getAttribute('aria-label')||x.innerText).trim() === " + json.dumps(label) + " && !x.disabled); if(b){b.click();return true} return false;"), label
        created = http_json(api + '/api/projects', 'POST', {'path': str(second), 'name': 'Second project'})
        print('Created second project:', created, flush=True)
        # Reload so the UI project registry includes the newly registered project.
        session.cdp('Page.navigate', {'url': api + '/'})
        click('Open control plane')
        # A control chat appears in its own group; create one first (the group's
        # "＋ New" button), then it is a normal row there.
        wait("const b=[...document.querySelectorAll('button')].find(x=>x.getAttribute('aria-label')==='New control chat' || x.innerText.includes('New control chat')); if(!b)return false; b.click(); return true;")
        wait("return !!document.querySelector('#composer-input');")
        central = http_json(api + '/api/control/sessions', 'POST', {})
        assert central.get('project_id') is None, central
        assert 'control-workspaces' in central.get('workspace', ''), central
        target = next(p for p in http_json(api + '/api/projects')['projects'] if p['path'] == str(second))
        prompt = 'Start target work. PROJECT_ID=' + target['id']
        js("const e=document.querySelector('#composer-input'); e.value=" + json.dumps(prompt) + "; e.dispatchEvent(new Event('input',{bubbles:true})); return true;")
        click('Send message (Command or Control + Enter)')
        def find_target():
            return next((s for s in http_json(api + '/api/sessions')['sessions'] if s['title'] == 'Browser target session'), None)
        end = time.monotonic() + 40
        while time.monotonic() < end and not find_target(): time.sleep(.2)
        target_session = find_target()
        assert target_session and target_session['project_id'] == target['id'], target_session
        assert target_session['id'] != central['session_id']
        print('PASS control chat dispatched target in second project', flush=True)
        wait("return [...document.querySelectorAll('.session-row')].some(b=>b.innerText.includes('Browser target session') && b.innerText.includes('Second project'));", 40)
        print('PASS global session list shows second-project session', flush=True)
        wait("const r=[...document.querySelectorAll('.session-row')].find(b=>b.innerText.includes('Browser target session')); if(!r)return false; const b=r.querySelector('.session-open')||r.querySelector('button'); if(!b)return false; b.click(); return true;")
        wait("return document.body.innerText.includes('TARGET-RUN') && !!document.querySelector('.button-queue');", 40)
        print('PASS navigation opened running target conversation', flush=True)
        assert js("""const e=document.querySelector('#composer-input'); e.value='STEER-FROM-CONTROL'; e.dispatchEvent(new Event('input',{bubbles:true})); await new Promise(r=>setTimeout(r,200)); const radio=[...document.querySelectorAll('.queue-mode-option')].find(x=>x.textContent.trim()==='Steer'); if(radio)radio.click(); await new Promise(r=>setTimeout(r,200)); const b=[...document.querySelectorAll('button')].find(x=>(x.getAttribute('aria-label')||'').includes('steer the running turn')); if(!b||b.disabled)return false; b.click(); return true;""")
        end = time.monotonic() + 10
        queues = {}
        while time.monotonic() < end:
            queues = http_json(api + '/api/sessions/' + target_session['id'] + '/queues')
            if queues.get('steers'): break
            time.sleep(.2)
        assert any('STEER-FROM-CONTROL' in q['content'] for q in queues.get('steers', [])), queues
        print('PASS browser steer persisted in running target session queue', flush=True)
        # Several control chats coexist, and they survive a refresh (they used to be
        # hidden by title after a reload).
        extra = http_json(api + '/api/control/sessions', 'POST', {})
        assert extra['session_id'] != central['session_id']
        wait('return document.querySelectorAll(\'[aria-label="Control chats"] .session-row\').length >= 2;', 15)
        session.cdp('Page.navigate', {'url': api + '/'})
        click('Open control plane')
        wait('return document.querySelectorAll(\'[aria-label="Control chats"] .session-row\').length >= 2;', 15)
        wait('const rows=[...document.querySelectorAll(\'[aria-label="Control chats"] .session-row\')]; if(!rows.length)return false; const b=rows[0].querySelector(\'.session-open\')||rows[0].querySelector(\'button\'); if(!b)return false; b.click(); return true;')
        wait("return !!document.querySelector('#composer-input');", 15)
        print('PASS several control chats survive a refresh and each opens', flush=True)
        # Restart: the control chat is durable, stays project-free, and its private
        # workspace must never be adopted into the project rail.
        server.terminate(); server.wait(timeout=10)
        server = subprocess.Popen([str(args.binary), '--port', str(port), '--workspace', str(workspace), '--data-dir', str(data), '--config', str(config)], env={**os.environ, 'KARUCODE_WEB_DIR': str(args.web_dir), 'KARUCODE_LOG': 'warn'}, stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for(api + '/api/health')
        control = http_json(api + '/api/control/sessions')['sessions']
        mine = next((session for session in control if session['id'] == central['session_id']), None)
        assert mine, control
        assert mine['project_id'] is None, mine
        projects_after = http_json(api + '/api/projects')['projects']
        assert not any('control-workspaces' in p['path'] for p in projects_after), projects_after
        print('PASS restart keeps control chats private and project-free', flush=True)
        print('PASS isolated workspace:', tmp)
        return 0
    finally:
        if session: session.close()
        for proc in (chrome, server):
            if proc:
                proc.terminate()
                try: proc.wait(timeout=5)
                except subprocess.TimeoutExpired: proc.kill(); proc.wait()
        provider.shutdown(); provider.server_close(); server_log.close(); chrome_log.close()
        if args.keep: print('Retained:', tmp)
        else: shutil.rmtree(tmp, ignore_errors=True)

if __name__ == '__main__':
    raise SystemExit(main())
