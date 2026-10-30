#!/bin/sh
# Fake karucode plugin used by the plugin-host tests.
#
# Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout: `initialize`,
# `tools/list`, `tools/call`, `hooks/run`, `panels/read`, `shutdown`, plus one
# startup `log` notification. Logs go to stderr (which the host drains at debug
# level).
#
# Environment knobs (set through [[plugins]].env or PluginConfig::env):
#   FAKE_PLUGIN_NAME          manifest name                       (default: fake)
#   FAKE_PLUGIN_PROTOCOL      protocol version to report          (default: 1)
#   FAKE_PLUGIN_HANG=1        never answer `initialize`           (default: off)
#   FAKE_PLUGIN_SLOW_SECS     how long the `slow` tool sleeps      (default: 5)
#   FAKE_PLUGIN_HANG_PANEL=1  never answer `panels/read`            (default: off)
#   FAKE_PLUGIN_NO_STDERR=1   do not write the stderr marker line  (default: off)
#
# Requires only POSIX sh and python3.
set -eu

exec python3 -c "
import json, os, sys, time

name = os.environ.get('FAKE_PLUGIN_NAME', 'fake')
protocol = os.environ.get('FAKE_PLUGIN_PROTOCOL', '1')
hang = os.environ.get('FAKE_PLUGIN_HANG', '') == '1'
# `panels/read` never answered: exercises the call timeout on the panel path
# without also breaking the handshake.
hang_panel = os.environ.get('FAKE_PLUGIN_HANG_PANEL', '') == '1'
try:
    slow_secs = float(os.environ.get('FAKE_PLUGIN_SLOW_SECS', '5'))
except ValueError:
    slow_secs = 5.0

manifest = {
    'name': name,
    'version': '0.1.0',
    'description': 'fixture plugin for the karucode plugin-host tests',
    'protocol': protocol,
    'capabilities': ['tools', 'hooks'],
    'permissions': ['files.read'],
    'hooks': ['before_prompt'],
    # One panel that answers `panels/read`, one that only names a tool (the
    # pre-panels/read fallback), and one that reports a failure.
    'panels': [
        {'id': 'todo', 'title': 'Todo', 'icon': 'check', 'source': 'static',
         'refresh_ms': 5000},
        {'id': 'via-tool', 'title': 'Via tool', 'source': 'tool', 'tool': 'add'},
        {'id': 'broken', 'title': 'Broken', 'source': 'static'},
        {'id': 'no-panels-read', 'title': 'Legacy', 'source': 'tool',
         'tool': 'echo'},
    ],
}

def schema(props):
    return {'type': 'object', 'properties': props}

tools = [
    {'name': 'echo', 'description': 'Echo the text argument back.',
     'parameters': schema({'text': {'type': 'string'}}), 'class': 'read_only'},
    {'name': 'add', 'description': 'Add two integers.',
     'parameters': schema({'a': {'type': 'integer'}, 'b': {'type': 'integer'}}),
     'class': 'read_only'},
    {'name': 'boom', 'description': 'Always answers ok=false.',
     'parameters': schema({}), 'class': 'read_only'},
    {'name': 'slow', 'description': 'Answers after a delay.',
     'parameters': schema({}), 'class': 'read_only'},
    {'name': 'crash', 'description': 'Kills the plugin process mid-call.',
     'parameters': schema({}), 'class': 'read_only'},
    {'name': 'deploy', 'description': 'Declares the shell class, which needs the shell permission.',
     'parameters': schema({}), 'class': 'shell'},
    {'name': 'broken', 'description': 'Listed but not implemented: exercises JSON-RPC error mapping.',
     'parameters': schema({}), 'class': 'read_only'},
    {'name': 'context', 'description': 'Echo the call context the host forwarded.',
     'parameters': schema({}), 'class': 'read_only'},
]

def send(msg):
    sys.stdout.write(json.dumps(msg) + '\n')
    sys.stdout.flush()

def error(rid, code, message):
    send({'jsonrpc': '2.0', 'id': rid, 'error': {'code': code, 'message': message}})

def result(rid, value):
    send({'jsonrpc': '2.0', 'id': rid, 'result': value})

# A notification on the protocol channel: the host logs it at debug level.
send({'jsonrpc': '2.0', 'method': 'log',
      'params': {'level': 'info', 'message': 'fake plugin starting'}})
if os.environ.get('FAKE_PLUGIN_NO_STDERR', '') != '1':
    sys.stderr.write('fake plugin: stderr drain check\n')
    sys.stderr.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        request = json.loads(line)
    except Exception as exc:
        sys.stderr.write('fake plugin: bad json: %s\n' % exc)
        continue
    method = request.get('method', '')
    rid = request.get('id')
    params = request.get('params') or {}

    if method == 'initialize':
        if hang:
            continue
        result(rid, manifest)
    elif method == 'tools/list':
        result(rid, {'tools': tools})
    elif method == 'tools/call':
        tool = params.get('name', '')
        args = params.get('arguments') or {}
        # The context the host now forwards with every call; the `context` tool
        # echoes it back so the tests can assert it really arrived.
        context = params.get('context')
        if tool == 'echo':
            result(rid, {'ok': True, 'content': 'echo: %s' % args.get('text', '')})
        elif tool == 'add':
            total = args.get('a', 0) + args.get('b', 0)
            result(rid, {'ok': True, 'content': str(total), 'meta': {'total': total}})
        elif tool == 'boom':
            result(rid, {'ok': False, 'content': 'boom failed on purpose'})
        elif tool == 'slow':
            time.sleep(slow_secs)
            result(rid, {'ok': True, 'content': 'slow done'})
        elif tool == 'crash':
            sys.stderr.write('fake plugin: crashing on purpose\n')
            sys.stderr.flush()
            sys.stdout.flush()
            os._exit(3)
        elif tool == 'deploy':
            result(rid, {'ok': True, 'content': 'deployed'})
        elif tool == 'context':
            result(rid, {'ok': True, 'content': 'context',
                         'meta': {'context': context}})
        else:
            error(rid, -32601, 'unknown tool %s' % tool)
    elif method == 'panels/read':
        if hang_panel:
            continue
        panel = params.get('panel', '')
        if panel == 'todo':
            result(rid, {'title': 'Todo', 'data': {
                'items': [{'id': 't1', 'text': 'write tests', 'status': 'open'},
                          {'id': 't2', 'text': 'ship it', 'status': 'done'}],
                'counts': {'open': 1, 'done': 1},
                # Echoed so the host can assert the call context arrived.
                'context': {
                    'session_id': params.get('session_id'),
                    'project': params.get('project'),
                    'workspace': params.get('workspace'),
                },
            }})
        elif panel == 'via-tool':
            result(rid, {'data': {'from': 'panels/read'}})
        elif panel == 'broken':
            error(rid, -32000, 'the todo list could not be read')
        elif panel == 'no-panels-read':
            # Answer as an older plugin would: the method does not exist, so the
            # host must fall back to the panel's tool.
            error(rid, -32601, 'unknown method panels/read')
        else:
            error(rid, -32601, 'unknown panel %s' % panel)
    elif method == 'hooks/run':
        hook = params.get('hook', '')
        payload = params.get('payload') or {}
        if hook == 'before_prompt':
            result(rid, {'output': 'fake plugin %s note for session %s'
                         % (name, payload.get('session_id', '?'))})
        elif hook == 'on_run_end':
            result(rid, {'output': None})
        else:
            result(rid, {'output': None})
    elif method == 'shutdown':
        result(rid, None)
        break
    else:
        error(rid, -32601, 'unknown method %s' % method)
"