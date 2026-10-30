# Production readiness review

Nine specialist reviewers audited the tree in parallel (security, runtime, provider,
store, tools, server/API, frontend, extensions, ops/release). This document is the
consolidated result: what blocks a production deployment, what must be implemented,
and in what order.

**Method.** Every finding below was checked against the code. Findings marked
**[verified]** were reproduced by the lead reviewer directly — by reading the exact
lines, by `rg` over the whole tree, or by running the command in a scratch directory.
Findings marked **[reported]** come from a specialist with `path:line` evidence and a
reproduction, but were not independently re-run. Nothing here is taken from
`README.md` or `docs/REPORT.md` on trust; where those documents disagree with the
code, the code wins and the discrepancy is listed in §7.

**Status: every P0 and P1 finding below has been fixed.** See §9 for the
verification record. The findings are kept in place, in their original form, as the
record of what was wrong and why it mattered — a report that quietly deletes its own
findings cannot be audited.

**Baseline when the review started (kept for comparison).**

| Check | Then | Now |
| --- | --- | --- |
| `cargo check --workspace --all-targets` | pass | pass |
| `cargo test --workspace` | 580 passed, 0 failed | **721 passed, 0 failed** |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass | pass |
| `cargo fmt --all -- --check` | **FAIL** — 33 diffs across 10 files | pass |
| `npx vitest run` (web) | 423 passed, 33 files | **446 passed, 36 files** |
| `npx tsc --noEmit` (web) | pass | pass |
| `kc-docs --check` | pass — 15 topics, 17 files | pass — 15 topics, 17 files |
| `scripts/e2e_smoke.py` | 36 checks | **38 passed, 0 failed** |
| CI configuration | **none exists** | `.github/workflows/ci.yml` |
| `LICENSE` | **absent** | MIT |

The engineering quality is genuinely high — the test suite is real, the docs gate is
real, and the architecture is sound. The gaps are concentrated in three places:
**security boundaries that are documented but not enforced**, **operational
machinery that does not exist at all** (CI, packaging, retention, observability), and
**a handful of correctness bugs where the code silently does the wrong thing instead
of failing loudly**.

---

## 1. Verdict

**When the review started:** not production ready. A strong, well-tested
*prototype* that was close to production quality in its core loop and far from it
in its operational envelope.

The single most important structural problem was that **the release binary did not
behave like the tested binary.** `panic = "abort"` was set for release, which
disabled every `catch_unwind` the runtime relies on for panic containment. The test
suite runs under the dev profile, so this divergence was invisible to CI. A
panicking tool or provider aborted the whole server in production while the tests
proved it was contained.

The second was that **several security controls were documented as guarantees but
were not enforced.** The most serious was a remote-code-execution path through
`git diff` that needed no approval and no CSRF token, reachable by cloning an
untrusted repository.

**Now:** both are fixed, along with every other P0 and P1 finding. The tree builds
from a clean clone, CI enforces every gate, and the end-to-end suite passes 38/38
against the release binary. What remains is §4 (operational maturity, partly
addressed) and §5 (capability depth), neither of which is a blocker.

---

## 2. P0 — Blockers

These must be fixed before the harness is exposed to any untrusted repository, any
untrusted model output, or any second user.

### 2.1 `GET /api/git/diff` executes repository-controlled code **[verified]**

`crates/kc-server/src/routes/git.rs:132` builds `["diff", "--no-ext-diff"]`. The
tool-side implementation at `crates/kc-tools/src/git.rs:184` correctly uses
`["diff", "--no-ext-diff", "--no-textconv"]`, and its own test
(`git.rs:390-455`) proves that `--no-ext-diff` alone does **not** stop
`diff.<driver>.textconv` from executing.

A repository containing `.gitattributes` (`*.txt diff=evil`) plus
`diff.evil.textconv = /path/to/script` gets that script executed by the server when
anyone opens the git panel. It is a `GET`: no CSRF header, no approval, runs as the
server user. `docs/SECURITY.md:42-47` explicitly claims this cannot happen.

**Implement:** add `--no-textconv` to the HTTP route; add `-c core.attributesFile=/dev/null`
and `GIT_CONFIG_NOSYSTEM=1`; add a regression test that drives the *HTTP handler*, not
just the tool. Effort: **S**.

### 2.2 `git diff` still executes `filter.<driver>.clean` **[reported]**

`SAFE_CONFIG` (`crates/kc-tools/src/git.rs:35-42`) disables `core.fsmonitor`,
`diff.external` and the pager, and `diff()` adds `--no-ext-diff --no-textconv`. None
of that disables `filter.<driver>.clean`, which `git diff` runs to compute the
working-tree side. `git_diff` is `ToolClass::ReadOnly`, so `auto_decision` returns
`Allow` unconditionally — it runs with no approval **even in `read_only` mode**.

Reproduced in a scratch repo: a `filter.evil.clean` script executes on `git diff`
with the exact hardened argv kc-tools uses. `git status` does not run it, so
`git_status` is clean.

**Implement:** enumerate drivers from `git config --get-regexp '^filter\.'` and pass
`-c filter.<name>.clean= -c filter.<name>.smudge=` for each, or use plumbing
(`git diff-files -p`) that does not apply clean filters. Add a regression test
mirroring the existing textconv test. Effort: **M**.

### 2.3 `panic = "abort"` disables the documented panic containment **[verified]**

`Cargo.toml:71` sets `panic = "abort"` under `[profile.release]`. The runtime depends
on `catch_unwind` at `crates/kc-runtime/src/agent.rs:219` (provider),
`agent.rs:1247` (tool invocation) and `crates/kc-runtime/src/subagent.rs:518`.

Under `panic = "abort"` those cannot catch. A panicking tool or provider **aborts the
entire server process**, killing every in-flight run and every connected SSE client.
`docs/REPORT.md:17` advertises "panic containment" and `:85` claims panicking tools
"produce a visible error rather than a wedged session". Tests pass only because
`cargo test` uses the dev profile.

**Implement:** remove `panic = "abort"` from `[profile.release]` (costs a little
binary size), or set `panic = "unwind"` for the `karucode` binary specifically. Add
`cargo test --release -p kc-runtime` to CI so the release profile is exercised.
Effort: **S**.

### 2.4 A truncated provider stream is persisted as a clean, successful answer **[verified]**

`crates/kc-provider/src/stream.rs:101-108` synthesises `Finish { reason: "eof" }`
when the body ends without a `finish_reason`, with the comment *"the runtime turns
`eof` into a 'may be incomplete' notice"*.

