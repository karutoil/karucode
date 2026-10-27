# karucode architecture

Web-first, self-hosted coding agent. One Rust binary serves the API, the SSE event
stream and the built frontend. No Node.js at runtime.

```
 browser (Preact + Vite)
        │  REST + SSE (fetch)
        ▼
 kc-server (axum)
   ├── config/settings layer (env + file + SQLite overrides)
   ├── kc-runtime ── agent loop, approvals, subagents, context budget
   │     ├── kc-provider  (OpenAI-compatible streaming)
   │     ├── kc-tools     (files, search, shell, git)
   │     ├── kc-plugin-host (out-of-process plugins)
   │     ├── kc-mcp       (MCP stdio + HTTP tools)
   │     └── kc-core      (types, trait contracts)
   └── kc-store (SQLite, WAL)
```

Dependency direction: `kc-core` has no workspace deps. `kc-store`, `kc-provider`,
`kc-tools` depend only on `kc-core`. `kc-runtime` depends only on `kc-core` and
implements the loop against traits (`Store`, `ProviderFactory`, `Settings`,
`Tool`, `Hooks`), which keeps it testable with fakes. `kc-server` wires the
concrete implementations together.

## Layers and ownership

| Crate | Responsibility |
| --- | --- |
| `kc-core` | ids, messages, config, approval policy, workspace path confinement, event schema, tool/provider/store/hooks traits, plugin protocol types, system prompt |
| `kc-store` | SQLite schema + queries (sessions, messages, runs, agents, approvals, settings, claims) |
| `kc-provider` | `POST /chat/completions` streaming, SSE parsing, tool-call deltas, errors |
| `kc-tools` | read/list/search/write/edit/command/git, output bounds, process-group kill |
| `kc-runtime` | multi-step loop, streaming aggregation, approvals, cancellation, subagents, context trimming, run recovery |
| `kc-plugin-host` | discovery, supervision, JSON-RPC over stdio, hook fan-out |
| `kc-mcp` | MCP client (stdio + streamable HTTP), tool namespacing |
| `kc-plugin-sdk` | small crate used to write plugins |
| `kc-server` | axum routes, SSE fan-out, auth/CSRF, static assets, wiring, shutdown |

## Agent loop

1. Persist the user message and a `runs` row (`running`).
2. Build the prompt: system prompt + hook contributions + trimmed history.
3. Stream the provider; emit `text_delta`/`reasoning_delta`; accumulate tool calls.
4. For each tool call (bounded count): emit `tool_call_started`, resolve approvals,
   execute with cancellation, truncate output, persist a `tool` message.
5. Append the assistant message (text + tool calls), feed results back, repeat.
6. Stop on completion (no tool calls), cancellation, iteration/time limit, or
   provider failure. Persist the terminal status and emit `run_finished`.

Every run holds a `CancellationToken`. Cancelling aborts the HTTP stream, kills
shell process groups, expires pending approvals, and marks the run `cancelled`.
On startup `recover()` marks leftover `running` runs/agents `interrupted` and
pending approvals `expired`, so interrupted work is visible instead of hidden.

## Trust boundaries

* File tools resolve through `Workspace`: lexical `..`/absolute checks plus
  canonicalisation of the existing ancestor. Symlink escapes are refused.
* Shell commands are **not sandboxed**. They run as the server user in their own
  process group. The approval policy is the only gate; this is stated in the UI
  and the system prompt.
* Model output, tool output, plugin output and MCP output are untrusted. The UI
  renders Markdown with raw HTML disabled and sanitises the result.
* API keys live in the config file/env or the SQLite settings table (file mode
  0600) and never leave the server; the browser only sees `api_key_set`.
* Browser CSRF: custom header required on mutations + Origin/Host match. Remote
  binds require `KARUCODE_AUTH_TOKEN` at startup.

## Plugin execution model

Plugins run **out of process**, speaking newline-delimited JSON-RPC 2.0 over
stdio, supervised by `kc-plugin-host`. See `docs/PLUGINS.md` for the tradeoff
analysis against Wasm and in-process Rust plugins.

## Extension points

`Provider` / `ProviderFactory`, `Tool`, `Store`, `Settings`, `Hooks`, plugin
protocol v1, MCP servers. Adding a native provider means implementing `Provider`
and registering it in the server's factory; nothing in the runtime changes.
