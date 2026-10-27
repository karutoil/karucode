# karucode HTTP API (v1)

All responses are JSON unless noted. Errors use:

```json
{ "error": { "code": "bad_request", "message": "human readable" } }
```

Codes: `bad_request`, `not_found`, `permission_denied`, `unauthorized`, `config`,
`provider`, `tool`, `store`, `plugin`, `timeout`, `path_escape`, `internal`.

## Auth and CSRF

* Default bind is `127.0.0.1`, no auth.
* If `KARUCODE_AUTH_TOKEN` is set, every request needs `Authorization: Bearer <token>`
  (the SSE stream uses `fetch`, so headers work). `GET /api/events` additionally
  accepts `?token=<token>`, because some stream clients cannot set headers; the
  token then appears in the URL, so prefer the header when you can.
* Every state-changing request (POST/PATCH/PUT/DELETE) must send
  `X-Karucode-Client: web`. Requests with an `Origin` header must have an origin
  whose authority matches the `Host` header (case-insensitive; values containing
  whitespace or control characters are rejected). This blocks simple cross-site
  form posts and DNS-rebinding-style cross-origin writes.
* While bound to loopback **without** a token, the request `Host` must be
  `127.0.0.1`, `localhost` or `[::1]` (any port), plus anything listed in
  `KARUCODE_ALLOWED_HOSTS` (`host`, `host:port`, or `*.example.com`). This is what
  defeats DNS rebinding; anything else is `403 permission_denied`.

## Health / config

`GET /api/health`
```json
{ "status": "ok", "version": "0.1.0", "uptime_secs": 12,
  "workspace": "/home/me/project", "auth_required": false,
  "provider_configured": true, "db": "ok" }
```
`status` is `"ok"` when the database probe passes and `"degraded"` otherwise
(the endpoint still returns 200 so a supervisor can see the reason).

`GET /api/config`
```json
{
  "workspace": "/home/me/project",
  "model": { "provider": "openai_compatible", "base_url": "http://127.0.0.1:4000/v1",
             "model": "gpt-4o-mini", "temperature": 0.2, "max_tokens": null,
             "top_p": null, "request_timeout_secs": 300,
             "parallel_tool_calls": true, "api_key_set": false },
  "limits": { "max_iterations": 32, "max_run_secs": 1800,
              "max_tool_output_bytes": 24000, "max_command_output_bytes": 32000,
              "command_timeout_secs": 120, "max_command_timeout_secs": 600,
              "max_file_read_bytes": 200000, "search_max_results": 200,
              "context_max_chars": 240000, "max_tool_calls_per_turn": 8 },
  "approvals": { "mode": "ask", "allow_commands": [], "auto_approve_paths": [],
                 "timeout_secs": 300 },
  "subagents": { "max_concurrent": 3, "max_iterations": 16,
                 "allow_recursive": false, "max_depth": 1, "timeout_secs": 900 },
  "plugins": [ { "name": "example-hello", "version": "0.1.0", "status": "ready",
                 "tools": ["plugin__example-hello__hello"], "error": null,
                 "permissions": ["files.read"] } ],
  "mcp_servers": [ { "name": "fs", "transport": "stdio", "status": "connected",
                     "tools": ["mcp__fs__read"], "error": null } ],
  "settings_locked": false,
  "available_providers": ["openai_compatible"],
  "version": "0.1.0"
}
```

`PATCH /api/config` — any subset; returns the same shape as `GET /api/config`.

```json
{ "model": { "base_url": "http://gw:4000/v1", "model": "coder", "api_key": "sk-..." },
  "approvals": { "mode": "ask_commands" },
  "limits": { "max_iterations": 40 } }
```

* `model.api_key` is write-only. Send `"clear_api_key": true` to erase it (the
  cleared state is persisted, so it also shadows `KARUCODE_API_KEY` from then on).
  It is never returned.
* Values are range-checked; out-of-range numbers are `400 bad_request` naming the
  field and its bounds rather than being silently clamped.
* `model.provider` must be one of `available_providers` (today only
  `openai_compatible`); an unknown id is refused instead of silently falling back.
* Response `403 permission_denied` when `settings_locked` is true.

`POST /api/provider/test` — probes the configured endpoint (`GET /models`, falling
back to a 1-token chat). Returns `{ "ok": bool, "models": [..], "error": null,
"latency_ms": 123 }`.

