#!/usr/bin/env python3
"""Minimal MCP server over stdio for karucode end-to-end tests.

Implements initialize, notifications/initialized, tools/list and tools/call
(one `echo` tool). JSON-RPC 2.0, newline-delimited.
"""

import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echo text back (mock MCP server).",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    }
]


def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = req.get("method")
        rid = req.get("id")
        params = req.get("params") or {}

        if method == "initialize":
            reply(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "result": {
                        "protocolVersion": params.get("protocolVersion", "2024-11-05"),
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "mock-mcp", "version": "0.1.0"},
                    },
                }
            )
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            reply({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            name = params.get("name")
            args = params.get("arguments") or {}
            if name == "echo":
                reply(
                    {
                        "jsonrpc": "2.0",
                        "id": rid,
                        "result": {
                            "content": [{"type": "text", "text": "mcp-echo: " + str(args.get("text", ""))}],
                            "isError": False,
                        },
                    }
                )
            else:
                reply(
                    {
                        "jsonrpc": "2.0",
                        "id": rid,
                        "error": {"code": -32601, "message": f"unknown tool {name}"},
                    }
                )
        elif rid is not None:
            reply(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {"code": -32601, "message": f"unknown method {method}"},
                }
            )


if __name__ == "__main__":
    main()