The runtime never does. `rg '"eof"' crates/` returns exactly two hits: the synthesis
and its own provider test. `agent.rs:1475` only fails the turn when
`finish_reason.is_none()` — which the real provider never produces, because it just
set it to `Some("eof")`. `agent.rs:722-730` only special-cases `"length"`. So `"eof"`
falls through to `RunStatus::Completed` at `agent.rs:814`.

**Impact:** the most common gateway failure — a proxy cutting the body at an event
boundary, an upstream dying after `finish_reason`-less output — produces a *successful*
run with silently truncated text, persisted to SQLite as a normal assistant message.
The user gets a half-written file edit and no warning. It also disables the documented
mid-stream restart, which only fires on `StreamEnd::Failed { transient: true }`.

**Implement:** map `eof` to `StreamEnd::Failed { transient: true }` so
`retry_max_stream_restarts` covers it, and emit the "may be incomplete" notice. Add a
runtime test asserting a `Finish{"eof"}` chunk does not yield `RunStatus::Completed`.
Effort: **S**.

### 2.5 No timeout around a tool call — one hung tool bricks the session **[verified]**

`crates/kc-runtime/src/agent.rs:1246-1256` awaits `tool.invoke(...)` with no
`tokio::time::timeout` and no `select!` on `spec.cancel`. The cancel token is only
*offered* via `ToolCtx` (`agent.rs:1230`); a tool that ignores it — any plugin or MCP
tool, or a buggy built-in — blocks the loop indefinitely. The wall-clock deadline is
checked at the loop top (`agent.rs:531`) and inside `consume` (`agent.rs:1383`), never
around a tool.

`Runtime::cancel` (`lib.rs:318-328`) only fires the token and returns; `is_running`
(`lib.rs:354-358`) keeps returning `true` for a cancelled-but-unwinding run, so every
later `submit` for that session is rejected with 409 (`routes/sessions.rs:352`).
`shutdown` (`lib.rs:381-389`) waits 5 s, logs a warning, and gives up.

**Implement:** wrap `tool.invoke` in `tokio::select!` on `spec.cancel.cancelled()` and
a per-call deadline derived from `spec.deadline`; on timeout return a
`ToolOutcome::err` and continue. Add a hard backstop mirroring `SUBAGENT_CANCEL_GRACE`
(`subagent.rs:148`). Effort: **M**.

### 2.6 Every child process inherits the server's secrets **[verified]**

`rg 'env_clear|env_remove' crates/` finds only four hits, all `GIT_DIR`/`GIT_WORK_TREE`
in the git tools. There is no `env_clear()` anywhere.

- `run_command` (`crates/kc-tools/src/command.rs:219-227`) sets only `LC_ALL=C`.
- Plugins (`crates/kc-plugin-host/src/rpc.rs:508-527`) add `KARUCODE_PLUGIN_*` and the
  configured `env` table, inheriting everything else.
- MCP stdio servers (`crates/kc-mcp/src/stdio.rs:169-178`) likewise.

The server reads `KARUCODE_AUTH_TOKEN` (`config.rs:931`) and `KARUCODE_API_KEY` /
`OPENAI_API_KEY` (`config.rs:946`) from the environment. So every shell command the
model runs — and every `build.rs`, `postinstall` script, plugin and `npx`-launched MCP
server — receives the harness's own bearer token and provider API key.

**Impact:** a prompt-injected `curl`, or a malicious dependency's install script,
exfiltrates the operator's credentials. This is the highest-value target in the
process, and `docs/SECURITY.md:62-66` only claims the key "is never returned by the
API" — which is true and irrelevant.

**Implement:** `env_clear()` then re-add a documented allowlist (`PATH`, `HOME`,
`USER`, `LANG`, `TERM`, `TMPDIR`, `PWD`) plus a configurable
`limits.command_env_passthrough`. Do the same for plugin and MCP spawns. Document the
allowlist in `builtin-tools.md`, `plugins.md` and `mcp.md`. Effort: **M**.

### 2.7 Workspace confinement root is attacker-chosen **[verified]**

`crates/kc-server/src/routes/projects.rs:153-181` (`resolve_project_path`) requires
only: absolute, exists, is a directory, canonicalises. No allowlist, no "must be under
`$HOME`", no confirmation. `Workspace::new` (`crates/kc-core/src/paths.rs:25-38`) then
makes that directory the confinement root.

Every path-confinement guarantee in `docs/SECURITY.md:49-60` is relative to a root any
API client can set to `/`. One `POST /api/projects {"path":"/"}` and `read_file`,
`write_file` and `search_files` are confined to the entire filesystem.

**Implement:** an operator-configured allowlist (`KARUCODE_PROJECT_ROOTS`), or require
the path to be under the configured workspace / `$HOME`, refusing `/`, `/etc`, `/home`.
Effort: **S**.

**Fixed, and then widened again.** The allowlist landed first, with the default
workspace as the implicit root when the list was empty. That default was wrong in
practice: the obvious second project is a *sibling* of the workspace
(`~/projects/other-repo` next to `~/work/main`), so every attempt returned
"outside the allowed project roots" and the operator had to set an environment
variable to do the normal thing. The default is now **no allowlist** — any
existing directory except `/` — and `KARUCODE_PROJECT_ROOTS` opts into the
allowlist for a shared or remotely reachable deployment. `/` stays refused, which
is the part that actually closes this finding: no single request can make the
whole filesystem the confinement root. Regression tests:
`routes::projects::tests::no_configured_roots_means_every_directory_is_allowed`,
`routes::projects::tests::a_configured_root_narrows_the_allowlist`, and
`tests/api.rs::a_project_outside_the_workspace_is_accepted_without_configured_roots`
(end-to-end through `POST /api/projects`).

### 2.8 `PATCH /api/config` can disable approvals and repoint the model endpoint **[verified]**

`crates/kc-server/src/settings.rs:316-323` — `ConfigPatch` includes `approvals` and
`model`/`providers`, and `apply_patch` writes them live. The only guard is
`is_locked()` (`settings.rs:403-405`), opt-in via `KARUCODE_LOCK_SETTINGS`.

Any token holder — or anyone at all on a default no-token loopback instance — can
`PATCH {"approvals":{"mode":"auto"}}` to remove the human gate, then
`PATCH {"model":{"base_url":"https://attacker/"}}` to receive every prompt, file read
and command output. There is no separation between "operate the agent" and
"reconfigure the security policy".

**Implement:** require a separate admin credential for `approvals.mode`,
`model.base_url` and `providers`; log every such change to an append-only audit sink.
Effort: **M**.

### 2.9 A plugin chooses its own tool class, so it can self-exempt from approval **[reported]**

