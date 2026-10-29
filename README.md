# karucode

A small, self-hosted coding agent with a browser workspace. Rust backend, one
binary, SQLite state, any OpenAI-compatible endpoint (including a local gateway),
and a UI designed for long sessions rather than for a dashboard demo.

```
┌──────────────────────────────── browser (Preact) ────────────────────────────────┐
│ sessions │                      chat / stream                    │ workspace     │
└──────────┴───────────────────────────────────────────────────────┴───────────────┘
                 REST + SSE  (same origin, no websocket server)
┌──────────────────────────────── karucode (Rust) ─────────────────────────────────┐
│ axum API  ·  agent runtime  ·  provider  ·  tools  ·  subagents  ·  plugins/MCP  │
│ SQLite (WAL): projects, sessions, runs, agents, settings, claims                      │
└──────────────────────────────────────────────────────────────────────────────────┘
```

It does the actual work: reads the project, searches it, edits files, runs
commands with streamed output, inspects git, delegates to subagents, and keeps
going through multi-step tasks until the work is done or you stop it. Every
mutation that matters asks for approval by default.

## What is implemented

**Agent runtime**
- Multi-step loop: stream → tool calls → results → repeat, bounded by iterations,
  wall-clock time, per-turn tool-call count and context budget.
- Streaming text, reasoning and tool-call deltas; tool calls split across chunks
  are reassembled.
- Cancellation aborts the provider stream, kills shell process groups, expires
  pending approvals and marks the run `cancelled`.
- Restart recovery marks leftover `running` runs/agents as `interrupted` and
  pending approvals as `expired`; history is durable in SQLite.
- Bounded tool output with explicit truncation markers (never silent loss).
- Practical system prompt, provider-agnostic, no model-specific assumptions.

**Tools**: `read_file`, `list_dir`, `search_files` (ripgrep, pure-Rust fallback),
`write_file`, `edit_file` (exact-match, ambiguity-safe), `run_command` (streamed,
process group, timeout, output cap), `git_status`, `git_diff`, plus `spawn_agent`.

**Workspace containment**: every file tool resolves through a confined workspace
(no `..` escapes, no absolute paths outside, canonicalisation blocks symlink
escapes, atomic writes). Shell commands are explicitly **not** sandboxed.

**Projects**: a project is a canonical directory plus a display name; sessions can
be grouped by project and each run is confined to its session's project directory
(a session without a project uses the default directory).
`--workspace` / `KARUCODE_WORKSPACE` is that default. On first start one project
is created for the configured workspace, and sessions from before projects existed
are attached to the project matching their recorded directory (creating it if
needed); a workspace that has disappeared keeps its history and stays unattached.
Subagents inherit the workspace of the run that spawned them. Deleting a project
removes its rows, never files on disk. The web client lists and switches projects
through `/api/projects` and the `?project=`-scoped session, workspace-file/tree and
git requests.

