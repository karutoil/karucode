# karucode architecture

Web-first, self-hosted coding agent. One Rust binary serves the API, the SSE event
stream and the built frontend. No Node.js at runtime.

```
 browser (Preact + Vite)
        │  REST + SSE (fetch)
        ▼
 kc-server (axum)
   ├── config/settings layer (env + file + SQLite overrides)
   ├── project registry (workspace per run)
   ├── kc-runtime ── agent loop, approvals, subagents, context budget
   │     ├── kc-provider  (OpenAI-compatible streaming)
   │     ├── kc-tools     (files, search, shell, git)
   │     ├── kc-plugin-host (out-of-process plugins)
   │     ├── kc-mcp       (MCP stdio + HTTP tools)
   │     └── kc-core      (types, trait contracts)
   └── kc-store (SQLite, WAL)
```

Dependency direction: `kc-core` depends only on `kc-docs` (the compiled doc
registry). `kc-store`, `kc-provider` depend only on `kc-core`; `kc-tools`
depends on `kc-core` and `kc-docs`. `kc-runtime` depends only on `kc-core` and
implements the loop against traits (`Store`, `ProviderFactory`, `Settings`,
`Tool`, `Hooks`), which keeps it testable with fakes. `kc-server` wires the
concrete implementations together.

## Layers and ownership

| Crate | Responsibility |
| --- | --- |
| `kc-core` | ids, messages, config, approval policy, workspace path confinement, event schema, tool/provider/store/hooks traits, plugin protocol types, system prompt |
| `kc-store` | SQLite schema + queries (projects, sessions, messages, runs, agents, approvals, settings, claims) |
| `kc-provider` | `POST /chat/completions` streaming, SSE parsing, tool-call deltas, errors |
| `kc-tools` | read/list/search/write/edit/command/git, output bounds, process-group kill |
| `kc-runtime` | multi-step loop, streaming aggregation, approvals, cancellation, subagents, context trimming, run recovery |
| `kc-plugin-host` | discovery, supervision, JSON-RPC over stdio, hook fan-out |
| `kc-mcp` | MCP client (stdio + streamable HTTP), tool namespacing |
| `kc-plugin-sdk` | small crate used to write plugins |
| `kc-docs` | `@kc-doc` block parser, compile-time doc table, `kc-docs` CLI (`--write`/`--check`), search registry |
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

## Workspace per run (projects)

A project is a row in `projects` (id, name, canonical `path`, timestamps); every
session carries a nullable `project_id` (`NULL` for rows written before schema v2
and for sessions a delete detached). One server instance therefore serves several
directories without a mutable "current workspace":

* `kc_runtime::RuntimeDeps` holds a `WorkspaceProvider`. `start_run` resolves the
  workspace from the session's `project_id` *before* it writes the run row, so a
  run cannot start in the wrong directory.
* Subagents share the parent's live workspace (`worktree.rs` retains only the
  shared guard). They see dirty and untracked parent files, and completed edits
  remain in place. There is no automatic checkout or destructive cleanup to hand
  work back. `owned_paths` provides advisory conflict reporting, not isolation or
  locking, so callers should assign disjoint paths.
* `kc-server` implements the trait with `state::ProjectRegistry`: the default
  workspace plus a map of project id → already-canonicalised `Workspace`. The
  registry is process memory, not a query, because every file tool, workspace
  listing and git panel resolves through it — a SQLite round-trip per tool call
  would put the database in the hot path, and `Workspace` canonicalisation should
  happen once rather than per request. SQLite stays the source of truth: rows are
  written first and the map is updated in step (`create` rolls the row back if the
  directory cannot be registered; `delete` removes the entry after the rows are
  gone).
* A project whose directory disappears keeps its row; resolution then fails with
  "not available; its directory may have been removed", so a run is refused
  instead of silently running in the default workspace.

`bootstrap::seed_projects` runs before the runtime is built:

* with no projects at all, one is created for the configured workspace;
* a session with `project_id IS NULL` is attached to the project whose `path`
  matches its recorded `workspace`, creating that project if this is the first
  session in that directory. A recorded workspace that no longer exists is logged
  and left unattached, not moved to a different repository.

## Provider profiles and resolution

Endpoints, credentials and the curated model library live in `ProviderProfile`s
(`[[providers]]` in the config file, `model.providers` rows in SQLite, or the
reserved `env` profile synthesised from environment variables). The runtime never
reads a profile directly: `SettingsHandle::model()` returns
`Config::effective_model()`, which flattens the active profile (base URL, provider
implementation id, key, timeout, `active_model`) into one `ModelConfig`. Sampling
knobs — `temperature`, `max_tokens`, `top_p`, `parallel_tool_calls` — stay
install-wide, so switching profiles does not silently change them.

`Config::normalize_providers()` is the single repair and normalisation point. It
runs when configuration is loaded, after the persisted rows are applied, and after
every accepted `PATCH /api/config`:

* no profiles → a `default` profile is synthesised from the flat `[model]`
  section, which is what a config file, the environment or a pre-profiles database
  provides;
* unknown `active_provider` → the first profile; unknown `active_model` → the
  profile's first model, then the flat `model.model` (a profile may legitimately
  have no models yet);
* the resolved connection is mirrored into the flat `model` section so legacy
  readers, `GET /api/config` and provider probes all see the same endpoint. The
  key is deliberately **not** mirrored: it lives on the profile, so it is stored
  and redacted in one place.

`KARUCODE_MODEL` / `KARUCODE_BASE_URL` / `KARUCODE_API_KEY` (with
`OPENAI_BASE_URL` / `OPENAI_API_KEY` as fallbacks) win over `[[providers]]`
tables: when any of them is set, an `env` profile is created and selected. That
keeps one resolution path for profiles, environment and the flat section.

Everything that needs to know which endpoint will be called reads that
resolution: the agent loop, `GET /api/config`'s `model`, `GET /api/health`'s
`provider_configured`, and `POST /api/provider/test`. A probe of a profile that
is not selected uses that profile's first model
(`Config::model_for_profile`).

## Trust boundaries

* File tools resolve through `Workspace`: lexical `..`/absolute checks plus
  canonicalisation of the existing ancestor. Symlink escapes are refused.
* Shell commands are **not sandboxed**. They run as the server user in their own
  process group. The approval policy is the only gate; this is stated in the UI
  and the system prompt.
* Model output, tool output, plugin output and MCP output are untrusted. The UI
  renders Markdown with raw HTML disabled and sanitises the result.
* API keys live in the config file/env or the SQLite settings table (file mode
  0600) and never leave the server; the browser only sees `api_key_set`, and every
  profile's key is added to the redaction set used for outgoing error messages.
* Browser CSRF: custom header required on mutations + Origin/Host match. Remote
  binds require `KARUCODE_AUTH_TOKEN` at startup.

## Plugin execution model

Plugins run **out of process**, speaking newline-delimited JSON-RPC 2.0 over
stdio, supervised by `kc-plugin-host`. See `docs/PLUGINS.md` for the tradeoff
analysis against Wasm and in-process Rust plugins.

## Extension points

`Provider` / `ProviderFactory`, `Tool`, `Store`, `Settings`, `Hooks`,
`WorkspaceProvider`, plugin protocol v1, MCP servers. Adding a native provider
means implementing `Provider` and registering it in the server's factory; nothing
in the runtime changes.