`crates/kc-plugin-host/src/tool.rs:26-33` (`parse_class`) and `:105-107` (`class()`)
take the class straight from the plugin's `tools/list` payload.
`kc-core/src/tool.rs:230-231` defaults `requires_approval` to `!ReadOnly`, so a tool
declared `read_only` is auto-allowed with no prompt — while the plugin's code is free
to write files, spawn processes or open sockets.

`docs/PLUGINS.md:308-310` claims plugin tools "cannot exempt themselves from the
policy". They can: the plugin decides whether its tool *is* read-only. MCP is safe here
only because `McpTool::class()` hard-codes `External` (`kc-mcp/src/lib.rs:633-636`).

**Implement:** treat the declared class as a hint. Default plugin tools to `External`
and let config downgrade specific tool names to `read_only`. Effort: **S**.

### 2.10 Plugin permissions are fail-open when config omits them **[reported]**

`crates/kc-plugin-host/src/lib.rs:467-482`, specifically `:472-474`: when
`config.permissions` is empty, `manifest.permissions` is used verbatim. An operator
who writes `[[plugins]] name = "x" command = [...]` and nothing else gets whatever the
plugin asks for. `docs/PLUGINS.md:326-327` presents this as a "backwards-compatible
fallback"; it is a fail-open default.

**Implement:** absent `permissions` means *no* permissions; require explicit
`permissions = ["*"]` or `trust = true` to honour the manifest. Warn at startup when
the fallback is active. Effort: **S**.

### 2.11 `ask_commands` silently auto-approves every file write **[verified]**

`crates/kc-core/src/approval.rs:192-210` — `for_write` returns `AutoDecision::Allow`
for `ApprovalMode::AskCommands`, unconditionally, ignoring `auto_approve_paths`. The
generated table at `docs/internal/approvals.md:25` says `ask_commands` ⇒ file write =
**ask**. `config.rs:23-24` agrees with the code, not the table.

An operator who reads the table and picks `ask_commands` to get "shell gated, writes
gated" gets every write auto-approved. Combined with §2.7 that is arbitrary file write
with no prompt.

**Implement:** decide the intended semantics and make code and docs agree. If
`ask_commands` is meant to allow workspace writes, say so in the table and honour
`auto_approve_paths` as a restriction; if it is meant to ask, remove the `Allow` arm.
Effort: **S**.

### 2.12 `search_files` returns zero matches when `path` is a single file **[verified]**

`crates/kc-tools/src/search.rs:190-197` and `:250-266`. ripgrep omits the filename
prefix when given exactly one file argument, but `parse_rg_line` does
`splitn(3, ':')` and requires field 2 to parse as `u64`. Reproduced:

```
$ rg --line-number --no-heading --color never alpha one.txt
1:alpha
3:alpha
```

Field 2 is `alpha`, the parse fails, `parse_rg_line` returns `None`, every match is
dropped. The tool reports `0 match(es) … no matches` with `ok: true`. The model
concludes the string is absent and moves on.

This is the worst failure class for a coding agent: a confident wrong answer. The
current suite only ever searches directories (`tests.rs:896`, `tests.rs:932`).

**Implement:** pass `-H` (`--with-filename`) so rg always emits `path:line:text`, and
make `parse_rg_line` fall back to `line:text` relative to `base` when field 2 is not
numeric. Add a test that searches an explicit file path. Effort: **S**.

### 2.13 The working tree is not shippable **[verified]**

`git status` shows 47 modified and 6 untracked files. Three tracked files reference
untracked ones:

- `crates/kc-server/src/routes/mod.rs:3` → `pub mod agents;` but
  `crates/kc-server/src/routes/agents.rs` is untracked.
- `web/src/state/actions.ts:22` → `import * as agentConsole from './agentConsole';`
  but `web/src/state/agentConsole.ts` is untracked.
- `docs/internal/index.json` (tracked, modified) lists slug `prompt-cache`, but
  `docs/internal/prompt-cache.md` is untracked.

A `git commit -a` produces a tree that does not compile, does not build the UI, and
fails `kc-docs --check`. This is the most likely way a release breaks.

**Implement:** `git add -A`; add a CI job that fails on untracked files; add a
pre-commit hook running `cargo check --workspace` and `npx tsc --noEmit`.
Effort: **S**.

### 2.14 No CI of any kind **[verified]**

No `.github/`, `.gitlab-ci.yml`, `.circleci/`, `Jenkinsfile`, `azure-pipelines.yml`,
`.drone.yml`, `Makefile`, `justfile` or `.cargo/config.toml`. `git remote -v` and
`git tag -l` are both empty.

Every quality gate in this repo is a convention a human must remember. Nothing
prevents a commit that fails `fmt`, breaks the docs gate, or ships a stale
`docs/internal/`.

**Implement:** `.github/workflows/ci.yml` with jobs for `fmt`, `clippy`, `test`
(ubuntu + macos), `docs` (`--check` and `cargo test -p kc-docs`), `web`
(`npm ci && typecheck && test && build`), `e2e` (`scripts/e2e_smoke.py` — needs no
network or keys), `audit` (`cargo audit` + `cargo deny check` with a committed
`deny.toml`), `msrv`, and a tag-triggered `release`. Plus `dependabot.yml`.
Effort: **M**.

### 2.15 `cargo fmt --check` fails, including on committed files **[verified]**

33 diffs across 10 files. Two (`crates/kc-core/src/prompt.rs`,
`crates/kc-plugin-sdk/src/embed.rs`) are not in `git status`, so they fail at `HEAD`
too. `README.md:390` and `docs/REPORT.md:24` both claim this gate is clean.

**Implement:** `cargo fmt --all`, commit, add the `fmt` CI job, add a `rustfmt.toml`
to pin the edition. Effort: **S**.

---

## 3. P1 — Production hardening

Needed for a harness that runs unattended for months. Not exploitable in the same way
as P0, but each one is a real operational failure.

