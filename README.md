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
│ SQLite (WAL): sessions, messages, runs, agents, approvals, settings, claims      │
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

**Approvals**: `ask` by default for shell + writes, with command prefix
allowlists, per-path auto-approval, `ask_commands`, `auto` and `read_only` modes;
`allow once` / `allow for session` / `deny` in the UI, persisted and audited.

**Subagents**: bounded concurrent delegation with a semaphore, inherited model
and settings (optional model override), nested transcripts persisted with
`agent_id`, progress + result in the UI, cancellation propagates, recursion off by
default, advisory `owned_paths` claims.

**UI**: streaming Markdown chat with code blocks and safe rendering, tool activity
cards, command output panel, approval cards, workspace explorer + file viewer,
git changes/diff view, subagent panel, model/connection settings, keyboard
shortcuts, responsive down to mobile, accessible controls.

**Extensions**: out-of-process plugins (JSON-RPC over stdio, supervised, crash
isolated, versioned protocol v1, capability/permission declarations, hooks) with a
working example plugin; MCP client for stdio and streamable-HTTP servers with
namespaced `mcp__<server>__<tool>` tools. See [docs/PLUGINS.md](docs/PLUGINS.md)
and [docs/MCP.md](docs/MCP.md).

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

See [config.example.toml](config.example.toml) and [.env.example](.env.example) for
every key. The important ones:

| Variable | Default | Meaning |
| --- | --- | --- |
| `KARUCODE_HOST` / `KARUCODE_PORT` | `127.0.0.1` / `8787` | bind address; non-loopback requires a token |
| `KARUCODE_WORKSPACE` | cwd | workspace root for file tools |
| `KARUCODE_DATA_DIR` | `~/.local/share/karucode` | SQLite + plugin data |
| `KARUCODE_AUTH_TOKEN` | unset | Bearer token for remote access (≥16 chars) |
| `KARUCODE_BASE_URL` | `http://127.0.0.1:4000/v1` | OpenAI-compatible base URL |
| `KARUCODE_API_KEY` | unset | provider key (falls back to `OPENAI_API_KEY`) |
| `KARUCODE_MODEL` | `gpt-4o-mini` | model id |
| `KARUCODE_TEMPERATURE` / `KARUCODE_MAX_TOKENS` | `0.2` / unset | sampling |
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
| `web/` | Preact + Vite frontend |
| `plugins/example-hello` | working example plugin |
| `docs/` | API, architecture, plugins, MCP, security |
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
* **One workspace per server instance.** Sessions share it; per-session workspace
  switching is not implemented.
* **No built-in TLS.** Put a reverse proxy in front for remote access.
* **Compaction is trimming**, not model-assisted summarisation: old turns are
  dropped (with a notice) once the context budget is exceeded.
* Token accounting is whatever the provider reports; absent usage is shown as
  unknown rather than invented.

## License

MIT.