#!/bin/sh
# A plugin whose only tool raises a `notify` frame.
#
# A plugin cannot reach a browser — no route mounting, no script execution — so
# `notify` is the single channel by which a plugin can ask a human to look at
# something. This fixture exists so that channel can be exercised end to end
# through a real plugin process: the host validates the frame, the server turns it
# into an `Event::Notification`, and the browser renders it.
#
# It is deliberately minimal (POSIX sh + python3, no cargo build) so the end-to-end
# suite does not depend on a compiled binary. `scripts/e2e_smoke.py` registers it
# when this file is present; the model reaches it with the `[tool:notify]` hint.
#
# It also advertises `on_notify`, which makes it observe every notification the
# runtime emits — the "out" half of the plugin notification story.
set -eu

exec python3 -c "
import json, sys

name = 'notifier-fixture'
tools = [{
    'name': 'ping',
    'description': 'Raise a notification for a human, then answer.',
    'parameters': {'type': 'object', 'properties': {}},
    'class': 'read_only',
}]
manifest = {
    'name': name,
    'version': '0.1.0',
    'description': 'fixture: raises a notification when its tool is called',
    'protocol': '1',
    'capabilities': ['tools', 'notifications'],
    'permissions': ['*'],
    'hooks': ['on_notify'],
    'tools': tools,
}

def send(message):
    sys.stdout.write(json.dumps(message) + '\n')
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        request = json.loads(line)
    except ValueError:
        continue
    method = request.get('method', '')
    rid = request.get('id')
    params = request.get('params') or {}

    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': rid, 'result': manifest})
    elif method == 'tools/list':
        send({'jsonrpc': '2.0', 'id': rid, 'result': {'tools': tools}})
    elif method == 'tools/call':
        context = params.get('context') or {}
        # The frame a real plugin would write: a kind, a title, optional body,
        # session and stickiness. The host validates every field.
        send({'jsonrpc': '2.0', 'method': 'notify', 'params': {
            'kind': 'needs_input',
            'title': 'The plugin needs a decision',
            'body': 'raised from plugin__notifier-fixture__ping',
            'session_id': context.get('session_id') or '',
            'sticky': True,
        }})
        send({'jsonrpc': '2.0', 'id': rid,
              'result': {'ok': True, 'content': 'notified'}})
    elif method == 'panels/read':
        send({'jsonrpc': '2.0', 'id': rid, 'result': {'data': {}}})
    elif method == 'hooks/run':
        # on_notify arrives here; echoing to stderr keeps stdout protocol-clean.
        hook = params.get('hook', '')
        if hook == 'on_notify':
            payload = params.get('payload') or {}
            sys.stderr.write('notifier-fixture saw %s: %s\n'
                             % (payload.get('kind', '?'), payload.get('title', '?')))
            sys.stderr.flush()
        send({'jsonrpc': '2.0', 'id': rid, 'result': {'output': None}})
    elif method == 'shutdown':
        send({'jsonrpc': '2.0', 'id': rid, 'result': None})
        break
    else:
        send({'jsonrpc': '2.0', 'id': rid,
              'error': {'code': -32601, 'message': 'unknown method %s' % method}})
"
