#!/bin/sh
# Fake karucode plugin used by the plugin-host tests.
#
# Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout: `initialize`,
# `tools/list`, `tools/call`, `hooks/run`, `shutdown`, plus one startup `log`
# notification. Logs go to stderr (which the host drains at debug level).
#
# Environment knobs (set through [[plugins]].env or PluginConfig::env):
#   FAKE_PLUGIN_NAME          manifest name                       (default: fake)
#   FAKE_PLUGIN_PROTOCOL      protocol version to report          (default: 1)
#   FAKE_PLUGIN_HANG=1        never answer `initialize`           (default: off)
#   FAKE_PLUGIN_SLOW_SECS     how long the `slow` tool sleeps      (default: 5)
#   FAKE_PLUGIN_NO_STDERR=1   do not write the stderr marker line  (default: off)
#
# Requires only POSIX sh and python3.
set -eu

exec python3 -c "
import json, os, sys, time

name = os.environ.get('FAKE_PLUGIN_NAME', 'fake')
protocol = os.environ.get('FAKE_PLUGIN_PROTOCOL', '1')
hang = os.environ.get('FAKE_PLUGIN_HANG', '') == '1'
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
        else:
            error(rid, -32601, 'unknown tool %s' % tool)
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