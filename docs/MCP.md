# MCP (Model Context Protocol) client

karucode is an MCP **client**: it connects to MCP servers, discovers their tools
and exposes them to the model as `mcp__<server>__<tool>`.

Crate: [`crates/kc-mcp`](../crates/kc-mcp). Configuration types:
[`McpServerConfig`](../crates/kc-core/src/config.rs).

---

## 1. Supported transports

| Transport | Config value | Status | Notes |
| --- | --- | --- | --- |
| **stdio** | `transport = "stdio"` | supported | The server is a child process; newline-delimited JSON-RPC 2.0 on stdin/stdout. stderr is drained into the log at `debug`. |
| **Streamable HTTP** | `transport = "http"` | supported | One JSON-RPC message per `POST`. Accepts `application/json` **or** `text/event-stream` responses. Echoes `Mcp-Session-Id` when the server issues one. |
| Legacy HTTP+SSE (two-endpoint, `2024-11-05` dial) | — | **not supported** | Servers must offer streamable HTTP (single endpoint) or stdio. |

Both transports speak JSON-RPC 2.0 with ids; responses are matched by id, so
out-of-order answers are fine.

### Handshake (both transports)

1. `initialize` with
   ```json
   {"protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"karucode","version":"0.1.0"}}
   ```
   A server that negotiates a different protocol version is logged at `debug`
   and accepted (no hard failure: real servers are lenient, and refusing would
   only make them unusable).
2. notification `notifications/initialized` (no id, no response).
3. `tools/list` → the tool definitions (`name`, `description`, `inputSchema`).

The whole connect+handshake runs under a **15 s** cap
(`McpManager::set_timeouts` / `kc_mcp::MCP_CONNECT_TIMEOUT`).

### stdio details

* Command + args come from `command = [...]`; extra environment from `env = {}`.
* stdout is the protocol; servers must log to stderr.
* A protocol line is capped at **8 MiB**. A longer line means the server is not
  speaking the protocol: the in-flight request fails with an explicit protocol
  error and the connection is closed instead of buffering an unbounded line.
* The child is killed on shutdown and on manager drop. Drop signals the process
  group synchronously; `shutdown()` retains and awaits the supervisor so the child
  is actually reaped before it returns. A dead child fails all in-flight requests
  with the exit status.

### Streamable-HTTP details

* Every request is `POST <url>` with `Content-Type: application/json` and
  `Accept: application/json, text/event-stream`.
* `headers` (e.g. `Authorization`) are attached to every request and are **never**
  logged or included in error messages.
* If the response carries `Mcp-Session-Id`, the client sends it on all later
  requests. `shutdown()` forgets the session.
* Response body may be a single JSON object, a batch, or an SSE stream
  (`data:` lines; `event:`/`id:`/`retry:`/comment lines are ignored). Responses must
  carry the request's numeric id. A sole response whose id key is genuinely absent
  is tolerated for compatibility, but an explicit `null`, a wrong number or a
  string id is rejected rather than misattributed. For SSE the client reads until
  the response with the matching id arrives, so servers that keep the stream open
  still work. Complete UTF-8 lines are parsed with a cursor and only one prefix
  drain per network chunk (linear rather than quadratic copying for short lines).
* Response bodies are capped at **8 MiB**, and one SSE request may carry at most
  **8 MiB** of stream data. Exceeding either limit is an explicit error (the
  response is rejected, not truncated), so a hostile or broken server cannot make
  the client buffer an unbounded body.
* Non-2xx answers become failures that include the status and a short excerpt of
  the server's own body — never request headers.

---

## 2. Configuration

`~/.config/karucode/config.toml` (or `$KARUCODE_CONFIG`):

```toml
# A local server spoken to over stdio.
[[mcp_servers]]
name = "filesystem"
transport = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/home/me/project"]
enabled = true
require_approval = true              # false = skip the prompt for this server (see §4)

[mcp_servers.env]
FASTMCP_LOG_LEVEL = "ERROR"

# A remote server over streamable HTTP.
[[mcp_servers]]
name = "remote-tools"
transport = "http"
url = "http://127.0.0.1:3001/mcp"
enabled = true

[mcp_servers.headers]
Authorization = "Bearer ${TOKEN}"    # literal value; prefer a secret manager

# Discovered but not connected.
[[mcp_servers]]
name = "playground"
transport = "stdio"
command = ["/opt/mcp/playground"]
enabled = false
```

