#!/usr/bin/env python3
"""Credential-free mission happy-path in real Chromium.

Build first: cargo build -p kc-server --bin karucode; (cd web && npm ci && npm run build).
Uses only temporary workspace, database, config, browser profile and free loopback ports.
Run: python3 scripts/verify_mission_browser.py [--keep] [--binary PATH]
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
        match = re.search(r'MISSION_ID=([a-zA-Z0-9_-]+)', user)
        if match and turns_so_far(messages) == 0:
            chunks = tool_chunks('draft-1', 'mission_draft_task', {
                'mission_id': match.group(1), 'title': 'Scripted task',
                'prompt': 'Execute scripted task and report completion', 'dependencies': []})
        else:
            chunks = text_chunks('Draft proposed for your approval.' if match else 'Scripted task completed successfully.')
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
    args = parser.parse_args()
    tmp = Path(tempfile.mkdtemp(prefix='kc-mission-browser-'))
    workspace = tmp / 'workspace'; workspace.mkdir()
    data = tmp / 'data'; data.mkdir()
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
        server = subprocess.Popen([str(args.binary), '--port', str(port), '--workspace', str(workspace), '--data-dir', str(data), '--config', str(config)], env={**os.environ, 'KARUCODE_WEB_DIR': str(ROOT / 'web/dist'), 'KARUCODE_LOG': 'warn'}, stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True)
        chrome_env = {k: v for k, v in os.environ.items() if k not in ('DBUS_SESSION_BUS_ADDRESS', 'QT_ACCESSIBILITY')}
        chrome = subprocess.Popen(['chromium', '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', f'--remote-debugging-port={cdp_port}', f'--user-data-dir={tmp / "profile"}', '--window-size=1280,900', 'about:blank'], env=chrome_env, stdout=chrome_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for(api + '/api/health'); wait_for(f'http://127.0.0.1:{cdp_port}/json/version')
        projects = http_json(api + '/api/projects')['projects']
        project = next(p for p in projects if p['path'] == str(workspace))
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
        def fill(label, value):
            assert wait("const l=[...document.querySelectorAll('label')].find(x=>x.innerText.trim().startsWith(" + json.dumps(label) + ")); const e=l?.querySelector('input,textarea'); if(!e)return false; e.value=" + json.dumps(value) + "; e.dispatchEvent(new Event('input',{bubbles:true})); return true;")
        click('Open mission control'); click('＋ New mission')
        fill('Mission title', 'Browser mission'); fill('Outcome', 'Verify real scheduling')
        click('Create mission')
        wait("return document.body.innerText.includes('MISSION CONVERSATION') && !!document.querySelector('.composer-input');")
        wait("return document.body.innerText.includes('No recorded usage; totals are unknown.') && document.body.innerText.includes('Cost: unknown. No token or cost hard cap is enforced.');")
        print('PASS mission usage and cost unknown state visible')
        missions = http_json(api + '/api/missions?project=' + project['id'])['missions']
        assert len(missions) == 1, missions
        mission_id = missions[0]['id']
        assert js("const e=document.querySelector('.composer-input'); e.value=" + json.dumps('Please draft a task. MISSION_ID=' + mission_id) + "; e.dispatchEvent(new Event('input',{bubbles:true})); return true;")
        click('Send message (Command or Control + Enter)')
        wait("return document.body.innerText.includes('Scripted task');", 40)
        mission = http_json(api + '/api/missions?project=' + project['id'])['missions'][0]
        assert len(mission['tasks']) == 1 and mission['tasks'][0]['state'] == 'draft', mission
        assert mission['schedule_state'] is None and mission['tasks'][0]['session_id'] is None, mission
        print('PASS coordinator drafted task without dispatch')
        click('Approve & start')
        wait("return document.body.innerText.includes('completed') && document.body.innerText.includes('1 / 1');", 50)
        mission = http_json(api + '/api/missions?project=' + project['id'])['missions'][0]
        task = mission['tasks'][0]
        assert task['state'] == 'completed' and task['session_id'] and task['run_id'], task
        assert any('Execute scripted task' in user_text(r.get('messages', [])) for r in requests), 'task never reached provider'
        print('PASS explicit approval dispatched separate task session and completed')
        click('View plan →')
        wait("return document.body.innerText.includes('Open task session');")
        click('Open task session →')
        wait("return document.body.innerText.includes('Scripted task completed successfully.');")
        print('PASS task session opens in browser and shows provider answer')
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