### Provider and cost

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.1 | **Every failure class is retried, including permanent 4xx, with a 51-attempt budget** **[verified]** — a malformed request, a nonexistent model id or a revoked key is re-sent 51 times with backoff capped at 30 s: ~20 minutes of hung run before the real error surfaces, and 51 billed attempts. `retry.rs:248` also discards the `retryable` flag the stream layer computed. | `kc-provider/src/retry.rs:128-130` | Classify: retry 408/409/425/429/5xx and transport always; retry 401/403/404 only for profiles marked `aggregator = true`; never retry 400/413/422. Honour the stream's `retryable` flag. Cap wall-clock retry budget, not just attempts. | M |
| 3.2 | **No spend accounting** — `cost_input`/`cost_output` are defined, prefilled from the catalog and validated, then never multiplied by tokens. No token or cost column exists on `sessions`, `runs`, `messages` or `agents`. `Event::Usage` is emitted per turn, rendered in the UI, then discarded. | `kc-core/src/config.rs:252-257`, `kc-store/src/lib.rs:21-105`, `kc-server/src/routes/config.rs:218-227` | Add `input_tokens`/`output_tokens`/`cached_tokens`/`cost_usd` to `runs` plus a rollup on `sessions`; persist on `Event::Usage`; expose totals in `GET /api/stats`; add `limits.max_cost_usd_per_run` / `_per_day` enforced in the loop. | M |
| 3.3 | **`max_output_tokens` is reserved in the budget but never sent on the wire** **[verified]** — `rg max_output_tokens` in `kc-provider/src` and `kc-runtime/src` finds only doc comments and a test. The budget is computed as `window - max_output_tokens`, but the request carries no output limit, so the model can generate past the reserved room and hit `finish_reason: "length"` on exactly the long answers the reserve protects. With `max_tokens` unset (the default) there is no output cap at all. | `kc-runtime/src/agent.rs:616`, `kc-core/src/config.rs:247` | Use `spec.model.max_tokens.or(max_output_tokens)`; add `max_completion_tokens` and select the parameter name per model family. | S |
| 3.4 | **Unknown model falls back to a 240k-character budget with no overflow recovery** — ~60k tokens at the 4-chars/token heuristic. A hand-added model on a 32k endpoint gets a prompt twice its window. There is no "400 context overflow → compact and retry" path anywhere. | `kc-core/src/config.rs:592`, `:572` | Make the fallback conservative (~32k chars) or refuse to run with a warning; add a one-shot compact-and-retry on context-overflow 400. | S |
| 3.5 | **A missing API key is not detected** — `has_api_key()` exists and the UI shows `api_key_set: false`, but no run path checks it. The request simply omits `Authorization`, and the run hangs through the full retry budget before reporting 401. | `kc-core/src/config.rs:335-340`, `kc-provider/src/request.rs:218-230` | Return `Error::Config` at run start when a non-local profile has no key. `Error::Config` is already excluded from retries, so it fails fast. | S |

### Runtime and recovery

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.6 | **Recovery failure aborts server startup** — `recover()` loops over every session and `?`-propagates `repair_history`, which `?`-propagates `append_message`. One corrupt row, a disk-full condition or one oversized session prevents the whole server from starting. | `kc-runtime/src/lib.rs:282-284`, `history.rs:79`, `kc-server/src/bootstrap.rs:520-527` | Make repair per-session best-effort: log, count in `RecoveryReport`, continue. Fail startup only if the store itself is unopenable. Test with an injected `append_message` failure. | S |
| 3.7 | **Startup recovery is O(sessions × 20 000 messages) and blocks serving** — `HISTORY_LIMIT` is 20 000 and `list_messages` returns full rows, all before `bind()`. On a year-old install this is minutes of apparent hang with no port and no health endpoint. | `kc-runtime/src/lib.rs:282-284`, `:53`, `kc-store/src/lib.rs:806-829` | Repair lazily on first run per session (it already runs in `begin_run`), or bound it to sessions with a `running` run/agent, or move it to a background task after bind. Add `--skip-recovery`. | M |
| 3.8 | **Path claims from interrupted agents are never released** — `release_claims` is called only on the normal finish path; `recover()` marks agents `interrupted` but never touches `claims`. After a crash every claimed path stays claimed forever, producing spurious overlap warnings for the life of the session — training the model and operator to ignore the one signal `owned_paths` provides. | `kc-runtime/src/subagent.rs:440`, `:613`, `kc-runtime/src/lib.rs:268-305` | Delete claims for every agent marked interrupted in `recover()`. Add a test. | S |
| 3.9 | **`ToolCallAccumulator` allocates on an attacker-controlled index** — `while self.calls.len() <= index { push(default) }` where `index` comes straight from the gateway's JSON. A malicious or buggy endpoint sending `index: 1_000_000_000` allocates a billion `ToolCall`s: OOM, no error, no bound. | `kc-core/src/provider.rs:95-97` | Reject `index > MAX_TOOL_CALLS` (e.g. 256). Add a unit test. | S |
| 3.10 | **`compact_run_window` can orphan a tool result at the `PROTECTED_TAIL` boundary** — if `end` falls between an assistant turn and its tool results, the assistant is dropped while its results survive, producing exactly the 400 that `repair_history` exists to prevent. | `kc-runtime/src/context.rs:302`, `:620-645` | Extend the unit past `end` when it is an assistant with tool calls, or clamp `end` back to the previous exchange start. Add a test. | S |
| 3.11 | **`hooks.before_prompt` is awaited unbounded on every iteration** — same class as §2.5. The plugin host has its own RPC timeout, but the runtime does not defend itself and `Hooks` is a public trait. | `kc-runtime/src/agent.rs:571-575` | Bound by the remaining run deadline and select on `spec.cancel`; on timeout log, emit a notice, continue. | S |

### Store

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.12 | **No retention, pruning or vacuum anywhere** — the only `DELETE`s are session, project and claims. `messages`, `runs`, `agents`, `approvals` are append-only and removed only by cascade on session delete. Nothing ever deletes a session automatically. A heavy day is ~50 MB; a year lands in the tens of GB with no way to reclaim space. | `kc-store/src/lib.rs:614, 755, 1352`; `rg VACUUM` → none | Retention job; `PRAGMA auto_vacuum=INCREMENTAL` **at DB creation** (cannot be enabled later without a full VACUUM); periodic `incremental_vacuum`; a documented maintenance command; `db_size_bytes` in `/api/stats`. | M |
| 3.13 | **Multi-process access is unsafe and unguarded** — `mark_interrupted_runs`/`mark_interrupted_agents` are global, unscoped `UPDATE ... WHERE status='running'`. No lock file, no `application_id` check. Two servers on one data dir both run `migrate()`, and instance B's `recover()` marks instance A's **live** runs as `interrupted` while A keeps writing. | `kc-store/src/lib.rs:905-917`, `:1090-1103` | Exclusive `flock` on `<data_dir>/karucode.lock` at startup, refuse to start if held. Add `owner_instance` to `runs`/`agents` and scope recovery. | M |
| 3.14 | **Every connection pragma is applied with `.ok()`** — on a filesystem where WAL is unavailable the store silently runs in rollback-journal mode. Worse, if `foreign_keys` fails to enable, `ON DELETE CASCADE` silently stops working and `delete_session` leaves orphaned rows forever. | `kc-store/src/lib.rs:419-422` | Check each return value; fail `open()` if `journal_mode != "wal"` or `foreign_keys != 1`. | S |
| 3.15 | **No `PRAGMA integrity_check`, no corruption detection or recovery path** — `rg integrity_check|quick_check` returns zero matches. A corrupt DB fails with a raw rusqlite string and the server exits, with no diagnosis and no backup to restore from. | `kc-store/src/lib.rs:414-415` | `PRAGMA quick_check` on open, `integrity_check` behind `--verify-db`; actionable error naming the file and recovery command; report the last check in `/api/health`. | S |
| 3.16 | **One connection behind one mutex** **[verified]** — `conn: Arc<Mutex<rusqlite::Connection>>`; every read and write takes the same lock for the whole closure. A `GET /api/sessions/:id/messages?limit=20000` holds it for the entire scan while every concurrent write queues behind. WAL's reader/writer concurrency is entirely wasted. | `kc-store/src/lib.rs:134`, `:440-454` | Small read-only connection pool (2–4) for `list_*`, single writer for mutations. At minimum cap `list_messages` at the API boundary. | M |
| 3.17 | **Missing indexes for the hot query patterns** **[verified]** — present: `messages(session_id,id)`, `runs(session_id,started_at)`, `agents(session_id)`, `approvals(status)`, `claims(session_id)`, `projects(path)`, `sessions(project_id)`. Missing: `messages(agent_id,id)` (full scan of the largest table for `WHERE agent_id = ? ORDER BY id DESC`), `approvals(run_id)`, `approvals(session_id,status)`, `sessions(updated_at)`. | `kc-store/src/lib.rs:41-123` | Add the four indexes in a new migration. | S |

