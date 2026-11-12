# Final report

Built from an empty directory into a working, self-hosted coding agent. This
document records what exists, what was verified and how, the measurements taken,
and what is still missing. It does not restate the feature list in the README.

## 1. What was built

A Rust workspace (10 crates) plus a Preact frontend, served by one binary.

| Component | Result |
| --- | --- |
| `kc-core` | ids, messages, config + precedence, approval policy, path confinement, event schema, all trait contracts (`Provider`, `Tool`, `Store`, `Settings`, `Hooks`), plugin protocol v1, system prompt. 32 tests. |
| `kc-store` | SQLite with `PRAGMA user_version` migrations, 8 tables, WAL, 0600/0700 permissions, per-id approval expiry, advisory path claims. 20 tests. |
| `kc-provider` | OpenAI-compatible streaming: SSE, split tool-call deltas, reasoning deltas, usage, cancellation, redacted errors, `list_models`. 49 tests against a real local HTTP mock. |
| `kc-tools` | 8 first-party tools: read/list/search (ripgrep + pure-Rust fallback), write/edit (atomic, ambiguity-safe), shell (process group, streamed, timeout, output cap), git status/diff. 48 tests. |
| `kc-runtime` | Multi-step loop, streaming aggregation, approvals, cancellation, context budgeting, history repair, subagents, panic containment, run recovery. 79 tests. |
| `kc-plugin-host` / `kc-plugin-sdk` / `example-hello` | Out-of-process plugins over JSON-RPC v1 stdio, supervision, restart accounting, hooks, config-authoritative permissions. 36 tests. |
| `kc-mcp` | MCP client: stdio + streamable HTTP, namespaced tools, bounded reads, approval opt-out per server. 36 tests. |
| `kc-server` | axum API implementing `docs/API.md`, SSE with a single monotonic sequence, auth, CSRF + Host allowlist, security headers, embedded/disk UI, graceful shutdown. 89 tests. |
| `web/` | Preact + Vite workspace: sessions rail, streaming chat, tool/command/approval cards, inspector (workspace/changes/commands/agents), settings, command palette, mobile sheets. 58 tests. |

Totals: **399 Rust tests + 58 frontend tests**, `cargo clippy --workspace
--all-targets -- -D warnings` clean, `cargo fmt --check` clean, `tsc --noEmit`
clean, production build green.

Release binary: 10.5 MiB, JS bundle 120.5 KB gzip, CSS 10.4 KB gzip.

## 2. Delegation and integration record

Work was split across seven parallel specialists (store, provider, tools, runtime,
plugins+MCP, frontend, server/API), each with frozen interfaces, owned paths and
acceptance criteria; then two independent reviewers (security, code) audited the
integrated result, and three fix agents closed what they found. Everything was
re-verified by me before integration — I did not accept reports at face value.
Concrete cases where that mattered:

* The `tools` agent crashed mid-run; I had already verified its crate (48 tests,
  clippy clean) and cancelled the retry instead of letting it rewrite working code
  on a ~8% context budget.
* Agents reported "already merged / already fixed" several times; I checked each
  claim against the tree (`git diff --stat`, test runs) rather than trusting it.
* Both review agents found defects that unit tests had missed (§4).

## 3. Verification performed

**Unit/integration:** `cargo test --workspace` → 399 passed. Coverage includes the
agent loop (tool round-trips, unknown tools, invalid args, per-turn caps,
iteration/limit paths, `finish_reason=length`), approvals (allow/deny/timeout/
read_only/auto/session-scope, concurrent scopes), cancellation, transcript repair
(orphan tool rows at the history boundary, failed persistence), path confinement
(traversal, absolute-outside, symlink escape, NUL), shell (timeouts killing the
process group, drop-kill, exit codes, UTF-8 truncation), SSE/tool-call
reassembly, store cascades and concurrency, plugin crash/timeout isolation, MCP
stdio+HTTP, CSRF/auth/Host checks, settings bounds and persistence.

**End-to-end** (`scripts/e2e_smoke.py`): starts the real release binary against
`scripts/mock_openai.py` (a deterministic streaming OpenAI-compatible mock) with a
real plugin binary and a real MCP stdio server. **36 checks pass**, covering:
health/config redaction, live plugin+MCP discovery, CSRF denial, DNS-rebinding
denial, path traversal, session CRUD, workspace tree/file, git status, streamed
text + tool execution, persistence, approval prompt → approved command output →
exit 0, deny not executed, multiple tool calls, unknown tool, provider failure,
mid-run cancellation, subagent spawn/completion, plugin tool (approval + call),
MCP tool (approval + call), `read_only` denial of external tools, out-of-range
settings rejection, API-key write-only storage, provider probe, UI served by the
same process, `kill -9` restart with history intact and the interrupted run marked
`interrupted`.