`GET /api/stats`
```json
{ "uptime_secs": 42, "rss_kb": 38211, "sessions": 3, "active_runs": 1,
  "events_emitted": 512, "db_path": "/home/me/.local/share/karucode/karucode.db" }
```
`rss_kb` is read from `/proc/self/status` and is `0` on platforms without it.
`events_emitted` is the number of events published on the shared sequence.

## Sessions

`GET /api/sessions` → `{ "sessions": [SessionSummary] }`
```json
{ "id": "s_ab12", "title": "Fix login bug", "workspace": "/w",
  "created_at": 1710000000000, "updated_at": 1710000001000,
  "message_count": 12, "last_run_status": "completed", "running": false }
```

`POST /api/sessions` body `{ "title": "optional" }` → `Session`
```json
{ "id": "s_ab12", "title": "New session", "workspace": "/w",
  "created_at": 1710000000000, "updated_at": 1710000000000 }
```

`GET /api/sessions/:id`
```json
{ "session": Session, "messages": [StoredMessage], "runs": [RunRecord],
  "agents": [AgentRecord], "approvals": [ApprovalRecord],
  "claims": [ClaimRecord], "running": false }
```
`approvals` contains **pending** approvals only; resolved and expired decisions are
not exposed by any endpoint yet. `messages` is the most recent page (newest 400,
returned in chronological order).

When the model asks for more tool calls than `limits.max_tool_calls_per_turn` in
one turn, the extra calls are not executed: each one gets a `tool` message
`[skipped: per-turn tool call limit reached …]` and a `notice` event is emitted.

`PATCH /api/sessions/:id` `{ "title": "..." }` → `Session`  
`DELETE /api/sessions/:id` → `{ "deleted": true }`

`POST /api/sessions/:id/messages` `{ "content": "..." }`
→ `202 { "run_id": "r_...", "session_id": "s_..." }`
Starts a run in the background; progress arrives on the event stream.
Returns `409` with `code: "bad_request"` if a run is already active for that
session (the HTTP status carries the conflict, the code stays in the documented
set).

`POST /api/sessions/:id/cancel` → `{ "cancelled": true }` — `false` when no run is
active (already finished or never started).

### Shapes

`StoredMessage`
```json
{ "id": 42, "session_id": "s_ab12", "run_id": "r_x", "agent_id": null,
  "role": "assistant", "content": "...", "tool_calls": [ { "id": "call_1",
  "name": "read_file", "arguments": "{\"path\":\"a\"}" } ],
  "tool_call_id": null, "name": null, "created_at": 1710000000000 }
```
`role` ∈ `system|user|assistant|tool`.

`RunRecord`
```json
{ "id": "r_x", "session_id": "s_ab12", "status": "running", "model": "gpt-4o-mini",
  "prompt": "...", "error": null, "started_at": 0, "finished_at": null }
```
`status` ∈ `running|completed|failed|cancelled|interrupted|limit_reached`.

`AgentRecord`
```json
{ "id": "a_x", "session_id": "s_ab12", "parent_run_id": "r_x", "parent_agent_id": null,
  "name": "reviewer", "role": "reviewer", "task": "Review the diff", "status": "running",
  "model": "gpt-4o-mini", "iterations": 3, "summary": "", "error": null,
  "owned_paths": ["crates/foo/**"], "created_at": 0, "updated_at": 0, "finished_at": null }
```
`status` ∈ `running|completed|failed|cancelled|interrupted`.

`ApprovalRecord`
```json
{ "id": "ap_x", "session_id": "s_ab12", "run_id": "r_x", "agent_id": null,
  "kind": "shell_command", "title": "Run cargo test", "detail": "cargo test",
  "payload": { "command": "cargo test" }, "status": "pending", "decision": null,
  "created_at": 0, "resolved_at": null }
```
`kind` ∈ `shell_command|file_write|file_edit|delete_file|network_tool|plugin|mcp|other`.
`status` ∈ `pending|resolved|expired`.

`ClaimRecord`
```json
{ "path": "crates/foo/src/lib.rs", "session_id": "s_ab12", "agent_id": "a_x",
  "mode": "write", "created_at": 0 }
```

## Approvals