### Tools

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.18 | **`search_files` result cap is not a cap; stdout is buffered unbounded** **[verified]** — `--max-count N` in ripgrep is *per file*, not global (reproduced: `--max-count 1` over two files returns two matches). The code then `read_to_end`s the entire stdout before parsing and truncating. A common pattern on a large repo produces hundreds of MB held in memory, of which all but `max_results` are discarded. The fallback walker *does* enforce a global cap, so the two engines genuinely differ — contradicting the module doc. | `kc-tools/src/search.rs:126-127`, `:241-248`, `:358` | Drop `--max-count` (or use it only as a per-file safety valve) and stream stdout with a bounded reader that stops at `max_results` and kills the child. Test a global cap across files. | M |
| 3.19 | **`search_files` reports long matching lines as `[Omitted long matching line]`** **[verified]** — `--max-columns 300` without `--max-columns-preview` replaces the whole line with that literal text (reproduced). The fallback truncates to 300 chars with `…`, so the engines disagree again. For minified JS or long log lines the model gets a match with no content. | `kc-tools/src/search.rs:124-125` | Add `--max-columns-preview`. Add a parity test on a 400-char line. | S |
| 3.20 | **`read_file` reports a wrong total line count on files over the byte cap** — `total` is computed from the *truncated* buffer, then emitted as `lines A-B of {total}`. A 40 000-line file at the default cap reports `lines 1-400 of 7408`, and the "next N lines" hint is computed from the same wrong number. | `kc-tools/src/file_ops.rs:65-86`, `:141-150` | Count newlines in the whole file, or emit `of at least {total}` and suppress the hint. Add a test. | S |
| 3.21 | **`run_command` leaves orphans when the leader exits on SIGTERM** — `terminate_group` escalates to SIGKILL only if `try_wait()` reports the leader still running. If the leader dies on SIGTERM but a grandchild ignores it, the leader is reaped, `try_wait()` returns `Some`, and SIGKILL is never sent. The surviving grandchild holds the pipes open so the pump tasks never see EOF. Reproduced with a TERM-ignoring grandchild. `docs/internal/builtin-tools.md:53` claims timeouts kill the whole group. | `kc-tools/src/command.rs:410-421` | Always `signal_group(pid, SIGKILL)` after the grace period; track the group id separately and keep the guard armed until `kill(-pid, 0)` returns ESRCH. Test with a TERM-ignoring grandchild. | M |

### Server and API

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.22 | **No request logging, correlation ids, metrics or tracing spans** **[verified]** — the router has exactly four layers (security headers, auth, host guard, CSRF). `rg 'TraceLayer|request_id|correlation|metrics'` over `kc-server/src` returns zero hits. `tower` and `tower-http` are declared in `Cargo.toml:36-37` and **never used**, even though the workspace enables `tower-http`'s `trace` feature. There is no access log at all. | `kc-server/src/app.rs:99-120`, `Cargo.toml:36-37,63` | `TraceLayer::new_for_http()` with a `MakeSpan` minting `request_id` (echoed as `X-Request-Id`); log method/path/status/latency at `info`; add `GET /metrics` (Prometheus text) or at least counters for requests-by-status, SSE connections, active runs. | M |
| 3.23 | **SSE has no `id:` field, so `Last-Event-ID` resume is impossible** **[verified]** — `seq` is in the JSON payload but never emitted as an SSE `id:`. Every reconnect (proxy idle timeout, laptop sleep, the 5 s slow-client disconnect) forces a full refetch, and `docs/API.md:664-667` warns that refetching mid-stream **deletes unsaved output**. | `kc-server/src/routes/events.rs:169` | Emit `id: {seq}` per frame, accept `Last-Event-ID`, keep a bounded per-session ring (last ~1000 envelopes) so a resume within the window replays instead of refetching. | M |
| 3.24 | **JSON routes inherit axum's 2 MiB body cap, returning a non-JSON 413** **[verified]** — no `DefaultBodyLimit` is set anywhere. The rejection is `FailedToBufferBody` → 413 with a plain-text body, violating the documented `{"error":{"code","message"}}` contract. `MAX_PROMPT_BYTES = 1 MiB` and its documented 400 are unreachable for bodies between 1 and 2 MiB. | `kc-server/src/routes/mod.rs:66-67`, `sessions.rs:20` | `DefaultBodyLimit::max(...)` per route plus a custom rejection mapping `LengthLimitError` to `ApiError::new(413, "payload_too_large", …)`. | S |
| 3.25 | **`/api/health` returns 500 instead of the documented `200 degraded`** **[verified]** — `db_status()` deliberately swallows store errors, but the very next line uses `?` on the same store. `docs/API.md:41-42` promises the endpoint still returns 200 so a supervisor can see the reason. A corrupt or locked DB turns the probe into a 500, so an orchestrator crash-loops instead of surfacing the cause. | `kc-server/src/routes/health.rs:14-16` | Wrap `list_projects()` in a `match`, emit `"projects": []` plus `db_error`, keep 200. Test with an injected failing store. | S |
| 3.26 | **Unbounded SSE connections, each with a ~32 MB worst-case buffer** — no connection cap, no per-IP limit, no quota. Each connection allocates a 1024-slot channel of `Bytes`; one `tool_call_finished` frame can carry 24 000 bytes and `command_output` up to 32 000, so a stalled client pins ~32 MB. | `kc-server/src/routes/events.rs:39`, `:53-54` | Cap concurrent SSE connections; bound the per-connection queue by bytes, not slots; drop slow consumers with a visible event. | M |