**Provider profiles**: a config file can declare several named OpenAI-compatible
endpoints, each with its own key, timeout and curated model list; `active_provider`
/ `active_model` (or the equivalent `PATCH /api/config`) select what every run
uses. Model metadata (context window, output limit, tool calling, reasoning, cost)
can be prefilled from the models.dev catalog, which the server fetches and caches.
See [Provider profiles](#provider-profiles-and-the-model-library).

**Approvals**: `ask` by default for shell + writes, with command prefix
allowlists, per-path auto-approval, `ask_commands`, `auto` and `read_only` modes;
`allow once` / `allow for session` / `deny` in the UI, persisted and audited.

**Subagents**: bounded concurrent delegation with a semaphore, inherited model
and settings (optional model override), nested transcripts persisted with
`agent_id`, progress + result in the UI, cancellation propagates, recursion off by
default, advisory `owned_paths` claims. A subagent runs under the same
checkpoint rule as a run: `subagents.max_iterations` per segment, refreshed up to
`subagents.max_continues` times inside `subagents.timeout_secs` (4h by default),
so a delegated coding task survives its first budget instead of dying at it.

**UI**: streaming Markdown chat with code blocks and safe rendering, tool activity
cards, command output panel, approval cards, workspace explorer + file viewer,
git changes/diff view, subagent panel, model/connection settings, keyboard
shortcuts, responsive down to mobile, accessible controls.

**Extensions**: out-of-process plugins (JSON-RPC over stdio, supervised, crash
isolated, versioned protocol v1, capability/permission declarations, hooks) with a
working example plugin; MCP client for stdio and streamable-HTTP servers with
namespaced `mcp__<server>__<tool>` tools. See [docs/PLUGINS.md](docs/PLUGINS.md)
and [docs/MCP.md](docs/MCP.md).

**Self-documentation**: the facts a user needs about karucode itself are written as
`@kc-doc` comments next to the code that implements them and compiled into the
binary, so they cannot drift from the build. The model searches and reads them
through the read-only `karucode_docs` tool; the same entries are mirrored to
[docs/internal](docs/internal/) for humans. See [docs/INTERNAL-DOCS.md](docs/INTERNAL-DOCS.md)
and the contribution contract in [AGENTS.md](AGENTS.md).

## Prerequisites

* Linux (primary target; other Unixes should work). Rust 1.82+ (built and tested
  with 1.96).
* Node.js 20+ **only for building the frontend**. Not needed at runtime: the
  built UI is embedded in, or served by, the Rust process.
* Optional: `ripgrep` (faster search; a pure-Rust fallback is built in), `git`.

## Quick start

```bash
git clone <repo> karucode && cd karucode

# 1. build the UI once
cd web && npm ci && npm run build && cd ..

# 2. build and run the server
export KARUCODE_BASE_URL=http://127.0.0.1:4000/v1   # your gateway
export KARUCODE_API_KEY=sk-...                     # if the gateway needs one
export KARUCODE_MODEL=gpt-4o-mini
cargo run --release -p kc-server -- --workspace /path/to/your/project
```

Open <http://127.0.0.1:8787>. Create a session, type a task, approve the commands
you want it to run.

Useful flags: `--help`, `--port`, `--host`, `--workspace`, `--data-dir`,
`--config`. Flags override environment variables.

Try it on the bundled demo (a Node project with a real bug):

```bash
cargo run --release -p kc-server -- --workspace examples/demo-app
# prompt: "Run the tests, find the failing cases, fix the bug in src/math.js, re-run the tests"
```

## Configuration

Precedence, highest first:

1. overrides saved from the UI (SQLite `settings` table);
2. environment variables;
3. config file (`KARUCODE_CONFIG`, default `~/.config/karucode/config.toml`);
4. built-in defaults.

`KARUCODE_LOCK_SETTINGS=1` removes layer 1, so a deployment can pin settings from
the environment and reject browser edits with 403.

`[[providers]]` tables in the config file are part of layer 3. When any of
`KARUCODE_MODEL`, `KARUCODE_BASE_URL`, `KARUCODE_API_KEY` (or `OPENAI_BASE_URL` /
`OPENAI_API_KEY`) is set, the environment wins for the connection: the server
expresses it as a reserved `env` profile and selects it, so the file's profiles
stay defined but are not active.

See [config.example.toml](config.example.toml) and [.env.example](.env.example) for
every key. The important ones:

| Variable | Default | Meaning |
| --- | --- | --- |
| `KARUCODE_HOST` / `KARUCODE_PORT` | `127.0.0.1` / `8787` | bind address; non-loopback requires a token |
| `KARUCODE_WORKSPACE` | cwd | default workspace root (for sessions without a project) |
| `KARUCODE_DATA_DIR` | `~/.local/share/karucode` | SQLite + plugin data |
| `KARUCODE_AUTH_TOKEN` | unset | Bearer token for remote access (≥16 chars) |
| `KARUCODE_BASE_URL` | `http://127.0.0.1:4000/v1` | OpenAI-compatible base URL (wins over `[[providers]]`) |
| `KARUCODE_API_KEY` | unset | provider key (falls back to `OPENAI_API_KEY`) |
| `KARUCODE_MODEL` | `gpt-4o-mini` | model id (wins over `[[providers]]`) |
| `KARUCODE_TEMPERATURE` / `KARUCODE_MAX_TOKENS` | `0.2` / unset | sampling |
| `KARUCODE_MODELS_DEV_URL` | `https://models.dev/api.json` | catalog source for the model library (read once at startup) |
| `KARUCODE_APPROVAL_MODE` | `ask` | `ask`, `ask_commands`, `auto`, `read_only` |
| `KARUCODE_ALLOW_COMMANDS` | empty | comma-separated command prefixes never prompted |
| `KARUCODE_MAX_ITERATIONS` | `32` | model turns per run |
| `KARUCODE_COMMAND_TIMEOUT_SECS` | `120` | default shell timeout |
| `KARUCODE_MAX_TOOL_OUTPUT_BYTES` | `24000` | output kept for the model |
| `KARUCODE_CONTEXT_MAX_CHARS` | `240000` | conversation budget before trimming |
| `KARUCODE_SUBAGENT_CONCURRENCY` | `3` | concurrent subagents |
| `KARUCODE_ALLOW_RECURSIVE_SUBAGENTS` | `false` | allow nested delegation |
| `KARUCODE_LOG` | `info` | tracing filter |
| `KARUCODE_ALLOWED_HOSTS` | empty | extra `Host` names accepted by the loopback-only guard (see below) |

### Loopback, the `Host` allowlist and reverse proxies

When the server is bound to a loopback address **and** no `KARUCODE_AUTH_TOKEN`
is set, requests are only served for `Host: 127.0.0.1`, `localhost` or `[::1]`
(any port). This is the DNS-rebinding defence: a rebound attacker domain sends a
matching `Origin` *and* `Host`, so comparing the two is not enough.

`KARUCODE_ALLOWED_HOSTS` extends the list for reverse proxies, comma separated:

```bash
# exact host (any port), exact host:port, or any subdomain of a domain
KARUCODE_ALLOWED_HOSTS="karucode.example.com,karucode.example.com:8443,*.internal.example"
```

`*.example.com` matches `a.example.com` and `a.b.example.com` but **not** the
bare `example.com` — list it separately. A request with any other `Host` gets
`403 permission_denied` naming the accepted hosts. With a token configured the
`Host` check is skipped (the `Origin == Host` rule remains), so remote
access over a proxy needs `KARUCODE_AUTH_TOKEN`, not this variable.

### Gateways

Anything that speaks the OpenAI chat-completions API with streaming tool calls:

```bash
# LiteLLM proxy
KARUCODE_BASE_URL=http://127.0.0.1:4000/v1 KARUCODE_MODEL=claude-sonnet KARUCODE_API_KEY=sk-litellm

# vLLM
KARUCODE_BASE_URL=http://127.0.0.1:8000/v1 KARUCODE_MODEL=Qwen/Qwen2.5-Coder-32B-Instruct

# Ollama (tool calling depends on the model)
KARUCODE_BASE_URL=http://127.0.0.1:11434/v1 KARUCODE_MODEL=qwen2.5-coder:14b

# llama.cpp server
KARUCODE_BASE_URL=http://127.0.0.1:8080/v1 KARUCODE_MODEL=local

# OpenRouter
KARUCODE_BASE_URL=https://openrouter.ai/api/v1 KARUCODE_API_KEY=sk-or-... KARUCODE_MODEL=anthropic/claude-sonnet-4
```

Extra headers and body fields are supported in the config file
(`[model.extra_headers]`, `[model.extra_body]`) for gateways that need routing
hints. Provider settings can be changed live in the UI (Settings), which writes to
SQLite; the API key is write-only and never returned to the browser.

A failed provider attempt is retried, by default 50 times with an exponential
backoff that grows from 500 ms to a 30 s ceiling (`retry_max_retries`,
`retry_base_delay_ms`, `retry_max_delay_ms` under `[model]`, or
`KARUCODE_RETRY_MAX_RETRIES` / `KARUCODE_RETRY_BASE_DELAY_MS` /
`KARUCODE_RETRY_MAX_DELAY_MS`). That covers every failure class — resets,
timeouts, any HTTP error status, an empty or unreadable body, and a stream that
dies before its first chunk — because a gateway fails transiently in arbitrary
ways.

A stream that dies *after* it started streaming cannot be replayed without
duplicating what you already saw, so it is handled separately: the partial
attempt is discarded, the browser is told to drop it (`stream_retry`), and the
same request is re-issued. That budget is `retry_max_stream_restarts` (default 3,
`KARUCODE_RETRY_MAX_STREAM_RESTARTS`); each restart is a full generation, and `0`
makes a mid-answer death fail the turn instead. When a restart succeeds, only the
successful attempt is persisted.

### Provider profiles and the model library

Instead of — or in addition to — the flat `[model]` table, a config file can
declare named endpoints. Each profile owns its base URL, key, timeout and a
curated list of models; `active_provider` / `active_model` pick the one every run
uses:

```toml
active_provider = "gateway"
active_model = "claude-sonnet"

[[providers]]
id = "gateway"                 # 1-64 chars: letters, digits, '.', '_', '-'
name = "LiteLLM gateway"
provider = "openai_compatible"
base_url = "http://127.0.0.1:4000/v1"
api_key = "sk-litellm"
request_timeout_secs = 300

  [[providers.models]]
  id = "claude-sonnet"         # the id sent to the endpoint
  name = "Claude Sonnet"       # label; falls back to the id
  context_window = 200000
  max_output_tokens = 8192
  tool_call = true
  reasoning = true
  cost_input = 3.0             # USD per million input tokens
  cost_output = 15.0

  [[providers.models]]
  id = "qwen2.5-coder-32b"
  name = "Qwen2.5 Coder 32B"
  context_window = 131072
  tool_call = true

[[providers]]
id = "local-vllm"
name = "vLLM on this box"
base_url = "http://127.0.0.1:8000/v1"

  [[providers.models]]
  id = "Qwen/Qwen2.5-Coder-32B-Instruct"
  context_window = 32768
  tool_call = true
```

* `active_provider` / `active_model` are install-wide. `temperature`, `max_tokens`,
  `top_p` and `parallel_tool_calls` stay global (read from `[model]`), so
  switching profiles does not change sampling.
* With `[[providers]]` present, the connection fields of `[model]` (`provider`,
  `base_url`, `api_key`, `request_timeout_secs`, `extra_headers`, `extra_body`)
  are not used — the active profile supplies them; `model` is only a fallback for
  a profile that has no models yet. Without profiles, a `default` profile is
  synthesised from `[model]`, so both shapes work.
* `KARUCODE_MODEL` / `KARUCODE_BASE_URL` / `KARUCODE_API_KEY` (and the `OPENAI_*`
  fallbacks) win over this file: they become the active `env` profile.
* Limits: 16 profiles, 64 models per profile, unique model ids within a profile,
  `base_url` must be `http://` or `https://` and must not embed credentials, model
  ids ≤200 characters. The same rules are enforced by `PATCH /api/config`, which
  returns `400 bad_request` naming the field — see [docs/API.md](docs/API.md).
* Keys are write-only: the browser and `GET /api/config` only ever see
  `api_key_set`.

The same data can be edited at runtime: `PATCH /api/config` takes
`providers: { upsert: [...], delete: [...] }` and `active: { provider, model }`,
persists them in SQLite and hot-swaps the configuration for the next run. The
legacy flat `model.*` patch still works and is mirrored onto the active profile.
The settings screen edits profiles and their model lists this way; each model
row also offers **Edit** (custom metadata values) and **Refresh** (re-read its
models.dev entry, keeping your custom label).

The composer at the bottom of the chat holds the model selector and a
reasoning-effort selector. Both pin the **current session**
(`PATCH /api/sessions/:id/model`), so two sessions can run different models at
the same time; a session without a pin follows the install-wide default, and the
selector's footer resets to it. Reasoning effort is `none` (off), `minimal`,
`low`, `medium`, `high` or `max` and is sent as the `reasoning_effort` request
field — the selector hides for models whose metadata says they cannot reason.
No restart is needed for any of this.

Model metadata can be prefilled from [models.dev](https://models.dev) instead of
typed by hand. The server fetches the catalog itself (`GET /api/models/catalog`),
caches it for six hours and serves one provider's models at a time; `refresh=true`
forces a refetch. Point it at a mirror or an air-gapped copy (read once at
startup; it must be an `http(s)://` URL, otherwise every fetch fails with
`ok: false`):

```bash
KARUCODE_MODELS_DEV_URL=https://mirror.internal/models-dev/api.json
```

A failed fetch with nothing cached answers `ok: false` with the reason; no other
part of the settings path depends on the catalog. Prefilled values are stored in
the profile and stay editable, because a gateway may serve the same model id with
different limits.

Model ids typed by hand get the same prefill: `GET /api/models/lookup?model=<id>`
finds the closest catalog entry, stripping gateway routing prefixes until a
suffix matches — `a6api/deepseek-v4.1-flash-1017` (provider/model) and
`a6api/ali/deepseek-v4.1-flash-1017` (provider/upstream/model) both resolve to
the catalog's `deepseek-v4.1-flash-1017`. When nothing matches, the id is added
without metadata, as before.

## Plugins and MCP

Plugins are **separate processes** that speak versioned JSON-RPC over stdio. The
core supervises them: a crash, hang or protocol error produces a diagnostic and
does not take the harness down. Plugins contribute tools and lifecycle hooks and
declare capabilities/permissions; they are not OS-sandboxed (see the tradeoffs in
[docs/PLUGINS.md](docs/PLUGINS.md)).

```toml
# ~/.config/karucode/config.toml
[[plugins]]
name = "example-hello"
command = ["/absolute/path/to/target/release/example-hello"]
enabled = true
permissions = ["files.read"]
```

MCP servers (stdio or streamable HTTP) are configured the same way and their tools
appear as `mcp__<server>__<tool>` with the same approval policy as external tools:

```toml
[[mcp_servers]]
name = "filesystem"
transport = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/home/me/project"]
require_approval = true
```

## Security

Read [docs/SECURITY.md](docs/SECURITY.md) before exposing the server. Short version:

* binds to loopback; remote binds require an auth token;
* CSRF protection for browser clients (custom header + Origin/Host check);
* file tools confined to the workspace (symlink escapes refused);
* secrets never returned to the browser or written to logs;
* Markdown rendered with raw HTML disabled and sanitised;
* **shell commands are not sandboxed** — approvals are the gate, not isolation;
* plugins/MCP servers are trusted code; subagents share the workspace (`owned_paths`
  is advisory, not locking).

## Development

```bash
cargo check --workspace          # backend
cargo test --workspace           # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

cd web
npm install
npm run dev                      # Vite on :5173, proxies /api -> :8787
npm test                         # vitest component/reducer tests
npm run typecheck
npm run build                    # emits web/dist (embedded on the next cargo build)
```

Documentation about karucode itself is compiled from `@kc-doc` blocks in the
source. After changing a feature, a config key, a route or a tool:

```bash
cargo run -p kc-docs -- --write   # regenerate docs/internal (commit it)
cargo run -p kc-docs -- --check   # CI gate: stale docs and invalid blocks fail
cargo test -p kc-docs
```

End-to-end without credentials — the mock provider streams real SSE, including
split tool-call deltas:

```bash
python3 scripts/e2e_smoke.py --build
```

It starts the real binary, points it at `scripts/mock_openai.py`, and checks
sessions, streaming, tool execution, approvals (allow + deny), cancellation,
subagents, plugin and MCP tools, path confinement, CSRF, settings redaction, UI
serving and restart/interrupted-state recovery. Results land in `reports/e2e.json`.

Against a **real** endpoint (slow, non-deterministic, costs tokens — this is the
gate for a given deployment rather than for the harness):

```bash
# copies endpoint/model/key from your existing karucode database, runs its own
# server instance so your sessions are untouched
python3 scripts/e2e_real.py --workspace /tmp/scratch-project
```

It checks the provider probe, real streaming, real tool selection, a multi-step
"run the tests → fix the bug → prove they pass" task (asserting the workspace
really ends up green), history continuity, cancellation, subagent delegation and
restart persistence. Pass `--config file.toml` instead of `--db` to use explicit
settings.

Self-documentation end to end (scripted provider, no credentials):

```bash
python3 scripts/e2e_docs.py           # the model calls karucode_docs and gets the MCP doc back
```

Performance:

```bash
python3 scripts/bench.py --build     # startup, idle RSS, peak RSS under load
```

See [reports/bench.md](reports/bench.md) for the latest numbers and the exact
methodology/host, and [docs/REPORT.md](docs/REPORT.md) for the full verification
record (including what was checked in a real browser). No memory target is
promised: measurements for this machine are in that report.

### Repository layout

| Path | Contents |
| --- | --- |
| `crates/kc-core` | types, config, path confinement, event schema, trait contracts, plugin protocol |
| `crates/kc-store` | SQLite persistence |
| `crates/kc-provider` | OpenAI-compatible streaming provider |
| `crates/kc-tools` | file/search/shell/git tools |
| `crates/kc-runtime` | agent loop, approvals, subagents, context budget |
| `crates/kc-plugin-host` / `kc-plugin-sdk` | plugin supervision + SDK |
| `crates/kc-mcp` | MCP client |
| `crates/kc-server` | axum API, SSE, static UI, wiring |
| `crates/kc-docs` | `@kc-doc` parser, build-time generator, search and the `kc-docs` CLI |
| `web/` | Preact + Vite frontend |
| `plugins/example-hello` | working example plugin |
| `docs/` | API, architecture, plugins, MCP, security, internal-documentation format |
| `scripts/` | mock provider, deterministic e2e gate, real-provider e2e gate, benchmark |

Architecture details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). HTTP contract:
[docs/API.md](docs/API.md).

## Known limitations

* **No shell sandbox.** Commands run as the server user. Use a container/VM for
  untrusted code or unattended `auto` mode.
* **No per-subagent worktree.** Subagents share the workspace; path ownership is
  advisory conflict reporting.
* **Sequential tool execution** within a turn (deterministic approvals/events);
  parallel calls from the model are executed in order.
* **MCP** supports stdio and streamable HTTP; SSE-only legacy transport is not
  implemented. Resources/prompts are not surfaced — tools only.
* **Plugins** are processes, not Wasm; no filesystem sandbox, no hot reload of
  changed binaries (disable/enable restarts the process).
* **One active model per install.** `active_provider` / `active_model` are
  install-wide, not per project: every project shares the selected model. Sessions
  are confined to their project directory, and per-session workspace overrides do
  not exist.
* **No built-in TLS.** Put a reverse proxy in front for remote access.
* **Compaction is trimming**, not model-assisted summarisation: old turns are
  dropped (with a notice) once the context budget is exceeded.
* Token accounting is whatever the provider reports; absent usage is shown as
  unknown rather than invented.

## License

MIT.