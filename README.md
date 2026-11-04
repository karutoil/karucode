<p align="center">
  <img src="branding/social/banner.png" alt="karucode — a self-hosted coding agent for long sessions" width="1280">
</p>

<h1 align="center">karucode</h1>

<p align="center">
  <strong>A self-hosted coding agent for long sessions.</strong><br>
  one Rust binary · SQLite state · any OpenAI-compatible endpoint · no Node at runtime
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-dfb680.svg" alt="MIT license"></a>
  <a href="rust-toolchain.toml"><img src="https://img.shields.io/badge/rust-1.82%2B-dfb680.svg" alt="Rust 1.82+"></a>
  <a href="docs/API.md"><img src="https://img.shields.io/badge/API-v1-dfb680.svg" alt="API v1"></a>
</p>

---

karucode reads your project, searches it, edits files, runs commands with streamed
output, inspects git, delegates to subagents and keeps going through multi-step
tasks until the work is done or you stop it. You answer approvals in a browser
workspace built for a long session, not for a dashboard demo.

```text
┌─────────────────────────────────────────────────────────┐
│ browser (Preact UI, served by the same origin)          │
└─────────────────────────────────────────────────────────┘
      REST + SSE
      ▼
┌─────────────────────────────────────────────────────────┐
│ karucode — one Rust binary                              │
│                                                         │
│ ├─ axum API + SSE .. embedded UI, same origin           │
│ ├─ agent loop ...... approvals, subagents, budgets      │
│ ├─ provider ........ any OpenAI-compatible endpoint     │
│ ├─ tools ........... files, search, shell, git, docs    │
│ ├─ plugins + MCP ... out of process, namespaced         │
│ └─ SQLite (WAL) .... projects, sessions, runs, agents   │
└─────────────────────────────────────────────────────────┘
```

## Contents