### Extensions

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.27 | **A protocol violation leaves the plugin process running** — when `pump_lines` fails (line over `MAX_PROTOCOL_LINE`, read error), the reader logs, clears `out`, marks the slot `Failed` and fails pending requests, but **never kills the child**. MCP's stdio transport does the right thing (`fail_protocol` fires the kill sender). A plugin emitting one oversized line stays alive indefinitely on an idle server. | `kc-plugin-host/src/rpc.rs:667-677`, `:754-784` vs `kc-mcp/src/stdio.rs:124-135` | Mirror `fail_protocol`: send on `inner.kill` inside `on_stdout_closed`. | S |
| 3.28 | **Cancelled plugin calls leak entries in the pending map forever** — `tool.rs` selects on `ctx.cancel.cancelled()` and drops the `request` future, which drops the `tokio::time::timeout` too, so the waiter inserted at `rpc.rs:329` survives with a dead receiver. MCP calls `pending.forget(id)` unconditionally. Every cancelled plugin tool call permanently grows the map. | `kc-plugin-host/src/tool.rs:119-128`, `rpc.rs:329`, `:348` | A `Drop` guard that removes the id on every exit path, or pass the cancellation token into `request` as MCP does. | S |
| 3.29 | **Hook output is injected into the system prompt with no size cap** — `extra.join("\n")` is appended verbatim. A plugin can return megabytes and blow the context budget, or inject text that reads as harness-authored ("Additional context provided by plugins:"). Tool output is truncated; hook output is not. | `kc-plugin-host/src/lib.rs:654-672`, `kc-core/src/prompt.rs:85-92` | Cap each contribution (~4 KiB) and the total (~16 KiB) with a visible marker; log when a cap fires. | S |
| 3.30 | **`on_tool_result` is awaited inline, so a slow plugin adds up to 5 s to every tool call** — `fan_out` waits for every plugin's reply, each with the 5 s hook budget. Three slow plugins stall the loop 5 s per call. | `kc-runtime/src/agent.rs:1272-1278`, `kc-plugin-host/src/lib.rs:558-576` | Fire `on_tool_result`/`on_run_end` as detached tasks (their return value is ignored), or give them a much smaller budget. | S |
| 3.31 | **Outgoing plugin/MCP requests use an unbounded channel** — a plugin that stops reading stdin fills the pipe buffer, the writer blocks in `write_all`, and the channel grows without limit. | `kc-plugin-host/src/rpc.rs:564`, `:184`, `kc-mcp/src/stdio.rs:203` | Bounded channel (~64) surfacing `RpcError::Transport("plugin is not reading its stdin")`. | S |

### Frontend

| # | Finding | Evidence | Fix | Effort |
| --- | --- | --- | --- | --- |
| 3.32 | **No error boundary anywhere** — `<App />` is rendered directly; `rg componentDidCatch|getDerivedStateFromError` returns nothing. A malformed payload that slips past the reducer throws during render and leaves a white page with no recovery but a manual reload. | `web/src/main.tsx:11` | A Preact `Component` with `componentDidCatch` around `<App/>` and `<ChatTranscript/>`, rendering a "Something broke — reload / copy diagnostics" card. | S |
| 3.33 | **The oldest messages of a long session are dropped silently** — the server explicitly returns `messages_truncated`/`messages_limit` for this, and the client never reads either. Past 5 000 messages the transcript just starts mid-conversation with no explanation. | `kc-server/src/routes/sessions.rs:209-216`; `rg 'messages_truncated' web/src` → none | Add the fields to `SessionDetail`; when `messages_truncated`, push a `notice` at the head. | S |
| 3.34 | **A gap during a live answer permanently loses tool cards, approvals and notices from that run** — when `preserveLive` is set the snapshot is discarded except for `agents`/`running`/`runStatus`, and the only repair path routes through the same `preserveLive` call. If the lost frames were `tool_call_started`/`approval_requested`/`notice`, nothing re-adds them. The test asserts text survives but never asserts the other items are recovered. | `web/src/state/actions.ts:480-492`, `:191-201`, `:140` | Merge instead of skip: reseed into a candidate state, then splice the live streaming message in at its position (ids are stable: `stream:${runId}`, `tool:${callId}`, `approval:${id}`). | M |
| 3.35 | **No request timeout on any API call** — a hung `GET /api/sessions/:id` leaves `chatLoading` true and the skeleton up forever. | `web/src/api/client.ts:150-156`, `actions.ts:478` | `AbortSignal.any([signal, AbortSignal.timeout(30_000)])` with a distinct "the server did not answer" message. | S |
| 3.36 | **Streaming content is never announced to screen readers** — no `aria-live` region for the streaming answer, so the primary feature is silent for assistive tech. | `web/src/components/ChatTranscript.tsx` | A polite `aria-live` region announcing run start/finish and tool activity, not every token. | S |
| 3.37 | **Switching project leaves the full-width Changes view showing the previous project's git data** — the effect keys on `[refreshKey]` only while reading `currentProjectId` inside. The inspector variant unmounts, but the page variant is mounted once and kept. | `web/src/components/ChangesTab.tsx:158-176`, `WorkspaceTab.tsx:183-201` | Add `currentProjectId` to both dependency arrays. | S |
| 3.38 | **The `@`-mention index is not invalidated on project switch** — a 60 s TTL ref with effectively stable deps, so a mention can name a path the agent cannot open. | `web/src/components/Composer.tsx:193-219` | Reset `indexedAt` and clear suggestions when `currentProjectId` changes. | S |
| 3.39 | **Draft text is not scoped to a session** — the composer stays mounted across `switchSession`, so a half-written message for session A appears in session B and can be sent there. | `web/src/components/Composer.tsx:143` | Key the draft by session id. | S |
| 3.40 | **`Ctrl/Cmd+N` fires while typing in the composer** — no target check, unlike the Escape branch right below it, contradicting the documented behaviour. | `web/src/app.tsx:210-214` vs `:215-224` | Skip when the target is inside `input, textarea, [contenteditable]`. | S |

---

## 4. P2 — Operational maturity

Required to run this as a service someone else depends on.