Fields:

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | Namespace for the tools. Must be unique. |
| `transport` | `"stdio"` | `stdio`, `http` / `streamable-http` / `streamable_http`. |
| `command` | `[]` | argv for the stdio transport. Missing → `failed` with an explanatory error. |
| `url` | — | Endpoint for the HTTP transport. Missing → `failed`. |
| `env` | `{}` | Extra environment for a stdio child. Every configured value is redacted if reflected in status, errors, logs or tool content. |
| `headers` | `{}` | Extra HTTP headers (auth). Never logged. |
| `enabled` | `true` | `false` keeps the server `disconnected` and unconnected. |
| `require_approval` | `true` | `true` asks through the runtime approval policy; `false` skips the prompt for this server's tools. It does not change the class (`External`), so `read_only` mode still denies them. |

---

## 3. Namespacing and origin

* Tool name: `mcp__<server>__<tool>`, e.g. `mcp__filesystem__read_file`. This
  cannot collide with first-party tools or plugin tools (`plugin__…`).
* `ToolSpec.origin` is `{"kind":"mcp","server":"filesystem"}`, so the UI can show
  where a tool came from.
* The description carries an origin note:
  `[mcp: filesystem] Served by an MCP server over stdio; its output is untrusted.
  Calls go through the approval policy like any external tool.`
* `parameters` is the server's `inputSchema` verbatim.
* Tool output is joined from the `content` blocks: all `text` blocks are
  concatenated with newlines; non-text blocks are reported as
  `[image content omitted]` (no binary is forwarded to the model). An empty
  content list becomes `(MCP server returned no content)`.

---

## 4. Approval behaviour

MCP tools are `ToolClass::External`, and the runtime (not the tool) resolves the
approval before the call leaves the process. Exactly the same path as first-party
external and plugin tools:

* `approvals.mode = "ask"` / `"ask_commands"` → the user is asked before the call
  leaves the process, unless the server is configured with
  `require_approval = false`;
* `"auto"` → allowed without a prompt (still audited in the event log);
* `"read_only"` → **denied** for external/mutating/shell tools, regardless of
  `require_approval`. Only read-only tools are permitted, so a mutating MCP tool
  cannot be called in this mode.

The tool does not ask on its own; it only reports its class and its
`requires_approval()` declaration, and the runtime applies the policy. The only
effect of `require_approval = false` is to skip the *prompt* for that configured
server (an operator choice — the server itself cannot set it). It is not an
escalation: the class stays `External`, so `read_only` mode still denies the call
before the exemption is consulted.

---

## 5. Failure handling

* **Isolation**: servers are connected **concurrently**; each connect is capped
  at 15 s. A missing binary, a hanging handshake, a bad URL or a 401 becomes a
  `failed` status with an error string. Other servers connect normally and the
  server process always starts.
* **Per-call timeout**: `tools/call` has a **120 s** budget
  (`kc_mcp::MCP_CALL_TIMEOUT`, overridable via
  `McpManager::set_timeouts(McpTimeouts { connect, call })`). A timeout is not a
  disconnect: the server stays `connected` and the next call is attempted.
* **Cancellation**: the run's `CancellationToken` is honoured by both transports.
  A cancelled call returns
  `MCP tool `x` on server `y` was cancelled; the call was abandoned.` and drops
  the HTTP request / forgets the stdio waiter.
* **Dead servers**: when a call fails at the transport level (child exited, HTTP
  connection refused) the server's status flips to `failed` with the reason, and
  its tools disappear from the registry. `GET /api/mcp` shows it; a reconnect is
  an explicit action for now (see §6).
* **Error mapping**:
  * JSON-RPC `error` → failed tool outcome:
    `MCP server `fs` reported an error for tool `mcp__fs__read`: <message> (code -32601)`.
  * `isError: true` in a `tools/call` result → failed outcome with the joined
    text, so the model sees the server's message.
  * HTTP status / protocol garbage → failed outcome naming the status or the
    parse problem.