`GET /api/approvals?session=s_ab12` → `{ "approvals": [ApprovalRecord] }` (pending only)  
`POST /api/approvals/:id` `{ "decision": "allow_once" | "allow_session" | "deny" }`
→ `{ "resolved": true, "decision": "allow_once" }`. `404` when unknown/expired.

## Workspace files

`GET /api/workspace/tree?path=.&depth=2` →
```json
{ "path": ".", "entries": [
  { "name": "src", "path": "src", "kind": "dir", "size": 0 },
  { "name": "Cargo.toml", "path": "Cargo.toml", "kind": "file", "size": 812 } ] }
```

`GET /api/workspace/file?path=src/main.rs` →
```json
{ "path": "src/main.rs", "content": "...", "size": 812, "truncated": false,
  "binary": false, "language": "rust" }
```
`404` for missing files, `400 path_escape` outside the workspace.

## Git

`GET /api/git/status` →
```json
{ "is_repo": true, "branch": "main", "error": null,
  "files": [ { "path": "src/x.rs", "status": "M", "staged": false } ] }
```
`GET /api/git/diff?path=&staged=false&base=` →
```json
{ "diff": "diff --git ...", "truncated": false }
```

## Plugins and MCP

`PATCH /api/config` and `GET /api/config` both return `plugins`/`mcp_servers` from
the **live** host and manager, not from startup config, so status changes are
visible immediately.

`GET /api/plugins` → `{ "plugins": [PluginDiagnostics] }`
```json
{ "name": "example-hello", "status": "ready|disabled|starting|failed|stopped",
  "manifest": { "name": "...", "version": "0.1.0", "protocol": "1",
                "capabilities": ["tools"], "permissions": ["files.read"],
                "hooks": ["before_prompt"] },
  "tools": [ { "name": "hello", "description": "...", "parameters": {} } ],
  "error": null, "pid": 1234, "restart_count": 0 }
```

`POST /api/plugins/:name/enable` `{ "enabled": true }` → `PluginDiagnostics`

`GET /api/mcp` → `{ "servers": [ { "name": "fs", "transport": "stdio",
  "status": "connected|failed|disconnected", "tools": ["mcp__fs__read"],
  "error": null } ] }`

## Event stream

`GET /api/events?session=<optional session id>` — `text/event-stream`.
Use `fetch()` plus a `ReadableStream` reader (not `EventSource`, so the auth
header travels). Each event is:

```
data: {"seq":12,"ts_ms":1710000000123,"session_id":"s_x","run_id":"r_y","agent_id":null,"type":"text_delta","delta":"Hello"}\n\n
```

A `: keepalive` comment is sent every 15 seconds. The envelope always has
`seq`, `ts_ms`, and optional `session_id`, `run_id`, `agent_id`; the event fields
are flattened alongside `type`.

| `type` | extra fields |
| --- | --- |
| `session_created` | `session_id`, `title` |
| `session_renamed` | `session_id`, `title` |
| `session_deleted` | `session_id` |
| `run_started` | `run_id`, `prompt` |
| `run_finished` | `run_id`, `status`, `error` |
| `run_cancelled` | `run_id` |
| `iteration_started` | `iteration` |
| `text_delta` | `delta` |
| `reasoning_delta` | `delta` |
| `usage` | `input_tokens`, `output_tokens` |
| `tool_call_started` | `call_id`, `name`, `arguments` (object; a **string** containing the raw text when the model emitted invalid JSON) |
| `tool_call_finished` | `call_id`, `name`, `ok`, `output`, `truncated`, `duration_ms` |
| `command_started` | `call_id`, `command`, `cwd` |
| `command_output` | `call_id`, `stream` (`stdout`/`stderr`), `chunk` |
| `command_finished` | `call_id`, `exit_code`, `duration_ms`, `truncated` |
| `approval_requested` | `approval` (ApprovalRecord shape + payload) |
| `approval_resolved` | `approval_id`, `decision` |
| `agent_spawned` | `agent_id`, `name`, `role`, `task`, `model`, `parent_run_id` |
| `agent_progress` | `agent_id`, `iterations`, `note` |
| `agent_finished` | `agent_id`, `status`, `summary`, `error` |
| `notice` | `level`, `message` |
| `error` | `message`, `kind` |

## Frontend build/dev

Vite dev server proxies `/api` to `KARUCODE_PORT` (default `8787`). In production
the Rust server serves the built `web/dist` (embedded at compile time, or from
`KARUCODE_WEB_DIR` when set).