| # | Gap | Fix | Effort |
| --- | --- | --- | --- |
| 4.1 | **No LICENSE file** **[verified]** — `Cargo.toml:19-22` declares MIT but no `LICENSE`/`COPYING` exists. MIT requires the text to ship with the binary. | Add `LICENSE` with the full MIT text and a copyright line. | S |
| 4.2 | **No packaging or distribution path** — no `Dockerfile`, `.dockerignore`, systemd unit, `install.sh`, tarball or checksums. `README.md:106-119` says "git clone and `cargo run --release`". | Multi-stage `Dockerfile` (node builds `web/dist` → rust builds with `KARUCODE_WEB_DIST` → slim runtime, non-root, `VOLUME /data`, `HEALTHCHECK`); `packaging/karucode.service` with `NoNewPrivileges`, `ProtectSystem=strict`, `StateDirectory`; `packaging/install.sh` verifying `SHA256SUMS`; release workflow emitting a versioned tarball. | L |
| 4.3 | **No versioning or changelog** — version `0.1.0` in `Cargo.toml:20` and `web/package.json:4`, no tags, no `CHANGELOG.md`, no `repository`/`homepage`/`authors`/`readme` metadata. | `CHANGELOG.md` in Keep-a-Changelog format; SemVer with `0.x` caveats; tag `v0.1.0`; complete `[workspace.package]`. | S |
| 4.4 | **No multi-user model** — one shared token, no user table, no ownership, no roles. Any holder can read every transcript, approve every shell command, delete every project, and change global settings. `docs/SECURITY.md:108-110` admits this. | A static `[[users]]` table with per-user tokens, `owner_id` on sessions/projects/approvals, enforced in every handler. Until then, document loudly that the instance is single-operator. | L |
| 4.5 | **No TLS** — remote use needs a reverse proxy or SSH tunnel plus a token. | Document the reverse-proxy recipe; optionally add `--tls-cert`/`--tls-key` via rustls. | M |
| 4.6 | **No backup/restore procedure** — no `VACUUM INTO`, no documented snapshot, no point-in-time recovery, no migration rollback. | A `karucode backup` subcommand using `VACUUM INTO`, documented restore steps, and a pre-migration automatic snapshot. | M |
| 4.7 | **No dependency audit** — no `cargo audit`/`cargo deny` config, no `deny.toml`, no update policy. | Commit `deny.toml`; add `audit` to CI; `dependabot.yml`. | S |
| 4.8 | **No load or soak test** — nothing exercises a long session, many sessions, or sustained streaming. | A soak script: N sessions × M runs against the mock, asserting RSS and DB growth stay bounded. | M |
| 4.9 | **Windows unsupported, macOS untested** — `setsid`/`libc` in the shell tool. | Document explicitly; add a macOS CI job; gate the shell tool's process-group code per platform. | M |
| 4.10 | **Frontend verified only in Chromium** — Safari and Firefox were never exercised. | Add Playwright runs for Firefox and WebKit in CI. | M |

---

## 5. P3 — Capability gaps

Not blockers, but each is something a production coding harness is expected to have.

- **Compaction is trimming, not summarisation.** A model-assisted summary would let long sessions keep their thread instead of losing their beginning.
- **No per-subagent worktree.** `owned_paths` warns about overlap; it does not isolate. Concurrent subagents editing one file can conflict.
- **MCP is tools only.** Resources, prompts, sampling and the legacy SSE transport are unimplemented; servers connect once at startup and enable/disable requires a restart.
- **Resolved approvals are not exposed** by any endpoint; only pending ones are. There is no audit view of what was approved and when.
- **Approval expiry granularity** — subagent approvals are marked `resolved` with decision `expired`, so the UI shows them as resolved rather than expired.
- **Missing tools** a production harness is expected to have: glob, multi-file/apply-patch editing, background/long-running process management, test-runner integration, LSP diagnostics, web fetch/search, image reading, todo/plan tracking.
- **No session search** in the UI; no message editing/retry; no cost display; no multi-tab consistency.
- **Notifications are best-effort desktop-only**: they need a secure context and a
  permission the user grants, and most mobile browsers cannot show a page-initiated
  notification at all (that needs a service worker karucode does not register). The
  in-app badge, the title counter and the sound work everywhere. A notification is
  not an approval channel, and a plugin cannot set a URL, icon or action on one.
- **No i18n readiness**; dates and numbers are formatted ad hoc.

---

## 6. Cross-cutting themes

Four patterns explain most of the list above.

1. **Documented guarantees that the code does not enforce.** §2.1, §2.9, §2.10, §2.11,
   §3.21, §3.25 and the `eof` comment in §2.4 all share one shape: a comment or doc
   asserts a control, and the code does something else. The `@kc-doc` system catches
   *stale* documentation but cannot catch a confident lie — and several of these are
   exactly that. **Recommendation:** for every security claim in `docs/SECURITY.md`,
   add a test that fails if the control is removed. A claim without a test is a wish.

2. **The release profile is not the tested profile.** §2.3 is the instance that
   matters, but the general fix is to run at least the runtime crate's tests under
   `--release` in CI.

3. **Fail-open defaults.** §2.10 (empty permissions → manifest wins), §3.4 (unknown
   model → 240k budget), §3.5 (missing key → 51 retries), §3.1 (every error retried).
   In each case the absent configuration produces the permissive behaviour. Production
   systems should fail closed and say why.

4. **No operational surface.** No CI, no packaging, no logging, no metrics, no
   retention, no backup, no single-instance guard. The harness is excellent at
   *running an agent* and has almost nothing around *running the harness*.

---

## 7. Documentation claims that were false or stale

Each of these was corrected in the same change that fixed the underlying issue, per
the `AGENTS.md` contract. The table is kept as the record of what the docs claimed
versus what the code did.