| | |
| --- | --- |
| [What it does](#what-it-does) | the runtime, tools, approvals, containment, crash recovery |
| [Quick start](#quick-start) | build it and run a real task in two commands |
| [Install](#install) | container or systemd, without a checkout |
| [Configuration](#configuration) | precedence, the keys that matter, remote access |
| [Providers](#providers) | gateways, named profiles, model metadata, retries |
| [Plugins and MCP](#plugins-and-mcp) | add tools with a separate process or a server |
| [Security](#security) | the trust boundaries, and the one that is missing |
| [Development](#development) | tests, gates, the docs contract, repository layout |
| [Known limitations](#known-limitations) | what it does not do, stated plainly |
| [License](#license) | MIT, and where the brand rules live |

## What it does

* **Long-horizon runs.** The iteration budget belongs to a *segment*: when it
  runs out mid-task the runtime refreshes it and keeps going in the same session,
  run and transcript. The ceiling is about
  `(max_continues + 1) × max_iterations` turns — 10,200 with the defaults — or a
  24 h wall clock, whichever comes first.
* **Real tools, real side effects.** Read, search, write and edit files; run
  shell commands with streamed output in their own process group; inspect git;
  spawn subagents. Every tool result is bounded with an explicit truncation
  marker, never silently dropped.
* **Approvals before mutations.** `ask` by default for shell and writes, with
  command-prefix allowlists, per-path auto-approval, `ask_commands`, `auto` and
  `read_only` modes; allow-once / allow-for-session / deny in the UI, audited.
* **Containment for file tools.** Every path resolves through a confined
  workspace: no `..` escapes, no absolute paths outside, symlink escapes
  canonicalised away, writes atomic. The system temp dir is an extra root by
  default (`KARUCODE_ALLOW_TEMP_DIR`) so scratch files, build output and
  screenshots work. Shell commands are explicitly **not** sandboxed — approvals
  are the gate, not isolation.
* **Crash-tolerant.** Cancellation aborts the provider stream, kills shell
  process groups and marks the run `cancelled`. On restart, leftover `running`
  runs become `interrupted` and pending approvals `expired`; nothing is silently
  retried.
* **Self-documenting.** The facts a user needs about karucode itself are written
  as `@kc-doc` comments next to the code that implements them and compiled into
  the binary, so they cannot drift from the build. The model reads them through
  the read-only `karucode_docs` tool; humans read the mirror in
  [docs/internal](docs/internal/).

## Quick start

Prerequisites: Linux (primary target; other Unixes should work) and Rust 1.82+
for the backend. Node.js 20+ is needed **only** to build the frontend — it is
embedded into the binary and not needed at runtime. Optional: `ripgrep` (a
pure-Rust fallback is built in) and `git`.

```bash
git clone https://github.com/karutoil/karucode && cd karucode

export KARUCODE_BASE_URL=http://127.0.0.1:4000/v1   # your gateway
export KARUCODE_API_KEY=sk-...                     # if the gateway needs one
export KARUCODE_MODEL=gpt-4o-mini

cargo run --release -p kc-server -- --workspace /path/to/your/project
```

`cargo build` runs the frontend build itself (`npm ci` when the tree is missing
or the lockfile moved, then `npm run build`) and embeds the result, so that is
the whole build. Open <http://127.0.0.1:8787>, create a session, type a task,
approve the commands you want it to run.

Prefer not to build the UI? `KARUCODE_WEB_DIST=/path/to/web/dist` embeds a
bundle built elsewhere, `KARUCODE_SKIP_WEB_BUILD=1` skips the frontend entirely
(Rust-only iteration, placeholder page), and `KARUCODE_WEB_DIR=web/dist` serves
the folder from disk while you edit it.

**Try it on a real bug.** `examples/demo-app` is a dependency-free Node project
whose `add()` subtracts instead of adding, so two of its five tests fail:

```bash
cargo run --release -p kc-server -- --workspace examples/demo-app
# prompt: Run the tests, find the failing cases, fix the bug in src/math.js,
#         and re-run the tests to prove it passes.
```

Useful flags: `--help`, `--host`, `--port`, `--workspace`, `--data-dir`,
`--config`. Flags beat environment variables for the process they start.

## Install

For a server rather than a checkout, [packaging/README.md](packaging/README.md)
covers the operator side (proxy, backup, upgrade, uninstall). Short version:

```bash
# container: multi-stage, non-root, /data volume, healthcheck
docker build -t karucode .
docker run -d --name karucode -p 8787:8787 \
  -e KARUCODE_AUTH_TOKEN=... -e KARUCODE_ADMIN_TOKEN=... \
  -v karucode-data:/data -v /path/to/project:/workspace karucode

# systemd: verified release tarball, binary + hardened unit
sudo ./packaging/install.sh --workspace /srv/karucode/workspace
# ... or install the build you already have, before any release exists:
cargo build --release -p kc-server
sudo ./packaging/install.sh --binary target/release/karucode --workspace /srv/karucode/workspace
```

`packaging/install.sh` downloads the release tarball, checks it against the
release's `SHA256SUMS` and only then unpacks it. Nothing is piped from the
network into a shell.

`--workspace` is the directory the agent starts in and the one sessions default
to working out of; it is created and owned by the service user. The agent is not
*confined* to it — it runs as the service user and can write anywhere that account
can — but it is where work happens by default. The path goes into a drop-in at
`/etc/systemd/system/karucode.service.d/10-install.conf` rather than into the
unit file, so re-running the installer for an upgrade cannot lose it. The
service is enabled at boot and started, and the installer waits until
`/api/health` answers before it reports success. `--dry-run` prints every action
without changing anything.

Once the service is running a binary you built yourself, upgrades are
`sudo sh packaging/update.sh`: it pulls, builds, snapshots the database and hands
the result to the installer, reading your unit so the workspace, user and port
survive. `sh packaging/update.sh --check` tells you whether there is anything to
update, and `--dry-run` prints the plan.

Two things to set before exposing it beyond loopback: `KARUCODE_AUTH_TOKEN` (a
non-loopback bind is refused without one, minimum 16 characters) and
`KARUCODE_ADMIN_TOKEN` (so an ordinary API client cannot turn approvals off or
repoint the provider). There is no TLS in the binary — put it behind a reverse
proxy or an SSH tunnel.

## Configuration

Precedence, highest first:

1. overrides saved from the UI (the SQLite `settings` table);
2. environment variables;
3. the config file — `KARUCODE_CONFIG`, default `~/.config/karucode/config.toml`;
4. built-in defaults.

`KARUCODE_LOCK_SETTINGS=1` removes layer 1, so a deployment can pin settings
from the environment and reject browser edits with `403`.

The keys that matter most:

| Variable | Default | Meaning |
| --- | --- | --- |
| `KARUCODE_HOST` / `KARUCODE_PORT` | `127.0.0.1` / `8787` | bind address; a non-loopback bind needs a token |
| `KARUCODE_WORKSPACE` | cwd | default workspace root |
| `KARUCODE_DATA_DIR` | `~/.local/share/karucode` | SQLite database + plugin data, mode `0700` |
| `KARUCODE_AUTH_TOKEN` | unset | bearer token for remote access (≥16 chars) |
| `KARUCODE_ADMIN_TOKEN` | unset | required to change approvals or the endpoint |
| `KARUCODE_PROJECT_ROOTS` | unset (any dir) | colon-separated allowlist for where a project may be created |
| `KARUCODE_ALLOW_TEMP_DIR` | `true` | let file tools also reach the system temp dir |
| `KARUCODE_BASE_URL` | `http://127.0.0.1:4000/v1` | OpenAI-compatible base URL |
| `KARUCODE_API_KEY` | unset | provider key (falls back to `OPENAI_API_KEY`) |
| `KARUCODE_MODEL` | `gpt-4o-mini` | model id |
| `KARUCODE_APPROVAL_MODE` | `ask` | `ask`, `ask_commands`, `auto`, `read_only` |
| `KARUCODE_ALLOW_COMMANDS` | empty | command prefixes never prompted for |
| `KARUCODE_MAX_ITERATIONS` | `200` | model turns per *segment* |
| `KARUCODE_MAX_CONTINUES` | `50` | budget refreshes per run |
| `KARUCODE_MAX_RUN_SECS` | `86400` | hard wall clock for a whole run |
| `KARUCODE_COMMAND_TIMEOUT_SECS` | `120` | default shell timeout |
| `KARUCODE_MAX_TOOL_OUTPUT_BYTES` | `24000` | tool result bytes kept for the model |
| `KARUCODE_CONTEXT_MAX_CHARS` | `0` | `0` = derive from the model's window |
| `KARUCODE_SUBAGENT_CONCURRENCY` | `3` | concurrent subagents (`0` = no ceiling) |
| `KARUCODE_SUBAGENT_MAX_BACKGROUND` | `8` | live detached agents per session (`0` = no ceiling) |
| `KARUCODE_ALLOW_RECURSIVE_SUBAGENTS` | `false` | nested delegation |
| `KARUCODE_LOG` | `info` | tracing filter, e.g. `info,karucode=debug` |
| `KARUCODE_ALLOWED_HOSTS` | empty | extra `Host` names for the loopback guard |

[config.example.toml](config.example.toml) and [.env.example](.env.example) list
every key with its default and rationale.

### Long-horizon runs

A run is not allowed to die because it used its first budget of model turns.
`max_iterations` belongs to a segment; when a segment ends mid-task the runtime
emits a notice, appends a `[runtime checkpoint]` turn so the transcript explains
the jump, resets the counter and continues in the same run:

```toml
[limits]
max_iterations = 200   # model turns per segment
max_run_secs = 86400   # hard wall clock for the whole run (24h)
auto_continue = true   # refresh the budget instead of stopping
max_continues = 50     # how many refreshes one run may chain
```

`auto_continue = false` (or `max_continues = 0`) restores a one-shot budget; the
run then stops with status `limit_reached`. A budget that runs out is never a
reason to re-send the prompt — the work is in the transcript, and the checkpoint
tells the model to continue rather than restart.

### Loopback, the `Host` allowlist and reverse proxies

When the server is bound to a loopback address **and** no `KARUCODE_AUTH_TOKEN`
is set, requests are served only for `Host: 127.0.0.1`, `localhost` or `[::1]`,
any port. That is the DNS-rebinding defence: a rebound attacker domain sends a
matching `Origin` *and* `Host`, so comparing the two is not enough by itself.

```bash
KARUCODE_ALLOWED_HOSTS="karucode.example.com,karucode.example.com:8443,*.internal.example"
```

Entries are an exact host (any port), an exact `host:port`, or `*.domain` —
which matches `a.domain` and `a.b.domain` but **not** the bare `domain`; list it
separately. Any other `Host` gets `403 permission_denied` naming the accepted
ones. With a token configured the `Host` check is skipped (the `Origin == Host`
rule remains), so remote access through a proxy needs `KARUCODE_AUTH_TOKEN`, not
this variable.

## Providers

Anything that speaks the OpenAI chat-completions API with streaming tool calls.
Three environment variables are the fastest path:

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

`base_url` is a prefix, not a full URL: karucode appends `/chat/completions`.
Extra headers and body fields (`[model.extra_headers]`, `[model.extra_body]`)
cover gateways that want routing hints. Provider settings can be changed live in
the UI; the API key is write-only and never returned to the browser.

### Named profiles and the model library

A config file can declare several endpoints, each with its own key, timeout and
curated model list. `active_provider` / `active_model` pick the one every run
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

[[providers]]
id = "local-vllm"
name = "vLLM on this box"
base_url = "http://127.0.0.1:8000/v1"

  [[providers.models]]
  id = "Qwen/Qwen2.5-Coder-32B-Instruct"
  context_window = 32768
  tool_call = true
```

* `active_provider` / `active_model` are install-wide. Sampling (`temperature`,
  `max_tokens`, `top_p`, `parallel_tool_calls`) stays global, so switching
  profiles does not change how a model samples.
* With `[[providers]]` present, the connection fields of `[model]` are not used;
  the active profile supplies them. Without profiles a `default` profile is
  synthesised from `[model]`, so both shapes work.
* `KARUCODE_MODEL` / `KARUCODE_BASE_URL` / `KARUCODE_API_KEY` (and the
  `OPENAI_*` fallbacks) win over the file and become the active `env` profile.
* Limits: 16 profiles, 64 models per profile, unique ids within a profile,
  `base_url` must be `http://` or `https://` with no embedded credentials, model
  ids ≤200 characters. `PATCH /api/config` enforces the same rules and returns
  `400 bad_request` naming the field.

The same data is editable at runtime: `PATCH /api/config` takes
`providers: { upsert: [...], delete: [...] }` and `active: { provider, model }`,
persists them in SQLite and hot-swaps the configuration for the next run. The
composer at the bottom of the chat holds the model and reasoning-effort
selectors; both pin the **current session**
(`PATCH /api/sessions/:id/model`), so two sessions can run different models at
once. A session without a pin follows the install-wide default. Reasoning effort
is `none`, `minimal`, `low`, `medium`, `high` or `max`, sent as
`reasoning_effort`; the selector hides for models whose metadata says they
cannot reason.

### Model metadata without typing it

Model metadata (context window, output limit, tool calling, reasoning, cost) can
be prefilled from [models.dev](https://models.dev). The server fetches the
catalog itself (`GET /api/models/catalog`), caches it for six hours and serves
one provider's models at a time; `refresh=true` forces a refetch. Point it at a
mirror or an air-gapped copy (read once at startup, `http(s)://` only):

```bash
KARUCODE_MODELS_DEV_URL=https://mirror.internal/models-dev/api.json
```

Ids typed by hand get the same prefill: `GET /api/models/lookup?model=<id>`
strips gateway routing prefixes until a suffix matches, so
`a6api/ali/deepseek-v4.1-flash-1017` resolves to the catalog's
`deepseek-v4.1-flash-1017`. When nothing matches, the id is added without
metadata. Prefilled values stay editable, because a gateway may serve the same
id with different limits.

### Retries

A provider attempt is retried by default 50 times with an exponential backoff
from 500 ms to a 30 s ceiling, plus a 300 s wall-clock ceiling for the chain
(`retry_max_retries`, `retry_base_delay_ms`, `retry_max_delay_ms`,
`retry_max_total_secs` under `[model]`, or the `KARUCODE_RETRY_*`
equivalents).

A stream that dies *after* it started cannot be replayed without duplicating
what you already saw, so it is handled one level up: the partial attempt is
discarded, the browser is told to drop it (`stream_retry`), and the request is
re-issued. That budget is `retry_max_stream_restarts` (default 3); each restart
is a full generation, and `0` makes a mid-answer death fail the turn instead.
When a restart succeeds, only the successful attempt is persisted.

## Plugins and MCP

Plugins are **separate processes** speaking versioned JSON-RPC over stdio. The
core supervises them: a crash, hang or protocol error produces a diagnostic and
never takes the harness down. Plugins contribute tools and lifecycle hooks and
declare capabilities and permissions; they are not OS-sandboxed. Tools are
namespaced `plugin__<plugin>__<tool>`.

```toml
[[plugins]]
name = "example-hello"
command = ["/absolute/path/to/target/release/example-hello"]
enabled = true
permissions = ["files.read"]
```

MCP servers (stdio or streamable HTTP) are configured the same way, and their
tools appear as `mcp__<server>__<tool>` under the same approval policy as
external tools:

```toml
[[mcp_servers]]
name = "filesystem"
transport = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/home/me/project"]
require_approval = true
```

Write a plugin: [docs/PLUGINS.md](docs/PLUGINS.md) — including
`karucode plugin init <dir>`, which scaffolds a project with the SDK vendored so
it depends on nothing from karucode. Connect a server: [docs/MCP.md](docs/MCP.md).

## Security

Read [docs/SECURITY.md](docs/SECURITY.md) before exposing the server.

* The server binds to loopback; a remote bind requires an auth token.
* Browser clients are CSRF-protected (a custom header plus the Origin/Host check).
* File tools are confined to the workspace — plus the temp dir, so scratch files
  work — and symlink escapes are refused.
* Child processes get a credential-free environment allowlist, so a
  prompt-injected `curl` cannot exfiltrate the harness token or the provider key.
* Secrets are never returned to the browser or written to logs.
* Markdown is rendered with raw HTML disabled and then sanitised.
* **Shell commands are not sandboxed** — approvals are the gate, not isolation.
* Plugins and MCP servers are trusted code; subagents share the workspace, and
  `owned_paths` is advisory, not locking.

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

### Gates

End-to-end without credentials: `scripts/mock_openai.py` streams real SSE,
including split tool-call deltas, and the harness starts the real binary and
checks sessions, streaming, tool execution, approvals (allow and deny),
cancellation, subagents, plugin and MCP tools, path confinement, CSRF, settings
redaction, UI serving and restart/interrupted-state recovery. Results land in
`reports/e2e.json`:

```bash
python3 scripts/e2e_smoke.py --build   # cargo build (debug) + example plugin, then run
```

Against a **real** endpoint (slow, non-deterministic, costs tokens — the gate for
a deployment rather than for the harness). It copies the endpoint/model/key from
your existing database and runs its own server, so your sessions are untouched:

```bash
python3 scripts/e2e_real.py --workspace /tmp/scratch-project
```

It checks the provider probe, real streaming, real tool selection, a multi-step
"run the tests → fix the bug → prove they pass" task (asserting the workspace
really ends up green), history continuity, cancellation, subagent delegation and
restart persistence.

Self-documentation end to end (`python3 scripts/e2e_docs.py`) proves the model can
call `karucode_docs` and get a real doc back. Performance (`scripts/bench.py`)
measures cold start, idle and peak RSS and binary size; the latest numbers and
the exact methodology are in [reports/bench.md](reports/bench.md), the full
verification record — including what was checked in a real browser — in
[docs/REPORT.md](docs/REPORT.md). No memory target is promised.

### The documentation contract

Documentation about karucode itself is compiled from `@kc-doc` blocks in the
source. After changing a feature, a config key, a route or a tool:

```bash
cargo run -p kc-docs -- --write   # regenerate docs/internal (commit it)
cargo run -p kc-docs -- --check   # CI gate: stale docs and invalid blocks fail
cargo test -p kc-docs
```

[AGENTS.md](AGENTS.md) is the contract for a change; [docs/INTERNAL-DOCS.md](docs/INTERNAL-DOCS.md)
explains the format.

### Repository layout

| Path | Contents |
| --- | --- |
| `crates/kc-core` | types, config, path confinement, event schema, trait contracts, plugin protocol |
| `crates/kc-store` | SQLite persistence |
| `crates/kc-provider` | OpenAI-compatible streaming provider |
| `crates/kc-tools` | file, search, shell and git tools |
| `crates/kc-runtime` | agent loop, approvals, subagents, context budget |
| `crates/kc-plugin-host` / `kc-plugin-sdk` | plugin supervision + the crate plugins use |
| `crates/kc-mcp` | MCP client |
| `crates/kc-server` | axum API, SSE, static UI, wiring |
| `crates/kc-docs` | `@kc-doc` parser, generator, search and the `kc-docs` CLI |
| `web/` | Preact + Vite frontend |
| `plugins/example-hello` | a working example plugin |
| `examples/demo-app` | a buggy Node project for trying the agent |
| `docs/` | API, architecture, plugins, MCP, security, production readiness |
| `scripts/` | mock provider, e2e gates, benchmark, README checker |
| `packaging/` | Dockerfile, systemd unit, install script |

Architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). HTTP contract:
[docs/API.md](docs/API.md).

## Known limitations

* **No shell sandbox.** Commands run as the server user. Use a container or VM
  for untrusted code or unattended `auto` mode.
* **Git worktrees isolate subagent work** when the workspace is a git repository.
  For non-git workspaces, subagents reuse the shared workspace and `owned_paths`
  provides advisory conflict reporting.
* **Sequential tool execution** within a turn (deterministic approvals and
  events): parallel calls from the model are executed in order.
* **MCP** supports stdio and streamable HTTP; the legacy SSE-only transport is
  not implemented, and resources/prompts are not surfaced — tools only.
* **Plugins** are processes, not Wasm: no filesystem sandbox, and no hot reload
  of a changed binary (disable/enable restarts the process).
* **Providers are install-wide, not per project.** `active_provider` /
  `active_model` are the default for every project; a *session* can pin its own
  provider/model and reasoning effort (`PATCH /api/sessions/:id/model`), but
  switching project does not switch model, and per-session workspace overrides do
  not exist.
* **No built-in TLS.** Put a reverse proxy in front for remote access.
* **Compaction is trimming**, not model-assisted summarisation: old turns are
  replaced by a digest once the context budget is exceeded, while the persisted
  transcript keeps everything.
* Token accounting is whatever the provider reports; absent usage is shown as
  unknown rather than invented.

## License

MIT. Brand assets and their usage rules: [branding/BRAND.md](branding/BRAND.md).
