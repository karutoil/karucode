#!/bin/sh
# Fake MCP server (stdio transport) used by the kc-mcp tests.
#
# Newline-delimited JSON-RPC 2.0 on stdin/stdout, implementing the MCP handshake
# (`initialize`, `notifications/initialized`, `tools/list`, `tools/call`).
#
# Environment knobs:
#   FAKE_MCP_NAME        server name reported in `serverInfo`   (default: fake)
#   FAKE_MCP_FAIL=1      exit immediately, before the handshake  (default: off)
#   FAKE_MCP_HANG=1      never answer `initialize`               (default: off)
#   FAKE_MCP_SLOW_SECS   how long the `sleep` tool sleeps        (default: 30)
#
# Requires only POSIX sh and python3.
set -eu

exec python3 -c "
import json, os, sys, time

name = os.environ.get('FAKE_MCP_NAME', 'fake')
fail = os.environ.get('FAKE_MCP_FAIL', '') == '1'
hang = os.environ.get('FAKE_MCP_HANG', '') == '1'
try:
    slow_secs = float(os.environ.get('FAKE_MCP_SLOW_SECS', '30'))
except ValueError:
    slow_secs = 30.0

if fail:
    sys.stderr.write('fake mcp server: failing on purpose\n')
    sys.exit(1)

def send(msg):
    sys.stdout.write(json.dumps(msg) + '\n')
    sys.stdout.flush()

def result(rid, value):
    send({'jsonrpc': '2.0', 'id': rid, 'result': value})

def error(rid, code, message):
    send({'jsonrpc': '2.0', 'id': rid, 'error': {'code': code, 'message': message}})

tools = [
    {'name': 'echo', 'description': 'Echo text back.', 'inputSchema': {'type': 'object',
     'properties': {'text': {'type': 'string'}}}},
    {'name': 'sleep', 'description': 'Answer after a long delay.', 'inputSchema': {'type': 'object'}},
    {'name': 'fail', 'description': 'Return a tool error.', 'inputSchema': {'type': 'object'}},
    {'name': 'ghost', 'description': 'Listed but not implemented: exercises JSON-RPC error mapping.',
     'inputSchema': {'type': 'object'}},
]

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception as exc:
        sys.stderr.write('fake mcp server: bad json: %s\n' % exc)
        continue
    method = msg.get('method', '')
    rid = msg.get('id')
    params = msg.get('params') or {}

    if method == 'initialize':
        if hang:
            continue
        result(rid, {
            'protocolVersion': params.get('protocolVersion', '2024-11-05'),
            'capabilities': {'tools': {'listChanged': False}},
            'serverInfo': {'name': name, 'version': '0.1.0'},
        })
    elif method == 'notifications/initialized':
        sys.stderr.write('fake mcp server: initialized\n')
        sys.stderr.flush()
    elif method == 'tools/list':
        result(rid, {'tools': tools})
    elif method == 'tools/call':
        tool = params.get('name', '')
        args = params.get('arguments') or {}
        if tool == 'echo':
            result(rid, {'content': [{'type': 'text', 'text': 'mcp echo: %s' % args.get('text', '')}],
                         'isError': False})
        elif tool == 'sleep':
            time.sleep(slow_secs)
            result(rid, {'content': [{'type': 'text', 'text': 'slept'}], 'isError': False})
        elif tool == 'fail':
            result(rid, {'content': [{'type': 'text', 'text': 'mcp tool failed on purpose'}],
                         'isError': True})
        else:
            error(rid, -32601, 'unknown tool %s' % tool)
    elif method == 'ping':
        result(rid, {})
    else:
        error(rid, -32601, 'unknown method %s' % method)
"