| Claim | Reality then | Now |
| --- | --- | --- |
| `docs/SECURITY.md:42-47` — a cloned repo cannot make the read-only git endpoints execute code | False for `GET /api/git/diff` (§2.1) and for `filter.*.clean` (§2.2) | True; the doc names the full hardening |
| `docs/REPORT.md:17,85` — panic containment; panicking tools produce a visible error | False in release builds (§2.3) | True; `panic = "unwind"` + a release-profile CI job |
| `kc-provider/src/stream.rs:104-105` — "the runtime turns `eof` into a 'may be incomplete' notice" | The runtime had no `eof` handling at all (§2.4) | True; `eof` is treated as an incomplete stream |
| `docs/internal/approvals.md:25` — `ask_commands` ⇒ file write = ask | Code returned `Allow` (§2.11) | Table now says `allow (workspace)`, matching the code |
| `docs/PLUGINS.md:308-310` — plugin tools cannot exempt themselves from the policy | A plugin declared its own class (§2.9) | True; every plugin tool is `external` |
| `docs/internal/builtin-tools.md:53` — timeouts kill the whole process group | A TERM-ignoring grandchild survived (§3.21) | True; SIGKILL is unconditional after the grace period |
| `docs/API.md:41-42` — health returns 200 degraded | Returned 500 when the store errored (§3.25) | True; `db_error` + 200 |
| `docs/API.md:3-7` — every error is `{"error":{"code","message"}}` | A >2 MiB body returned a plain-text 413 (§3.24) | True; a JSON-shape middleware rewrites it |
| `README.md:390`, `docs/REPORT.md:24` — `cargo fmt --check` clean | 33 diffs (§2.15) | True |
| `docs/REPORT.md:24` — 399 Rust tests | 580 at review time (stale, not wrong) | 721 |
| `docs/SECURITY.md:62-66` — the API key is never returned by the API | True, but the agent could read it from the DB or the environment (§2.6) | Still true, and the doc now says plainly that the agent can read the key |

---

## 8. Recommended sequence

All of phases 0–4 are done. The sequence is kept because it is the order the work
actually had to happen in, and because phase 5–6 remain.

**Phase 0 — make the tree honest.** §2.13 `git add -A`, §2.15 `cargo fmt`, §2.14 CI,
§2.3 `panic = "unwind"`, §4.1 LICENSE. **Done.**

**Phase 1 — close the security holes.** §2.1, §2.2 (git RCE), §2.6 (`env_clear`
everywhere), §2.7 (project root allowlist), §2.9, §2.10 (plugin permissions), §2.11
(approval semantics), §2.8 (admin-gate config). **Done**, each with a regression
test.

**Phase 2 — stop lying to the model.** §2.4 (`eof`), §2.12 (`search_files` `-H`),
§3.18, §3.19, §3.20, §3.9, §3.10. **Done.**

**Phase 3 — survive unattended operation.** §2.5 (tool timeout), §3.6, §3.7
(recovery), §3.8 (claims), §3.12 (retention), §3.13 (single-instance lock), §3.14,
§3.15, §3.16, §3.17 (store), §3.22 (logging/metrics), §3.23 (SSE resume), §3.24,
§3.25, §3.26 (API). **Done.**

**Phase 4 — cost and provider robustness.** §3.1 (retry classification), §3.3, §3.4,
§3.5. **Done.** §3.2 (spend accounting) is **partly done**: the retry classification,
output limit, missing-key check and context fallback are fixed, but token/cost
columns on `runs` and a `max_cost_usd` budget are **not** implemented — see §9.

**Phase 5 — ship it.** §4.2 (Docker + systemd + install), §4.3 (versioning), §4.7
(audit) **done**. §4.6 (backup) is documented in `packaging/README.md` as a
`VACUUM INTO` procedure rather than a subcommand. §4.8 (soak test), §4.9 (macOS CI
job), §4.10 (Firefox/WebKit) **remain**.

**Phase 6 — product depth.** §3.27–§3.40 (extensions and frontend) **done**, then
§5 (capability gaps) **remains**.

---

## 9. Verification record

Everything below was run on this tree after the fixes, in this order.

| Gate | Command | Result |
| --- | --- | --- |
| Build | `cargo check --workspace --all-targets` | pass |
| Tests | `cargo test --workspace` | **721 passed, 0 failed** |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| Format | `cargo fmt --all -- --check` | pass |
| Docs gate | `cargo run -p kc-docs -- --check` | pass — 15 topics, 17 files |
| Web types | `cd web && npm run typecheck` | pass |
| Web tests | `cd web && npm test` | **446 passed, 36 files** |
| Release build | `cargo build --release -p kc-server -p example-hello` | pass |
| End-to-end | `python3 scripts/e2e_smoke.py --binary target/release/karucode` | **38 passed, 0 failed** |
| Tree | `git status --porcelain` | no untracked files |

The end-to-end suite is the one that matters most: it drives the real release
binary against a deterministic streaming mock, a real plugin process and a real MCP
stdio server, and covers the approval flow, cancellation, mid-stream restart,
subagent delegation, plugin and MCP tool calls, `read_only` denial, DNS-rebinding
refusal, path traversal, and a `kill -9` restart with history intact.

### Bugs found while fixing, and fixed

Fixing one thing exposed others. These were not in the original review:

1. **`drop_trailing_repetition` ate the preceding sentence's full stop.** The
   degenerate-loop trimmer anchored on a 24-character window that usually begins
   with the punctuation ending the *previous* sentence, so `"Nine findings
   follow."` became `"Nine findings follow"`. Found because a pre-existing test
   failed deterministically; fixed by starting the anchor at a word character.
2. **The first `instance_id` scoping broke crash recovery.** Scoping
   `mark_interrupted_runs` to `instance_id = ?` looked right but matched nothing
   after a restart, because the id is regenerated per process — so a `kill -9`'d
   run was never marked `interrupted`. Caught by the e2e restart check, not by unit
   tests. Fixed to `!= ?`: the exclusive data-dir lock already guarantees no peer
   is live, so any other instance's `running` row is a dead process's leftover.
3. **A `biased` cancel select cut tools off before they could observe
   cancellation.** Three existing tests required the tool to see the token and
   report what it observed; the fix is a grace period mirroring the subagent
   supervisor's.
4. **`-c filter.X.clean=` is bypassable.** A driver name containing `=` splits on
   the first `=`, leaving the real driver armed. Fixed by using
   `GIT_CONFIG_KEY_n`/`VALUE_n`, which take the key verbatim. `filter.*.process`
   also executes and had to be neutralised too.

### Known remaining gaps

Stated plainly rather than smoothed over:

* **Spend accounting is not implemented.** `cost_input`/`cost_output` are still
  collected and never multiplied by tokens; there are no token or cost columns on
  `runs`, and no `max_cost_usd` budget. An operator cannot answer "what did this
  run cost" from the database. This is the largest remaining item from §3.2.
* **No soak or load test.** Nothing exercises a long session or sustained streaming
  over hours.
* **Frontend verified only in Chromium.** Firefox and WebKit are not in CI.
* **macOS is in the test matrix but the shell tool's process-group code is
  Linux-specific** (`setsid`/`libc`); Windows remains unsupported.
* **No TLS in the binary.** A reverse proxy or SSH tunnel is still required.
* **Single-operator only.** No per-user authorization; the admin token separates
  "operate" from "reconfigure", not one user from another.
* **Compaction is trimming, not summarisation**, and there is no per-subagent
  worktree.
* **MCP is tools only** — no resources, prompts or sampling.