**Browser** (real Chromium via CDP, no mocks): loaded the built SPA, created a
session, sent a prompt, watched streamed text and a tool card, observed the
approval card ("Shell commands and network tools are not sandboxed. Read the
command before allowing."), clicked *Allow once*, saw the command card (exit 0)
and the Commands panel, opened the git Changes tab and loaded a real diff, opened a
file in the viewer, opened Settings, ran *Test connection* against the live
endpoint (200), verified the provider field is a real `<select>` driven by
`available_providers`, and re-checked the layout at 1500×960 and 390×844 with the
inspector and sessions panels as mobile sheets. No console errors.

**Clean checkout:** `git clone` into an empty directory, `npm ci && npm run build`,
`cargo build --release`, then the same e2e script → 36/36 again. With no
`web/dist`, the server still starts and serves a page explaining how to build the
UI (verified), so a source checkout is never a blank screen or a crash.

**Failure injection:** provider 500s, malformed SSE, aborted streams, panicking
tools and providers, failing store writes, broken plugins and unreachable MCP
servers — each produces a visible error rather than a wedged session (tests in
`kc-runtime`, `kc-server`, `kc-plugin-host`).

## 4. Defects found by review and fixed

1. **Plugin/MCP tools bypassed the approval policy entirely** (reproduced by the
   security agent: an MCP tool performed a write with zero prompts in `ask` *and*
   `read_only`). Fixed with a runtime-level gate keyed on `ToolOrigin` and
   `Tool::requires_approval`, with `read_only` as a hard pre-deny. Covered by
   tests and by two new e2e checks.
2. **DNS rebinding**: `Origin == Host` alone accepted a rebound attacker domain
   (reproduced: `PATCH` flipped the approval mode to `auto`). Fixed with a
   loopback Host allowlist plus `KARUCODE_ALLOWED_HOSTS`; e2e now asserts 403 and
   that the mode did not change.
3. **Server shut down ~10 s after start** because the whole serve future was
   wrapped in a 10 s timeout. Found by my own probing, not by tests; fixed and
   guarded by an e2e uptime check.
4. **Permanently broken transcripts**: a history window starting mid
   tool-exchange, or a failed tool-result write, left an orphan `tool` message, so
   every later request 400'd. Fixed with `history::repair_history`, used by both
   `begin_run` and `recover`.
5. **Cancellation reported as failure** when it landed during the provider request
   (found in my own probe). Fixed.
6. **Read-only git tools executed repo-controlled code** (`core.fsmonitor`,
   `diff.external`) — a cloned repo could run commands in `read_only` mode. Fixed
   in both the tool and the server git endpoints, with marker-script tests.
7. **Shell allowlist auto-approved chained commands** (`cargo test; curl … | sh`).
   Now any control/metacharacter context forces a prompt.
8. **Orphaned process groups**: dropping the run future killed only the `setsid`
   leader, so a timed-out subagent's command kept running. Fixed with a Drop guard.
9. **Failed commands reported `ok: true`** (`exit_code=7` painted green in the UI).
   Now success requires exit 0.
10. **Unbounded settings** (e.g. a 5 GB read limit) and unbounded MCP HTTP reads;
    both bounded with validation errors.
11. **Corrupt persisted setting prevented startup**; now each DB override is
    applied independently and invalid rows are logged and skipped.
12. Smaller: provider-id validation at startup and in the UI selector, UTF-8-safe
    output truncation, per-scope approval expiry (a subagent could expire a
    sibling's approval), path-claim warnings that never fired, approval-waiter
    leaks, newer-schema databases refused instead of silently accepted, and
    `arguments` sent as a JSON object by some gateways no longer dropped.

## 5. Measurements

Host: Debian 13, kernel 6.12.113, AMD Ryzen 7 3700X (8C/16T), 31.3 GiB RAM,
rustc 1.96.0. Method and raw runs: `reports/bench.md`, produced by
`python3 scripts/bench.py --runs 3` (release binary, temp DB, local streaming
mock; `VmRSS` from `/proc`, cold start measured to first healthy `/api/health`).

| metric | result (3 runs) |
| --- | --- |
| cold start to healthy | 5.7 – 10.1 ms (avg **7.2 ms**) |
| idle RSS (one session, 3 s after start) | **10.0 MiB** avg |
| peak RSS during 3 concurrent agent runs | **12.3 MiB** avg |
| 3 concurrent runs, wall clock (mock provider) | **158 ms** |
| release binary | **10.5 MiB** |

Caveats, stated rather than smoothed over: RSS during a real run against a remote
HTTPS provider will be higher (TLS buffers, larger responses) — the mock keeps
every response small. These numbers say the idle footprint and startup are small;
they do not promise a figure for someone else's workload.

## 5b. Verification against a real provider

The mock proves the harness; it cannot prove a specific gateway. A second, opt-in
gate (`scripts/e2e_real.py`) therefore runs the same flows against a live
OpenAI-compatible endpoint. It boots its own server instance from a snapshot of
the configured settings (so no key is pasted anywhere) and asserts things the
mock cannot: real model output, real tool selection, and the workspace actually
ending up fixed.

Run against `https://ai.karutoil.site/v1` with `surplus/deepseek-v4.1-flash-1017`
(non-deterministic, costs tokens): **8/8 checks passed**.

| check | measured result |
| --- | --- |
| provider probe (`/models` + chat) | 203 ms, 501 models |
| text response | 13 deltas, 191 chars, 1.3 s |
| real tool call | model chose `list_dir` ×2 then `read_file` |
| multi-step coding task | 8.6 s: ran `node --test`, edited `src/math.js`, re-ran to 5/5 green, diff = 1 line |
| history continuity | correctly recalled "I changed `src/math.js`… the `add` function subtracted" |
| cancellation | cancelled in <1 s, session immediately reusable |
| subagent delegation | 1 agent completed, 2 iterations, summary relayed |
| restart persistence | 32 messages and the agent record survived |

Additional real-model checks done by hand (+ = passed):

* **Reasoning streams.** The gateway emits `reasoning_content`; a hard puzzle
  produced 12 `reasoning_delta` events plus 25 `text_delta` events, which the UI
  renders in the collapsible "Thinking" block.
* **Per-subagent model override.** Parent on deepseek, child explicitly on
  `a6api/gpt-5.3-codex`; `AgentRecord.model` recorded the override and the child
  completed in 2 iterations.
* **Crash recovery.** `kill -9` mid-`run_command`: on restart the run is
  `interrupted` and the session still works. The model's next answer even flagged
  that its earlier tool call never returned — the synthesized "tool result
  missing" row from `history::repair_history` doing its job.
* **Full UI flow in Chromium** against the real model: send → live streamed
  reasoning and text → shell approval → `exit 0` output in-chat → file-edit
  approval showing the exact `-`/`+` diff → "Allow for session" honoured for
  later commands → final answer with a copyable DIFF block and a verification
  section. Files on disk were verified independently (`git diff`, `node --test`
  5/5 green).

### The defect this found

Our request builder sent `"tool_choice": "auto"` whenever tools were present.
Measured against this gateway with unique prompts, that single redundant field
pushed time-to-first-token from **~1 s to 9–30 s** (one request produced no data
at all), for two different models. Omitting it — `auto` is already the server
default — restored streaming: the same agent turn went from one blob after 32.7 s
to 11 deltas starting at 1.45 s. Fixed in `kc-provider/src/request.rs`, guarded by
tests, with `model.extra_body.tool_choice` still available for operators who need
an explicit value.

Two smaller things the live run surfaced: the sessions rail showed every session
as "New session" (now auto-titled from the first message), and the chat view could
stop following the stream when content reflowed without a new delta (now
sticky-bottom with a "Jump to latest" affordance).

## 6. Known gaps

* **Provider-specific behaviour.** Everything above now includes a live gateway,
  but only one: `ai.karutoil.site` (a multi-model proxy). Behaviour verified there
  — `reasoning_content` streaming, the `tool_choice` latency trap, 30 s
  heartbeat-then-burst responses under some conditions — may differ on LiteLLM,
  vLLM, Ollama or a vendor's own API. The deterministic mock remains the
  regression gate for the harness itself; `scripts/e2e_real.py` is the gate for a
  given deployment.
* **Shell is not sandboxed**, by design and documented prominently. Approvals are
  a gate, not isolation.
* **Subagents share the parent's live workspace** (including dirty and untracked
  files); automatic git worktrees are intentionally not used. `owned_paths`
  provides advisory conflict reporting, not locking or worktree isolation.
* **Approval expiry granularity**: subagent approvals are marked `resolved` with
  decision `expired` (the store has no per-id `expired` status), so the UI shows
  them as resolved rather than expired.
* **Resolved approvals are not exposed** by any endpoint; only pending ones are.
* **MCP**: tools only. Resources/prompts, sampling and the legacy SSE transport are
  not implemented; servers are connected once at startup — there is no reconnect
  path, so a failed server is reconnected by restarting the process.
* **Plugins** are supervised processes, not a sandbox; a malicious plugin has the
  user's permissions. Permissions are declarative and config-authoritative only.
* **Many projects per instance**; each session runs in its project's workspace,
  resolved through the `WorkspaceProvider` trait (`POST /api/projects` registers
  a project).
* **No TLS**; remote use needs a reverse proxy or SSH tunnel plus a token.
* **Compaction is trimming**, not model-assisted summarisation.
* Windows is not supported (`setsid`/`libc` in the shell tool); macOS is untested.
* Frontend verified only in Chromium; Safari/Firefox were not exercised.

## 7. How to reproduce the verification

```bash
cd web && npm ci && npm run build && cd ..
cargo test --workspace                      # 399 tests
cargo clippy --workspace --all-targets -- -D warnings
cd web && npm test && npm run typecheck && cd ..   # 58 tests
cargo build --release -p kc-server -p example-hello
python3 scripts/e2e_smoke.py --binary target/release/karucode   # 36 checks
python3 scripts/bench.py --runs 3                               # reports/bench.md
```