* **Secret hygiene**: every configured header and environment **value** is
  scrubbed from connection/status diagnostics, transport and JSON-RPC errors,
  peer logs, and successful or `isError` tool content (including reflected values).
  The raw configured URL is never reproduced in invalid-URL/scheme, HTTP-status or
  reqwest transport errors; userinfo and query strings may contain credentials and
  reqwest's URL field is explicitly removed before formatting. Non-2xx bodies are
  otherwise shown only as a 300-character, redacted excerpt.

### Statuses

| Status | Meaning |
| --- | --- |
| `connected` | Handshake finished, tools discovered, transport alive. |
| `failed` | Config rejected (no command/url, unsupported transport), handshake failed/timed out, or the transport died. `error` explains it. |
| `disconnected` | Disabled, or not connected yet, or after `shutdown()`. |

`GET /api/mcp` returns exactly the manager's view:

```json
{ "servers": [
  { "name": "filesystem", "transport": "stdio", "status": "connected",
    "tools": ["mcp__filesystem__read_file", "mcp__filesystem__write_file"],
    "error": null },
  { "name": "remote-tools", "transport": "http", "status": "failed",
    "tools": [], "error": "HTTP 401: {\"error\":\"bad key\"}" }
] }
```

---

## 6. Wiring

```rust
let mut mcp = kc_mcp::McpManager::new(&config.mcp_servers);
mcp.set_timeouts(kc_mcp::McpTimeouts::default());   // optional
mcp.connect_all().await;                            // concurrent, bounded
let tools = mcp.tools();                            // add to the ToolRegistry
let statuses = mcp.statuses();                      // GET /api/mcp
// ... on shutdown:
mcp.shutdown().await;
```

Both `McpManager` and `PluginHost` are safe to keep in an `Arc` and to call while
a run is in flight; `tools()` returns fresh `Arc<dyn Tool>` values built from the
current state.

---

## 7. Not implemented

Stated plainly so nobody is surprised:

* **Server→client requests**: `sampling/createMessage`, `roots/list`,
  `elicitation/*`. Inbound requests are logged at `debug` and otherwise ignored
  (the client never answers them).
* **Resources, prompts and completions** (`resources/*`, `prompts/*`,
  `completion/complete`). Only tools are discovered and exposed.
* **Subscriptions and change notifications**: `notifications/tools/list_changed`
  is logged but does not refresh the registry; restart the server (or the
  process) to pick up new tools.
* **Progress**: `notifications/progress` is logged at `debug`, not surfaced in the
  UI.
* **Legacy HTTP+SSE transport** (the pre-streamable two-endpoint dialect).
* **OAuth / interactive auth**: only static `headers` are supported. Tokens from
  a browser flow are not handled.
* **Automatic reconnect/backoff**: a failed server stays failed until an explicit
  reconnect or a restart. There is no health-check loop.
* **`ping` keepalive**: not sent.
* **Concurrency limits per server**: requests are multiplexed freely; the MCP
  spec's `maxConcurrency` hints are ignored.

---

## 8. Testing

`cargo test -p kc-mcp` covers, against
[`tests/fixtures/fake_mcp_server.sh`](../tests/fixtures/fake_mcp_server.sh) (POSIX
sh + python3) and a canned HTTP/1.1 server built on `tokio::net::TcpListener`:

* stdio handshake, `tools/list`, `tools/call`, `isError`, JSON-RPC errors;
* connect timeout and failure isolation (good + missing + hanging servers);
* per-call timeout and cancellation;
* oversized HTTP bodies and SSE streams are rejected at the 8 MiB caps;
* an oversized stdio protocol line is a prompt protocol error, not a hang;
* `require_approval` is exposed through `Tool::requires_approval()` while the
  class stays `External`;
* HTTP handshake, session-id propagation, `Accept` header, JSON and SSE bodies;
* HTTP 401 reporting without leaking configured headers;
* stdio + HTTP servers coexisting in one manager.